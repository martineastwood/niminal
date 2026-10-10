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
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

use niminal_engine::{DEFAULT_MAX_VOICES, BLOCK, MAX_CHANNELS, Master, Mixer, Transfer, VoiceId};
use niminal_lang::{
    Action, Clip, CompileOptions, Layout, Samples, Program, Quantize, Schedule, Scheduled, Statement, StatementKind,
    Expanded, InputSource, analyze, compile_with,
};
use niminal_score::{Event, Tempo, Time};

use crate::log::{LogEntry, LogInput};
use crate::project::{Entry, Located, Origin, Piece, Problem, Project};
use crate::quantize::{Grid, QuantizeDefaults};

/// The master limiter's ceiling: -0.3db.
const CEILING: f32 = 0.966_051;

/// A command that has happened, by track name so that it survives the
/// program being replaced.
#[derive(Clone)]
enum NamedAction {
    Play { track: String, clip: Clip },
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

/// What compiling an evaluation needs to know about the session.
#[derive(Clone)]
struct PrepareConfig {
    layout: Layout,
    samples: Samples,
    sample_rate: f32,
}

#[derive(Clone)]
struct HistoryEntry {
    at: f64,
    action: NamedAction,
}

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
}

/// A statement that runs when a change lands, with the program it was
/// compiled in (so landing does no compiling).
struct Compiled {
    statement: Statement,
    program: Program,
}

/// An evaluation that has compiled and is waiting to land. Everything that
/// could be slow, such as compiling and building the mixer, was done when it
/// was evaluated, so that landing only has to move things into place.
struct Pending {
    id: u64,
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
}

#[derive(Clone)]
struct Waiting {
    id: u64,
    lands_at: u64,
    snippet: String,
    statements: Vec<Statement>,
}

struct Prepared {
    queue: Vec<Pending>,
    notices: Vec<String>,
}

/// The first part of an evaluation: either it is already settled, or there is
/// compiling to do.
pub enum Begin {
    Done(Result<Accepted, Vec<Problem>>),
    Job(Box<EvalJob>),
}

/// Parsed source that can be prepared without borrowing the live session.
pub struct ParsedEval {
    source: String,
    quantize: Option<String>,
    parsed: Result<(Vec<Statement>, Option<Quantize>), Vec<Problem>>,
    uses: Option<Arc<Uses>>,
}

/// The modules joined into an evaluation, and the code as it was sent, so a
/// problem in a module can be pointed at the line that uses it.
struct Uses {
    expanded: Expanded,
    sent: String,
}

impl Uses {
    fn locate(&self, problems: Vec<Problem>) -> Vec<Problem> {
        problems.into_iter().map(|p| p.in_module(&self.expanded, &self.sent)).collect()
    }
}

impl ParsedEval {
    pub fn new(source: &str, quantize: Option<&str>) -> Self {
        Self::from_text(source, quantize, None)
    }

    /// Code sent from `dir`, whose `use` lines are joined in. The log keeps the
    /// joined text, so a replay needs no files.
    pub fn from_dir(sent: &str, dir: &std::path::Path, quantize: Option<&str>) -> Self {
        match crate::project::expand_uses(sent, dir) {
            Ok(expanded) => {
                let text = expanded.text.clone();
                let uses = Arc::new(Uses { expanded, sent: sent.to_string() });
                Self::from_text(&text, quantize, Some(uses))
            }
            Err(problems) => Self { source: sent.to_string(), quantize: quantize.map(str::to_string), parsed: Err(problems), uses: None },
        }
    }

    /// The module files that were joined in.
    pub fn module_files(&self) -> Vec<std::path::PathBuf> {
        self.uses.iter().flat_map(|u| u.expanded.files().map(std::path::Path::to_path_buf)).collect()
    }

    fn from_text(source: &str, quantize: Option<&str>, uses: Option<Arc<Uses>>) -> Self {
        let parsed = (|| {
            let fallback = quantize.map(parse_quantum).transpose()?;
            let statements = analyze(source).map_err(|d| {
                let problems = vec![Problem::from_diagnostic(&d, source, Located::Snippet(d.span))];
                match &uses {
                    Some(u) => u.locate(problems),
                    None => problems,
                }
            })?;
            Ok((statements, fallback))
        })();
        Self { source: source.to_string(), quantize: quantize.map(str::to_string), parsed, uses }
    }
}

/// An evaluation waiting to be compiled. Holds copies of everything the
/// compiler needs, so compiling it touches no session.
pub struct EvalJob {
    source: String,
    uses: Option<Arc<Uses>>,
    quantize: Option<String>,
    fallback: Option<Quantize>,
    statements: Vec<Statement>,
    base: Project,
    playing: BTreeMap<String, Playing>,
    /// Which state of the session this was prepared against.
    version: u64,
    config: PrepareConfig,
    committed: Project,
    committed_playing: BTreeMap<String, Playing>,
    waiting: Vec<Waiting>,
    lands_at: u64,
    /// The later steps of a tempo ramp: how long after `lands_at` each lands, and what it does.
    ramp: Vec<(u64, Statement)>,
}

/// Ids below this are real. A ramp's steps are new transactions too, numbered from here until they are queued.
const NEW_BASE: u64 = 1 << 62;

fn is_new(id: u64) -> bool {
    id == 0 || id >= NEW_BASE
}

impl EvalJob {
    /// Compile. This is the slow step.
    pub fn compile(self) -> EvalDone {
        let result = self.prepare();
        EvalDone { job: self, result }
    }

    fn prepare(&self) -> Result<Prepared, Vec<Problem>> {
        self.prepare_all().map_err(|problems| match &self.uses {
            Some(u) => u.locate(problems),
            None => problems,
        })
    }

