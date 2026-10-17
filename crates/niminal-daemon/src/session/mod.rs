//! A live session: a running mixer that code can be evaluated into.
//!
//! Evaluating code compiles it immediately, and if it compiles the result waits
//! for a musical boundary before landing. Everything one evaluation says lands
//! together (a transaction), at the coarsest boundary among its parts. New
//! writes supersede older pending writes to the same destination; independent
//! changes keep their own boundaries. A snippet that does not compile changes
//! nothing.
//!
//! The session is driven by a sample clock rather than a real one, so it is
//! deterministic: the same inputs at the same samples give the same audio,
//! however the audio is divided into blocks.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use niminal_engine::{
    BLOCK, DEFAULT_CEILING, DEFAULT_MAX_VOICES, MAX_CHANNELS, Master, Mixer, Transfer,
};
use niminal_lang::{
    Action, Clip, CommandAction, CompileOptions, Expanded, InputSource, Layout, PlayInfo, Program,
    Quantize, Samples, Schedule, Scheduled, Statement, StatementData, StatementKind, analyze,
    compile_with,
};
use niminal_score::{Event, Tempo, Time, Value};

use crate::events::{At, Cancelled, Op, Sent};
use crate::protocol::ClientId;
use crate::keyboard::Keyboard;
use crate::log::{Expression, LogEntry, LogInput};
use crate::project::{Entry, Located, Origin, Piece, Problem, Project};
use crate::quantize::{Grid, QuantizeDefaults};
use crate::sounding::Sounding;

