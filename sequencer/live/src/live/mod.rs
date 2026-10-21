//! The live sequencer: a project that code can be evaluated into.
//!
//! Evaluating code compiles it immediately, and if it compiles the result waits for a musical boundary
//! before landing. Everything one evaluation says lands together (a transaction), at the coarsest
//! boundary among its parts. New writes supersede older pending writes to the same destination;
//! independent changes keep their own boundaries. A snippet that does not compile changes nothing.
//!
//! Nothing here makes sound. The sequencer works a little ahead of the engine's clock: it asks the engine
//! where it is, lands the changes that are due, and sends the notes, parameter changes, definitions and
//! tempo changes that the project says fall in the next stretch, as timestamped events.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use niminal_server::{
    At, ClockInfo, Endpoint, EngineError, EngineReply, EngineRequest, Op, RoutingEdit,
};
use niminal_compose::{
    Action, Clip, CommandAction, Composition, InputSource, PlayInfo, Quantize, Schedule, Scheduled,
    Statement, StatementData, StatementKind, analyze, compile_with,
};
use niminal_lang::{CompileOptions, Entry, Expanded, Layout, Located, Origin, Piece, Problem, Project, Samples};
use niminal_score::{Event, Tempo, Time, Value};

use crate::keyboard::Keyboard;
use crate::link::EngineLink;
use crate::log::{Expression, LiveInput, LiveLogEntry};
use crate::quantize::{Grid, QuantizeDefaults};

mod eval;
mod input;
mod playing;

use eval::options;
pub use eval::{Begin, CancelDone, CancelJob, EvalDone, EvalJob, ParsedEval};
pub use input::ControlSignal;
use playing::{Playing, apply_launch, apply_to_playing, join_edit, mentions_any};

/// A command that has happened, by node name so that it survives the program being replaced.
#[derive(Clone)]
pub(crate) enum NamedAction {
    Play {
        node: String,
        clip: Clip,
    },
    Stop(String),
    Mute(String),
    Unmute(String),
    Solo(String),
    Unsolo(Option<String>),
    Hush,
    Panic,
    /// Stop everything; bars count from here.
    Reset,
    /// Bars count from here. The session starts with one, so a new tempo or meter can stretch it
    /// along with the clips.
    Origin,
}

#[derive(Clone)]
pub(crate) struct HistoryEntry {
    pub at: f64,
    pub action: NamedAction,
}

/// A tempo ramp changes the tempo this many times a beat.
const RAMP_STEPS_PER_BEAT: f64 = 4.0;
const MAX_RAMP_STEPS: f64 = 1024.0;

/// The tag on the notes and parameter changes taken from the schedule, so that they can be taken back
/// and fetched again when the schedule changes.
const SCHEDULED: &str = "scheduled";
/// The tag on notes that were written out in the code.
const WRITTEN: &str = "written";
/// How far ahead of the engine's clock the sequencer works, in seconds.
const LOOKAHEAD_SECONDS: f64 = 0.1;
/// The most events sent to the engine in one request.
const BATCH: usize = 1000;

/// A statement that runs when a change lands, with the program it was compiled in (so landing does no
/// compiling).
pub(crate) struct Compiled {
    pub statement: Statement,
    pub program: Composition,
}

/// An evaluation that has compiled and is waiting to land. Everything that could be slow, such as
/// compiling, was done when it was evaluated, so that landing only has to move things into place.
pub(crate) struct Pending {
    pub id: u64,
    /// The first change of the tempo ramp this is a step of: cancelling any of them cancels the rest.
    pub group: Option<u64>,
    pub lands_at: u64,
    /// What was evaluated, so that the change can be redone if one before it is cancelled.
    pub snippet: String,
    pub statements: Vec<Statement>,
    pub summary: Vec<String>,
    /// The project once this has landed.
    pub project: Project,
    /// The new program, if definitions changed.
    pub program: Option<Arc<Composition>>,
    pub actions: Vec<Compiled>,
    /// Plays to start again because a pattern they use was redefined.
    pub replays: Vec<Compiled>,
    /// What each node is playing once this has landed.
    pub playing: BTreeMap<String, Playing>,
    /// Its changes have been made here, though the engine's clock has not reached it yet.
    pub done: bool,
}

/// What came of an accepted evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    /// `None` if the snippet had nothing to do.
    pub id: Option<u64>,
    /// The sample at which it lands.
    pub lands_at: u64,
    pub position: (u64, f64),
    pub changes: Vec<String>,
}

/// A change that has landed.
#[derive(Debug, Clone, PartialEq)]
pub struct Landed {
    pub id: u64,
    pub at: u64,
    pub position: (u64, f64),
    pub changes: Vec<String>,
}

/// A change waiting to land.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingInfo {
    pub id: u64,
    pub lands_at: u64,
    /// Samples until it lands.
    pub in_samples: u64,
    pub position: (u64, f64),
    pub changes: Vec<String>,
}

/// A node as an editor shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct PartStatus {
    pub name: String,
    /// The clip playing on it now.
    pub clip: Option<String>,
    /// What the node was told to play (base clip text before live edits).
    pub playing: Option<String>,
    pub muted: bool,
    pub soloed: bool,
    /// The changes made to it live (`reverse`, `thin(25%)`), oldest first.
    pub edits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transport {
    pub sample: u64,
    pub seconds: f64,
    pub bar: u64,
    pub beat: f64,
    pub bpm: f64,
    pub voices: usize,
    pub pending: usize,
}

/// A note written out in the code, kept for `capture`.
struct Written {
    start: u64,
    dur: u64,
    event: Event,
}