    fn prepare_all(&self) -> Result<Prepared, Vec<Problem>> {
        // Validate the complete transaction before superseding anything.
        prepare_pending(&self.config, 0, self.lands_at, &self.source,
            self.statements.clone(), &self.base, &self.playing)?;
        let writes: Vec<&str> = self.statements.iter().filter_map(|s| s.write_key.as_deref()).collect();
        let mut waiting = self.waiting.clone();
        for p in &mut waiting {
            p.statements.retain(|s| s.write_key.as_deref().is_none_or(|key| !writes.contains(&key)));
        }
        waiting.retain(|p| !p.statements.is_empty());
        waiting.push(Waiting { id: 0, lands_at: self.lands_at, snippet: self.source.clone(), statements: self.statements.clone() });
        for (k, (after, statement)) in self.ramp.iter().enumerate() {
            waiting.push(Waiting { id: NEW_BASE + k as u64, lands_at: self.lands_at + after, snippet: self.source.clone(), statements: vec![statement.clone()] });
        }
        waiting.sort_by_key(|p| (p.lands_at, !is_new(p.id), p.id));

        let latest = waiting.iter().map(|p| p.lands_at).max().unwrap_or(self.lands_at);
        let (mut project, mut playing) = (self.committed.clone(), self.committed_playing.clone());
        let mut queue = Vec::new();
        let mut deferred = Vec::new();
        let mut notices = Vec::new();
        for p in waiting {
            match prepare_pending(&self.config, p.id, p.lands_at, &p.snippet, p.statements.clone(), &project, &playing) {
                Ok(next) => {
                    project = next.project.clone();
                    playing = next.playing.clone();
                    queue.push(next);
                }
                // A transaction can refer to definitions waiting for a later
                // boundary. Keep it together and move it after those definitions.
                Err(_) => deferred.push(p),
            }
        }
        for p in deferred {
            match prepare_pending(&self.config, p.id, latest, &p.snippet, p.statements.clone(), &project, &playing) {
                Ok(next) => {
                    project = next.project.clone();
                    playing = next.playing.clone();
                    queue.push(next);
                }
                Err(problems) if is_new(p.id) => return Err(problems),
                Err(_) => notices.push(format!("change {} was dropped: a newer pending write invalidated it", p.id)),
            }
        }
        Ok(Prepared { queue, notices })
    }

}

/// A compiled evaluation, ready for [`Session::finish_eval`].
pub struct EvalDone {
    job: EvalJob,
    result: Result<Prepared, Vec<Problem>>,
}

pub struct CancelJob {
    id: Option<u64>,
    version: u64,
    config: PrepareConfig,
    project: Project,
    playing: BTreeMap<String, Playing>,
    waiting: Vec<Waiting>,
}

pub struct CancelDone {
    job: CancelJob,
    prepared: Prepared,
    dropped: usize,
}

