//! A live session: a running mixer that code can be evaluated into.
//!
//! Evaluating code compiles it immediately, and if it compiles the result waits
//! for a musical boundary before landing. Everything one evaluation says lands
//! together (a transaction), at the coarsest boundary among its parts, and
//! after anything evaluated earlier. A snippet that does not compile changes
//! nothing.
//!
//! The session is driven by a sample clock rather than a real one, so it is
//! deterministic: the same inputs at the same samples give the same audio,
//! however the audio is divided into blocks.

use std::collections::{BTreeMap, HashMap};

use niminal_engine::{BLOCK, MAX_CHANNELS, Master, Mixer, Transfer, VoiceId};
use niminal_lang::{
    Action, Clip, CompileOptions, Layout, Program, Quantize, Schedule, Scheduled, Statement, StatementKind,
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
    program: Option<Program>,
    mixer: Option<Mixer>,
    actions: Vec<Compiled>,
    /// Plays to start again because a pattern they use was redefined.
    replays: Vec<Compiled>,
    /// What each track is playing once this has landed.
    playing: BTreeMap<String, String>,
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
    defaults: QuantizeDefaults,
    clock: u64,

    project: Project,
    program: Program,
    mixer: Mixer,
    limiter: Option<Master>,

    history: Vec<HistoryEntry>,
    schedule: Schedule,
    /// Notes up to this sample have been taken from the schedule.
    fetched_until: u64,
    upcoming: Vec<Upcoming>,
    held: Vec<(u64, VoiceId)>,
    next_panic: usize,

    pending: Vec<Pending>,
    next_id: u64,
    /// What each track was last told to play, as source, so that redefining a
    /// pattern can start the new version.
    playing: BTreeMap<String, String>,
    landed: Vec<Landed>,
    notices: Vec<String>,
    log: Vec<LogEntry>,
    silenced: usize,
    peaks: Vec<f32>,
}

fn options(layout: Layout) -> CompileOptions {
    CompileOptions { default_layout: layout }
}

