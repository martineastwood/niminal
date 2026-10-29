//! The stage's performance: a live sequencer embedded in the app, and the engine it plays through.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use niminal_lang::{Layout, Problem};
use niminal_live::{
    Accepted, Begin, ClipStatus, Landed, Levels, Live, LiveServer, ParsedEval, PendingInfo, SceneStatus,
    Transport, WsLink,
};
use niminal_server::EngineReply;
use serde::Serialize;

/// The stage's one sequencer, shared by the worker that advances it and the window that asks about it.
pub struct Shared {
    session: Mutex<Option<LiveServer>>,
    /// Changes whenever the project does, so the window can redraw only when it needs to.
    revision: AtomicU64,
    /// The engine's last health reading and when it was taken: asking is a full status request.
    health: Mutex<Option<(Instant, Option<Health>)>>,
    /// The MIDI inputs that are open, and the last message that came in on them.
    midi: Mutex<Midi>,
    /// MIDI clock sent to an output, and followed from an input.
    clock: Mutex<Clock>,
}

#[derive(Default)]
struct Clock {
    out: Option<niminal_live_cli::clock::ClockOutput>,
    /// Held so that the input stays open until it is replaced.
    _input: Vec<midir::MidiInputConnection<()>>,
}

#[derive(Default)]
struct Midi {
    /// Held so that the inputs stay open until they are replaced.
    _open: Vec<midir::MidiInputConnection<()>>,
    names: Vec<String>,
    last: Arc<Mutex<Option<String>>>,
}

impl Shared {
    pub fn new() -> Shared {
        Shared {
            session: Mutex::new(None),
            revision: AtomicU64::new(0),
            health: Mutex::new(None),
            midi: Mutex::new(Midi::default()),
            clock: Mutex::new(Clock::default()),
        }
    }