impl CancelJob {
    pub fn compile(self) -> CancelDone {
        let (mut project, mut playing) = (self.project.clone(), self.playing.clone());
        let mut queue = Vec::new();
        let mut notices = Vec::new();
        let mut dropped = 0;
        for p in &self.waiting {
            if self.id.is_none_or(|id| id == p.id) { dropped += 1; continue; }
            match prepare_pending(&self.config, p.id, p.lands_at, &p.snippet,
                                  p.statements.clone(), &project, &playing) {
                Ok(next) => {
                    project = next.project.clone();
                    playing = next.playing.clone();
                    queue.push(next);
                }
                Err(_) => {
                    dropped += 1;
                    notices.push(format!("change {} was dropped: it depended on a change that was cancelled", p.id));
                }
            }
        }
        CancelDone { job: self, prepared: Prepared { queue, notices }, dropped }
    }
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

/// What arrived from outside to move a bound `ctl`. MIDI channels count from 1, and
/// an OSC value is 0 to 1 (outside is clamped).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlSignal<'a> {
    MidiCc { cc: u8, channel: u8, value: u8 },
    /// A key pressed (`velocity` from 1) or let go (`velocity` 0), on a MIDI channel from 1.
    MidiNote { note: u8, channel: u8, velocity: u8 },
    Osc { address: &'a str, value: f64 },
}

pub struct Session {
    controls: Vec<crate::controls::State>,
    control_changes: Vec<crate::controls::Change>,
    next_control: u64,
    /// The control change each `midi.learn` control took, and what is waiting to be reported.
    learned: BTreeMap<String, u8>,
    learned_out: Vec<(String, String)>,
    /// The notes each held key started, by channel and note, and the keys let go of lately.
    keys_down: BTreeMap<(u8, u8), Vec<u32>>,
    key_offs: Vec<(u32, u64)>,
    next_key: u32,
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
    /// Tracks whose clips are not searched for notes because they asked for too many.
    overloaded: Vec<usize>,
    /// Sounding notes: when each is released, and the key holding it open (or `NO_KEY`).
    held: Vec<(u64, VoiceId, u32)>,
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
}

#[path = "realtime.rs"]
pub mod realtime;

/// Compile everything `statements` will need when they land, against the
/// project as it will be then (`base`, with `base_playing` running). This is
/// the slow part of an evaluation, and needs nothing from a running session.
fn prepare_pending(
    cfg: &PrepareConfig,
    id: u64,
    lands_at: u64,
    snippet: &str,
    statements: Vec<Statement>,
    base: &Project,
    base_playing: &BTreeMap<String, Playing>,
) -> Result<Pending, Vec<Problem>> {
    let definitions: Vec<&Statement> =
        statements.iter().filter(|s| s.kind == StatementKind::Definition).collect();
    let delta: Vec<Entry> = definitions
        .iter()
        .map(|s| Entry { key: s.key.clone().expect("definitions have keys"), text: s.text.clone() })
        .collect();
    let from_snippet: HashMap<String, usize> =
        definitions.iter().map(|s| (s.key.clone().expect("definitions have keys"), s.offset)).collect();

    let mut project = base.clone();
    project.apply(&delta);
    let check = |extra: &[Piece]| -> Result<Program, Vec<Problem>> {
        let (full, map) = project.source(&from_snippet, extra);
        compile_with(&full, &options(cfg.layout, &cfg.samples)).map_err(|diags| {
            diags
                .iter()
                .map(|d| Problem::from_diagnostic(d, snippet, map.locate(d.span)))
                .collect::<Vec<_>>()
        })
    };

    let program = check(&[])?;
    if program.master != cfg.layout {
        return Err(vec![Problem::plain(format!(
            "this session outputs {}, but the project says {}; the output layout can only be chosen when the daemon starts",
            cfg.layout, program.master
        ))]);
    }

    // The commands run in order, each seeing what the ones before it left playing.
    let mut playing = base_playing.clone();
    let mut actions = Vec::new();
    for s in statements.iter().filter(|s| s.kind != StatementKind::Definition) {
        for (s, edited) in join_edit(s, &playing, &project)? {
            let piece = Piece { text: s.text.clone(), origin: Origin::Snippet { offset: s.offset } };
            let program = check(&[piece])?;
            // A command for later (`at +4 bars chorus.start`) is not what is playing now.
            let later = program.performance.iter().any(|p| p.at.to_seconds(program.tempo) > 0.0);
            if !later {
                apply_to_playing(&mut playing, &program, &s);
                if let Some(scene) = &s.launch {
                    apply_launch(&mut playing, &program, &project, scene);
                }
                if let Some((track, edited)) = edited {
                    playing.insert(track, edited);
                }
            }
            actions.push(Compiled { statement: s, program });
        }
    }

    let changed: Vec<String> = definitions.iter().filter_map(|s| s.binding.clone()).collect();
    // Tracks that play something this change touches start it over, unless the commands
    // here already start them.
    let relaunched = actions.iter().any(|a| a.statement.launch.is_some());
    let mut replays = Vec::new();
    for (track, now) in playing.clone() {
        let started = relaunched || actions.iter().any(|a| a.statement.plays.as_ref().is_some_and(|p| p.track == track));
        let what = now.text();
        if started || !mentions_any(&what, &changed) {
            continue;
        }
        let text = format!("{track}.play({what})");
        let Some(statement) = analyze(&text).ok().and_then(|mut s| s.pop()) else { continue };
        let piece = Piece { text: text.clone(), origin: Origin::Snippet { offset: 0 } };
        // If the new version no longer works for the track, leave it playing as it was.
        if let Ok(program) = check(&[piece]) {
            replays.push(Compiled { statement, program });
        }
    }

    let has_definitions = !definitions.is_empty();
    let mixer = has_definitions.then(|| program.mixer(cfg.sample_rate));
    Ok(Pending {
        id,
        lands_at,
        snippet: snippet.to_string(),
        summary: statements.iter().map(summarize).collect(),
        statements,
        project,
        program: has_definitions.then(|| Arc::new(program)),
        mixer,
        actions,
        replays,
        playing,
    })
}


/// The most notices that wait to be collected, and how long before the same one is said again.
const MAX_NOTICES: usize = 100;
const NOTICE_REPEAT_SECONDS: u64 = 10;

fn options(layout: Layout, samples: &Samples) -> CompileOptions {
    CompileOptions { default_layout: layout, samples: samples.clone() }
}

impl Session {
    /// A session that outputs `layout` at `sample_rate`, with the limiter on.
    pub fn new(sample_rate: f32, layout: Layout) -> Session {
        let program = compile_with("", &options(layout, &Samples::default())).expect("an empty project compiles");
        let mixer = program.mixer(sample_rate);
        let schedule = program.schedule();
        Session {
            sample_rate,
            layout,
            panic_epoch: Arc::new(AtomicU64::new(0)),
            samples: Samples::default(),
            controls: Vec::new(), control_changes: Vec::new(), next_control: 0, learned: BTreeMap::new(), learned_out: Vec::new(),
            keys_down: BTreeMap::new(), key_offs: Vec::new(), next_key: 0,
            sample_dirs: Vec::new(),
            defaults: QuantizeDefaults::default(),
            clock: 0,
            project: Project::default(),
            program: Arc::new(program),
            mixer,
            limiter: Some(Master::with_channels(sample_rate, CEILING, layout.channels())),
            history: vec![HistoryEntry { at: 0.0, action: NamedAction::Origin }],
            schedule,
            fetched_until: 0,
            upcoming: VecDeque::new(),
            overloaded: Vec::new(),
            held: Vec::new(),
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
        }
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
        self.limiter.as_ref().map(Master::latency).unwrap_or_else(|| self.live.as_ref().map_or(0, |p| p.state.latency))
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

    /// Evaluate `source`. If it compiles, its changes are queued to land at the
    /// quantum each asks for (or `quantize`, or the default for its kind), all
    /// together at the latest of them. If it does not, nothing changes.
    pub fn eval(&mut self, source: &str, quantize: Option<&str>) -> Result<Accepted, Vec<Problem>> {
        self.eval_parsed(ParsedEval::new(source, quantize))
    }

    pub fn eval_parsed(&mut self, input: ParsedEval) -> Result<Accepted, Vec<Problem>> {
        match self.begin_parsed(input) {
            Begin::Done(result) => result,
            Begin::Job(job) => self.finish_eval(job.compile()),
        }
    }

    /// The quick first part of [`Session::eval`]: read the code and note what
    /// the project is like now. Compiling is the slow part, and
    /// [`EvalJob::compile`] needs no access to the session, so a daemon can do
    /// it without holding the render worker's control lock.
    pub fn begin_eval(&mut self, source: &str, quantize: Option<&str>) -> Begin {
        self.begin_parsed(ParsedEval::new(source, quantize))
    }

    pub fn begin_parsed(&mut self, input: ParsedEval) -> Begin {
        self.sync_realtime();
        let ParsedEval { source, quantize, parsed, uses } = input;
        let (statements, fallback) = match parsed {
            Ok(parsed) => parsed,
            Err(problems) => {
                self.log_eval(&source, quantize.as_deref());
                return Begin::Done(Err(problems));
            }
        };
        if statements.is_empty() {
            let nothing = Accepted { id: None, lands_at: self.clock, position: self.grid().position(self.clock), changes: vec![] };
            self.log_eval(&source, quantize.as_deref());
            return Begin::Done(Ok(nothing));
        }
        let mut statements = statements;
        let ramp = match self.expand_ramp(&mut statements) {
            Ok(ramp) => ramp,
            Err(problems) => {
                self.log_eval(&source, quantize.as_deref());
                return Begin::Done(Err(problems));
            }
        };
        let (base, playing) = self.projected();
        Begin::Job(Box::new(EvalJob {
            source,
            uses,
            quantize,
            fallback,
            statements: statements.clone(),
            ramp,
            base,
            playing,
            version: self.version,
            config: self.prepare_config(),
            committed: self.project.clone(),
            committed_playing: self.playing.clone(),
            waiting: self.pending.iter().map(|p| Waiting { id: p.id, lands_at: p.lands_at,
                snippet: p.snippet.clone(), statements: p.statements.clone() }).collect(),
            lands_at: self.landing(&statements, fallback),
        }))
    }

    /// `tempo 124bpm @ next bar over 8 bars` becomes a tempo for each quarter beat on the way:
    /// the first replaces the statement, and the rest are returned with how long after it each lands.
    fn expand_ramp(&self, statements: &mut [Statement]) -> Result<Vec<(u64, Statement)>, Vec<Problem>> {
        let Some(index) = statements.iter().position(|s| s.ramp.is_some()) else { return Ok(Vec::new()) };
        let Some(ramp) = statements[index].ramp.take() else { return Ok(Vec::new()) };
        let tempo = self.program.tempo;
        let beats = ramp.over.to_seconds(tempo) * tempo.bpm / 60.0;
        let steps = (beats * RAMP_STEPS_PER_BEAT).ceil();
        if !(1.0..=MAX_RAMP_STEPS).contains(&steps) {
            return Err(vec![Problem::plain(format!(
                "a tempo ramp takes from a quarter of a beat up to {} beats", MAX_RAMP_STEPS / RAMP_STEPS_PER_BEAT
            ))]);
        }
        let steps = steps as usize;
        let step_text = |bpm: f64| format!("tempo {bpm:.3}bpm");
        let (mut extras, mut after) = (Vec::new(), 0u64);
        for k in 0..steps {
            let bpm = tempo.bpm + (ramp.bpm - tempo.bpm) * (k + 1) as f64 / steps as f64;
            if k == 0 {
                statements[index].text = step_text(bpm);
            } else {
                let mut step = statements[index].clone();
                step.text = step_text(bpm);
                step.quantize = None;
                extras.push((after, step));
            }
            after += self.samples(60.0 / bpm / RAMP_STEPS_PER_BEAT);
        }
        Ok(extras)
    }

    fn landing(&self, statements: &[Statement], fallback: Option<Quantize>) -> u64 {
        let grid = self.grid();
        statements.iter().map(|s| grid.landing(self.defaults.for_statement(s, fallback), self.clock))
            .max().expect("an evaluation has statements")
    }

    /// Return a fresh compilation job when the project changed while compiling.
    /// Servers run this job outside their control lock.
    pub fn retry_eval(&self, done: &EvalDone) -> Option<Box<EvalJob>> {
        let job = &done.job;
        if job.version == self.version { return None; }
        let (base, playing) = self.projected();
        Some(Box::new(EvalJob {
            source: job.source.clone(), uses: job.uses.clone(), quantize: job.quantize.clone(), fallback: job.fallback,
            statements: job.statements.clone(), ramp: job.ramp.clone(), base, playing, version: self.version,
            config: self.prepare_config(), committed: self.project.clone(),
            committed_playing: self.playing.clone(),
            waiting: self.pending.iter().map(|p| Waiting { id: p.id, lands_at: p.lands_at,
                snippet: p.snippet.clone(), statements: p.statements.clone() }).collect(),
            lands_at: self.landing(&job.statements, job.fallback),
        }))
    }

    /// Queue a compiled transaction. The synchronous API retries inline;
    /// servers use `retry_eval` to do that compilation outside their lock.
    pub fn finish_eval(&mut self, mut done: EvalDone) -> Result<Accepted, Vec<Problem>> {
        self.sync_realtime();
        while let Some(job) = self.retry_eval(&done) { done = job.compile(); }
        let EvalDone { job, result } = done;
        self.log_eval(&job.source, job.quantize.as_deref());
        let Prepared { mut queue, notices } = result?;
        let landing = self.landing(&job.statements, job.fallback);
        let pending = queue.iter_mut().find(|p| p.id == 0).expect("the new transaction is queued");
        // A compile that missed its grid waits for the next one. `now` lands
        // when compilation completes, rather than at its original clock sample.
        pending.lands_at = pending.lands_at.max(landing);
        pending.id = self.next_id;
        self.next_id += 1;
        let first_at = pending.lands_at;
        let accepted = Accepted { id: Some(pending.id), lands_at: pending.lands_at,
            position: self.grid().position(pending.lands_at), changes: pending.summary.clone() };
        // Preserve chronological snapshots if a compile crossed a boundary.
        // Existing transactions already prepared against this one cannot land
        // before it. Their original order remains deterministic.
        // the steps of a tempo ramp follow the first by the spacing worked out when it was written
        for step in queue.iter_mut().filter(|p| p.id >= NEW_BASE) {
            step.lands_at = first_at + job.ramp[(step.id - NEW_BASE) as usize].0;
            step.id = self.next_id;
            self.next_id += 1;
        }
        let new_index = queue.iter().position(|p| p.id == accepted.id.unwrap()).unwrap();
        let mut at = queue[new_index].lands_at;
        for p in &mut queue[new_index + 1..] { p.lands_at = p.lands_at.max(at); at = p.lands_at; }
        self.pending = queue;
        self.invalidate_live();
        if accepted.lands_at > job.lands_at && self.clock > job.lands_at
            && job.statements.iter().any(|s| self.defaults.for_statement(s, job.fallback).unit != niminal_lang::QuantizeUnit::Now)
        {
            self.notify(format!("change {} missed its boundary while compiling; moved to bar {} beat {:.2}",
                accepted.id.unwrap(), accepted.position.0, accepted.position.1));
        }
        for notice in notices { self.notify(notice); }
        self.version += 1;
        self.land_due();
        Ok(accepted)
    }

    fn log_eval(&mut self, source: &str, quantize: Option<&str>) {
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::Eval { source: source.to_string(), quantize: quantize.map(str::to_string) },
        });
    }