/// A command that has happened, by track name so that it survives the
/// program being replaced.
#[derive(Clone)]
enum NamedAction {
    Play {
        track: String,
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
struct HistoryEntry {
    at: f64,
    action: NamedAction,
}

/// How far a keyboard's pitch bend goes each way, in semitones.
const BEND_RANGE: f64 = 2.0;
/// The most MIDI messages kept waiting to be taken.
const MAX_MIDI_QUEUE: usize = 65_536;
/// How far an MPE note's own pitch bend goes, each way, in semitones.
const MPE_BEND_RANGE: f64 = 48.0;

/// A tempo ramp changes the tempo this many times a beat.
const RAMP_STEPS_PER_BEAT: f64 = 4.0;
const MAX_RAMP_STEPS: f64 = 1024.0;

/// The key of a note that is not held open by one.
pub(crate) const NO_KEY: u32 = u32::MAX;

/// A note waiting for its moment.
struct Upcoming {
    start: u64,
    dur: u64,
    event: Event,
    /// A key being held plays this note until it is let go, whatever `dur` says.
    key: Option<u32>,
    /// Taken from the schedule (so it is dropped and fetched again if the
    /// schedule changes), rather than written out or sent from outside.
    scheduled: bool,
    /// The client that sent it, if one did, and the tag it gave.
    client: Option<ClientId>,
    tag: Option<String>,
    /// Given in beats (`Some(false)`) or bars (`Some(true)`): it moves when the tempo or meter does.
    musical: Option<bool>,
}

/// A note a client holds open or may change, by the id the client chose.
struct VoiceEntry {
    client: ClientId,
    id: u64,
    key: u32,
    /// The instrument it plays, by name so that it survives the program being replaced.
    instrument: String,
    tag: Option<String>,
}

/// A change to a parameter of the note a key holds. The parameter is found by name in the instrument
/// when the change is planned.
#[derive(Clone)]
pub(crate) struct VoiceSet {
    pub key: u32,
    pub instrument: String,
    pub name: String,
    pub value: f32,
    pub frames: u64,
    pub at: u64,
    done: bool,
    client: ClientId,
    tag: Option<String>,
}

/// A track parameter, which is a control, moving at a sample: a lane or the `track` event. Planned
/// into the windows like notes, since a lane can move one a great many times.
#[derive(Clone)]
pub(crate) struct TrackSet {
    pub key: String,
    pub value: f32,
    pub frames: u64,
    pub at: u64,
    client: Option<ClientId>,
    tag: Option<String>,
    done: bool,
}

/// A waiting release of the note a key holds.
struct KeyOff {
    key: u32,
    at: u64,
    client: Option<ClientId>,
    tag: Option<String>,
}

/// Who sent a control change that has not happened yet.
struct ControlOwner {
    serial: u64,
    client: ClientId,
    tag: Option<String>,
}

/// A statement that runs when a change lands, with the program it was
/// compiled in (so landing does no compiling).
struct Compiled {
    statement: Statement,
    program: Program,
}

/// What a change sent through the engine API carries with it.
#[derive(Clone, Default)]
pub(super) struct Meta {
    client: Option<ClientId>,
    tag: Option<String>,
    /// Given in beats (`Some(false)`) or bars (`Some(true)`), so it moves when the tempo or meter does.
    musical: Option<bool>,
    /// This point is bar 1.
    origin: bool,
}

/// An evaluation that has compiled and is waiting to land. Everything that
/// could be slow, such as compiling and building the mixer, was done when it
/// was evaluated, so that landing only has to move things into place.
struct Pending {
    id: u64,
    /// The first change of the tempo ramp this is a step of: cancelling any of them cancels the rest.
    group: Option<u64>,
    lands_at: u64,
    /// What was evaluated, so that the change can be redone if one before it
    /// is cancelled.
    snippet: String,
    statements: Vec<Statement>,
    summary: Vec<String>,
    /// The project once this has landed.
    project: Project,
    /// The new program and a mixer built for it, if definitions changed.
    program: Option<Arc<Program>>,
    mixer: Option<Mixer>,
    actions: Vec<Compiled>,
    /// Plays to start again because a pattern they use was redefined.
    replays: Vec<Compiled>,
    /// What each track is playing once this has landed.
    playing: BTreeMap<String, Playing>,
    meta: Meta,
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

/// A track as an editor shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackStatus {
    pub name: String,
    /// The clip playing on it now.
    pub clip: Option<String>,
    /// What the track was told to play (base clip text before live edits).
    pub playing: Option<String>,
    pub muted: bool,
    pub soloed: bool,
    /// The changes made to it live (`reverse`, `thin(25%)`), oldest first.
    pub edits: Vec<String>,
}

/// Where the engine is in time, for a client that wants to schedule against it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockInfo {
    /// The sample the output is at, as its audio callback last said; the render clock if it hasn't.
    pub sample: u64,
    pub bar: u64,
    /// The beat in the bar, counting from 1, fractions included.
    pub beat: f64,
    pub bpm: f64,
    pub beats_per_bar: f64,
    pub sample_rate: f32,
    /// The sample up to which audio has been prepared.
    pub prepared_until: u64,
    /// How far ahead of the clock the engine prepares, in samples.
    pub lookahead: u64,
    /// Seconds from the audio callback to the speakers, as the audio API reports it.
    pub output_latency: f64,
    /// The Unix time in microseconds at which the audio callback was handed the buffer that starts at
    /// `sample`, or `None` before there is an output.
    pub wall_micros: Option<u64>,
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

/// What arrived from outside to move a bound `ctl`. MIDI channels count from 1, and
/// an OSC value is 0 to 1 (outside is clamped).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlSignal<'a> {
    MidiCc {
        cc: u8,
        channel: u8,
        value: u8,
    },
    /// A key pressed (`velocity` from 1) or let go (`velocity` 0), on a MIDI channel from 1.
    MidiNote {
        note: u8,
        channel: u8,
        velocity: u8,
    },
    /// Pitch bend on a channel, 0 to 16383 with 8192 in the middle.
    MidiBend {
        channel: u8,
        value: u16,
    },
    /// Pressure on a channel, or on one key (`note`), 0 to 127.
    MidiPressure {
        channel: u8,
        note: Option<u8>,
        value: u8,
    },
    Osc {
        address: &'a str,
        value: f64,
    },
}

pub struct Session {
    controls: Vec<crate::controls::State>,
    control_changes: Vec<crate::controls::Change>,
    next_control: u64,
    /// The control change each `midi.learn` control took, and what is waiting to be reported.
    learned: BTreeMap<String, u8>,
    learned_out: Vec<(String, String)>,
    /// The notes each held key started, by channel and note, and the keys let go of lately.
    keyboard: Keyboard,
    key_offs: Vec<KeyOff>,
    /// Changes of a held key's bend or pressure: `(key, parameter, value, when)`.
    key_params: Vec<(u32, &'static str, f32, u64)>,
    /// Audio for tracks that read `input(..)`, from a file being rendered: a buffer for each channel, from sample 0.
    audio_input: Vec<Vec<f32>>,
    /// Notes sent to `midi_out` instruments and not yet taken, and those still held: when they end and the key that holds them.
    midi_out: Vec<crate::MidiEvent>,
    sample_rate: f32,
    layout: Layout,
    panic_epoch: Arc<AtomicU64>,
    /// Where the project's sample files are found, and the ones already read.
    samples: Samples,
    sample_dirs: Vec<std::path::PathBuf>,
    defaults: QuantizeDefaults,
    clock: u64,

    project: Project,
    program: Arc<Program>,
    mixer: Mixer,
    limiter: Option<Master>,

    history: Vec<HistoryEntry>,
    schedule: Schedule,
    /// Notes up to this sample have been taken from the schedule.
    fetched_until: u64,
    upcoming: VecDeque<Upcoming>,
    /// Envelope lanes' changes to track parameters that are waiting for their sample, in order.
    control_sets: VecDeque<(u64, niminal_lang::ControlSet)>,
    /// Tracks whose clips are not searched for notes because they asked for too many.
    overloaded: Vec<usize>,
    /// Sounding notes: when each is released, and the key holding it open (or `NO_KEY`).
    sounding: Sounding,
    next_panic: usize,

    pending: Vec<Pending>,
    next_id: u64,
    /// What each track was last told to play, as source, so that redefining a
    /// pattern can start the new version.
    playing: BTreeMap<String, Playing>,
    landed: Vec<Landed>,
    notices: Vec<String>,
    recent_notices: Vec<(String, u64)>,
    log: Vec<LogEntry>,
    log_corrections: u64,
    silenced: usize,
    peaks: Vec<f32>,
    /// Changes whenever what is waiting to land changes, so that an evaluation
    /// compiled in the meantime can tell it is out of date.
    version: u64,
    live: Option<realtime::Planner>,
    detached: bool,
    /// Notes clients hold by id, and their parameter changes still to come.
    voices: Vec<VoiceEntry>,
    /// Notes whose release has been asked for but has not come yet, in case that is taken back.
    closed: Vec<VoiceEntry>,
    voice_sets: Vec<VoiceSet>,
    track_sets: Vec<TrackSet>,
    control_owners: Vec<ControlOwner>,
    /// Where the audio output is, as its callback last said.
    probe: Arc<crate::ClockProbe>,
    /// Things added since the planner last looked, that it may have to put into windows already prepared.
    fresh: Vec<realtime::Fresh>,
    /// How many mixers have been built for the renderer.
    mixer_builds: Arc<std::sync::atomic::AtomicUsize>,
}

mod eval;
mod input;
mod playing;
pub mod realtime;

use eval::options;
pub use eval::{Begin, CancelDone, CancelJob, EvalDone, EvalJob, ParsedEval};
use playing::{Playing, apply_launch, apply_to_playing, join_edit, mentions_any};

/// The most notices that wait to be collected, and how long before the same one is said again.
const MAX_NOTICES: usize = 100;
const NOTICE_REPEAT_SECONDS: u64 = 10;

impl Session {
    /// A session that outputs `layout` at `sample_rate`, with the limiter on.
    pub fn new(sample_rate: f32, layout: Layout) -> Session {
        let program = compile_with("", &options(layout, &Samples::default()))
            .expect("an empty project compiles");
        let mixer = program.mixer(sample_rate);
        let schedule = program.schedule();
        Session {
            sample_rate,
            layout,
            panic_epoch: Arc::new(AtomicU64::new(0)),
            samples: Samples::default(),
            controls: Vec::new(),
            control_changes: Vec::new(),
            next_control: 0,
            learned: BTreeMap::new(),
            learned_out: Vec::new(),
            keyboard: Keyboard::default(),
            key_offs: Vec::new(),
            key_params: Vec::new(),
            audio_input: Vec::new(),
            midi_out: Vec::new(),
            sample_dirs: Vec::new(),
            defaults: QuantizeDefaults::default(),
            clock: 0,
            project: Project::default(),
            program: Arc::new(program),
            mixer,
            limiter: Some(Master::with_channels(
                sample_rate,
                DEFAULT_CEILING,
                layout.channels(),
            )),
            history: vec![HistoryEntry {
                at: 0.0,
                action: NamedAction::Origin,
            }],
            schedule,
            fetched_until: 0,
            upcoming: VecDeque::new(),
            control_sets: VecDeque::new(),
            overloaded: Vec::new(),
            sounding: Sounding::default(),
            next_panic: 0,
            pending: Vec::new(),
            next_id: 1,
            playing: BTreeMap::new(),
            landed: Vec::new(),
            notices: Vec::new(),
            recent_notices: Vec::new(),
            log: Vec::new(),
            log_corrections: 0,
            silenced: 0,
            peaks: vec![0.0; layout.channels()],
            version: 0,
            live: None,
            detached: false,
            voices: Vec::new(),
            closed: Vec::new(),
            voice_sets: Vec::new(),
            track_sets: Vec::new(),
            control_owners: Vec::new(),
            probe: Arc::default(),
            fresh: Vec::new(),
            mixer_builds: Arc::default(),
        }
    }