/// What the engine has been asked to do about a change that is waiting to land.
struct Asked {
    id: u64,
    lands_at: u64,
}

pub struct Live {
    link: Box<dyn EngineLink>,
    clock_names: [String; 3],
    sample_rate: f32,
    layout: Layout,
    /// Where the project's sample files are found, and the ones already read.
    samples: Samples,
    sample_dirs: Vec<std::path::PathBuf>,
    defaults: QuantizeDefaults,
    /// The engine's clock when it was last asked.
    clock: u64,
    /// How far ahead of the clock events are sent, in samples.
    lookahead: u64,
    engine: Option<ClockInfo>,
    /// The notes held open by a keyboard, by the id the engine knows them by, and the instrument of each.
    held: HashMap<u64, String>,
    keyboard: Keyboard,
    /// The control change each `midi.learn` control took, and what is waiting to be reported.
    learned: BTreeMap<String, u8>,
    learned_out: Vec<(String, String)>,

    project: Project,
    program: Arc<Composition>,
    history: Vec<HistoryEntry>,
    schedule: Schedule,
    /// Notes and parameter changes up to this sample have been taken from the schedule and sent.
    fetched_until: u64,
    /// Nodes whose clips are not searched for notes because they asked for too many.
    overloaded: Vec<usize>,
    /// How many of the schedule's panics have been sent.
    next_panic: usize,
    written: Vec<Written>,
    /// Changes of definitions or the tempo that the engine has been asked to make, in order.
    asked: Vec<Asked>,

    pending: Vec<Pending>,
    next_id: u64,
    /// What each node was last told to play, as source, so that redefining a pattern can start the new
    /// version.
    playing: BTreeMap<String, Playing>,
    landed: Vec<Landed>,
    notices: Vec<String>,
    recent_notices: Vec<(String, u64)>,
    /// Changes whenever what is waiting to land changes, so that an evaluation compiled in the meantime can
    /// tell it is out of date.
    version: u64,
    log: Vec<LiveLogEntry>,
    peaks: Vec<f32>,
}

/// The most notices that wait to be collected, and how long before the same one is said again.
const MAX_NOTICES: usize = 100;
const NOTICE_REPEAT_SECONDS: u64 = 10;