    fn prepare_config(&self) -> PrepareConfig {
        PrepareConfig { layout: self.layout, samples: self.samples.clone(), sample_rate: self.sample_rate }
    }

    /// Cancel a waiting evaluation, or all of them. Returns how many. What is
    /// left is compiled again, since it may have depended on what was cancelled.
    pub fn cancel(&mut self, id: Option<u64>) -> usize {
        let done = self.begin_cancel(id).compile();
        self.finish_cancel(done)
    }

    pub fn begin_cancel(&self, id: Option<u64>) -> Box<CancelJob> {
        Box::new(CancelJob { id, version: self.version, config: self.prepare_config(),
            project: self.project.clone(), playing: self.playing.clone(),
            waiting: self.pending.iter().map(|p| Waiting { id: p.id, lands_at: p.lands_at,
                snippet: p.snippet.clone(), statements: p.statements.clone() }).collect() })
    }

    pub fn retry_cancel(&self, done: &CancelDone) -> Option<Box<CancelJob>> {
        (done.job.version != self.version).then(|| self.begin_cancel(done.job.id))
    }

    pub fn finish_cancel(&mut self, mut done: CancelDone) -> usize {
        self.sync_realtime();
        while let Some(job) = self.retry_cancel(&done) { done = job.compile(); }
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Cancel { id: done.job.id } });
        self.pending = done.prepared.queue;
        self.invalidate_live();
        for notice in done.prepared.notices { self.notify(notice); }
        self.version += 1;
        done.dropped
    }

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
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Panic });
        let at = self.seconds(self.clock);
        self.history.push(HistoryEntry { at, action: NamedAction::Panic });
        self.playing.clear();
        self.rebuild_schedule();
        self.apply_panics();
    }

    /// Stop every clip, leaving what is sounding to ring out.
    pub fn hush(&mut self) -> Result<Accepted, Vec<Problem>> {
        self.eval("hush", Some("now"))
    }

    /// Notes from outside, such as another program playing live. Each carries
    /// the time (since the session started) at which it should sound; ones that
    /// are already late sound immediately.
    pub fn stream_events(&mut self, events: Vec<Event>) -> Result<(), Vec<Problem>> {
        self.sync_realtime();
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Events { events: events.clone() } });
        let mut problems = Vec::new();
        for event in &events {
            if let Err(e) = self.program.plan(event) {
                problems.push(Problem::plain(format!("note for `{}`: {e}", event.target)));
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        self.invalidate_live();
        for event in events {
            let start = self.samples(event.at.to_seconds(self.program.tempo)).max(self.clock);
            let dur = self.samples(event.dur.to_seconds(self.program.tempo));
            self.insert_upcoming(Upcoming { start, dur, event, key: None, scheduled: false });
        }
        Ok(())
    }

    /// Queue a typed control update at an absolute sample, or at the render
    /// clock when no timestamp is given. No parsing or graph compilation occurs.
    pub fn set_control(&mut self, name: &str, value: f64, unit: Option<&str>, at: Option<u64>, smooth_ms: Option<f64>) -> Result<u64, Vec<Problem>> {
        self.sync_realtime();
        let error = |message: String| vec![Problem::plain(message)];
        let index = self.program.controls.iter().position(|c| c.name == name)
            .ok_or_else(|| error(format!("unknown control `{name}`")))?;
        let def = &self.program.controls[index];
        let converted = def.convert(value, unit, self.program.tempo).map_err(error)?;
        let at = at.unwrap_or(self.clock);
        if at < self.clock { return Err(error("a control timestamp cannot be in the past".into())); }
        let seconds = smooth_ms.map_or(def.smooth_seconds, |ms| ms / 1000.0);
        if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
            return Err(error("control smoothing must be between 0ms and 60sec".into()));
        }
        if self.control_changes.len() >= crate::controls::CAPACITY {
            return Err(error("the control update queue is full".into()));
        }
        let change = crate::controls::Change { index, unit: def.unit, revision: self.controls[index].revision, at, value: converted,
            frames: (seconds * f64::from(self.sample_rate)).round() as u64, serial: self.next_control, log_index: self.log.len(), applied: !self.detached };
        if let Some(planner) = &mut self.live {
            planner.push_control(change).map_err(|_| error("the control update queue is full".into()))?;
        }
        self.next_control += 1;
        self.control_changes.push(change);
        self.control_changes.sort_unstable_by_key(|c| (c.at, c.serial));
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Control {
            name: name.into(), value, unit: unit.map(str::to_owned), at, smooth_ms,
        } });
        Ok(at)
    }

    /// A message from a MIDI device or OSC sender. Every control bound to it moves,
    /// exactly as if `control.set` had been called, so it is logged and replays.
    /// Returns how many controls took it.
    pub fn control_input(&mut self, signal: &ControlSignal) -> usize {
        if let ControlSignal::MidiNote { note, channel, velocity } = signal {
            return self.key_input(*channel, *note, *velocity);
        }
        self.learn(signal);
        let targets: Vec<(String, f64, Option<&'static str>)> = self.program.controls.iter().filter_map(|def| {
            let binding = def.input.as_ref()?;
            // a learning control listens to the control change it was given
            let learned = self.learned.get(&def.name).map(|&cc| InputSource::MidiCc { cc, channel: None });
            let source = if binding.source == InputSource::MidiLearn { learned.as_ref()? } else { &binding.source };
            let t = match (source, signal) {
                (InputSource::MidiCc { cc, channel }, ControlSignal::MidiCc { cc: got, channel: on, value }) =>
                    (cc == got && channel.is_none_or(|c| c == *on)).then(|| f64::from(*value) / 127.0)?,
                (InputSource::Osc(address), ControlSignal::Osc { address: got, value }) =>
                    (address == got && value.is_finite()).then_some(*value)?,
                _ => return None,
            };
            Some((def.name.clone(), def.map_input(t)?, def.unit.suffix()))
        }).collect();
        targets.iter().filter(|(name, value, unit)| self.set_control(name, *value, *unit, None, None).is_ok()).count()
    }

    /// A key pressed (`velocity` from 1) or let go (0) on a MIDI keyboard. Every part whose clip is
    /// played from keys on this channel (`keys: midi.keys(channel: 1)`) plays the note until the key
    /// comes up. Returns how many parts took it. It is logged, so a set replays.
    pub fn key_input(&mut self, channel: u8, note: u8, velocity: u8) -> usize {
        self.sync_realtime();
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Key { channel, note, velocity } });
        self.key_event(channel, note, velocity)
    }

    fn key_event(&mut self, channel: u8, note: u8, velocity: u8) -> usize {
        if velocity == 0 {
            let keys = self.keys_down.remove(&(channel, note)).unwrap_or_default();
            for &key in &keys {
                self.release_key(key);
            }
            return keys.len();
        }
        // pressing a key that is still down lets the first one go
        for key in self.keys_down.remove(&(channel, note)).unwrap_or_default() {
            self.release_key(key);
        }
        let now = self.seconds(self.clock);
        let gain = -30.0 + 30.0 * f64::from(velocity.min(127)) / 127.0;
        let events: Vec<Event> = (1..self.program.tracks.len())
            .filter_map(|track| self.schedule.key_event(track, now, channel, i32::from(note), gain))
            .collect();
        let mut keys = Vec::new();
        for event in events {
            if let Err(e) = self.program.plan(&event) {
                self.notify(format!("key for `{}` could not play: {e}", event.target));
                continue;
            }
            let key = self.next_key;
            self.next_key = self.next_key.wrapping_add(1) % NO_KEY;
            let start = self.samples(event.at.to_seconds(self.program.tempo)).max(self.clock);
            self.insert_upcoming(Upcoming { start, dur: 0, event, key: Some(key), scheduled: false });
            keys.push(key);
        }
        self.invalidate_live();
        let taken = keys.len();
        if taken > 0 {
            self.keys_down.insert((channel, note), keys);
        }
        taken
    }

    /// Let go of the note a key started: now, if it is sounding, or as soon as it starts.
    fn release_key(&mut self, key: u32) {
        let stale = self.clock.saturating_sub(self.samples(2.0));
        self.key_offs.retain(|&(_, at)| at > stale);
        self.key_offs.push((key, self.clock));
        for held in self.held.iter_mut().filter(|h| h.2 == key) {
            held.0 = self.clock;
        }
        // a note that has not started yet is played as a blip, long enough to hear
        let blip = self.samples(0.01);
        for note in self.upcoming.iter_mut().filter(|u| u.key == Some(key)) {
            note.key = None;
            note.dur = blip;
        }
        self.invalidate_live();
    }

    /// The first control waiting on `midi.learn` takes the control change that just arrived.
    fn learn(&mut self, signal: &ControlSignal) {
        let ControlSignal::MidiCc { cc, .. } = signal else { return };
        let waiting = self.program.controls.iter()
            .find(|d| d.input.as_ref().is_some_and(|i| i.source == InputSource::MidiLearn) && !self.learned.contains_key(&d.name))
            .map(|d| d.name.clone());
        if let Some(name) = waiting {
            let source = format!("midi.cc({cc})");
            self.notify(format!("learned: ctl {name} = {source}"));
            self.learned.insert(name.clone(), *cc);
            self.learned_out.push((name, source));
        }
    }

    /// The controls that learned a control change since this was last called, with what
    /// `midi.learn` should be rewritten to.
    pub fn take_learned(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.learned_out)
    }

    pub fn control_values(&self) -> Vec<(String, f32, f32, Option<&'static str>)> {
        self.program.controls.iter().zip(&self.controls)
            .map(|(def, state)| (def.name.clone(), state.value(self.clock), state.target, def.unit.suffix())).collect()
    }

    fn apply_controls(&mut self) {
        while let Some(index) = self.control_changes.iter().position(|c| c.at <= self.clock && c.applied) {
            let change = self.control_changes.remove(index);
            if self.program.controls.get(change.index).is_some_and(|d| d.unit == change.unit)
                && let Some(state) = self.controls.get_mut(change.index)
            {
                state.set(change.at, change.value, change.frames);
                if !self.detached {
                    self.mixer.set_control(&state.key, state.start, state.target, state.frames, self.clock.saturating_sub(state.at));
                }
            } else { self.notify("a queued control update was dropped after its unit changed".into()); }
        }
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

    pub fn transport(&self) -> Transport {
        let grid = self.grid();
        let (bar, beat) = grid.position(self.clock);
        Transport {
            sample: self.clock,
            seconds: self.seconds(self.clock),
            bar,
            beat,
            bpm: grid.bpm,
            voices: self.live.as_ref().map_or_else(|| self.mixer.active_voices(), |p| p.state.voices.load(Ordering::Relaxed)),
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
                TrackStatus {
                    name: t.name.clone(),
                    clip: state.clip,
                    muted: state.muted,
                    soloed: state.soloed,
                    edits: self.playing.get(&t.name).map(|p| p.edits.clone()).unwrap_or_default(),
                }
            })
            .collect()
    }

    /// The names of the project's definitions of one kind (`scene`, `clip`...), in order.
    pub fn defined(&self, kind: &str) -> Vec<String> {
        let prefix = format!("{kind}:");
        self.project.entries.iter().filter_map(|e| e.key.strip_prefix(&prefix)).map(str::to_string).collect()
    }

    /// Changes that have landed since this was last called.
    pub fn take_landed(&mut self) -> Vec<Landed> {
        std::mem::take(&mut self.landed)
    }

    /// Queue a notice, unless the same one is already waiting: a problem that
    /// recurs every fraction of a second must not bury everything else.
    fn notify(&mut self, notice: String) {
        let quiet_until = u64::from(self.sample_rate as u32) * NOTICE_REPEAT_SECONDS;
        self.recent_notices.retain(|(_, at)| self.clock.saturating_sub(*at) < quiet_until);
        if self.notices.len() < MAX_NOTICES && !self.recent_notices.iter().any(|(n, _)| *n == notice) {
            self.recent_notices.push((notice.clone(), self.clock));
            self.notices.push(notice);
        }
    }

    /// Things worth telling the user that aren't errors in what they sent: a
    /// note that couldn't play, a change that failed to land.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
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
    pub fn log_version(&self) -> (usize, u64) { (self.log.len(), self.log_corrections) }

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
        while self.pending.first().is_some_and(|p| p.lands_at <= self.clock) {
            let pending = self.pending.remove(0);
            self.land(pending);
        }
    }

    fn land(&mut self, p: Pending) {
        self.version += 1;
        let at = self.clock;
        let Pending { id, statements, summary, project, program, mixer, actions, replays, playing, .. } = p;

        if let (Some(program), Some(mixer)) = (program, mixer) {
            let now = self.seconds(at);
            retime(&mut self.history, now, self.program.tempo, program.tempo);
            self.swap_program(project, program, mixer, &statements, id);
        }
        for a in actions.iter().chain(&replays) {
            self.run_action(a, at);
        }
        self.playing = playing;

        self.rebuild_schedule();
        self.landed.push(Landed { id, at, position: self.grid().position(at), changes: summary });
    }

    /// Replace the running program, keeping what is sounding.
    fn swap_program(&mut self, project: Project, program: Arc<Program>, mut mixer: Mixer, statements: &[Statement], revision: u64) {
        let mut controls = crate::controls::build(&program);
        crate::controls::mark(&mut controls, &program, statements, revision);
        crate::controls::adopt(&mut controls, &self.controls, &program, self.clock, self.sample_rate);
        self.controls = controls;
        if self.detached {
            self.program = program;
            self.project = project;
            return;
        }
        let mapping: Vec<Option<usize>> = self.program.buses.iter().enumerate().map(|(i, name)| {
            program.buses.iter().position(|n| n == name)
                .filter(|&j| self.program.bus_layouts[i] == program.bus_layouts[j])
        }).collect();
        self.mixer.remap_buses(&mapping);
        let transfers = self.plan_transfers(&project, &program);
        mixer.adopt(&mut self.mixer, &transfers);
        crate::controls::sync_mixer(&self.controls, &mut mixer, self.clock);
        self.mixer = mixer;
        self.program = program;
        self.project = project;
    }

    /// Which running voices and effect chains the new program's tracks take over.
    fn plan_transfers(&self, _next: &Project, program: &Program) -> Vec<Transfer> {
        let old = &self.program;

        program
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let from = if i == 0 { Some(0) } else { old.track_index(&track.name) };
                Transfer {
                    voices_from: from,
                    chain_from: from.filter(|_| i > 0),
                }
            })
            .collect()
    }

    /// Carry out one `play`, `mute`, note... statement that has landed. It was
    /// compiled when it was evaluated.
    fn run_action(&mut self, action: &Compiled, at: u64) {
        let (statement, program) = (&action.statement, &action.program);
        let tempo = program.tempo;
        let landing_seconds = self.seconds(at);
        let origin = origin_of(&self.history);

        for event in &program.notes {
            let offset = event.at.to_seconds(tempo);
            let start = if statement.has_at { self.samples(origin + offset) } else { at + self.samples(offset) };
            let dur = self.samples(event.dur.to_seconds(tempo));
            self.insert_upcoming(Upcoming { start: start.max(at), dur, event: event.clone(), key: None, scheduled: false });
        }
        for scheduled in &program.performance {
            let after = scheduled.at.to_seconds(tempo);
            let when = if statement.has_at { origin + after } else { landing_seconds + after };
            self.history.push(HistoryEntry { at: when, action: name_action(program, &scheduled.action) });
        }
    }

    fn rebuild_schedule(&mut self) {
        self.schedule = realtime::schedule_for(&self.program, &self.history);
        self.overloaded.clear();
        // Notes already taken from the old schedule may no longer be right.
        self.upcoming.retain(|u| !u.scheduled);
        self.fetched_until = self.clock;
        self.next_panic = self.schedule.panics().iter().take_while(|&&t| self.samples(t) < self.clock).count();
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
            if let Some(limiter) = &mut self.limiter { limiter.clear(); }
            self.held.clear();
            self.upcoming.clear();
            self.next_panic += 1;
        }
    }

    /// Mix `frames` samples, calling `sink` with each piece (a block per
    /// channel and how many of its samples are valid).
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[[f32; BLOCK]], usize)) {
        assert!(!self.detached, "a detached session is driven by its renderer");
        let end = self.clock + frames as u64;
        let channels = self.layout.channels();
        let mut block = [[0.0f32; BLOCK]; MAX_CHANNELS];

        while self.clock < end {
            self.land_due();
            self.apply_panics();
            self.apply_controls();
            self.release_due();

            // How far we can go before something needs doing.
            let mut n = (end - self.clock).min(BLOCK as u64);
            if let Some(p) = self.pending.first() {
                n = n.min(p.lands_at - self.clock);
            }
            if let Some(change) = self.control_changes.first() {
                n = n.min(change.at.saturating_sub(self.clock).max(1));
            }
            for &(off, _, _) in &self.held {
                n = n.min(off.saturating_sub(self.clock).max(1));
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
            self.start_due();

            let n = n.max(1) as usize;
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
                LogInput::Control { name, value, unit, at, smooth_ms } => {
                    let _ = session.set_control(name, *value, unit.as_deref(), Some(*at), *smooth_ms);
                }
                LogInput::Events { events } => {
                    let _ = session.stream_events(events.clone());
                }
                LogInput::Key { channel, note, velocity } => {
                    session.key_input(*channel, *note, *velocity);
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
        for event in events {
            let start = self.samples(event.at.to_seconds(self.program.tempo)).max(self.clock);
            let dur = self.samples(event.dur.to_seconds(self.program.tempo));
            self.insert_upcoming(Upcoming { start, dur, event, key: None, scheduled: true });
        }
        if let Some(e) = problem {
            self.notify(e.to_string());
        }
        self.fetched_until = horizon;
    }

    fn start_due(&mut self) {
        while self.upcoming.front().is_some_and(|u| u.start <= self.clock) {
            let Some(note) = self.upcoming.pop_front() else { break };
            match self.program.plan(&note.event) {
                Ok(plan) => {
                    let graph = self.program.instruments[plan.instrument].graph.clone();
                    let id = self.mixer.note_on_with(plan.track, graph, &plan.params, plan.choke, plan.rule);
                    crate::controls::sync_note(&self.controls, &mut self.mixer, id, self.clock);
                    let off = if note.key.is_some() { u64::MAX } else { self.clock + note.dur };
                    self.held.push((off, id, note.key.unwrap_or(NO_KEY)));
                    if self.mixer.take_stolen() > 0 {
                        self.notify(format!("too many notes at once: the oldest are ended early to keep to {} voices", DEFAULT_MAX_VOICES));
                    }
                }
                Err(e) => self.notify(format!("note for `{}` could not play: {e}", note.event.target)),
            }
        }
    }

    fn release_due(&mut self) {
        let now = self.clock;
        let mixer = &mut self.mixer;
        self.held.retain(|&(off, id, _)| {
            if off <= now {
                mixer.release(id);
            }
            off > now
        });
    }
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
    history.iter().rev().find(|h| matches!(h.action, NamedAction::Reset | NamedAction::Origin)).map_or(0.0, |h| h.at)
}

fn name_action(program: &Program, action: &Action) -> NamedAction {
    let name = |t: &usize| program.tracks[*t].name.clone();
    match action {
        Action::Play { track, clip } => NamedAction::Play { track: name(track), clip: clip.clone() },
        Action::Stop { track } => NamedAction::Stop(name(track)),
        Action::Mute { track } => NamedAction::Mute(name(track)),
        Action::Unmute { track } => NamedAction::Unmute(name(track)),
        Action::Solo { track } => NamedAction::Solo(name(track)),
        Action::Unsolo { track } => NamedAction::Unsolo(track.as_ref().map(name)),
        Action::Hush => NamedAction::Hush,
        Action::Panic => NamedAction::Panic,
        Action::Reset => NamedAction::Reset,
    }
}

/// `@ next 4 bars` as text, for an override.
fn parse_quantum(text: &str) -> Result<Quantize, Vec<Problem>> {
    analyze(&format!("hush @ {text}"))
        .ok()
        .and_then(|mut s| s.pop())
        .and_then(|s| s.quantize)
        .ok_or_else(|| {
            vec![Problem::plain(format!(
                "`{text}` isn't a quantum: use `now`, `beat`, `bar`, `cycle`, `next 4 bars`, `in 2 beats`..."
            ))]
        })
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 { format!("{}...", line.chars().take(57).collect::<String>()) } else { line.to_string() }
}

fn summarize(s: &Statement) -> String {
    first_line(&s.text)
}

/// Does `source` use any of `names` as a name?
fn mentions_any(source: &str, names: &[String]) -> bool {
    source
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| names.iter().any(|n| n == word))
}

/// What a track is playing: a clip, as written, and the changes made to it live with
/// `bass.reverse` and the like, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Playing {
    base: String,
    edits: Vec<String>,
}

