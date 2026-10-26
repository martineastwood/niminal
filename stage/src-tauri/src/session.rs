//! The stage's performance: a live sequencer embedded in the app, and the engine it plays through.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use niminal_lang::{Layout, Problem};
use niminal_live::{
    Accepted, Begin, ClipStatus, Landed, Live, LiveServer, ParsedEval, PendingInfo, SceneStatus, Transport,
};
use niminal_server::{EngineError, EngineReply, EngineRequest, PROTOCOL_VERSION};
use niminal_ws::Client;
use serde::Serialize;
use serde_json::{Value, json};

/// The stage's one sequencer, shared by the worker that advances it and the window that asks about it.
pub struct Shared {
    session: Mutex<Option<LiveServer>>,
    /// The engine this app started, if it started one. Left alone when the user runs their own.
    engine: Mutex<Option<Child>>,
    /// Changes whenever the project does, so the window can redraw only when it needs to.
    revision: AtomicU64,
}

impl Shared {
    pub fn new() -> Shared {
        Shared {
            session: Mutex::new(None),
            engine: Mutex::new(None),
            revision: AtomicU64::new(0),
        }
    }

    pub fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Let the engine this app started go when the window closes.
    pub fn shutdown(&self) {
        if let Some(mut child) = lock(&self.engine).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keep the sequencer a little ahead of the engine's clock. Every 25ms is what `nimlive live` uses.
pub fn start_worker(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("niminal-stage".into())
        .spawn(move || {
            loop {
                if let Some(session) = lock(&shared.session).as_mut() {
                    session.advance();
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .expect("the stage worker starts");
}

/// The engine across a WebSocket. Unlike `nimlive`'s link, a lost engine is reported in a notice and the
/// app stays up: the stage is a window first, and a session can be opened again from it.
struct StageLink {
    client: Client,
}

impl StageLink {
    fn connect(port: u16, token: Option<&str>) -> Result<(StageLink, EngineReply), String> {
        let (mut client, reply) = Client::connect_as(port, token, "niminal-stage")?;
        let hello = EngineReply::from_json(
            &EngineRequest::Hello {
                protocol: PROTOCOL_VERSION,
                client: None,
                token: None,
            },
            &reply,
        )?;
        client.call("subscribe", json!({ "topics": ["notices"] }))?;
        Ok((StageLink { client }, hello))
    }
}

impl niminal_live::EngineLink for StageLink {
    fn request(&mut self, request: EngineRequest) -> Result<EngineReply, EngineError> {
        let wire = serde_json::to_value(&request).map_err(|e| EngineError {
            code: -32603,
            message: e.to_string(),
            data: None,
        })?;
        let method = wire["method"].as_str().unwrap_or_default().to_string();
        let params = wire.get("params").cloned().unwrap_or_else(|| json!({}));
        match self.client.call_raw(&method, params) {
            Ok(Ok(result)) => {
                EngineReply::from_json(&request, &result).map_err(|message| EngineError {
                    code: -32603,
                    message,
                    data: None,
                })
            }
            Ok(Err(error)) => Err(EngineError::from_json(&error)),
            Err(message) => Err(EngineError {
                code: -32603,
                message,
                data: None,
            }),
        }
    }

    fn notifications(&mut self) -> Vec<(String, Value)> {
        self.client
            .take_notifications()
            .into_iter()
            .filter_map(|m| Some((m["method"].as_str()?.to_string(), m["params"].clone())))
            .collect()
    }
}

/// The `niminal` program: on the search path, in cargo's install folder, or beside the stage.
fn find_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) { "niminal.exe" } else { "niminal" };
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        folders.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    folders.extend(
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(PathBuf::from)),
    );
    folders
        .into_iter()
        .map(|folder| folder.join(name))
        .find(|path| path.is_file())
}

/// Runs `niminal server` on `port`, unless one is already listening. The caller owns the child.
fn open_engine(port: u16) -> Result<Option<Child>, String> {
    if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        return Ok(None);
    }
    let binary = find_binary().ok_or(
        "Could not find the niminal engine. Install it with `cargo install --path crates/niminal-cli`, or run `niminal server` yourself.",
    )?;
    let mut child = Command::new(binary)
        .args(["server", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start the niminal engine: {e}"))?;
    // The server opens its port after it has set up audio; give it a few seconds.
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(Some(child));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "the niminal engine did not open port {port}; is audio available?"
    ))
}

/// Open a session, starting the engine if nothing is listening.
pub fn connect(shared: &Shared, port: u16, token: Option<&str>) -> Result<(), String> {
    if lock(&shared.session).is_some() {
        return Ok(());
    }
    let mut started = None;
    let (link, hello) = match StageLink::connect(port, token) {
        Ok(connected) => connected,
        Err(_) => {
            started = open_engine(port)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match StageLink::connect(port, token) {
                    Ok(connected) => break connected,
                    Err(e) if Instant::now() >= deadline => {
                        if let Some(mut child) = started.take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        return Err(e);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
    };
    let EngineReply::Hello {
        sample_rate,
        channels,
        ..
    } = hello
    else {
        return Err("the engine did not say what its audio format is".into());
    };
    let layout = Layout::standard(channels as u16)?;
    let live = Live::new(Box::new(link), sample_rate, layout)
        .with_clock_sync(Duration::from_millis(100));
    if let Some(child) = started {
        if let Some(old) = lock(&shared.engine).replace(child) {
            let mut old = old;
            let _ = old.kill();
            let _ = old.wait();
        }
    }
    *lock(&shared.session) = Some(LiveServer::new(live));
    shared.bump();
    Ok(())
}

/// Close the session, and stop the engine if this app started it.
pub fn disconnect(shared: &Shared) {
    drop(lock(&shared.session).take());
    shared.shutdown();
    shared.bump();
}

pub fn snapshot(shared: &Shared) -> Snapshot {
    let mut guard = lock(&shared.session);
    let Some(session) = guard.as_mut() else {
        return Snapshot {
            connected: false,
            revision: shared.revision.load(Ordering::Relaxed),
            ..Default::default()
        };
    };
    let live = session.live_mut();
    let tempo = live.tempo();
    Snapshot {
        connected: true,
        revision: shared.revision.load(Ordering::Relaxed),
        transport: Some(live.transport()),
        beats_per_bar: tempo.beats_per_bar,
        meter: tempo.meter(),
        sample_rate: live.sample_rate(),
        channels: live.channels(),
        clips: live.clips(),
        scenes: live.scenes(),
        pending: live.pending(),
        landed: live.take_landed(),
        notices: live.take_notices(),
        error: None,
    }
}

/// A problem with the code that was sent, in a shape the window can show.
#[derive(Debug, Clone, Serialize)]
pub struct ProblemView {
    pub message: String,
    pub help: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub in_definition: Option<String>,
}

impl From<Problem> for ProblemView {
    fn from(p: Problem) -> ProblemView {
        ProblemView {
            message: p.message,
            help: p.help,
            line: p.line,
            column: p.column,
            in_definition: p.in_definition,
        }
    }
}

/// The result of evaluating code: either it landed or it will, or it did not compile.
#[derive(Debug, Clone, Serialize)]
pub struct EvalOutcome {
    pub accepted: Option<Accepted>,
    pub problems: Vec<ProblemView>,
}

pub fn eval(
    shared: &Shared,
    source: &str,
    quantize: Option<&str>,
    dir: Option<&Path>,
    origin: u64,
) -> Result<EvalOutcome, String> {
    let dir = dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    // Compiling can take a while, so it happens without the session lock: the worker keeps playing.
    let begin = {
        let mut guard = lock(&shared.session);
        let session = guard.as_mut().ok_or("the stage is not connected to an engine")?;
        let live = session.live_mut();
        live.set_sample_dir(&dir);
        live.begin_parsed(ParsedEval::new(source, quantize).with_origin(origin))
    };
    let result = match begin {
        Begin::Done(result) => result,
        Begin::Job(mut job) => loop {
            let done = job.compile();
            let (retry, result) = {
                let mut guard = lock(&shared.session);
                let session = guard
                    .as_mut()
                    .ok_or("the stage disconnected while the code was compiling")?;
                let live = session.live_mut();
                match live.retry_eval(&done) {
                    Some(job) => (Some(job), None),
                    None => (None, Some(live.finish_eval(done))),
                }
            };
            if let Some(next) = retry {
                job = next;
                continue;
            }
            break result.expect("a finished evaluation has a result");
        },
    };
    shared.bump();
    Ok(match result {
        Ok(accepted) => EvalOutcome {
            accepted: Some(accepted),
            problems: Vec::new(),
        },
        Err(problems) => EvalOutcome {
            accepted: None,
            problems: problems.into_iter().map(ProblemView::from).collect(),
        },
    })
}

pub fn cancel(shared: &Shared, id: Option<u64>) -> Result<usize, String> {
    let mut guard = lock(&shared.session);
    let session = guard.as_mut().ok_or("the stage is not connected to an engine")?;
    let cancelled = session.live_mut().cancel(id);
    shared.bump();
    Ok(cancelled)
}

pub fn panic(shared: &Shared) -> Result<(), String> {
    let mut guard = lock(&shared.session);
    let session = guard.as_mut().ok_or("the stage is not connected to an engine")?;
    session.live_mut().panic();
    shared.bump();
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub connected: bool,
    pub revision: u64,
    pub transport: Option<Transport>,
    pub beats_per_bar: f64,
    pub meter: (u32, u32),
    pub sample_rate: f32,
    pub channels: usize,
    pub clips: Vec<ClipStatus>,
    pub scenes: Vec<SceneStatus>,
    pub pending: Vec<PendingInfo>,
    pub landed: Vec<Landed>,
    pub notices: Vec<String>,
    pub error: Option<String>,
}