impl Live {
    /// A sequencer for an engine that outputs `layout` at `sample_rate`.
    pub fn new(link: Box<dyn EngineLink>, sample_rate: f32, layout: Layout) -> Live {
        let program = compile_with("", &options(layout, &Samples::default()))
            .expect("an empty project compiles");
        let schedule = program.schedule();
        Live {
            link,
            clock_names: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ["bpm", "origin", "meter"]
                    .map(|name| format!("__live_{}_{}_{name}", std::process::id(), id))
            },
            sample_rate,
            layout,
            samples: Samples::default(),
            sample_dirs: Vec::new(),
            defaults: QuantizeDefaults::default(),
            clock: 0,
            lookahead: (f64::from(sample_rate) * LOOKAHEAD_SECONDS) as u64,
            engine: None,
            held: HashMap::new(),
            keyboard: Keyboard::default(),
            learned: BTreeMap::new(),
            learned_out: Vec::new(),
            project: Project::default(),
            program: Arc::new(program),
            history: vec![HistoryEntry {
                at: 0.0,
                action: NamedAction::Origin,
            }],
            schedule,
            fetched_until: 0,
            overloaded: Vec::new(),
            next_panic: 0,
            written: Vec::new(),
            asked: Vec::new(),
            pending: Vec::new(),
            next_id: 1,
            playing: BTreeMap::new(),
            landed: Vec::new(),
            notices: Vec::new(),
            recent_notices: Vec::new(),
            version: 0,
            log: Vec::new(),
            peaks: vec![0.0; layout.channels()],
        }
    }

    pub fn with_defaults(mut self, defaults: QuantizeDefaults) -> Live {
        self.defaults = defaults;
        self
    }

    /// Work this far ahead of the engine's clock, in seconds.
    pub fn with_lookahead(mut self, seconds: f64) -> Live {
        self.lookahead = (f64::from(self.sample_rate) * seconds) as u64;
        self
    }

    pub fn lookahead(&self) -> u64 {
        self.lookahead
    }

    pub fn channels(&self) -> usize {
        self.layout.channels()
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// The engine's clock when the sequencer last looked.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    pub fn program(&self) -> &Composition {
        &self.program
    }

    /// The sample files this project has read.
    pub fn sample_files(&self) -> &Samples {
        &self.samples
    }

    /// Find the paths in `sample` and `kit` declarations from `dir`. Folders given earlier are still
    /// searched, after this one.
    pub fn set_sample_dir(&mut self, dir: &std::path::Path) {
        if self.sample_dirs.first().is_none_or(|d| d != dir) {
            self.sample_dirs.retain(|d| d != dir);
            self.sample_dirs.insert(0, dir.to_path_buf());
            self.samples = self.samples.with_bases(self.sample_dirs.clone());
        }
    }

    fn grid(&self) -> Grid {
        Grid {
            sample_rate: f64::from(self.sample_rate),
            bpm: self.program.tempo.bpm,
            beats_per_bar: self.program.tempo.beats_per_bar,
            origin: (origin_of(&self.history) * f64::from(self.sample_rate)).round() as i64,
        }
    }

    fn seconds(&self, sample: u64) -> f64 {
        sample as f64 / f64::from(self.sample_rate)
    }

    fn samples(&self, seconds: f64) -> u64 {
        (seconds * f64::from(self.sample_rate)).round().max(0.0) as u64
    }

    // ---- the engine ------------------------------------------------------------------------

    /// Send the engine events, a batch at a time. If a batch is refused, its events are sent one by one so
    /// that one that can't play doesn't keep the rest from sounding.
    fn send(&mut self, ops: Vec<Op>) {
        for batch in ops.chunks(BATCH) {
            match self.link.request(EngineRequest::EventsSend {
                events: batch.to_vec(),
            }) {
                Ok(_) => {}
                Err(_) if batch.len() > 1 => {
                    for op in batch {
                        if let Err(e) = self.link.request(EngineRequest::EventsSend {
                            events: vec![op.clone()],
                        }) {
                            self.notify(describe(&e));
                        }
                    }
                }
                Err(e) => self.notify(describe(&e)),
            }
        }
    }

    /// Take back the events the engine is waiting on that carry `tag`, from sample `from`.
    fn take_back(&mut self, tag: &str, from: u64) {
        let request = EngineRequest::EventsCancel {
            tag: Some(tag.to_string()),
            from: Some(At::Sample(from)),
            release: false,
        };
        if let Err(e) = self.link.request(request) {
            self.notify(describe(&e));
        }
    }

    /// Ask the engine where it is and take in what it has to say.
    fn look(&mut self) -> bool {
        match self.link.request(EngineRequest::Clock {}) {
            Ok(EngineReply::Clock(info)) => {
                self.clock = self.clock.max(info.sample);
                self.engine = Some(info);
            }
            Ok(_) => {}
            Err(e) => {
                self.notify(describe(&e));
                return false;
            }
        }
        for (method, params) in self.link.notifications() {
            match method.as_str() {
                "notice" => {
                    if let Some(message) = params["message"].as_str() {
                        self.notify(message.to_string());
                    }
                }
                "meters" => {
                    if let Some(peaks) = params["peaks"].as_array() {
                        for (mine, theirs) in self.peaks.iter_mut().zip(peaks) {
                            *mine = mine.max(theirs.as_f64().unwrap_or(0.0) as f32);
                        }
                    }
                }
                _ => {}
            }
        }
        true
    }

    /// Do what is due: land the changes that fall within the lookahead, and send the notes and parameter
    /// changes up to its end. Call it often. It is what makes the music play.
    pub fn advance(&mut self) {
        self.advance_by(None);
    }

    /// Like [`Live::advance`], but works ahead to sample `horizon`, however far that is. For an offline render,
    /// where there is no hurry and the whole of the next stretch can be sent at once.
    pub fn advance_to(&mut self, horizon: u64) {
        self.advance_by(Some(horizon));
    }

    fn advance_by(&mut self, horizon: Option<u64>) {
        if !self.look() {
            return;
        }
        let horizon = horizon
            .unwrap_or(self.clock + self.lookahead)
            .max(self.clock);
        // the engine knows of every change before it is sent notes that depend on one
        self.sync_pending();
        while let Some(i) = self
            .pending
            .iter()
            .position(|p| !p.done && p.lands_at <= horizon)
        {
            // what falls before the change is as the project was
            self.fetch_until(self.pending[i].lands_at);
            self.land(i);
            self.sync_pending();
        }
        self.fetch_until(horizon);
        let clock = self.clock;
        self.pending.retain(|p| !p.done || p.lands_at > clock);
        self.asked.retain(|a| a.lands_at > clock);
        self.written
            .retain(|w| w.start + w.dur + 1 > clock.saturating_sub(self.sample_rate as u64 * 64));
        self.sync_pending();
    }

    // ---- state ----------------------------------------------------------------

    pub fn pending(&self) -> Vec<PendingInfo> {
        let grid = self.grid();
        self.pending
            .iter()
            .filter(|p| !p.done)
            .map(|p| PendingInfo {
                id: p.id,
                lands_at: p.lands_at,
                in_samples: p.lands_at.saturating_sub(self.clock),
                position: grid.position(p.lands_at),
                changes: p.summary.clone(),
            })
            .collect()
    }

    pub fn transport(&self) -> Transport {
        let grid = self.grid();
        let (bar, beat) = grid.position(self.clock);
        Transport {
            sample: self.clock,
            seconds: self.seconds(self.clock),
            bar,
            beat,
            bpm: grid.bpm,
            voices: self.engine.as_ref().map_or(0, |e| e.voices),
            pending: self.pending.iter().filter(|p| !p.done).count(),
        }
    }

    /// The project's nodes and what they are doing now.
    pub fn parts(&self) -> Vec<PartStatus> {
        let now = self.seconds(self.clock);
        self.program
            .nodes
            .iter()
            .enumerate()
            .skip(1)
            // nodes that only process buses are not parts to play
            .filter(|(_, t)| t.instrument.is_some())
            .map(|(i, t)| {
                let state = self.schedule.node_state(i, now);
                let record = self.playing.get(&t.name);
                PartStatus {
                    name: t.name.clone(),
                    clip: state.clip,
                    playing: record.map(|p| p.base.clone()),
                    muted: state.muted,
                    soloed: state.soloed,
                    edits: record.map(|p| p.edits.clone()).unwrap_or_default(),
                }
            })
            .collect()
    }

    /// The names of the project's definitions of one kind (`scene`, `clip`...), in order.
    pub fn defined(&self, kind: &str) -> Vec<String> {
        let prefix = format!("{kind}:");
        self.project
            .entries
            .iter()
            .filter_map(|e| e.key.strip_prefix(&prefix))
            .map(str::to_string)
            .collect()
    }

    /// Changes that have landed since this was last called.
    pub fn take_landed(&mut self) -> Vec<Landed> {
        let clock = self.clock;
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.landed)
            .into_iter()
            .partition(|l| l.at <= clock);
        self.landed = later;
        due
    }

    /// Queue a notice, unless the same one is already waiting: a problem that recurs every fraction of a
    /// second must not bury everything else.
    pub(crate) fn notify(&mut self, notice: String) {
        let quiet_until = u64::from(self.sample_rate as u32) * NOTICE_REPEAT_SECONDS;
        self.recent_notices
            .retain(|(_, at)| self.clock.saturating_sub(*at) < quiet_until);
        if self.notices.len() < MAX_NOTICES
            && !self.recent_notices.iter().any(|(n, _)| *n == notice)
        {
            self.recent_notices.push((notice.clone(), self.clock));
            self.notices.push(notice);
        }
    }

    /// Things worth telling the user that aren't errors in what they sent: a note that couldn't play, a
    /// change that failed to land.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// Ask the engine for its meters, or not. Reading them resets them, so it is only done for someone who listens.
    pub fn set_metering(&mut self, on: bool) {
        let topics = vec!["meters".to_string()];
        let request = if on {
            EngineRequest::Subscribe { topics }
        } else {
            EngineRequest::Unsubscribe { topics }
        };
        let _ = self.link.request(request);
    }

    /// The loudest sample on each output channel since this was last called.
    pub fn take_peaks(&mut self) -> Vec<f32> {
        std::mem::replace(&mut self.peaks, vec![0.0; self.layout.channels()])
    }

    /// Everything that was sent to the sequencer, with when.
    pub fn log(&self) -> &[LiveLogEntry] {
        &self.log
    }

    /// The engine's own account of itself, for a status that wants it.
    pub fn engine_status(&mut self) -> serde_json::Value {
        match self.link.request(EngineRequest::Status {}) {
            Ok(EngineReply::Status(mut v)) => {
                let client = v["client_id"].as_u64().unwrap_or(0);
                if let Some(controls) = v["controls"].as_array_mut() {
                    controls.retain_mut(|control| {
                        let Some(name) = control["name"]
                            .as_str()
                            .and_then(|n| self.control_name(n, client))
                        else {
                            return false;
                        };
                        control["name"] = serde_json::Value::String(name);
                        true
                    });
                }
                v
            }
            _ => serde_json::json!({}),
        }
    }

    pub(crate) fn control_name(&self, name: &str, client: u64) -> Option<String> {
        let name = name
            .strip_prefix(&format!("__client_{client}::"))
            .unwrap_or(name);
        if !name.starts_with("__routing_") {
            return Some(name.to_string());
        }
        self.program.nodes.iter().skip(1).find_map(|node| {
            let prefix = format!("__routing_node_{client}_{}::", routing_id(&node.name));
            name.strip_prefix(&prefix)
                .map(|param| format!("{}::{param}", node.name))
        })
    }

    // ---- the project --------------------------------------------------------------

    /// The project and what is playing as they will be once everything waiting has landed.
    fn projected(&self) -> (Project, BTreeMap<String, Playing>) {
        match self.pending.last() {
            Some(p) => (p.project.clone(), p.playing.clone()),
            None => (self.project.clone(), self.playing.clone()),
        }
    }

    // ---- landing ------------------------------------------------------------------------

    /// Make the changes of pending change `index`, which lands at its own sample.
    fn land(&mut self, index: usize) {
        self.version += 1;
        let at = self.pending[index].lands_at;
        if std::env::var_os("NIMINAL_DEBUG_NOTES").is_some() {
            eprintln!("LAND {} {:?}", at, self.pending[index].summary);
        }
        self.pending[index].done = true;
        let (id, statements, summary, project, program, actions, replays, playing) = {
            let p = &mut self.pending[index];
            (
                p.id,
                p.statements.clone(),
                p.summary.clone(),
                p.project.clone(),
                p.program.clone(),
                std::mem::take(&mut p.actions),
                std::mem::take(&mut p.replays),
                p.playing.clone(),
            )
        };
        let _ = statements;
        if let Some(program) = program {
            let now = self.seconds(at);
            retime(&mut self.history, now, self.program.tempo, program.tempo);
            let (old, new) = (self.program.tempo, program.tempo);
            let mut pending = std::mem::take(&mut self.pending);
            rescale_pending(&mut pending, at, old, new, |p| self.landing_unit_is_bar(p));
            self.pending = pending;
            self.program = program;
            self.project = project;
        }
        let mut captures = Vec::new();
        let mut ops = Vec::new();
        for a in actions.iter().chain(&replays) {
            captures.extend(self.run_action(a, at, &mut ops));
        }
        self.send(ops);
        self.playing = playing;

        self.rebuild_schedule(at);
        self.send(self.clock_ops(at));
        let mut changes = summary;
        for (bars, name) in captures {
            if let Some(notice) = self.capture_section(bars, &name, at) {
                changes.push(notice);
            }
        }
        let position = self.grid().position(at);
        self.landed.push(Landed {
            id,
            at,
            position,
            changes,
        });
    }

    /// Carry out one `play`, `mute`, note... statement that has landed. It was compiled when it was
    /// evaluated. Returns any captures to run after the schedule is rebuilt.
    fn run_action(&mut self, action: &Compiled, at: u64, ops: &mut Vec<Op>) -> Vec<(f64, String)> {
        let (statement, program) = (&action.statement, &action.program);
        let tempo = program.tempo;
        let landing_seconds = self.seconds(at);
        let origin = origin_of(&self.history);
        let mut captures = Vec::new();

        for event in &program.notes {
            let offset = event.at.to_seconds(tempo);
            let start = if statement.has_at() {
                self.samples(origin + offset)
            } else {
                at + self.samples(offset)
            };
            let dur = self.samples(event.dur.to_seconds(tempo));
            self.written.push(Written {
                start: start.max(at),
                dur,
                event: event.clone(),
            });
            if let Some(op) = self.note_op(event, start.max(at), dur, WRITTEN) {
                ops.push(op);
            }
        }
        for scheduled in &program.performance {
            if let Action::Capture { bars, name } = &scheduled.action {
                captures.push((*bars, name.clone()));
                continue;
            }
            let after = scheduled.at.to_seconds(tempo);
            let when = if statement.has_at() {
                origin + after
            } else {
                landing_seconds + after
            };
            let Some(named) = name_action(program, &scheduled.action) else {
                continue;
            };
            self.history.push(HistoryEntry {
                at: when,
                action: named,
            });
        }
        captures
    }

    /// A note for the engine, if it can play in the project as it is now.
    fn note_op(&mut self, event: &Event, start: u64, dur: u64, tag: &str) -> Option<Op> {
        if let Err(e) = self.program.plan(event) {
            self.notify(format!("note for `{}` could not play: {e}", event.target));
            return None;
        }
        Some(Op::Note {
            to: None,
            target: if self.program.node_index(&event.target).is_some() {
                routing_id(&event.target)
            } else {
                event.target.clone()
            },
            at: At::Sample(start),
            args: self.program.dsp_args(event).expect("event validated above"),
            dur: Some(Time::Seconds(dur as f64 / f64::from(self.sample_rate))),
            id: None,
            tag: Some(tag.to_string()),
        })
    }

    /// Write the notes sounding in the last `bars` bars to `score/{name}.nms`.
    fn capture_section(&self, bars: f64, name: &str, at: u64) -> Option<String> {
        let tempo = self.program.tempo;
        let now = self.seconds(at);
        let length = Time::Bars(bars).to_seconds(tempo);
        let from = (now - length).max(0.0);
        let found = self.schedule.events_lenient(from, now, &[]);
        let mut events = found.events;
        // Notes written out (not from a clip) that started in the window.
        for w in &self.written {
            let start = self.seconds(w.start);
            if start < from || start >= now {
                continue;
            }
            let mut event = w.event.clone();
            event.at = Time::Seconds(start);
            event.dur = Time::Seconds(self.seconds(w.dur));
            events.push(event);
        }
        events.sort_by(|a, b| {
            a.at.to_seconds(tempo)
                .partial_cmp(&b.at.to_seconds(tempo))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // Re-base times so the section starts at 0, and prefer beats for editing.
        let beat = |sec: f64| Time::Beats(sec * tempo.bpm / 60.0);
        for event in &mut events {
            let at = event.at.to_seconds(tempo) - from;
            let dur = event.dur.to_seconds(tempo);
            event.at = beat(at);
            event.dur = beat(dur);
        }
        let section = niminal_compose::Section {
            name: name.to_string(),
            length: Time::Bars(bars),
            events,
        };
        let dir = std::path::Path::new("score");
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Some(format!("capture failed: could not create score/: {e}"));
        }
        let path = dir.join(format!("{name}.nms"));
        let count = section.events.len();
        if let Err(e) = std::fs::write(&path, section.to_json()) {
            return Some(format!(
                "capture failed: could not write {}: {e}",
                path.display()
            ));
        }
        Some(format!("wrote {} ({count} events)", path.display()))
    }

    /// Whether a pending change waits for a bar line, not a beat.
    fn landing_unit_is_bar(&self, p: &Pending) -> bool {
        use niminal_compose::QuantizeUnit;
        p.statements.iter().any(|s| {
            matches!(
                self.quantums().for_statement(s, None).unit,
                QuantizeUnit::Bar | QuantizeUnit::Cycle
            )
        })
    }

    /// The schedule is made again from the project as it is at `at`, and what was sent for after that
    /// is taken back, to be fetched again.
    fn rebuild_schedule(&mut self, at: u64) {
        self.schedule = schedule_for(&self.program, &self.history);
        self.overloaded.clear();
        if self.fetched_until > at {
            self.take_back(SCHEDULED, at);
        }
        self.fetched_until = at;
        self.next_panic = self
            .schedule
            .panics()
            .iter()
            .take_while(|&&t| self.samples(t) < at)
            .count();
    }

    // ---- sending ---------------------------------------------------------------------------

    /// Take the notes, parameter changes and panics that fall before `horizon` from the schedule and send
    /// them.
    fn fetch_until(&mut self, horizon: u64) {
        // a long stretch is searched a piece at a time, so that a search that finds too much doesn't drop the rest
        while self.fetched_until < horizon {
            let piece = (self.fetched_until + self.sample_rate as u64 / 2).min(horizon);
            self.fetch_piece(piece);
        }
    }

    fn fetch_piece(&mut self, horizon: u64) {
        let (from, to) = (self.seconds(self.fetched_until), self.seconds(horizon));
        let found = self.schedule.events_lenient(from, to, &self.overloaded);
        for node in found.overloaded {
            if !self.overloaded.contains(&node) {
                self.overloaded.push(node);
            }
        }
        let mut ops = Vec::new();
        for origin in self.history.iter().filter(|h| {
            h.at >= from
                && h.at < to
                && matches!(h.action, NamedAction::Reset | NamedAction::Origin)
        }) {
            ops.extend(self.clock_ops(self.samples(origin.at)));
        }
        // panics
        let panics = self.schedule.panics().to_vec();
        while let Some(&t) = panics.get(self.next_panic) {
            let at = self.samples(t);
            if at >= horizon {
                break;
            }
            ops.push(Op::Panic {
                at: At::Sample(at),
                tag: Some(SCHEDULED.into()),
            });
            // notes written out before a panic that start after it never sound
            self.take_back(WRITTEN, at);
            self.next_panic += 1;
        }
        // envelope lanes moving node parameters
        for set in found.controls {
            let Some(def) = self.program.controls.iter().find(|c| c.name == set.name) else {
                continue;
            };
            let Some((node, param)) = set.name.split_once("::") else {
                continue;
            };
            let value = match def.unit {
                niminal_lang::Unit::Hz => Value::Hz(set.target),
                // the engine takes a bare number as a linear gain
                niminal_lang::Unit::Db | niminal_lang::Unit::Num => Value::Num(set.target),
                niminal_lang::Unit::Time => Value::Seconds(set.target),
                niminal_lang::Unit::Angle => Value::Degrees(set.target),
                niminal_lang::Unit::Semitones => Value::Semitones(set.target),
            };
            ops.push(Op::Routing {
                at: At::Sample(self.samples(set.at)),
                edits: vec![RoutingEdit::Set {
                    id: routing_id(node),
                    args: [(param.to_string(), value)].into(),
                    glide: set.glide,
                }],
                tag: Some(SCHEDULED.into()),
            });
        }
        let (events, problem) = (found.events, found.problem);
        for event in events {
            let start = self.samples(event.at.to_seconds(self.program.tempo));
            let dur = self.samples(event.dur.to_seconds(self.program.tempo));
            if let Some(op) = self.note_op(&event, start, dur, SCHEDULED) {
                ops.push(op);
            }
        }
        if let Some(e) = problem {
            self.notify(e.to_string());
        }
        self.fetched_until = horizon;
        self.send(ops);
    }

    /// Keep the engine's pending definitions and routing in sync with the client, at
    /// the samples they land at, and no others.
    fn sync_pending(&mut self) {
        let wanted: Vec<(u64, u64)> = self
            .pending
            .iter()
            .filter(|p| p.program.is_some() || p.statements.iter().any(Statement::clears))
            .map(|p| (p.id, p.lands_at))
            .collect();
        let keep = self
            .asked
            .iter()
            .zip(&wanted)
            .take_while(|(a, w)| a.id == w.0 && a.lands_at == w.1)
            .count();
        // a change may depend on the ones before it, so everything from the first that differs is asked again
        let stale: Vec<u64> = self.asked.drain(keep..).map(|a| a.id).collect();
        for id in stale.into_iter().rev() {
            self.take_back(&change_tag(id), 0);
        }
        for &(id, lands_at) in &wanted[keep..] {
            let Some(index) = self.pending.iter().position(|p| p.id == id) else {
                continue;
            };
            let p = &self.pending[index];
            let mut ops =
                self.definition_ops(&p.statements, p.program.as_deref(), At::Sample(p.lands_at));
            for op in &mut ops {
                tag_op(op, &change_tag(id));
            }
            match self.link.request(EngineRequest::EventsSend { events: ops }) {
                Ok(_) => self.asked.push(Asked { id, lands_at }),
                Err(e) => {
                    // the engine would not take it, so it must not land here either
                    self.notify(format!(
                        "change {id} was refused by the engine: {}",
                        describe(&e)
                    ));
                    self.pending.remove(index);
                    self.version += 1;
                    return;
                }
            }
        }
    }

    fn definition_ops(
        &self,
        statements: &[Statement],
        program: Option<&Composition>,
        at: At,
    ) -> Vec<Op> {
        let mut ops = definition_ops(statements, program, at, self.sample_dirs.first());
        for op in &mut ops {
            if let Op::Define { source, .. } = op {
                let names = [
                    niminal_compose::LIVE_BPM,
                    niminal_compose::LIVE_ORIGIN,
                    niminal_compose::LIVE_METER,
                ];
                let tempo = program.map_or(self.program.tempo, |p| p.tempo);
                let values = [
                    tempo.bpm.to_string(),
                    format!(
                        "{}sec",
                        self.clock_origin(match at {
                            At::Sample(n) => n,
                            At::Now => self.clock,
                        })
                    ),
                    tempo.beats_per_bar.to_string(),
                ];
                let mut controls = String::new();
                for ((old, name), value) in names.iter().zip(&self.clock_names).zip(values) {
                    if source.contains(old) {
                        *source = source.replace(old, name);
                        controls.push_str(&format!("ctl {name} = {value}\n"));
                    }
                }
                source.insert_str(0, &controls);
            }
        }
        if let Some(program) = program {
            let mut edits = Vec::new();
            let mut nodes = Vec::new();
            let mut buses = Vec::new();
            let mut connections = Vec::new();
            for (name, layout) in program.buses.iter().zip(&program.bus_layouts) {
                let id = routing_id(name);
                buses.push(id.clone());
                edits.push(RoutingEdit::Bus {
                    id,
                    channels: layout.channels(),
                    shared: false,
                    source: Some(name.clone()),
                });
            }
            for node in program.nodes.iter().skip(1) {
                let id = routing_id(&node.name);
                nodes.push(id.clone());
                edits.push(RoutingEdit::Import {
                    name: id.clone(),
                    node: node.name.clone(),
                });
                edits.push(RoutingEdit::Node {
                    id: id.clone(),
                    processor: Some(id.clone()),
                    instrument: None,
                    args: BTreeMap::new(),
                });
                let connection = format!("output_{id}");
                connections.push(connection.clone());
                let to = match node.def.route {
                    niminal_engine::Route::Master => Endpoint::Master,
                    niminal_engine::Route::Bus(bus) => Endpoint::Bus {
                        id: routing_id(&program.buses[bus]),
                        owner: None,
                    },
                };
                edits.push(RoutingEdit::Connect {
                    id: connection,
                    from: Endpoint::Node { id },
                    to,
                    gain: 1.0,
                    glide: 0.0,
                    delayed: false,
                });
            }
            edits.push(RoutingEdit::Retain {
                processors: nodes.clone(),
                nodes,
                buses,
                connections,
            });
            let (source, dir) =
                if let Some(i) = ops.iter().position(|op| matches!(op, Op::Define { .. })) {
                    let Op::Define { source, dir, .. } = ops.remove(i) else {
                        unreachable!()
                    };
                    (source, dir)
                } else {
                    (String::new(), None)
                };
            ops.push(Op::Transaction {
                at,
                source,
                dir,
                edits,
                tag: None,
            });
        }
        ops
    }

    fn clock_origin(&self, at: u64) -> f64 {
        self.history
            .iter()
            .rev()
            .find(|h| {
                self.samples(h.at) <= at
                    && matches!(h.action, NamedAction::Reset | NamedAction::Origin)
            })
            .map_or(0.0, |h| h.at)
    }

    fn clock_ops(&self, at: u64) -> Vec<Op> {
        let tempo = self.program.tempo;
        let source = niminal_compose::dsp_source(&self.program.source, tempo)
            .expect("compiled source projects");
        let values = [tempo.bpm, self.clock_origin(at), tempo.beats_per_bar];
        [
            niminal_compose::LIVE_BPM,
            niminal_compose::LIVE_ORIGIN,
            niminal_compose::LIVE_METER,
        ]
        .iter()
        .enumerate()
        .filter(|(_, name)| source.contains(**name))
        .map(|(i, _)| Op::Control {
            name: self.clock_names[i].clone(),
            at: At::Sample(at),
            value: values[i],
            unit: None,
            glide: Some(Time::Seconds(0.0)),
            tag: Some(SCHEDULED.into()),
        })
        .collect()
    }

    /// Play `program` as it is compiled: the notes it writes out, and `schedule` (usually
    /// `program.schedule()`) for the rest. This is how a file is rendered offline.
    pub fn play_program(&mut self, program: Composition, schedule: Schedule) {
        self.look();
        for dir in program.samples.bases().iter().rev() {
            self.set_sample_dir(dir);
        }
        let at = self.clock;
        if let Ok(statements) = analyze(&program.source) {
            let ops = self.definition_ops(&statements, Some(&program), At::Sample(at));
            if !ops.is_empty() {
                self.send(ops);
            }
        }
        self.program = Arc::new(program);
        self.schedule = schedule;
        self.fetched_until = at;
        self.next_panic = 0;
        let tempo = self.program.tempo;
        let mut ops = Vec::new();
        for event in self.program.notes.clone() {
            let start = self.samples(event.at.to_seconds(tempo)).max(at);
            let dur = self.samples(event.dur.to_seconds(tempo));
            self.written.push(Written {
                start,
                dur,
                event: event.clone(),
            });
            ops.extend(self.note_op(&event, start, dur, WRITTEN));
        }
        self.send(ops);
    }

    /// Start no more notes from the schedule; what is sounding carries on. Notes already sent from it that
    /// are still to start are taken back.
    pub fn end_schedule(&mut self) {
        let names = self.program.nodes.iter().map(|t| t.name.clone()).collect();
        self.schedule = Schedule::from_commands(self.program.tempo, names, &[]);
        self.take_back(SCHEDULED, self.clock);
        self.fetched_until = self.clock;
        self.next_panic = 0;
    }

    /// Notes from outside, such as a score: each carries the time (since the start) at which it should sound.
    pub fn send_notes(&mut self, events: &[Event]) -> Result<(), Vec<Problem>> {
        let tempo = self.program.tempo;
        let ops: Vec<Op> = events
            .iter()
            .map(|e| {
                Ok(Op::Note {
                    to: None,
                    target: if self.program.node_index(&e.target).is_some() {
                        routing_id(&e.target)
                    } else {
                        e.target.clone()
                    },
                    at: At::Sample(self.samples(e.at.to_seconds(tempo))),
                    args: self
                        .program
                        .dsp_args(e)
                        .map_err(|e| vec![Problem::plain(e.to_string())])?,
                    dur: Some(Time::Seconds(e.dur.to_seconds(tempo))),
                    id: None,
                    tag: Some(WRITTEN.into()),
                })
            })
            .collect::<Result<_, Vec<Problem>>>()?;
        self.link
            .request(EngineRequest::EventsSend { events: ops })
            .map(|_| ())
            .map_err(|e| {
                let mut problems: Vec<Problem> =
                    e.problems().into_iter().map(Problem::plain).collect();
                if problems.is_empty() {
                    problems.push(Problem::plain(e.message));
                }
                problems
            })
    }
}