/// The most changes one track can carry.
const MAX_EDITS: usize = 16;

impl Playing {
    fn new(base: impl Into<String>) -> Playing {
        Playing { base: base.into(), edits: Vec::new() }
    }

    /// `riff.reverse.thin(25%)`
    fn text(&self) -> String {
        self.edits.iter().fold(self.base.clone(), |text, edit| format!("{text}.{edit}"))
    }
}

/// A statement to compile, and for a change to a track, what the track is playing after it.
type Edited = (Statement, Option<(String, Playing)>);

/// `bass.reverse` becomes `bass.play(<what bass is playing>.reverse)`, `bass.undo` takes
/// the last change off, `bass.undo(reverse)` the last `reverse`, and `bass.clear` all of
/// them. Returns the statements to compile and what each track is playing after them.
///
/// The name before the dot is usually a part. For a clip that is only material, such as
/// `answer`, the change goes to every part playing it.
fn join_edit(statement: &Statement, playing: &BTreeMap<String, Playing>, project: &Project) -> Result<Vec<Edited>, Vec<Problem>> {
    let (Some(range), Some(plays)) = (&statement.edit, &statement.plays) else { return Ok(vec![(statement.clone(), None)]) };
    let name = plays.track.clone();
    if plays.what.trim() == "start" {
        // `verse.start` launches the scene of that name, `bass.start` plays the clip of that name
        let mut statement = statement.clone();
        statement.edit = None;
        if project.text_of(&format!("scene:{name}")).is_some() {
            statement.plays = None;
            statement.launch = Some(name);
            statement.write_key = Some("launch".into());
        } else {
            statement.write_key = Some(format!("play:{name}"));
            statement.plays = Some(niminal_lang::PlayInfo { track: name.clone(), what: name });
        }
        return Ok(vec![(statement, None)]);
    }
    let parts: Vec<&String> = if playing.contains_key(&name) {
        vec![&name]
    } else {
        playing.iter().filter(|(_, p)| root_name(&p.base) == name).map(|(track, _)| track).collect()
    };
    if parts.is_empty() {
        return Err(vec![Problem::plain(format!(
            "`{name}` isn't playing anything to change; start it first, for example `{name}.start`, or start a scene that has it"
        ))]);
    }
    let mut out = Vec::new();
    for track in parts {
        let next = change_playing(&playing[track], plays.what.trim(), track)?;
        let text = next.text();
        let mut one = statement.clone();
        one.text.replace_range(range.clone(), &format!("{track}.play({text})"));
        one.plays = Some(niminal_lang::PlayInfo { track: track.clone(), what: text });
        one.write_key = Some(format!("play:{track}"));
        one.edit = None;
        out.push((one, Some((track.clone(), next))));
    }
    Ok(out)
}