    /// A session that plays `program` as it is compiled: the notes it writes out, and `schedule`
    /// (usually `program.schedule()`) for the rest. This is how a file is rendered offline.
    pub fn playing(program: Program, schedule: Schedule, sample_rate: f32) -> Session {
        let mut session = Session::new(sample_rate, program.master).without_limiter();
        session.mixer = program.mixer(sample_rate);
        session.controls = crate::controls::build(&program);
        crate::controls::sync_mixer(&session.controls, &mut session.mixer, 0);
        session.schedule = schedule;
        for event in &program.notes {
            let start = session.samples(event.at.to_seconds(program.tempo));
            let dur = session.samples(event.dur.to_seconds(program.tempo));
            session.insert_upcoming(Upcoming {
                start,
                dur,
                event: event.clone(),
                key: None,
                scheduled: false,
                client: None,
                tag: None,
                musical: None,
            });
        }
        session.program = Arc::new(program);
        session
    }

    /// Start no more notes from the schedule; what is sounding carries on. Notes already taken from it
    /// that are still to start are dropped.
    pub fn end_schedule(&mut self) {
        let names = self.program.tracks.iter().map(|t| t.name.clone()).collect();
        self.schedule = Schedule::from_commands(self.program.tempo, names, &[]);
        self.upcoming.retain(|u| !u.scheduled);
        self.control_sets.clear();
        self.next_panic = 0;
    }

    /// Find the paths in `sample` and `kit` declarations from `dir`.
    /// The sample files this session has read.
    pub fn sample_files(&self) -> &Samples {
        &self.samples
    }

    /// Folders given earlier are still searched, after this one.
    pub fn set_sample_dir(&mut self, dir: &std::path::Path) {
        if self.sample_dirs.first().is_none_or(|d| d != dir) {
            self.sample_dirs.retain(|d| d != dir);
            self.sample_dirs.insert(0, dir.to_path_buf());
            self.samples = self.samples.with_bases(self.sample_dirs.clone());
        }
    }

    pub fn with_defaults(mut self, defaults: QuantizeDefaults) -> Session {
        self.defaults = defaults;
        self
    }

    /// Turn the master limiter off. Only for offline use.
    pub fn without_limiter(mut self) -> Session {
        self.limiter = None;
        self
    }

    /// Samples of delay the limiter adds to the output.
    pub fn latency(&self) -> usize {
        self.limiter
            .as_ref()
            .map(Master::latency)
            .unwrap_or_else(|| self.live.as_ref().map_or(0, |p| p.state.latency))
    }

    /// For the audio callback to say where the output is.
    pub fn clock_probe(&self) -> Arc<crate::ClockProbe> {
        self.probe.clone()
    }