    pub fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keep the sequencer a little ahead of the engine's clock. Every 25ms is what `niminal-live live` uses.
pub fn start_worker(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("niminal-stage".into())
        .spawn(move || {
            loop {
                if let Some(session) = lock(&shared.session).as_mut() {
                    session.advance();
                    if let Some(out) = &lock(&shared.clock).out {
                        niminal_live_cli::clock::pump_clock_out(session.live_mut(), out);
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .expect("the stage worker starts");
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

/// Open a session with the engine on `port`. The engine is the user's to run (`niminal server`).
pub fn connect(shared: &Shared, port: u16, token: Option<&str>) -> Result<(), String> {
    if lock(&shared.session).is_some() {
        return Ok(());
    }
    let link = WsLink::connect(port, token, "niminal-stage")?;
    let hello = link.hello.clone();
    let EngineReply::Hello {
        sample_rate,
        channels,
        ..
    } = hello
    else {
        return Err("the engine did not say what its audio format is".into());
    };
    let layout = Layout::standard(channels as u16)?;
    let mut live = Live::new(Box::new(link), sample_rate, layout)
        .with_clock_sync(Duration::from_millis(100));
    live.set_metering(true);
    *lock(&shared.session) = Some(LiveServer::new(live));
    shared.bump();
    Ok(())
}

/// Close the session. The engine keeps running.
pub fn disconnect(shared: &Shared) {
    drop(lock(&shared.session).take());
    shared.bump();
}

/// Open the MIDI inputs whose names contain `filter` (all of them when it is empty), in place of any that
/// were open. Controls bound with `midi.cc` and `midi.learn`, and clips played `midi.keys`, hear them.
pub fn open_midi(shared: &Arc<Shared>, filter: &str) -> Result<Vec<String>, String> {
    let mut midi = lock(&shared.midi);
    midi._open.clear();
    midi.names.clear();
    let last = midi.last.clone();
    let session = shared.clone();
    let opened = niminal_live_cli::input::open_midi(filter, move |signal| {
        *lock(&last) = describe(signal);
        if let Some(server) = lock(&session.session).as_mut() {
            server.live_mut().control_input(signal);
        }
    });
    match opened {
        Ok((connections, names)) => {
            midi._open = connections;
            midi.names = names.clone();
            Ok(names)
        }
        // no devices is not a failure of the stage
        Err(e) if e.starts_with("no MIDI input") => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The names of the MIDI inputs and outputs, for choosing where the clock comes from and goes to.
#[derive(Serialize)]
pub struct MidiPorts {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

pub fn midi_ports() -> Result<MidiPorts, String> {
    let input = midir::MidiInput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
    let output = midir::MidiOutput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
    Ok(MidiPorts {
        inputs: input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect(),
        outputs: output.ports().iter().filter_map(|p| output.port_name(p).ok()).collect(),
    })
}

/// Send MIDI clock to the output called `name` (making one of that name if there is none), or stop sending.
pub fn set_clock_out(shared: &Shared, name: Option<&str>) -> Result<(), String> {
    let mut session = lock(&shared.session);
    let live = session.as_mut().ok_or("the stage is not connected to an engine")?.live_mut();
    let mut clock = lock(&shared.clock);
    match name {
        None => {
            live.set_clock_out(false);
            if let Some(out) = &clock.out {
                niminal_live_cli::clock::pump_clock_out(live, out);
            }
            clock.out = None;
        }
        Some(name) => {
            clock.out = Some(niminal_live_cli::clock::ClockOutput::open(name)?);
            live.set_clock_out(true);
        }
    }
    Ok(())
}

/// Follow the MIDI clock of the input called `name`, or stop following.
pub fn set_clock_in(shared: &Arc<Shared>, name: Option<&str>) -> Result<(), String> {
    let mut session = lock(&shared.session);
    let live = session.as_mut().ok_or("the stage is not connected to an engine")?.live_mut();
    let mut clock = lock(&shared.clock);
    clock._input.clear();
    live.set_clock_in(false);
    let Some(name) = name else { return Ok(()) };
    let target = shared.clone();
    let (connections, _) = niminal_live_cli::input::open_clock_in(name, move |message, arrived| {
        if let Some(server) = lock(&target.session).as_mut() {
            server.live_mut().midi_clock(message, arrived);
        }
    })?;
    live.set_clock_in(true);
    clock._input = connections;
    Ok(())
}

fn describe(signal: &niminal_live::ControlSignal) -> Option<String> {
    use niminal_live::ControlSignal::*;
    match signal {
        MidiCc { cc, channel, value } => Some(format!("cc {cc} = {value} · ch {channel}")),
        MidiNote { note, velocity, .. } if *velocity > 0 => Some(format!("note {note}")),
        _ => None,
    }
}

/// What was evaluated between engine samples `from` and `to`, as a script.
pub fn transcript(shared: &Shared, from: u64, to: u64) -> Result<String, String> {
    let mut guard = lock(&shared.session);
    let session = guard.as_mut().ok_or("the stage is not connected to an engine")?;
    Ok(session.live_mut().transcript(from, to))
}

/// The newest session log the engine wrote: every server records what it is sent in `~/.niminal/sessions`.
fn newest_session_log() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    std::fs::read_dir(PathBuf::from(home).join(".niminal").join("sessions"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "log"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()
        .map(|(_, path)| path)
}

/// Render engine samples `from` to `to` of the session to a WAV file by replaying the engine's own log.
pub fn render_take(shared: &Shared, from: u64, to: u64, out: &Path) -> Result<String, String> {
    let (rate, channels) = {
        let mut guard = lock(&shared.session);
        let session = guard.as_mut().ok_or("the stage is not connected to an engine")?;
        let live = session.live_mut();
        (live.sample_rate(), live.channels())
    };
    let log = newest_session_log().ok_or(
        "There is no session log to render from. The engine writes one to ~/.niminal/sessions unless it was started with --no-log.",
    )?;
    let binary = find_binary().ok_or("Could not find the niminal program to replay the log with.")?;
    let seconds = |sample: u64| (sample as f64 / f64::from(rate)).to_string();
    let output = Command::new(binary)
        .arg("replay")
        .arg(&log)
        .arg("--out")
        .arg(out)
        .args(["--channels", &channels.to_string()])
        .args(["--sample-rate", &(rate.round() as u32).to_string()])
        .args(["--from", &seconds(from), "--to", &seconds(to)])
        .output()
        .map_err(|e| format!("Could not run niminal replay: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// How often to ask the engine how it is doing.
const HEALTH_EVERY: Duration = Duration::from_millis(1000);

/// A `ctl` that was bound with `midi.learn` and what the line should now say.
#[derive(Debug, Clone, Serialize)]
pub struct Learned {
    pub control: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MidiStatus {
    /// The MIDI inputs that are open.
    pub inputs: Vec<String>,
    /// The last control change or note that came in, to show that something is arriving.
    pub last: Option<String>,
}

/// The engine's count of things that go wrong while it plays. They only ever go up.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Health {
    /// Notes that arrived after the moment they were for.
    pub late_events: u64,
    /// Audio frames the engine had to make up because it was not ready.
    pub starved_frames: u64,
    /// Audio blocks that took longer to make than they last.
    pub deadline_misses: u64,
    /// Voices or events dropped because the engine was full.
    pub capacity_drops: u64,
    /// The longest a block took to make, in microseconds.
    pub max_render_micros: u64,
}

fn health_from(status: &serde_json::Value) -> Option<Health> {
    let realtime = status.get("realtime").filter(|r| r.is_object())?;
    let count = |name: &str| realtime[name].as_u64().unwrap_or(0);
    Some(Health {
        late_events: count("late_events"),
        starved_frames: count("starved_frames"),
        deadline_misses: count("deadline_misses"),
        capacity_drops: count("capacity_drops"),
        max_render_micros: count("max_render_micros"),
    })
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
    let health = {
        let mut last = lock(&shared.health);
        if last.as_ref().is_none_or(|(at, _)| at.elapsed() >= HEALTH_EVERY) {
            // a status that can't be had means the engine has gone: let go of the session, so the window
            // looks for the engine again and opens a new one
            let Ok(status) = live.engine_status() else {
                *last = None;
                drop(last);
                *guard = None;
                shared.bump();
                return Snapshot {
                    connected: false,
                    revision: shared.revision.load(Ordering::Relaxed),
                    ..Default::default()
                };
            };
            *last = Some((Instant::now(), health_from(&status)));
        }
        last.as_ref().and_then(|(_, health)| health.clone())
    };
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
        master: live.take_peaks(),
        levels: live.take_levels(),
        health,
        learned: live
            .take_learned()
            .into_iter()
            .map(|(control, source)| Learned { control, source })
            .collect(),
        midi: {
            let midi = lock(&shared.midi);
            let last = lock(&midi.last).clone();
            MidiStatus {
                inputs: midi.names.clone(),
                last,
            }
        },
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
    /// The loudest sample on each output channel since the last snapshot.
    pub master: Vec<f32>,
    /// The same for each node and bus.
    pub levels: Levels,
    /// How the engine is coping, if it plays in real time.
    pub health: Option<Health>,
    /// Every control: `name`, `value`, `unit`, `range`, `log`, `source` and `kind`, as the engine reports them.
    /// Controls that took a MIDI control change since the last snapshot, to be written into the code.
    pub learned: Vec<Learned>,
    pub midi: MidiStatus,
    pub error: Option<String>,
}