/// What a track plays after `change`: one more change, or `undo`, `undo(name)` or `clear`.
fn change_playing(current: &Playing, change: &str, track: &str) -> Result<Playing, Vec<Problem>> {
    let mut next = current.clone();
    let problem = |message: String| Err(vec![Problem::plain(message)]);
    match change {
        "undo" => {
            if next.edits.pop().is_none() {
                return problem(format!("nothing to undo: `{track}` has no changes made to it"));
            }
        }
        "clear" => next.edits.clear(),
        named if named.starts_with("undo(") && named.ends_with(')') => {
            let wanted = named[5..named.len() - 1].trim();
            match next.edits.iter().rposition(|edit| root_name(edit) == wanted) {
                Some(at) => {
                    next.edits.remove(at);
                }
                None => return problem(format!("`{track}` has no `{wanted}` change to undo")),
            }
        }
        change if next.edits.len() >= MAX_EDITS => {
            return problem(format!(
                "`{track}` already has {MAX_EDITS} changes (`{change}` would be one more); `{track}.undo` or `{track}.clear` takes some off"
            ));
        }
        change => next.edits.push(change.to_string()),
    }
    Ok(next)
}

/// The first name in an expression: `riff` in `riff.reverse.thin(25%)`, `thin` in `thin(25%)`.
fn root_name(text: &str) -> &str {
    let text = text.trim_start();
    let end = text.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(text.len());
    &text[..end]
}