    pub fn channels(&self) -> usize {
        self.layout.channels()
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn clock(&self) -> u64 {
        self.clock
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// The program in effect at sample `at`: the running one, or what the last change of definitions
    /// landing by then makes it.
    pub(super) fn program_at(&self, at: u64) -> &Arc<Program> {
        self.pending
            .iter()
            .rev()
            .find(|p| p.lands_at <= at && p.program.is_some())
            .and_then(|p| p.program.as_ref())
            .unwrap_or(&self.program)
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

    // ---- inputs -------------------------------------------------------------

    /// Changes on an immediate panic, allowing an output queue to discard audio
    /// that was rendered before it. Scheduled panics are already in the audio.
    pub fn panic_epoch(&self) -> Arc<AtomicU64> {
        self.panic_epoch.clone()
    }

    /// Stop everything at once, including effect tails.
    pub fn panic(&mut self) {
        self.sync_realtime();
        self.panic_epoch.fetch_add(1, Ordering::Release);
        self.invalidate_live();
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::Panic,
        });
        let at = self.seconds(self.clock);
        self.history.push(HistoryEntry {
            at,
            action: NamedAction::Panic,
        });
        self.playing.clear();
        self.rebuild_schedule();
        self.apply_panics();
    }

    /// Stop every clip, leaving what is sounding to ring out.
    pub fn hush(&mut self) -> Result<Accepted, Vec<Problem>> {
        self.eval("hush", Some("now"))
    }

    pub fn control_values(&self) -> Vec<(String, f32, f32, Option<&'static str>)> {
        self.program
            .controls
            .iter()
            .zip(&self.controls)
            .map(|(def, state)| {
                (
                    def.name.clone(),
                    state.value(self.clock),
                    state.target,
                    def.unit.suffix(),
                )
            })
            .collect()
    }

    // ---- state ----------------------------------------------------------------

    pub fn pending(&self) -> Vec<PendingInfo> {
        let grid = self.grid();
        self.pending
            .iter()
            .map(|p| PendingInfo {
                id: p.id,
                lands_at: p.lands_at,
                in_samples: p.lands_at.saturating_sub(self.clock),
                position: grid.position(p.lands_at),
                changes: p.summary.clone(),
            })
            .collect()
    }

    pub fn clock_info(&mut self) -> ClockInfo {
        self.sync_realtime();
        let probe = self.probe.read();
        let sample = probe.map_or(self.clock, |(sample, ..)| sample);
        let grid = self.grid();
        let (bar, beat) = grid.position(sample);
        ClockInfo {
            sample,
            bar,
            beat,
            bpm: grid.bpm,
            beats_per_bar: grid.beats_per_bar,
            sample_rate: self.sample_rate,
            prepared_until: self.prepared_until(),
            lookahead: self.live.as_ref().map_or(0, |p| p.lookahead()),
            output_latency: probe.map_or(0.0, |(_, _, micros)| micros as f64 / 1e6),
            wall_micros: probe.map(|(_, wall, _)| wall),
        }
    }

    /// The instruments and tracks that are defined, with their parameters, for a client that wants to offer them.
    pub fn definitions(&self) -> (Vec<niminal_lang::Instrument>, Vec<String>, Vec<String>) {
        (
            self.program.instruments.clone(),
            self.program.tracks.iter().skip(1).map(|t| t.name.clone()).collect(),
            self.program.buses.clone(),
        )
    }

    /// Each client's held notes (by the ids it chose) and how many of its events are still waiting.
    pub fn client_status(&self) -> Vec<(ClientId, Vec<u64>, usize)> {
        let mut clients: Vec<ClientId> = self.voices.iter().map(|v| v.client).collect();
        clients.extend(self.upcoming.iter().filter_map(|u| u.client));
        clients.extend(self.control_owners.iter().map(|o| o.client));
        clients.extend(self.pending.iter().filter_map(|p| p.meta.client));
        clients.sort_unstable();
        clients.dedup();
        clients
            .into_iter()
            .map(|c| {
                let ids = self.voices.iter().filter(|v| v.client == c).map(|v| v.id).collect();
                let waiting = self.upcoming.iter().filter(|u| u.client == Some(c) && u.start >= self.clock && !u.scheduled).count()
                    + self.control_owners.iter().filter(|o| o.client == c).count()
                    + self.voice_sets.iter().filter(|s| s.client == c && !s.done).count()
                    + self.pending.iter().filter(|p| p.meta.client == Some(c)).count();
                (c, ids, waiting)
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
            voices: self.live.as_ref().map_or_else(
                || self.mixer.active_voices(),
                |p| p.state.voices.load(Ordering::Relaxed),
            ),
            pending: self.pending.len(),
        }
    }

    /// The project's tracks and what they are doing now.
    pub fn tracks(&self) -> Vec<TrackStatus> {
        let now = self.seconds(self.clock);
        self.program
            .tracks
            .iter()
            .enumerate()
            .skip(1)
            // tracks that only process buses are not parts to play
            .filter(|(_, t)| t.instrument.is_some())
            .map(|(i, t)| {
                let state = self.schedule.track_state(i, now);
                let record = self.playing.get(&t.name);
                TrackStatus {
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
        std::mem::take(&mut self.landed)
    }

    /// Queue a notice, unless the same one is already waiting: a problem that
    /// recurs every fraction of a second must not bury everything else.
    fn notify(&mut self, notice: String) {
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

    /// Things worth telling the user that aren't errors in what they sent: a
    /// note that couldn't play, a change that failed to land.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// The audio that tracks reading `input(..)` hear, a buffer for each channel starting at sample 0 of the
    /// session. For rendering a file; a live session listens to a device instead.
    pub fn set_audio_input(&mut self, channels: Vec<Vec<f32>>) {
        self.audio_input = channels;
    }

    /// Start keeping what each track and bus puts out, for an offline render's stems. Only for renders:
    /// it grows without bound.
    pub fn capture_stems(&mut self) {
        self.mixer.capture_stems();
    }

    /// The stems kept so far, no longer kept after this.
    pub fn take_stems(&mut self) -> Option<niminal_engine::Stems> {
        self.mixer.take_stems()
    }

    /// Voices silenced for producing NaN or infinity since this was last called.
    pub fn take_silenced(&mut self) -> usize {
        std::mem::take(&mut self.silenced)
    }

    /// The loudest sample on each output channel since this was last called.
    pub fn take_peaks(&mut self) -> Vec<f32> {
        self.sync_realtime();
        std::mem::replace(&mut self.peaks, vec![0.0; self.layout.channels()])
    }

    /// Changes when entries are appended or actual control timing is acknowledged.
    pub fn log_version(&self) -> (usize, u64) {
        (self.log.len(), self.log_corrections)
    }

    /// Everything that has been sent to the session, with when.
    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    // ---- the project --------------------------------------------------------------

    /// The project and what is playing as they will be once everything
    /// waiting has landed.
    fn projected(&self) -> (Project, BTreeMap<String, Playing>) {
        match self.pending.last() {
            Some(p) => (p.project.clone(), p.playing.clone()),
            None => (self.project.clone(), self.playing.clone()),
        }
    }

    // ---- landing ------------------------------------------------------------------------

    fn land_due(&mut self) {
        while self
            .pending
            .first()
            .is_some_and(|p| p.lands_at <= self.clock)
        {
            let pending = self.pending.remove(0);
            self.land(pending);
        }
    }

    fn land(&mut self, p: Pending) {
        self.version += 1;
        let at = self.clock;
        let Pending {
            id,
            statements,
            summary,
            project,
            program,
            mixer,
            actions,
            replays,
            playing,
            meta,
            ..
        } = p;

        if let (Some(program), Some(mixer)) = (program, mixer) {
            let now = self.seconds(at);
            retime(&mut self.history, now, self.program.tempo, program.tempo);
            let (old, new) = (self.program.tempo, program.tempo);
            let mut pending = std::mem::take(&mut self.pending);
            rescale_pending(&mut pending, at, old, new, |p| self.landing_unit_is_bar(p));
            self.pending = pending;
            rescale_notes(&mut self.upcoming, at, old, new);
            self.swap_program(project, program, mixer, &statements, id);
        }
        if meta.origin {
            let now = self.seconds(at);
            self.history.push(HistoryEntry { at: now, action: NamedAction::Origin });
        }
        let mut captures = Vec::new();
        for a in actions.iter().chain(&replays) {
            captures.extend(self.run_action(a, at));
        }
        self.playing = playing;

        self.rebuild_schedule();
        let mut changes = summary;
        for (bars, name) in captures {
            if let Some(notice) = self.capture_section(bars, &name) {
                changes.push(notice);
            }
        }
        self.landed.push(Landed {
            id,
            at,
            position: self.grid().position(at),
            changes,
        });
    }

    /// Replace the running program, keeping what is sounding.
    fn swap_program(
        &mut self,
        project: Project,
        program: Arc<Program>,
        mut mixer: Mixer,
        statements: &[Statement],
        revision: u64,
    ) {
        let mut controls = crate::controls::build(&program);
        crate::controls::mark(&mut controls, &program, statements, revision);
        crate::controls::adopt(
            &mut controls,
            &self.controls,
            &program,
            self.clock,
            self.sample_rate,
        );
        self.controls = controls;
        if self.detached {
            self.program = program;
            self.project = project;
            return;
        }
        let mapping: Vec<Option<usize>> = self
            .program
            .buses
            .iter()
            .enumerate()
            .map(|(i, name)| {
                program
                    .buses
                    .iter()
                    .position(|n| n == name)
                    .filter(|&j| self.program.bus_layouts[i] == program.bus_layouts[j])
            })
            .collect();
        self.mixer.remap_buses(&mapping);
        let mut transfers = Vec::new();
        plan_transfers(&self.program, &program, &mut transfers);
        mixer.adopt(&mut self.mixer, &transfers);
        crate::controls::sync_mixer(&self.controls, &mut mixer, self.clock);
        carry_buffers(&self.program, &program);
        self.mixer = mixer;
        self.program = program;
        self.project = project;
    }

    /// Carry out one `play`, `mute`, note... statement that has landed. It was
    /// compiled when it was evaluated. Returns any captures to run after the
    /// schedule is rebuilt.
    fn run_action(&mut self, action: &Compiled, at: u64) -> Vec<(f64, String)> {
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
            self.insert_upcoming(Upcoming {
                start: start.max(at),
                dur,
                event: event.clone(),
                key: None,
                scheduled: false,
                client: None,
                tag: None,
                musical: None,
            });
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

    /// Write the notes sounding in the last `bars` bars to `score/{name}.nms`.
    fn capture_section(&self, bars: f64, name: &str) -> Option<String> {
        if self.detached {
            return None;
        }
        let tempo = self.program.tempo;
        let now = self.seconds(self.clock);
        let length = Time::Bars(bars).to_seconds(tempo);
        let from = (now - length).max(0.0);
        let found = self.schedule.events_lenient(from, now, &[]);
        let mut events = found.events;
        // Notes written out (not from a clip) that started in the window.
        for u in &self.upcoming {
            let start = self.seconds(u.start);
            if u.scheduled || start < from || start >= now {
                continue;
            }
            let mut event = u.event.clone();
            event.at = Time::Seconds(start);
            event.dur = Time::Seconds(self.seconds(u.dur));
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
        let section = niminal_score::Section {
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
            return Some(format!("capture failed: could not write {}: {e}", path.display()));
        }
        Some(format!("wrote {} ({count} events)", path.display()))
    }

    /// Whether a pending change waits for a bar line, not a beat.
    fn landing_unit_is_bar(&self, p: &Pending) -> bool {
        use niminal_lang::QuantizeUnit;
        if let Some(by_bar) = p.meta.musical {
            return by_bar;
        }
        p.statements.iter().any(|s| {
            matches!(
                self.quantums().for_statement(s, None).unit,
                QuantizeUnit::Bar | QuantizeUnit::Cycle
            )
        })
    }

    fn rebuild_schedule(&mut self) {
        self.schedule = realtime::schedule_for(&self.program, &self.history);
        self.overloaded.clear();
        // Notes already taken from the old schedule may no longer be right.
        self.upcoming.retain(|u| !u.scheduled);
        self.control_sets.clear();
        self.fetched_until = self.clock;
        self.next_panic = self
            .schedule
            .panics()
            .iter()
            .take_while(|&&t| self.samples(t) < self.clock)
            .count();
    }

    // ---- running ---------------------------------------------------------------------------

    fn insert_upcoming(&mut self, note: Upcoming) {
        let at = self.upcoming.partition_point(|u| u.start <= note.start);
        self.upcoming.insert(at, note);
    }

    fn apply_panics(&mut self) {
        let panics = self.schedule.panics();
        while let Some(&t) = panics.get(self.next_panic) {
            if self.samples(t) > self.clock {
                break;
            }
            if !self.detached {
                self.mixer.panic();
                crate::controls::sync_mixer(&self.controls, &mut self.mixer, self.clock);
            }
            if let Some(limiter) = &mut self.limiter {
                limiter.clear();
            }
            self.sounding.held.clear();
            for (_, _, note) in std::mem::take(&mut self.sounding.midi) {
                if self.midi_out.len() < MAX_MIDI_QUEUE {
                    self.midi_out.push(crate::MidiEvent {
                        at: self.clock,
                        on: false,
                        port: note.port,
                        channel: note.channel,
                        note: note.note,
                        velocity: note.velocity,
                    });
                }
            }
            self.upcoming.clear();
            self.control_sets.clear();
            self.next_panic += 1;
        }
    }

    /// Mix `frames` samples, calling `sink` with each piece (a block per
    /// channel and how many of its samples are valid).
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[[f32; BLOCK]], usize)) {
        assert!(
            !self.detached,
            "a detached session is driven by its renderer"
        );
        // the same mode as the live renderer, so that the two render alike
        let _flush = niminal_engine::denormals::FlushToZero::new();
        let end = self.clock + frames as u64;
        let channels = self.layout.channels();
        let mut block = [[0.0f32; BLOCK]; MAX_CHANNELS];

        while self.clock < end {
            self.land_due();
            self.apply_panics();
            self.apply_controls();
            self.release_due();
            self.apply_voice_sets();
            self.apply_track_sets();

            // How far we can go before something needs doing.
            let mut n = (end - self.clock).min(BLOCK as u64);
            if let Some(p) = self.pending.first() {
                n = n.min(p.lands_at - self.clock);
            }
            if let Some(change) = self.control_changes.first() {
                n = n.min(change.at.saturating_sub(self.clock).max(1));
            }
            n = self.sounding.limit(self.clock, n);
            let upcoming = self.key_offs.iter().map(|k| k.at).chain(self.voice_sets.iter().filter(|s| !s.done).map(|s| s.at)).chain(self.track_sets.iter().filter(|s| !s.done).map(|s| s.at));
            for at in upcoming.filter(|&at| at > self.clock) {
                n = n.min(at - self.clock);
            }
            if let Some(&t) = self.schedule.panics().get(self.next_panic) {
                n = n.min(self.samples(t).saturating_sub(self.clock).max(1));
            }

            // Take the notes that start in this stretch from the schedule.
            let horizon = self.clock + n;
            self.fetch_until(horizon);
            if let Some(next) = self.upcoming.iter().find(|u| u.start > self.clock) {
                n = n.min(next.start - self.clock);
            }
            if let Some((at, _)) = self.control_sets.iter().find(|(at, _)| *at > self.clock) {
                n = n.min(at - self.clock);
            }
            self.apply_control_sets();
            self.start_due();

            let n = n.max(1) as usize;
            for (c, samples) in self
                .audio_input
                .iter()
                .enumerate()
                .take(niminal_engine::MAX_CHANNELS)
            {
                let from = (self.clock as usize).min(samples.len());
                self.mixer
                    .set_audio_input(c, &samples[from..(from + n).min(samples.len())]);
            }
            let grid = self.grid();
            self.mixer.set_clock(niminal_engine::Clock {
                beat: grid.beats(self.clock),
                bpm: grid.bpm as f32,
                beats_per_bar: grid.beats_per_bar as f32,
            });
            self.mixer.process(&mut block[..channels], n);
            self.silenced += self.mixer.take_silenced();
            if let Some(limiter) = &mut self.limiter {
                limiter.process_block(&mut block[..channels], n);
            }
            for (peak, b) in self.peaks.iter_mut().zip(&block) {
                *peak = b[..n].iter().fold(*peak, |m, s| m.max(s.abs()));
            }
            sink(&block[..channels], n);
            self.clock += n as u64;
        }
        self.land_due();
    }

    /// Run a recorded session from the start and return its audio, `frames`
    /// samples of it. Because inputs are applied at the sample they were first
    /// applied at, this is the audio the original produced, however it was
    /// divided into blocks.
    pub fn replay(
        sample_rate: f32,
        layout: Layout,
        defaults: QuantizeDefaults,
        log: &[LogEntry],
        frames: u64,
        limiter: bool,
    ) -> Vec<Vec<f32>> {
        let mut session = Session::new(sample_rate, layout).with_defaults(defaults);
        if !limiter {
            session = session.without_limiter();
        }
        let mut out = vec![Vec::new(); layout.channels()];
        let mut extend = |session: &mut Session, upto: u64| {
            let gap = upto.saturating_sub(session.clock) as usize;
            for (o, p) in out.iter_mut().zip(session.process(gap)) {
                o.extend(p);
            }
        };
        for entry in log {
            extend(&mut session, entry.sample);
            // Rejected inputs were rejected the first time too.
            match &entry.input {
                LogInput::Eval { source, quantize } => {
                    let _ = session.eval(source, quantize.as_deref());
                }
                LogInput::Cancel { id } => {
                    session.cancel(*id);
                }
                LogInput::Panic => session.panic(),
                LogInput::CancelEvents { client, tag, from, release } => {
                    session.cancel_events(*client, tag.as_deref(), At::Sample(*from), *release);
                }
                LogInput::Send { client, ops } => {
                    let _ = session.send_events(*client, ops.clone());
                }
                LogInput::Key {
                    channel,
                    note,
                    velocity,
                } => {
                    session.key_input(*channel, *note, *velocity);
                }
                LogInput::Expression {
                    channel,
                    what,
                    value,
                    note,
                } => {
                    session.expression_input(*channel, *what, *value, *note);
                }
            }
        }
        extend(&mut session, frames);
        out
    }

    /// Mix `frames` samples and return them, one buffer per channel.
    pub fn process(&mut self, frames: usize) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::with_capacity(frames); self.layout.channels()];
        self.render(frames, &mut |block, n| {
            for (o, b) in out.iter_mut().zip(block) {
                o.extend_from_slice(&b[..n]);
            }
        });
        out
    }

    fn fetch_until(&mut self, horizon: u64) {
        if horizon <= self.fetched_until {
            return;
        }
        let (from, to) = (self.seconds(self.fetched_until), self.seconds(horizon));
        let found = self.schedule.events_lenient(from, to, &self.overloaded);
        for track in found.overloaded {
            if !self.overloaded.contains(&track) {
                self.overloaded.push(track);
            }
        }
        let (events, problem) = (found.events, found.problem);
        for set in found.controls {
            let at = self.samples(set.at).max(self.clock);
            let place = self.control_sets.partition_point(|(t, _)| *t <= at);
            self.control_sets.insert(place, (at, set));
        }
        for event in events {
            let start = self
                .samples(event.at.to_seconds(self.program.tempo))
                .max(self.clock);
            let dur = self.samples(event.dur.to_seconds(self.program.tempo));
            self.insert_upcoming(Upcoming {
                start,
                dur,
                event,
                key: None,
                scheduled: true,
                client: None,
                tag: None,
                musical: None,
            });
        }
        if let Some(e) = problem {
            self.notify(e.to_string());
        }
        self.fetched_until = horizon;
    }

    /// Move the track parameters that envelope lanes say should move by now.
    fn apply_control_sets(&mut self) {
        while self
            .control_sets
            .front()
            .is_some_and(|(at, _)| *at <= self.clock)
        {
            let Some((_, set)) = self.control_sets.pop_front() else {
                break;
            };
            let Some(index) = self
                .program
                .controls
                .iter()
                .position(|c| c.name == set.name)
            else {
                continue;
            };
            let frames = (set.glide * f64::from(self.sample_rate)).round() as u64;
            let state = &mut self.controls[index];
            state.set(self.clock, set.target as f32, frames);
            self.mixer
                .set_control(&state.key, state.start, state.target, state.frames, 0);
        }
    }

    fn start_due(&mut self) {
        while self.upcoming.front().is_some_and(|u| u.start <= self.clock) {
            let Some(note) = self.upcoming.pop_front() else {
                break;
            };
            match self.program.plan(&note.event) {
                Ok(plan) => {
                    let graph = self.program.instruments[plan.instrument].graph.clone();
                    for (key, value, smooth) in &plan.controls {
                        let frames = (smooth * f64::from(self.sample_rate)).round() as u64;
                        if let Some(c) = self.controls.iter_mut().find(|c| c.key == *key) {
                            c.set(self.clock, *value, frames);
                            self.mixer
                                .set_control(&c.key, c.start, c.target, c.frames, 0);
                        }
                    }
                    let id = self.mixer.note_on_with(
                        plan.track,
                        graph,
                        &plan.params,
                        plan.choke,
                        plan.rule,
                    );
                    self.mixer.set_voice_pan(id, plan.pan);
                    self.mixer.set_voice_dur(id, if note.key.is_some() && note.dur == 0 { 0.0 } else { note.dur as f32 / self.sample_rate });
                    crate::controls::sync_note(&self.controls, &mut self.mixer, id, self.clock);
                    let off = if note.key.is_some() && note.dur == 0 {
                        u64::MAX
                    } else {
                        self.clock + note.dur
                    };
                    self.sounding
                        .held
                        .push((off, id, note.key.unwrap_or(NO_KEY)));
                    if let Some(midi) = plan.midi {
                        self.send_midi(true, &midi, self.clock);
                        self.sounding
                            .midi
                            .push((off, note.key.unwrap_or(NO_KEY), midi));
                    }
                    if self.mixer.take_stolen() > 0 {
                        self.notify(format!("too many notes at once: the oldest are ended early to keep to {} voices", DEFAULT_MAX_VOICES));
                    }
                }
                Err(e) => self.notify(format!(
                    "note for `{}` could not play: {e}",
                    note.event.target
                )),
            }
        }
    }

    fn send_midi(&mut self, on: bool, note: &niminal_lang::MidiNote, at: u64) {
        if self.midi_out.len() < MAX_MIDI_QUEUE {
            self.midi_out.push(crate::MidiEvent {
                at,
                on,
                port: note.port.clone(),
                channel: note.channel,
                note: note.note,
                velocity: note.velocity,
            });
        }
    }

    /// The notes that went to `midi_out` instruments since this was last called, in order.
    pub fn take_midi(&mut self) -> Vec<crate::MidiEvent> {
        std::mem::take(&mut self.midi_out)
    }

    /// Move the track parameters that `track` events say should move by now.
    fn apply_track_sets(&mut self) {
        let now = self.clock;
        for i in 0..self.track_sets.len() {
            let set = &self.track_sets[i];
            if set.done || set.at > now {
                continue;
            }
            let (key, value, frames) = (set.key.clone(), set.value, set.frames);
            if let Some(state) = self.controls.iter_mut().find(|c| c.key == key) {
                state.set(now, value, frames);
                self.mixer.set_control(&state.key, state.start, state.target, state.frames, 0);
            }
            self.track_sets[i].done = true;
        }
        let stale = now.saturating_sub(self.samples(2.0));
        self.track_sets.retain(|s| !s.done || s.at > stale);
    }

    /// Move the parameters of held notes that their changes say should move by now.
    fn apply_voice_sets(&mut self) {
        let now = self.clock;
        for i in 0..self.voice_sets.len() {
            let set = &self.voice_sets[i];
            if set.done || set.at > now {
                continue;
            }
            let index = self
                .program
                .instruments
                .iter()
                .find(|inst| inst.name == set.instrument)
                .and_then(|inst| inst.graph.param_index(&set.name));
            if let Some(index) = index {
                for held in self.sounding.held.iter().filter(|h| h.2 == set.key) {
                    self.mixer.set_voice_param_index(held.1, index, set.value, set.frames);
                }
            }
            self.voice_sets[i].done = true;
        }
        let stale = now.saturating_sub(self.samples(2.0));
        self.voice_sets.retain(|s| !s.done || s.at > stale);
    }

    fn release_due(&mut self) {
        let now = self.clock;
        for off in &self.key_offs {
            if off.at <= now {
                self.sounding.release_key(off.key, off.at);
            }
        }
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.sounding.midi)
            .into_iter()
            .partition(|h| h.0 <= now);
        self.sounding.midi = kept;
        for (_, _, note) in &ended {
            self.send_midi(false, note, now);
        }
        let mixer = &mut self.mixer;
        self.sounding.held.retain(|&(off, id, _)| {
            if off <= now {
                mixer.release(id);
            }
            off > now
        });
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

/// Whether a pending change sets the tempo or meter.
/// Which running voices and effect chains the new program's tracks take over from the old one's.
pub(super) fn plan_transfers(old: &Program, new: &Program, out: &mut Vec<Transfer>) {
    out.clear();
    for (i, track) in new.tracks.iter().enumerate() {
        let from = if i == 0 {
            Some(0)
        } else {
            old.track_index(&track.name)
        };
        out.push(Transfer {
            voices_from: from,
            chain_from: from.filter(|_| i > 0),
        });
    }
}

/// Keep shared buffer contents across a live re-eval when name and length match.
pub(super) fn carry_buffers(old: &Program, new: &Program) {
    for (name, next) in &new.buffers {
        if let Some((_, prev)) = old.buffers.iter().find(|(n, b)| n == name && b.len() == next.len()) {
            prev.copy_into(next);
        }
    }
}

/// Notes given in beats or bars that are still to start keep their musical position when the tempo or
/// meter changes at `now`.
fn rescale_notes(upcoming: &mut VecDeque<Upcoming>, now: u64, old: Tempo, new: Tempo) {
    if old == new {
        return;
    }
    for note in upcoming.iter_mut().filter(|u| u.start > now) {
        if let Some(by_bar) = note.musical {
            note.start = rescale_landing(note.start, now, landing_factor(by_bar, old, new));
        }
    }
    upcoming.make_contiguous().sort_by_key(|u| u.start);
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
    for p in pending.iter_mut().filter(|p| !is_timing(p)) {
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
fn origin_of(history: &[HistoryEntry]) -> f64 {
    history
        .iter()
        .rev()
        .find(|h| matches!(h.action, NamedAction::Reset | NamedAction::Origin))
        .map_or(0.0, |h| h.at)
}

fn name_action(program: &Program, action: &Action) -> Option<NamedAction> {
    let name = |t: &usize| program.tracks[*t].name.clone();
    Some(match action {
        Action::Play { track, clip } => NamedAction::Play {
            track: name(track),
            clip: clip.clone(),
        },
        Action::Stop { track } => NamedAction::Stop(name(track)),
        Action::Mute { track } => NamedAction::Mute(name(track)),
        Action::Unmute { track } => NamedAction::Unmute(name(track)),
        Action::Solo { track } => NamedAction::Solo(name(track)),
        Action::Unsolo { track } => NamedAction::Unsolo(track.as_ref().map(name)),
        Action::Hush => NamedAction::Hush,
        Action::Panic => NamedAction::Panic,
        Action::Reset => NamedAction::Reset,
        Action::Capture { .. } => return None,
    })
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        format!("{}...", line.chars().take(57).collect::<String>())
    } else {
        line.to_string()
    }
}

/// A statement to compile, and for a change to a track, what the track is playing after it.
type Edited = (Statement, Option<(String, Playing)>);
