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
    analyze, compile_with,
};
use niminal_score::{Event, Time};

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

/// A note waiting for its moment.
struct Upcoming {
    start: u64,
    dur: u64,
    event: Event,
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
    playing: BTreeMap<String, String>,
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
}

impl ParsedEval {
    pub fn new(source: &str, quantize: Option<&str>) -> Self {
        let parsed = (|| {
            let fallback = quantize.map(parse_quantum).transpose()?;
            let statements = analyze(source).map_err(|d| {
                vec![Problem::from_diagnostic(&d, source, Located::Snippet(d.span))]
            })?;
            Ok((statements, fallback))
        })();
        Self { source: source.to_string(), quantize: quantize.map(str::to_string), parsed }
    }
}

/// An evaluation waiting to be compiled. Holds copies of everything the
/// compiler needs, so compiling it touches no session.
pub struct EvalJob {
    source: String,
    quantize: Option<String>,
    fallback: Option<Quantize>,
    statements: Vec<Statement>,
    base: Project,
    playing: BTreeMap<String, String>,
    /// Which state of the session this was prepared against.
    version: u64,
    config: PrepareConfig,
    committed: Project,
    committed_playing: BTreeMap<String, String>,
    waiting: Vec<Waiting>,
    lands_at: u64,
}

impl EvalJob {
    /// Compile. This is the slow step.
    pub fn compile(self) -> EvalDone {
        let result = self.prepare();
        EvalDone { job: self, result }
    }

    fn prepare(&self) -> Result<Prepared, Vec<Problem>> {
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
        waiting.sort_by_key(|p| (p.lands_at, p.id == 0));

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
                Err(problems) if p.id == 0 => return Err(problems),
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
    playing: BTreeMap<String, String>,
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

pub struct Session {
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
    held: Vec<(u64, VoiceId)>,
    next_panic: usize,

    pending: Vec<Pending>,
    next_id: u64,
    /// What each track was last told to play, as source, so that redefining a
    /// pattern can start the new version.
    playing: BTreeMap<String, String>,
    landed: Vec<Landed>,
    notices: Vec<String>,
    recent_notices: Vec<(String, u64)>,
    log: Vec<LogEntry>,
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
    base_playing: &BTreeMap<String, String>,
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

    let mut actions = Vec::new();
    for s in statements.iter().filter(|s| s.kind != StatementKind::Definition) {
        let piece = Piece { text: s.text.clone(), origin: Origin::Snippet { offset: s.offset } };
        actions.push(Compiled { statement: s.clone(), program: check(&[piece])? });
    }

    // What will be playing, and which of it has to start over.
    let mut playing = base_playing.clone();
    for a in &actions {
        apply_to_playing(&mut playing, &a.program, &a.statement);
    }
    let changed: Vec<String> = definitions.iter().filter_map(|s| s.binding.clone()).collect();
    let retimes = definitions.iter().any(|s| matches!(s.key.as_deref(), Some("tempo" | "meter")));
    let mut replays = Vec::new();
    for (track, what) in base_playing {
        if !(retimes || mentions_any(what, &changed)) {
            continue;
        }
        let text = format!("play {track} = {what}");
        let Some(statement) = analyze(&text).ok().and_then(|mut s| s.pop()) else { continue };
        let piece = Piece { text: text.clone(), origin: Origin::Snippet { offset: 0 } };
        // If the new version no longer works for the track, leave it playing as it was.
        if let Ok(program) = check(&[piece]) {
            playing.insert(track.clone(), what.clone());
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
            sample_dirs: Vec::new(),
            defaults: QuantizeDefaults::default(),
            clock: 0,
            project: Project::default(),
            program: Arc::new(program),
            mixer,
            limiter: Some(Master::with_channels(sample_rate, CEILING, layout.channels())),
            history: Vec::new(),
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
        match self.begin_eval(source, quantize) {
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
        let ParsedEval { source, quantize, parsed } = input;
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
        let (base, playing) = self.projected();
        Begin::Job(Box::new(EvalJob {
            source,
            quantize,
            fallback,
            statements: statements.clone(),
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
            source: job.source.clone(), quantize: job.quantize.clone(), fallback: job.fallback,
            statements: job.statements.clone(), base, playing, version: self.version,
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
        let accepted = Accepted { id: Some(pending.id), lands_at: pending.lands_at,
            position: self.grid().position(pending.lands_at), changes: pending.summary.clone() };
        // Preserve chronological snapshots if a compile crossed a boundary.
        // Existing transactions already prepared against this one cannot land
        // before it. Their original order remains deterministic.
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
            self.insert_upcoming(Upcoming { start, dur, event, scheduled: false });
        }
        Ok(())
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
            .map(|(i, t)| {
                let state = self.schedule.track_state(i, now);
                TrackStatus { name: t.name.clone(), clip: state.clip, muted: state.muted, soloed: state.soloed }
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

    /// Everything that has been sent to the session, with when.
    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    // ---- the project --------------------------------------------------------------

    /// The project and what is playing as they will be once everything
    /// waiting has landed.
    fn projected(&self) -> (Project, BTreeMap<String, String>) {
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
        let Pending { id, summary, project, program, mixer, actions, replays, playing, .. } = p;

        if let (Some(program), Some(mixer)) = (program, mixer) {
            self.swap_program(project, program, mixer);
        }
        for a in actions.iter().chain(&replays) {
            self.run_action(a, at);
        }
        self.playing = playing;

        self.rebuild_schedule();
        self.landed.push(Landed { id, at, position: self.grid().position(at), changes: summary });
    }

    /// Replace the running program, keeping what is sounding.
    fn swap_program(&mut self, project: Project, program: Arc<Program>, mut mixer: Mixer) {
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

        for event in &program.notes {
            let offset = event.at.to_seconds(tempo);
            let start = if statement.has_at { self.samples(offset) } else { at + self.samples(offset) };
            let dur = self.samples(event.dur.to_seconds(tempo));
            self.insert_upcoming(Upcoming { start: start.max(at), dur, event: event.clone(), scheduled: false });
        }
        for scheduled in &program.performance {
            let when = if statement.has_at { scheduled.at.to_seconds(tempo) } else { landing_seconds };
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
            if !self.detached { self.mixer.panic(); }
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
            self.release_due();

            // How far we can go before something needs doing.
            let mut n = (end - self.clock).min(BLOCK as u64);
            if let Some(p) = self.pending.first() {
                n = n.min(p.lands_at - self.clock);
            }
            for &(off, _) in &self.held {
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
                LogInput::Events { events } => {
                    let _ = session.stream_events(events.clone());
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
            self.insert_upcoming(Upcoming { start, dur, event, scheduled: true });
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
                    let id = self.mixer.note_on(plan.track, graph, &plan.params, plan.choke);
                    self.held.push((self.clock + note.dur, id));
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
        self.held.retain(|&(off, id)| {
            if off <= now {
                mixer.release(id);
            }
            off > now
        });
    }
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

/// Update the record of what each track is playing for a statement that has
/// been compiled: `play` records its source, `stop` and `hush` forget.
fn apply_to_playing(playing: &mut BTreeMap<String, String>, program: &Program, statement: &Statement) {
    for scheduled in &program.performance {
        match name_action(program, &scheduled.action) {
            NamedAction::Play { track, .. } => {
                if let Some(plays) = statement.plays.as_ref().filter(|p| p.track == track) {
                    playing.insert(track, plays.what.clone());
                }
            }
            NamedAction::Stop(track) => {
                playing.remove(&track);
            }
            NamedAction::Hush | NamedAction::Panic => playing.clear(),
            _ => {}
        }
    }
}