/// Record what a launched scene plays: its tracks play what it says, and every other
/// track with an instrument stops.
fn apply_launch(playing: &mut BTreeMap<String, Playing>, program: &Program, project: &Project, scene: &str) {
    let Some(text) = project.text_of(&format!("scene:{scene}")) else { return };
    let entries = niminal_lang::scene_entries(text);
    for track in program.tracks.iter().filter(|t| t.instrument.is_some()) {
        if !entries.iter().any(|(name, _)| *name == track.name) {
            playing.remove(&track.name);
        }
    }
    for (track, what) in entries {
        match what {
            Some(what) => playing.insert(track, Playing::new(what)),
            None => playing.remove(&track),
        };
    }
}

/// Update the record of what each track is playing for a statement that has
/// been compiled: `play` records its source, `stop` and `hush` forget.
fn apply_to_playing(playing: &mut BTreeMap<String, Playing>, program: &Program, statement: &Statement) {
    for scheduled in &program.performance {
        match name_action(program, &scheduled.action) {
            NamedAction::Play { track, .. } => {
                if let Some(plays) = statement.plays.as_ref().filter(|p| p.track == track) {
                    playing.insert(track, Playing::new(plays.what.clone()));
                }
            }
            NamedAction::Stop(track) => {
                playing.remove(&track);
            }
            NamedAction::Hush | NamedAction::Panic | NamedAction::Reset => playing.clear(),
            _ => {}
        }
    }
}
