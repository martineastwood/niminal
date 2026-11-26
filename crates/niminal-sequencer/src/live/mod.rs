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
use std::time::Duration;

use niminal_engine::time::Instant;

use niminal_compose::{
    Action, Clip, CommandAction, Composition, Finish, InputSource, LanePreview, PlayInfo, Quantize,
    Schedule, Scheduled, Statement, StatementData, StatementKind, analyze, compile_with,
};
use niminal_lang::{
    CompileOptions, Entry, Expanded, Layout, Located, Origin, Piece, Problem, Project, Samples,
};
use niminal_score::{Event, Tempo, Time, Value};
use niminal_server::{
    At, ClockInfo, Endpoint, EngineError, EngineReply, EngineRequest, Op, RoutingEdit,
};

use crate::keyboard::Keyboard;
use crate::link::InProcess;
use crate::log::{Expression, LiveInput, LiveLogEntry};
use crate::quantize::{Grid, QuantizeDefaults};
use crate::sync::{ClockIn, ClockOut, Follow};
pub use crate::sync::{ClockMessage, Realtime};

mod eval;
mod input;
mod playing;
mod scenes;
mod advance;
mod clock;
mod status;

use advance::describe;
pub(crate) use advance::name_action;
use clock::{rescale_pending, retime};
pub(crate) use clock::origin_of;
use status::note_peaks;
pub(crate) use status::routing_id;

use eval::options;
pub use eval::{Begin, CancelDone, CancelJob, EvalDone, EvalJob, ParsedEval};
pub use input::ControlSignal;
use playing::{Playing, apply_to_playing, depends_on, join_edit, mentions_any};

/// A command that has happened, by node name so that it survives the program being replaced.
#[derive(Clone)]
pub(crate) enum NamedAction {
    Start {
        node: String,
        clip: Clip,
    },
    Update {
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
    /// A parameter lane of an arrangement, from this moment.
    Lane {
        control: String,
        lane: niminal_compose::EnvLane,
    },
    /// Bars count from here. The session starts with one, so a new tempo or meter can stretch it
    /// along with the clips.
    Origin,
}

#[derive(Clone)]
pub(crate) struct HistoryEntry {
    pub at: f64,
    pub action: NamedAction,
    /// The arrangement whose lines put this here, so that stopping it can take back what is still to come.
    pub arrangement: Option<String>,
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
    /// Definitions this took out: the name was taken for something else, or what they used was.
    pub removed: Vec<String>,
    /// Drums a clip names that its kit no longer has: left out, the rest of the clip plays.
    pub left_out: Vec<String>,
    /// Clips that go on playing their old version because the new one did not work, and why.
    pub kept: Vec<String>,
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
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Accepted {
    /// `None` if the snippet had nothing to do.
    pub id: Option<u64>,
    /// The sample at which it lands.
    pub lands_at: u64,
    #[serde(serialize_with = "position_object")]
    pub position: (u64, f64),
    pub changes: Vec<String>,
}

/// A change that has landed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Landed {
    pub id: u64,
    pub at: u64,
    #[serde(serialize_with = "position_object")]
    pub position: (u64, f64),
    pub changes: Vec<String>,
}

/// How loud each node (a clip's sound) and bus has been, as a linear peak, since the last reading.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Levels {
    pub nodes: BTreeMap<String, f32>,
    pub buses: BTreeMap<String, f32>,
}

/// A change waiting to land.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PendingInfo {
    pub id: u64,
    pub lands_at: u64,
    /// Samples until it lands.
    pub in_samples: u64,
    #[serde(serialize_with = "position_object")]
    pub position: (u64, f64),
    pub changes: Vec<String>,
}

/// A clip as an editor shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ClipStatus {
    pub name: String,
    /// The instrument it plays.
    pub instrument: String,
    /// Is it playing? A clip that plays once is not, once it has finished.
    pub running: bool,
    pub muted: bool,
    pub soloed: bool,
    /// The changes made to it live (`reverse`, `drop(25%)`), oldest first.
    pub edits: Vec<String>,
    /// Its length in cycles (bars).
    pub length: f64,
    /// Does it start over when it ends?
    pub looped: bool,
    /// How many times it plays before it is finished, if it is not for ever.
    pub plays: Option<u32>,
    /// Is it free-running, never starting over?
    pub endless: bool,
    /// How many cycles it has been playing for, or `None` if it is not.
    pub cycles: Option<f64>,
    /// Its lanes over one pass, for drawing, including any live changes.
    pub lanes: Vec<LanePreview>,
}

/// A scene as an editor shows it: a list of clips. It has no state of its own.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SceneStatus {
    pub name: String,
    pub clips: Vec<String>,
    /// How many of its clips are playing. A scene is active when all of them are.
    pub running: usize,
}

