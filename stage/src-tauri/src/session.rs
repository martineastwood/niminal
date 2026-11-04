//! The stage's performance: a live sequencer embedded in the app, and the engine it plays through.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::audio::{self, AudioChoice, AudioState};
use crate::settings::{self, Settings};
use niminal_host::host::{Host, HostConfig};
use niminal_lang::Problem;
use niminal_sequencer::{
    Accepted, Begin, ClipStatus, InProcess, Landed, Levels, Live, LiveServer, ParsedEval, PendingInfo,
    SceneStatus, Transport,
};
use niminal_cli::input::LiveShared;
use niminal_ws::{Server, ServerConfig};
use serde::Serialize;

/// The stage's one sequencer, shared by the worker that advances it and the window that asks about it.
pub struct Shared {
    /// The sequencer, made when the engine starts and kept for as long as the program runs. The VS Code extension
    /// and the stage's own window both play through it.
    live: OnceLock<LiveShared>,
    /// The engine, which lives in this program and plays through the sound card from the first connect.
    host: Mutex<Option<Host>>,
    /// The sockets other tools reach the stage on.
    tools: Mutex<Tools>,
    /// The tools port and token in force, to open the sockets again when the engine is started again.
    tools_at: Mutex<Option<(u16, Option<String>)>>,
    /// Why a socket other tools would use could not be opened, for the window to say.
    tools_problem: Mutex<Option<String>>,
    /// What the player chose to play through.
    audio: Mutex<AudioChoice>,
    /// The port other tools reach the stage on.
    tools_port: Mutex<u16>,
    /// Why the engine is not running, if it is not.
    engine_error: Mutex<Option<String>>,
    /// The OSC input that is open, and the port it listens on.
    osc: Mutex<Option<(niminal_cli::input::OscInput, u16)>>,
    /// Held while the engine is being started, so two requests to start it start it once.
    starting: Mutex<()>,
    /// Things that went wrong inside the stage, which the window hears of once.
    faults: Mutex<Vec<String>>,
    /// Run the engine with no sound card (for tests).
    no_audio: AtomicBool,
    /// Make the worker's next pass panic, to see that it carries on.
    #[cfg(test)]
    panic_once: AtomicBool,
    /// The engine's last health reading and when it was taken: asking is a full status request.
    health: Mutex<Option<(Instant, Option<Health>)>>,
    /// The MIDI inputs that are open, and the last message that came in on them.
    midi: Mutex<Midi>,
    /// MIDI clock sent to an output, and followed from an input.
    clock: Mutex<Clock>,
}

/// The two APIs the stage offers on this machine: the sequencer's, which the VS Code extension plays through,
/// and the engine's, which other sequencers and clients use.
#[derive(Default)]
struct Tools {
    live: Option<Server>,
    engine: Option<Server>,
}

#[derive(Default)]
struct Clock {
    out: Option<niminal_cli::clock::ClockOutput>,
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
            live: OnceLock::new(),
            host: Mutex::new(None),
            tools: Mutex::default(),
            tools_at: Mutex::new(None),
            tools_problem: Mutex::new(None),
            audio: Mutex::new(AudioChoice::default()),
            tools_port: Mutex::new(settings::DEFAULT_TOOLS_PORT),
            engine_error: Mutex::new(None),
            osc: Mutex::new(None),
            starting: Mutex::new(()),
            faults: Mutex::new(Vec::new()),
            no_audio: AtomicBool::new(false),
            #[cfg(test)]
            panic_once: AtomicBool::new(false),
            health: Mutex::new(None),
            midi: Mutex::new(Midi::default()),
            clock: Mutex::new(Clock::default()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The sequencer, locked, once the engine has started.
fn sequencer(shared: &Shared) -> Option<MutexGuard<'_, LiveServer>> {
    shared.live.get().map(|live| lock(live))
}

/// Run `f` on the live sequencer, or fail when the engine has not started.
fn with_live<T>(shared: &Shared, f: impl FnOnce(&mut Live) -> T) -> Result<T, String> {
    let mut session = sequencer(shared).ok_or("the engine has not started")?;
    Ok(f(session.live_mut()))
}

/// The user's home folder.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// How often the engine is looked after.
const HOST_TICK: Duration = Duration::from_millis(250);

/// The words of a panic.
fn panic_words(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "an internal error".into())
}