/// Send DSP definitions only. Implicit clip nodes and musical literals are resolved in this client.
fn definition_ops(
    statements: &[Statement],
    program: Option<&Composition>,
    at: At,
    dir: Option<&std::path::PathBuf>,
) -> Vec<Op> {
    let mut ops = Vec::new();
    if statements.iter().any(Statement::clears) {
        ops.push(Op::Forget { at, tag: None });
    }
    if let Some(program) = program {
        let source = niminal_compose::dsp_source(&program.source, program.tempo)
            .expect("compiled source can be projected to DSP");
        if !source.is_empty() {
            ops.push(Op::Define {
                source,
                at,
                dir: dir.map(|d| d.display().to_string()),
                tag: None,
            });
        }
    }
    ops
}

pub(super) fn routing_id(name: &str) -> String {
    format!(
        "live_{}",
        name.as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn change_tag(id: u64) -> String {
    format!("change-{id}")
}

fn tag_op(op: &mut Op, tag: &str) {
    match op {
        Op::Define { tag: t, .. }
        | Op::Forget { tag: t, .. }
        | Op::Routing { tag: t, .. }
        | Op::Transaction { tag: t, .. } => *t = Some(tag.to_string()),
        _ => {}
    }
}

/// What the engine said was wrong, in a sentence.
fn describe(error: &EngineError) -> String {
    let problems = error.problems();
    if problems.is_empty() {
        format!("the engine: {}", error.message)
    } else {
        format!("the engine: {}", problems.join("; "))
    }
}

/// Where a change that waits for a boundary lands once the tempo or meter has changed at `now`: the same
/// number of bars (or beats) on. Changes that are themselves a tempo or meter keep their time, as the steps
/// of a ramp are spaced by the tempo they pass through.
pub(super) fn rescale_landing(lands_at: u64, now: u64, factor: f64) -> u64 {
    now + (lands_at.saturating_sub(now) as f64 * factor).round() as u64
}

/// How much longer a bar (`by_bar`) or a beat gets when the tempo or meter changes from `old` to `new`.
pub(super) fn landing_factor(by_bar: bool, old: Tempo, new: Tempo) -> f64 {
    if by_bar {
        (new.beats_per_bar / new.bpm) / (old.beats_per_bar / old.bpm)
    } else {
        old.bpm / new.bpm
    }
}

fn is_timing(p: &Pending) -> bool {
    p.statements
        .iter()
        .any(|s| matches!(s.key(), Some("tempo" | "meter")))
}

fn rescale_pending(
    pending: &mut [Pending],
    now: u64,
    old: Tempo,
    new: Tempo,
    by_bar: impl Fn(&Pending) -> bool,
) {
    if old == new {
        return;
    }
    for p in pending.iter_mut().filter(|p| !p.done && !is_timing(p)) {
        p.lands_at = rescale_landing(p.lands_at, now, landing_factor(by_bar(p), old, new));
    }
    pending.sort_by_key(|p| p.lands_at);
}

/// A tempo or meter change keeps every clip where it is in its bar: what has happened is stretched, and
/// what is still to come, `at bar 9` for one, is stretched the same way around `now`. So are the bar
/// lines, which keeps the bar and beat that `now` is at.
fn retime(history: &mut [HistoryEntry], now: f64, old: Tempo, new: Tempo) {
    let (was, is) = (old.beats_per_bar / old.bpm, new.beats_per_bar / new.bpm);
    if was == is {
        return;
    }
    let ratio = is / was;
    for entry in history {
        entry.at = now + (entry.at - now) * ratio;
    }
}

/// When bar 1 is, in session seconds: where the last `reset` landed.
pub(crate) fn origin_of(history: &[HistoryEntry]) -> f64 {
    history
        .iter()
        .rev()
        .find(|h| matches!(h.action, NamedAction::Reset | NamedAction::Origin))
        .map_or(0.0, |h| h.at)
}

pub(crate) fn name_action(program: &Composition, action: &Action) -> Option<NamedAction> {
    let name = |t: &usize| program.nodes[*t].name.clone();
    Some(match action {
        Action::Play { node, clip } => NamedAction::Play {
            node: name(node),
            clip: clip.clone(),
        },
        Action::Stop { node } => NamedAction::Stop(name(node)),
        Action::Mute { node } => NamedAction::Mute(name(node)),
        Action::Unmute { node } => NamedAction::Unmute(name(node)),
        Action::Solo { node } => NamedAction::Solo(name(node)),
        Action::Unsolo { node } => NamedAction::Unsolo(node.as_ref().map(name)),
        Action::Hush => NamedAction::Hush,
        Action::Panic => NamedAction::Panic,
        Action::Reset => NamedAction::Reset,
        Action::Capture { .. } => return None,
    })
}

pub(crate) fn schedule_for(program: &Composition, history: &[HistoryEntry]) -> Schedule {
    let names = program.nodes.iter().map(|t| t.name.clone()).collect();
    let index = |name: &String| program.node_index(name);
    let commands = history
        .iter()
        .filter_map(|h| {
            let action = match &h.action {
                NamedAction::Play { node, clip } => Action::Play {
                    node: index(node)?,
                    clip: clip.clone(),
                },
                NamedAction::Stop(t) => Action::Stop { node: index(t)? },
                NamedAction::Mute(t) => Action::Mute { node: index(t)? },
                NamedAction::Unmute(t) => Action::Unmute { node: index(t)? },
                NamedAction::Solo(t) => Action::Solo { node: index(t)? },
                NamedAction::Unsolo(t) => Action::Unsolo {
                    node: t.as_ref().and_then(index),
                },
                NamedAction::Hush => Action::Hush,
                NamedAction::Panic => Action::Panic,
                NamedAction::Reset => Action::Reset,
                NamedAction::Origin => return None,
            };
            Some(Scheduled {
                at: Time::Seconds(h.at),
                action,
                quantize: None,
            })
        })
        .collect::<Vec<_>>();
    Schedule::from_commands(program.tempo, names, &commands)
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        format!("{}...", line.chars().take(57).collect::<String>())
    } else {
        line.to_string()
    }
}

/// A statement to compile, and for a change to a node, what the node is playing after it.
type Edited = (Statement, Option<(String, Playing)>);