/// An arrangement as an editor shows it: whether it is playing, and its sections.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ArrangementStatus {
    pub name: String,
    pub running: bool,
    /// How long it runs, in seconds, and how far it has got while it is running.
    pub length: f64,
    pub elapsed: Option<f64>,
    /// The section it is in, if it has sections.
    pub section: Option<String>,
    pub sections: Vec<niminal_compose::SectionMark>,
}

impl SceneStatus {
    pub fn active(&self) -> bool {
        !self.clips.is_empty() && self.running == self.clips.len()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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
    link: InProcess,
    /// Whether the engine has been told the tempo yet, so that the first count of beats starts from the origin.
    tempo_sent: std::cell::Cell<bool>,
    /// The controls the engine keeps to a number of beats, as they were last told.
    linked_sent: std::cell::RefCell<HashMap<String, (f64, String)>>,
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
    /// How long [`Live::advance`] goes on counting the engine's clock for itself before asking again.
    clock_sync: Duration,
    /// When the engine last said where its clock was, and the sample it said.
    synced: Option<(Instant, u64)>,
    engine: Option<ClockInfo>,
    /// The notes held open by a keyboard, by the id the engine knows them by, and the instrument of each.
    held: HashMap<u64, String>,
    /// What the last notice about things left out of clips named, so that it is said once.
    left_out_said: std::collections::BTreeSet<String>,
    /// Instrument parameters set from outside (a Lab fader), by name, in the parameter's own unit. Keys struck
    /// later start with them.
    lab_params: BTreeMap<String, f64>,
    keyboard: Keyboard,
    /// The control change each `midi.learn` control took, and what is waiting to be reported.
    learned: BTreeMap<String, u8>,
    learned_out: Vec<(String, String)>,
    /// The controls that moved since this was last taken, each once, for an editor to light their names.
    moved_out: Vec<String>,

    /// When each arrangement that was started began, in session seconds, until it is stopped.
    arrangement_starts: BTreeMap<String, f64>,

    project: Project,
    program: Arc<Composition>,
    history: Vec<HistoryEntry>,
    schedule: Schedule,
    /// Notes and parameter changes up to this sample have been taken from the schedule and sent.
    fetched_until: u64,
    /// Control moves that wait for a boundary, as (sample, control, value), in the order they land: the bank that
    /// patterns read takes each when the notes asked for reach its sample, so a pattern is played with the value
    /// a control has at the time, not the one it is about to have.
    control_changes: Vec<(u64, String, f64)>,
    /// Nodes whose clips are not searched for notes because they asked for too many.
    overloaded: Vec<usize>,
    /// How many of the schedule's panics have been sent.
    next_panic: usize,
    written: Vec<Written>,
    /// Changes of definitions or the tempo that the engine has been asked to make, in order.
    asked: Vec<Asked>,
    /// Changes the engine refused, with what it said, until the evaluation that sent them has reported it.
    refused: Vec<(u64, String)>,

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
    /// The loudest sample of each node and bus (by name) since [`Live::take_levels`], across channels.
    node_peaks: BTreeMap<String, f32>,
    bus_peaks: BTreeMap<String, f32>,
    /// Where the next evaluation lands, whatever its quantum says: a clip's `then:` lands where the
    /// clip's last pass ends.
    landing_at: Option<u64>,
    /// The `then:` of clips that are about to finish, already queued.
    armed: Vec<Armed>,
    arming: bool,
    /// The `on` controllers and OSC addresses that are held down, so that they set off once.
    triggers_down: std::collections::BTreeSet<String>,
    /// MIDI clock sent to others, and the clock of another followed.
    clock_out: ClockOut,
    clock_in: ClockIn,
    /// Tempo changes made to follow a clock, which are not reported when they land.
    followed: Vec<u64>,
}

/// A clip's `then:` that has been queued to land where the clip finishes.
struct Armed {
    node: usize,
    origin: f64,
    /// The sample the clip finishes at.
    at: u64,
    /// The evaluations queued for it.
    ids: Vec<u64>,
}

/// How far ahead of a clip's last pass its `then:` is queued, so that compiling is done in time.
const ARM_AHEAD_SECONDS: f64 = 2.0;

/// The most notices that wait to be collected, and how long before the same one is said again.
const MAX_NOTICES: usize = 100;
const NOTICE_REPEAT_SECONDS: u64 = 10;

impl Live {
    /// A sequencer for an engine that outputs `layout` at `sample_rate`.
    pub fn new(link: InProcess, sample_rate: f32, layout: Layout) -> Live {
        let program = compile_with("", &options(layout, &Samples::default()))
            .expect("an empty project compiles");
        let schedule = program.schedule();
        Live {
            arrangement_starts: BTreeMap::new(),
            link,
            tempo_sent: std::cell::Cell::new(false),
            linked_sent: std::cell::RefCell::new(HashMap::new()),
            sample_rate,
            layout,
            samples: Samples::default(),
            sample_dirs: Vec::new(),
            defaults: QuantizeDefaults::default(),
            clock: 0,
            lookahead: (f64::from(sample_rate) * LOOKAHEAD_SECONDS) as u64,
            clock_sync: Duration::ZERO,
            synced: None,
            engine: None,
            held: HashMap::new(),
            lab_params: BTreeMap::new(),
            left_out_said: Default::default(),
            keyboard: Keyboard::default(),
            learned: BTreeMap::new(),
            learned_out: Vec::new(),
            moved_out: Vec::new(),
            project: Project::default(),
            program: Arc::new(program),
            history: vec![HistoryEntry {
                at: 0.0,
                action: NamedAction::Origin,
                arrangement: None,
            }],
            schedule,
            fetched_until: 0,
            control_changes: Vec::new(),
            overloaded: Vec::new(),
            next_panic: 0,
            written: Vec::new(),
            asked: Vec::new(),
            refused: Vec::new(),
            pending: Vec::new(),
            next_id: 1,
            playing: BTreeMap::new(),
            landed: Vec::new(),
            notices: Vec::new(),
            recent_notices: Vec::new(),
            version: 0,
            log: Vec::new(),
            peaks: vec![0.0; layout.channels()],
            node_peaks: BTreeMap::new(),
            bus_peaks: BTreeMap::new(),
            landing_at: None,
            armed: Vec::new(),
            arming: false,
            triggers_down: std::collections::BTreeSet::new(),
            clock_out: ClockOut::default(),
            clock_in: ClockIn::default(),
            followed: Vec::new(),
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

    /// Count the engine's clock between asking, which costs the engine a round trip: [`Live::advance`] asks
    /// again only after this long. By default it asks every time.
    pub fn with_clock_sync(mut self, every: Duration) -> Live {
        self.clock_sync = every;
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

    /// The text of the project's definitions, which evaluated on its own makes the same project.
    pub fn project_source(&self) -> String {
        let texts: Vec<&str> = self
            .project
            .entries
            .iter()
            .map(|e| e.text.as_str())
            .collect();
        texts.join("\n")
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

    /// What was evaluated between engine samples `from` and `to`, as a script (see [`crate::log::transcript`]).
    pub fn transcript(&self, from: u64, to: u64) -> String {
        crate::log::transcript(&self.log, from, to, self.sample_rate)
    }

    /// Everything that was sent to the sequencer, with when.
    pub fn log(&self) -> &[LiveLogEntry] {
        &self.log
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

    /// Like [`Live::projected`], without copying.
    fn projected_ref(&self) -> (&Project, &BTreeMap<String, Playing>) {
        match self.pending.last() {
            Some(p) => (&p.project, &p.playing),
            None => (&self.project, &self.playing),
        }
    }

    /// The project and what is playing as they will be once everything waiting has landed.
    fn projected(&self) -> (Project, BTreeMap<String, Playing>) {
        match self.pending.last() {
            Some(p) => (p.project.clone(), p.playing.clone()),
            None => (self.project.clone(), self.playing.clone()),
        }
    }

    /// Play `program` as it is compiled: the notes it writes out, and `schedule` (usually
    /// `program.schedule()`) for the rest. This is how a file is rendered offline.
    pub fn play_program(&mut self, program: Composition, schedule: Schedule) -> Result<(), String> {
        self.look();
        let mut refused = Ok(());
        for dir in program.samples.bases().iter().rev() {
            self.set_sample_dir(dir);
        }
        let at = self.clock;
        if let Ok(statements) = analyze(&program.source) {
            let ops = self.definition_ops(&statements, Some(&program), At::Sample(at));
            if !ops.is_empty() {
                refused = self.send_definitions(ops);
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
        refused
    }

    /// Send definitions, and say what the engine said if it would not take them.
    fn send_definitions(&mut self, ops: Vec<Op>) -> Result<(), String> {
        match self.link.request(EngineRequest::EventsSend { events: ops }) {
            Ok(_) => Ok(()),
            Err(e) => Err(describe(&e)),
        }
    }

    /// Start no more notes from the schedule; what is sounding carries on. Notes already sent from it that
    /// are still to start are taken back. The effect controls those clips were holding go back to where the
    /// effects start, as they do when a clip stops.
    pub fn end_schedule(&mut self) {
        let now = self.seconds(self.clock);
        let released: Vec<Op> = self
            .schedule
            .release_all(now)
            .iter()
            .filter_map(|set| self.control_op(set))
            .collect();
        let names = self.program.nodes.iter().map(|t| t.name.clone()).collect();
        self.schedule = Schedule::from_commands(self.program.tempo, names, &[]);
        self.take_back(SCHEDULED, self.clock);
        self.fetched_until = self.clock;
        self.next_panic = 0;
        self.send(released);
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

/// A bar and beat as `{"bar": 3, "beat": 1.5}`, the shape the protocol and its clients use.
fn position_object<S: serde::Serializer>(position: &(u64, f64), out: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;
    let mut object = out.serialize_struct("Position", 2)?;
    object.serialize_field("bar", &position.0)?;
    object.serialize_field("beat", &position.1)?;
    object.end()
}