/// One pass of the worker's job: look after the engine now and then, and keep the sequencer ahead of its clock.
fn work(shared: &Shared, last_tick: &mut Instant) {
    #[cfg(test)]
    if shared.panic_once.swap(false, Ordering::Relaxed) {
        panic!("a fault the test asked for");
    }
    // the engine looks after its sound card, its log and its health a few times a second
    if last_tick.elapsed() >= HOST_TICK {
        *last_tick = Instant::now();
        if let Some(host) = lock(&shared.host).as_mut() {
            host.tick();
        }
    }
    // with no socket serving the sequencer, nothing else collects what it announces, and the window reads that
    let served = lock(&shared.tools).live.is_some();
    if let Some(mut session) = sequencer(shared) {
        session.advance();
        if !served {
            session.notifications();
        }
        if let Some(out) = &lock(&shared.clock).out {
            niminal_cli::clock::pump_clock_out(session.live_mut(), out);
        }
    }
}

/// Keep the sequencer a little ahead of the engine's clock. Every 25ms is what `niminal play` uses.
///
/// A bug that panics here must not end the worker: the app would carry on looking alive with nothing
/// playing. The panic is written to the crash log by the hook, the window is told once, and the next pass runs.
pub fn start_worker(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("niminal-stage".into())
        .spawn(move || {
            let mut last_tick = Instant::now();
            let mut last_fault: Option<(String, Instant)> = None;
            loop {
                let pass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&shared, &mut last_tick)));
                if let Err(payload) = pass {
                    let words = panic_words(payload.as_ref());
                    // a fault that repeats every pass would fill the window with the same line
                    let repeated = last_fault.as_ref().is_some_and(|(w, at)| *w == words && at.elapsed() < Duration::from_secs(10));
                    if !repeated {
                        lock(&shared.faults).push(format!(
                            "The stage hit an internal error and carried on ({words}). Details are in ~/.niminal/crash.log."
                        ));
                        last_fault = Some((words, Instant::now()));
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .expect("the stage worker starts");
}

/// The engine's settings for what the player chose.
fn host_config(choice: &AudioChoice, no_audio: bool) -> HostConfig {
    HostConfig {
        out_device: choice.output.clone(),
        buffer_size: choice.buffer_size,
        sample_rate: choice.sample_rate,
        no_audio,
        log: niminal_host::host::new_session_log(),
        ..Default::default()
    }
}

/// Start the engine in this program, playing through the sound card the player chose last time, and open the
/// sockets other tools reach the stage on. Called when the program opens, and again to try again if that failed.
pub fn start(shared: &Shared) -> Result<(), String> {
    if shared.live.get().is_some() {
        return Ok(());
    }
    let saved = settings::load();
    *lock(&shared.audio) = saved.audio;
    *lock(&shared.tools_port) = saved.tools_port;
    let result = connect_with(shared, saved.tools_port, None);
    *lock(&shared.engine_error) = result.as_ref().err().cloned();
    if result.is_ok()
        && let Some(port) = saved.osc_port
        && let Err(e) = osc_listen(shared, Some(port))
    {
        lock(&shared.faults).push(format!("OSC input is off: {e}"));
    }
    result
}

/// Use `port` and `token` for the sockets other tools reach the stage on, opening any that are not open.
pub fn tools_apply(shared: &Shared, port: u16, token: Option<&str>) -> Result<(), String> {
    let live = shared.live.get().ok_or("The engine is not running; restart it first.")?;
    let engine = lock(&shared.host).as_ref().map(Host::engine).ok_or("The engine is not running; restart it first.")?;
    *lock(&shared.tools_port) = port;
    retarget_tools(shared, &engine, live, port, token);
    Ok(())
}

/// Listen for OSC on UDP `port` (or stop listening, with none): controls bound with `osc_in` hear it.
pub fn osc_listen(shared: &Shared, port: Option<u16>) -> Result<(), String> {
    let live = shared.live.get().ok_or("The engine is not running.")?.clone();
    let mut osc = lock(&shared.osc);
    *osc = None;
    if let Some(port) = port {
        let input = niminal_cli::input::OscInput::start(live, port)?;
        *osc = Some((input, port));
    }
    Ok(())
}

/// What is remembered between runs, as it is now.
pub fn settings_now(shared: &Shared) -> Settings {
    Settings {
        audio: lock(&shared.audio).clone(),
        tools_port: *lock(&shared.tools_port),
        osc_port: lock(&shared.osc).as_ref().map(|(_, port)| *port),
    }
}

fn connect_with(shared: &Shared, port: u16, token: Option<&str>) -> Result<(), String> {
    let _starting = lock(&shared.starting);
    if let Some(live) = shared.live.get() {
        let engine = lock(&shared.host).as_ref().map(Host::engine);
        if let Some(engine) = engine {
            retarget_tools(shared, &engine, live, port, token);
        }
        return Ok(());
    }
    let choice = lock(&shared.audio).clone();
    let no_audio = shared.no_audio.load(Ordering::Relaxed);
    let host = match Host::start(host_config(&choice, no_audio)) {
        Ok(host) => host,
        // a sound card that is gone, or settings it won't take, must not leave the player with no engine
        Err(first) if choice != AudioChoice::default() => {
            let host = Host::start(host_config(&AudioChoice::default(), no_audio))
                .map_err(|second| format!("{first}; and the default sound card would not open either: {second}"))?;
            *lock(&shared.audio) = AudioChoice::default();
            lock(&shared.faults).push(format!("The saved audio settings did not work ({first}), so the stage is on the default sound card."));
            host
        }
        Err(e) => return Err(e),
    };
    let engine = host.engine();
    let mut live = Live::new(Box::new(InProcess::new(engine.clone())), host.sample_rate() as f32, host.layout())
        .with_clock_sync(Duration::from_millis(100));
    live.set_metering(true);
    *lock(&shared.host) = Some(host);
    let live: LiveShared = Arc::new(Mutex::new(LiveServer::new(live).with_tap()));
    if shared.live.set(live.clone()).is_err() {
        return Ok(());
    }
    retarget_tools(shared, &engine, &live, port, token);
    Ok(())
}

/// Let other tools on this machine reach the stage, on `port` and the one above it. Not being able to (another
/// program may have a port) does not stop the stage from playing; the window is told why.
fn retarget_tools(shared: &Shared, engine: &niminal_host::audio::Shared, live: &LiveShared, port: u16, token: Option<&str>) {
    let wanted = Some((port, token.map(str::to_string)));
    let same = *lock(&shared.tools_at) == wanted;
    let (have_live, have_engine) = {
        let mut tools = lock(&shared.tools);
        if !same {
            // another port or token: everything is opened again
            tools.live = None;
            tools.engine = None;
        }
        (tools.live.is_some(), tools.engine.is_some())
    };
    if same && have_live && have_engine {
        return;
    }
    *lock(&shared.tools_at) = wanted;
    *lock(&shared.tools_problem) = None;
    // only what is not already open: a socket that works is left alone
    if !(same && have_live) {
        open_live_tool(shared, live);
    }
    if !(same && have_engine) {
        open_engine_tool(shared, engine);
    }
}

fn tools_config(shared: &Shared, offset: u16) -> ServerConfig {
    let (port, token) = lock(&shared.tools_at).clone().unwrap_or((0, None));
    ServerConfig {
        listen: Ipv4Addr::LOCALHOST.into(),
        port: if port == 0 { 0 } else { port.saturating_add(offset) },
        token,
    }
}

fn open_live_tool(shared: &Shared, live: &LiveShared) {
    let config = tools_config(shared, 0);
    let port = config.port;
    match Server::start(live.clone(), config) {
        Ok(server) => lock(&shared.tools).live = Some(server),
        Err(e) => note_tools_problem(shared, format!("The VS Code extension can't reach this stage: port {port} is in use ({e}).")),
    }
}

fn open_engine_tool(shared: &Shared, engine: &niminal_host::audio::Shared) {
    let config = tools_config(shared, 1);
    let port = config.port;
    match Server::start(engine.clone(), config) {
        Ok(server) => lock(&shared.tools).engine = Some(server),
        Err(e) => note_tools_problem(shared, format!("Other sequencers can't reach this engine: port {port} is in use ({e}). Is a `niminal server` running?")),
    }
}

/// Remember why a socket could not be opened (all of them, if there are several), until they all open.
fn note_tools_problem(shared: &Shared, message: String) {
    eprintln!("niminal: {message}");
    let mut problem = lock(&shared.tools_problem);
    *problem = Some(match problem.take() {
        Some(earlier) => format!("{earlier} {message}"),
        None => message,
    });
}

/// What the window shows of the audio settings, with the sound cards there are.
pub fn audio_state(shared: &Shared) -> Result<AudioState, String> {
    let host = lock(&shared.host);
    let host = host.as_ref().ok_or("the engine has not started")?;
    Ok(AudioState {
        devices: audio::devices(),
        choice: lock(&shared.audio).clone(),
        sample_rate: host.sample_rate(),
        output: host.output_description().trim_start_matches("audio to ").to_string(),
        osc_port: lock(&shared.osc).as_ref().map(|(_, port)| *port),
    })
}

/// Play through other audio settings: the engine is started again on them. Notes and definitions are lost with
/// the old engine, so the player evaluates their code again. If the new settings don't work the previous ones are
/// restored and the error says why.
pub fn apply_audio(shared: &Shared, choice: AudioChoice) -> Result<AudioState, String> {
    let live = shared.live.get().ok_or("the engine has not started")?.clone();
    let previous = lock(&shared.audio).clone();
    let no_audio = shared.no_audio.load(Ordering::Relaxed);
    // the old engine goes first, so that the sound card is free for the new one
    lock(&shared.tools).engine = None;
    *lock(&shared.host) = None;
    let (host, used, failure) = match Host::start(host_config(&choice, no_audio)) {
        Ok(host) => (host, choice, None),
        Err(failure) => match Host::start(host_config(&previous, no_audio)) {
            Ok(host) => (host, previous, Some(failure)),
            Err(again) => return Err(format!("{failure}; and the previous sound card would not open again: {again}")),
        },
    };
    let engine = host.engine();
    let mut fresh = Live::new(Box::new(InProcess::new(engine.clone())), host.sample_rate() as f32, host.layout())
        .with_clock_sync(Duration::from_millis(100));
    fresh.set_metering(true);
    lock(&live).replace_live(fresh);
    *lock(&shared.host) = Some(host);
    *lock(&shared.audio) = used;
    *lock(&shared.health) = None;
    open_engine_tool(shared, &engine);
    match failure {
        Some(failure) => Err(format!("{failure}. Still playing through the previous settings.")),
        None => audio_state(shared),
    }
}

/// Open the MIDI inputs whose names contain `filter` (all of them when it is empty), in place of any that
/// were open. Controls bound with `midi.cc` and `midi.learn`, and clips played `midi.keys`, hear them.
pub fn open_midi(shared: &Arc<Shared>, filter: &str) -> Result<Vec<String>, String> {
    let mut midi = lock(&shared.midi);
    midi._open.clear();
    midi.names.clear();
    let last = midi.last.clone();
    let session = shared.clone();
    let opened = niminal_cli::input::open_midi(filter, move |signal| {
        *lock(&last) = describe(signal);
        if let Some(mut server) = sequencer(&session) {
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
    with_live(shared, |live| {
        let mut clock = lock(&shared.clock);
        match name {
            None => {
                live.set_clock_out(false);
                if let Some(out) = &clock.out {
                    niminal_cli::clock::pump_clock_out(live, out);
                }
                clock.out = None;
            }
            Some(name) => {
                clock.out = Some(niminal_cli::clock::ClockOutput::open(name)?);
                live.set_clock_out(true);
            }
        }
        Ok(())
    })?
}

/// Follow the MIDI clock of the input called `name`, or stop following.
pub fn set_clock_in(shared: &Arc<Shared>, name: Option<&str>) -> Result<(), String> {
    with_live(shared, |live| {
        let mut clock = lock(&shared.clock);
        clock._input.clear();
        live.set_clock_in(false);
        let Some(name) = name else { return Ok(()) };
        let target = shared.clone();
        let (connections, _) = niminal_cli::input::open_clock_in(name, move |message, arrived| {
            if let Some(mut server) = sequencer(&target) {
                server.live_mut().midi_clock(message, arrived);
            }
        })?;
        live.set_clock_in(true);
        clock._input = connections;
        Ok(())
    })?
}

fn describe(signal: &niminal_sequencer::ControlSignal) -> Option<String> {
    use niminal_sequencer::ControlSignal::*;
    match signal {
        MidiCc { cc, channel, value } => Some(format!("cc {cc} = {value} · ch {channel}")),
        MidiNote { note, velocity, .. } if *velocity > 0 => Some(format!("note {note}")),
        _ => None,
    }
}

/// What was evaluated between engine samples `from` and `to`, as a script.
pub fn transcript(shared: &Shared, from: u64, to: u64) -> Result<String, String> {
    with_live(shared, |live| live.transcript(from, to))
}

/// The newest session log the engine wrote: every server records what it is sent in `~/.niminal/sessions`.
fn newest_session_log() -> Option<PathBuf> {
    std::fs::read_dir(home()?.join(".niminal").join("sessions"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "log"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()
        .map(|(_, path)| path)
}

/// Render engine samples `from` to `to` of the session to a WAV file by replaying the engine's own log.
pub fn render_take(shared: &Shared, from: u64, to: u64, out: &Path) -> Result<String, String> {
    let (rate, channels) = with_live(shared, |live| (live.sample_rate(), live.channels()))?;
    let log = newest_session_log().ok_or(
        "There is no session log to render from. The engine writes one to ~/.niminal/sessions as it plays.",
    )?;
    let text = std::fs::read_to_string(&log).map_err(|e| format!("Could not read {}: {e}", log.display()))?;
    let entries = niminal_server::from_lines(&text).map_err(|e| format!("{}: {e}", log.display()))?;
    let sample_rate = rate.round() as u32;
    let seconds = |sample: u64| sample as f64 / f64::from(rate);
    let audio = niminal_host::replay::replay(&entries, channels as u16, sample_rate, true)?;
    let audio = niminal_host::replay::slice(audio, seconds(from), Some(seconds(to)), sample_rate);
    if audio.first().is_none_or(Vec::is_empty) {
        return Err("That part of the session made no sound.".into());
    }
    niminal_host::render::write_wav(out, &audio, sample_rate).map_err(|e| format!("Could not write {}: {e}", out.display()))?;
    Ok(format!(
        "replayed {} inputs: wrote {} ({:.2}s)",
        entries.len(),
        out.display(),
        audio[0].len() as f64 / f64::from(sample_rate)
    ))
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
}

fn health_from(status: &serde_json::Value) -> Option<Health> {
    let realtime = status.get("realtime").filter(|r| r.is_object())?;
    let count = |name: &str| realtime[name].as_u64().unwrap_or(0);
    Some(Health {
        late_events: count("late_events"),
        starved_frames: count("starved_frames"),
        deadline_misses: count("deadline_misses"),
        capacity_drops: count("capacity_drops"),
    })
}

pub fn snapshot(shared: &Shared) -> Snapshot {
    let Some(mut session) = sequencer(shared) else {
        return Snapshot {
            engine_error: lock(&shared.engine_error).clone(),
            tools_port: *lock(&shared.tools_port),
            ..Snapshot::default()
        };
    };
    let tap = session.take_tap();
    let live = session.live_mut();
    let tempo = live.tempo();
    let health = {
        let mut last = lock(&shared.health);
        if last.as_ref().is_none_or(|(at, _)| at.elapsed() >= HEALTH_EVERY) {
            let health = live.engine_status().ok().and_then(|status| health_from(&status));
            *last = Some((Instant::now(), health));
        }
        last.as_ref().and_then(|(_, health)| health.clone())
    };
    Snapshot {
        connected: true,
        transport: Some(live.transport()),
        beats_per_bar: tempo.beats_per_bar,
        meter: tempo.meter(),
        sample_rate: live.sample_rate(),
        channels: live.channels(),
        // clips named `__…` are the app's own
        clips: live.clips().into_iter().filter(|c| !c.name.starts_with("__")).collect(),
        scenes: live.scenes(),
        pending: live.pending(),
        landed: tap.landed,
        notices: tap.notices.into_iter().chain(lock(&shared.faults).drain(..)).collect(),
        tools_problem: lock(&shared.tools_problem).clone(),
        tools_port: *lock(&shared.tools_port),
        osc_port: lock(&shared.osc).as_ref().map(|(_, port)| *port),
        engine_error: None,
        master: if tap.peaks.is_empty() { vec![0.0; live.channels()] } else { tap.peaks },
        levels: live.take_levels(),
        health,
        learned: tap
            .learned
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
    let begin = with_live(shared, |live| {
        live.set_sample_dir(&dir);
        live.begin_parsed(ParsedEval::from_dir(source, &dir, quantize).with_origin(origin))
    })?;
    let result = match begin {
        Begin::Done(result) => result,
        Begin::Job(mut job) => loop {
            let done = job.compile();
            let mut session = sequencer(shared).ok_or("the engine stopped while the code was compiling")?;
            let live = session.live_mut();
            match live.retry_eval(&done) {
                Some(next) => job = next,
                None => break live.finish_eval(done),
            }
        },
    };
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
    with_live(shared, |live| live.cancel(id))
}

pub fn panic(shared: &Shared) -> Result<(), String> {
    with_live(shared, |live| live.panic())
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub connected: bool,
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
    /// Why the sockets other tools use are not open, if they are not.
    pub tools_problem: Option<String>,
    /// The port other tools reach the stage on.
    pub tools_port: u16,
    /// The UDP port OSC is listened for on, if it is.
    pub osc_port: Option<u16>,
    /// Why the engine is not running, when it is not.
    pub engine_error: Option<String>,
    /// The loudest sample on each output channel since the last snapshot.
    pub master: Vec<f32>,
    /// The same for each node and bus.
    pub levels: Levels,
    /// How the engine is coping, if it plays in real time.
    pub health: Option<Health>,
    /// Controls that took a MIDI control change since the last snapshot, to be written into the code.
    pub learned: Vec<Learned>,
    pub midi: MidiStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_plays_through_an_engine_it_hosts() {
        let shared = Arc::new(Shared::new());
        assert!(!snapshot(&shared).connected);
        shared.no_audio.store(true, Ordering::Relaxed);
        // port 0: any free port, so a running niminal server does not matter
        connect_with(&shared, 0, None).expect("the stage starts its own engine");
        assert!(snapshot(&shared).connected);
        start_worker(shared.clone());

        let source = "instr tone(freq: hz = 440hz) { osc(sine, freq) * env[1 | 20ms 0] }\nclip beat {\n  instrument: tone\n  notes: [c4 c4 c4 c4]\n}\nbeat.start";
        let outcome = eval(&shared, source, None, None, 1).expect("the code is sent");
        assert!(outcome.problems.is_empty(), "{:?}", outcome.problems.iter().map(|p| &p.message).collect::<Vec<_>>());
        // the clip starts on the next bar
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let snap = snapshot(&shared);
            if snap.transport.as_ref().is_some_and(|t| t.voices > 0) {
                break;
            }
            assert!(Instant::now() < deadline, "no voice sounded in the engine the stage hosts");
            std::thread::sleep(Duration::from_millis(50));
        }
        let health = snapshot(&shared).health;
        assert!(health.is_some(), "the engine reports how it is coping");
        // asking to connect again finds the engine already running
        connect_with(&shared, 0, None).unwrap();
        assert!(snapshot(&shared).connected);
    }

    #[test]
    fn code_sent_over_the_socket_plays_in_the_stage_and_both_hear_what_lands() {
        let shared = Arc::new(Shared::new());
        shared.no_audio.store(true, Ordering::Relaxed);
        connect_with(&shared, 0, None).unwrap();
        start_worker(shared.clone());
        let port = lock(&shared.tools).live.as_ref().expect("the stage serves the sequencer").port();

        // what the VS Code extension does: join the main chair, listen, and evaluate a block
        let (mut client, hello) = niminal_ws::Client::connect_as(port, None, "niminal-vscode").expect("the extension reaches the stage");
        // the hello says which release the stage is, so the extension can notice a mismatch (null in a source build)
        assert!(hello.get("result").unwrap_or(&hello).get("release").is_some(), "{hello}");
        client.call("subscribe", serde_json::json!({"topics": ["landed", "transport"]})).unwrap();
        let source = "instr tone(freq: hz = 440hz) { osc(sine, freq) * env[1 | 20ms 0] }\nclip beat {\n  instrument: tone\n  notes: [c4 c4 c4 c4]\n}\nbeat.start";
        client.call("eval", serde_json::json!({"source": source, "quantize": "now"})).expect("the code is accepted");

        // the clip it defined is in the stage's own view of the performance
        let deadline = Instant::now() + Duration::from_secs(8);
        let (mut stage_heard_landing, mut clip_seen) = (false, false);
        while Instant::now() < deadline && !(stage_heard_landing && clip_seen) {
            let snap = snapshot(&shared);
            clip_seen |= snap.clips.iter().any(|c| c.name == "beat");
            stage_heard_landing |= !snap.landed.is_empty();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(clip_seen, "the clip evaluated over the socket shows in the stage");
        assert!(stage_heard_landing, "the stage hears the change land");
        // and the client heard it too: the stage reading the events did not take them from the socket
        client.wait_for("landed", Duration::from_secs(5)).expect("the extension hears the change land");
    }

    #[test]
    fn the_engine_can_be_started_again_on_other_settings_with_the_tools_kept() {
        let shared = Arc::new(Shared::new());
        shared.no_audio.store(true, Ordering::Relaxed);
        connect_with(&shared, 0, None).unwrap();
        start_worker(shared.clone());
        let port = lock(&shared.tools).live.as_ref().unwrap().port();
        let mut client = niminal_ws::Client::connect(port, None).unwrap();
        let source = "instr tone(freq: hz = 440hz) { osc(sine, freq) * env[1 | 20ms 0] }\nclip beat {\n  instrument: tone\n  notes: [c4 c4 c4 c4]\n}\nbeat.start";
        client.call("eval", serde_json::json!({"source": source, "quantize": "now"})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !snapshot(&shared).clips.iter().any(|c| c.name == "beat") {
            assert!(Instant::now() < deadline, "the clip never showed");
            std::thread::sleep(Duration::from_millis(50));
        }

        let state = apply_audio(&shared, AudioChoice { sample_rate: Some(44_100), ..Default::default() })
            .expect("the engine starts again");
        assert_eq!(state.sample_rate, 44_100);
        let snap = snapshot(&shared);
        assert_eq!(snap.sample_rate, 44_100.0);
        assert!(snap.clips.is_empty(), "what was defined goes with the old engine");
        assert!(snap.connected);

        // the socket the extension is on is still open, and what it evaluates plays on the new engine
        client.call("eval", serde_json::json!({"source": source, "quantize": "now"})).expect("the extension is still connected");
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if snapshot(&shared).transport.as_ref().is_some_and(|t| t.voices > 0) {
                break;
            }
            assert!(Instant::now() < deadline, "nothing sounded on the new engine");
            std::thread::sleep(Duration::from_millis(50));
        }
        // and the engine's own socket is open again, on the new engine
        assert!(lock(&shared.tools).engine.is_some());
    }

    #[test]
    fn a_port_that_is_taken_is_reported_and_taken_again_once_it_is_free() {
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = blocker.local_addr().unwrap().port();
        let shared = Arc::new(Shared::new());
        shared.no_audio.store(true, Ordering::Relaxed);
        connect_with(&shared, taken, None).expect("the stage plays whether or not its port is free");
        let problem = snapshot(&shared).tools_problem.expect("the window is told");
        assert!(problem.contains(&taken.to_string()), "{problem}");
        assert!(lock(&shared.tools).live.is_none());

        drop(blocker);
        connect_with(&shared, taken, None).unwrap();
        assert!(lock(&shared.tools).live.is_some(), "the port is taken once it is free");
        // (the engine's port, one above, belongs to whatever else is running, so only this port is checked)
        let problem = snapshot(&shared).tools_problem.unwrap_or_default();
        assert!(!problem.contains(&format!("port {taken} ")), "{problem}");
    }

    #[test]
    fn a_panic_in_the_worker_is_reported_and_the_sequencer_carries_on() {
        let shared = Arc::new(Shared::new());
        shared.no_audio.store(true, Ordering::Relaxed);
        connect_with(&shared, 0, None).unwrap();
        start_worker(shared.clone());
        let before = snapshot(&shared).transport.map_or(0, |t| t.sample);
        shared.panic_once.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut told = false;
        while Instant::now() < deadline && !told {
            told = snapshot(&shared).notices.iter().any(|n| n.contains("internal error"));
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(told, "the window hears of it");
        std::thread::sleep(Duration::from_millis(600));
        assert!(snapshot(&shared).transport.map_or(0, |t| t.sample) > before + 1000, "the worker is still advancing the sequencer");
    }

    #[test]
    fn the_port_and_token_other_tools_use_can_be_changed_once_the_engine_runs() {
        let shared = Arc::new(Shared::new());
        shared.no_audio.store(true, Ordering::Relaxed);
        assert!(tools_apply(&shared, 0, None).is_err(), "there is nothing to reach before the engine starts");
        connect_with(&shared, 0, None).unwrap();
        let open = lock(&shared.tools).live.as_ref().unwrap().port();
        assert!(niminal_ws::Client::connect(open, None).is_ok());

        tools_apply(&shared, 0, Some("secret")).unwrap();
        let guarded = lock(&shared.tools).live.as_ref().unwrap().port();
        assert!(niminal_ws::Client::connect(guarded, None).is_err(), "a client without the token is refused");
        assert!(niminal_ws::Client::connect(guarded, Some("secret")).is_ok());
        assert_eq!(settings_now(&shared).tools_port, 0);
    }

    #[test]
    fn osc_can_be_listened_for_and_stopped() {
        let shared = Arc::new(Shared::new());
        shared.no_audio.store(true, Ordering::Relaxed);
        assert!(osc_listen(&shared, Some(0)).is_err(), "there is nothing to hear it before the engine starts");
        connect_with(&shared, 0, None).unwrap();
        osc_listen(&shared, Some(0)).expect("a free UDP port opens");
        assert!(settings_now(&shared).osc_port.is_some());
        osc_listen(&shared, None).unwrap();
        assert_eq!(settings_now(&shared).osc_port, None);
    }
}