impl Session {
    /// A session that outputs `layout` at `sample_rate`, with the limiter on.
    pub fn new(sample_rate: f32, layout: Layout) -> Session {
        let program = compile_with("", &options(layout)).expect("an empty project compiles");
        let mixer = program.mixer(sample_rate);
        let schedule = program.schedule();
        Session {
            sample_rate,
            layout,
            defaults: QuantizeDefaults::default(),
            clock: 0,
            project: Project::default(),
            program,
            mixer,
            limiter: Some(Master::with_channels(sample_rate, CEILING, layout.channels())),
            history: Vec::new(),
            schedule,
            fetched_until: 0,
            upcoming: Vec::new(),
            held: Vec::new(),
            next_panic: 0,
            pending: Vec::new(),
            next_id: 1,
            playing: BTreeMap::new(),
            landed: Vec::new(),
            notices: Vec::new(),
            log: Vec::new(),
            silenced: 0,
            peaks: vec![0.0; layout.channels()],
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
        self.limiter.as_ref().map_or(0, Master::latency)
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
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::Eval { source: source.to_string(), quantize: quantize.map(str::to_string) },
        });
        self.eval_inner(source, quantize)
    }

    fn eval_inner(&mut self, source: &str, quantize: Option<&str>) -> Result<Accepted, Vec<Problem>> {
        let fallback = match quantize {
            None => None,
            Some(text) => Some(parse_quantum(text)?),
        };
        let statements = analyze(source)
            .map_err(|d| vec![Problem::from_diagnostic(&d, source, Located::Snippet(d.span))])?;
        if statements.is_empty() {
            return Ok(Accepted { id: None, lands_at: self.clock, position: self.grid().position(self.clock), changes: vec![] });
        }

        // One transaction lands together, at the coarsest of its boundaries,
        // and never before something evaluated earlier.
        let grid = self.grid();
        let mut lands_at = statements
            .iter()
            .map(|s| grid.landing(self.defaults.for_statement(s, fallback), self.clock))
            .max()
            .expect("there is at least one statement");
        if let Some(last) = self.pending.last() {
            lands_at = lands_at.max(last.lands_at);
        }

        let id = self.next_id;
        let (project, playing) = self.projected();
        let pending = self.prepare(id, lands_at, source, statements, &project, &playing)?;
        self.next_id += 1;
        let accepted = Accepted {
            id: Some(id),
            lands_at,
            position: grid.position(lands_at),
            changes: pending.summary.clone(),
        };
        self.pending.push(pending);
        self.land_due();
        Ok(accepted)
    }

    /// Compile everything `statements` will need when they land, against the
    /// project as it will be then (`base`, with `base_playing` running).
    fn prepare(
        &self,
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
            compile_with(&full, &options(self.layout)).map_err(|diags| {
                diags
                    .iter()
                    .map(|d| Problem::from_diagnostic(d, snippet, map.locate(d.span)))
                    .collect::<Vec<_>>()
            })
        };

        let program = check(&[])?;
        if program.master != self.layout {
            return Err(vec![Problem::plain(format!(
                "this session outputs {}, but the project says {}; the output layout can only be chosen when the daemon starts",
                self.layout, program.master
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
        let mixer = has_definitions.then(|| program.mixer(self.sample_rate));
        Ok(Pending {
            id,
            lands_at,
            snippet: snippet.to_string(),
            summary: statements.iter().map(summarize).collect(),
            statements,
            project,
            program: has_definitions.then_some(program),
            mixer,
            actions,
            replays,
            playing,
        })
    }

    /// Cancel a waiting evaluation, or all of them. Returns how many. What is
    /// left is compiled again, since it may have depended on what was cancelled.
    pub fn cancel(&mut self, id: Option<u64>) -> usize {
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Cancel { id } });
        let before = self.pending.len();
        let remaining: Vec<Pending> = std::mem::take(&mut self.pending);
        let (mut project, mut playing) = (self.project.clone(), self.playing.clone());
        let mut dropped = 0;
        for p in remaining {
            if id.is_none_or(|i| p.id == i) {
                dropped += 1;
                continue;
            }
            match self.prepare(p.id, p.lands_at, &p.snippet, p.statements.clone(), &project, &playing) {
                Ok(redone) => {
                    project = redone.project.clone();
                    playing = redone.playing.clone();
                    self.pending.push(redone);
                }
                Err(_) => {
                    dropped += 1;
                    self.notices.push(format!(
                        "change {} was dropped: it depended on a change that was cancelled",
                        p.id
                    ));
                }
            }
        }
        debug_assert_eq!(before - dropped, self.pending.len());
        dropped
    }

    /// Stop everything at once, including effect tails.
    pub fn panic(&mut self) {
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
            voices: self.mixer.active_voices(),
            pending: self.pending.len(),
        }
    }

    /// Changes that have landed since this was last called.
    pub fn take_landed(&mut self) -> Vec<Landed> {
        std::mem::take(&mut self.landed)
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
    fn swap_program(&mut self, project: Project, program: Program, mut mixer: Mixer) {
        let transfers = self.plan_transfers(&project, &program);
        mixer.adopt(&mut self.mixer, &transfers);
        self.mixer = mixer;
        self.program = program;
        self.project = project;
    }

    /// Which running voices and effect chains the new program's tracks take over.
    fn plan_transfers(&self, next: &Project, program: &Program) -> Vec<Transfer> {
        let old = &self.program;
        let buses_same = old.buses == program.buses
            && old.bus_layouts == program.bus_layouts
            && old.master == program.master;
        let environment_same = self.project.environment() == next.environment();

        program
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let from = if i == 0 { Some(0) } else { old.track_index(&track.name) };
                let key = format!("track:{}", track.name);
                let unchanged = self.project.text_of(&key).is_some() && self.project.text_of(&key) == next.text_of(&key);
                Transfer {
                    voices_from: from.filter(|_| buses_same),
                    chain_from: from.filter(|_| i > 0 && buses_same && environment_same && unchanged),
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
        let names: Vec<String> = self.program.tracks.iter().map(|t| t.name.clone()).collect();
        let index = |track: &String| self.program.track_index(track);
        let commands: Vec<Scheduled> = self
            .history
            .iter()
            .filter_map(|h| {
                let action = match &h.action {
                    NamedAction::Play { track, clip } => Action::Play { track: index(track)?, clip: clip.clone() },
                    NamedAction::Stop(t) => Action::Stop { track: index(t)? },
                    NamedAction::Mute(t) => Action::Mute { track: index(t)? },
                    NamedAction::Unmute(t) => Action::Unmute { track: index(t)? },
                    NamedAction::Solo(t) => Action::Solo { track: index(t)? },
                    NamedAction::Unsolo(t) => Action::Unsolo { track: t.as_ref().and_then(index) },
                    NamedAction::Hush => Action::Hush,
                    NamedAction::Panic => Action::Panic,
                };
                Some(Scheduled { at: Time::Seconds(h.at), action, quantize: None })
            })
            .collect();
        self.schedule = Schedule::from_commands(self.program.tempo, names, &commands);
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
            self.mixer.panic();
            self.held.clear();
            self.upcoming.clear();
            self.next_panic += 1;
        }
    }

    /// Mix `frames` samples, calling `sink` with each piece (a block per
    /// channel and how many of its samples are valid).
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[[f32; BLOCK]], usize)) {
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
    ) -> Vec<Vec<f32>> {
        let mut session = Session::new(sample_rate, layout).with_defaults(defaults);
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
        match self.schedule.events(from, to) {
            Ok(events) => {
                for event in events {
                    let start = self.samples(event.at.to_seconds(self.program.tempo)).max(self.clock);
                    let dur = self.samples(event.dur.to_seconds(self.program.tempo));
                    self.insert_upcoming(Upcoming { start, dur, event, scheduled: true });
                }
            }
            Err(e) => self.notices.push(e.to_string()),
        }
        self.fetched_until = horizon;
    }

    fn start_due(&mut self) {
        while self.upcoming.first().is_some_and(|u| u.start <= self.clock) {
            let note = self.upcoming.remove(0);
            match self.program.plan(&note.event) {
                Ok(plan) => {
                    let graph = self.program.instruments[plan.instrument].graph.clone();
                    let id = self.mixer.note_on(plan.track, graph, &plan.params);
                    self.held.push((self.clock + note.dur, id));
                }
                Err(e) => self.notices.push(format!("note for `{}` could not play: {e}", note.event.target)),
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
