//! Evaluating and cancelling: compiling code against the session as it is, then queueing it to land.

use super::*;

/// What compiling an evaluation needs to know about the session.
#[derive(Clone)]
pub(super) struct PrepareConfig {
    layout: Layout,
    samples: Samples,
}

#[derive(Clone)]
pub(super) struct Waiting {
    id: u64,
    group: Option<u64>,
    lands_at: u64,
    snippet: String,
    statements: Vec<Statement>,
}

pub(super) struct Prepared {
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
pub(super) struct Uses {
    expanded: Expanded,
    sent: String,
}

impl Uses {
    pub(super) fn locate(&self, problems: Vec<Problem>) -> Vec<Problem> {
        problems
            .into_iter()
            .map(|p| p.in_module(&self.expanded, &self.sent))
            .collect()
    }
}

impl ParsedEval {
    pub fn new(source: &str, quantize: Option<&str>) -> Self {
        Self::from_text(source, quantize, None)
    }

    /// Code sent from `dir`, whose `use` lines are joined in. The log keeps the
    /// joined text, so a replay needs no files.
    pub fn from_dir(sent: &str, dir: &std::path::Path, quantize: Option<&str>) -> Self {
        match niminal_compose::expand(sent, dir) {
            Ok(expanded) => {
                let text = expanded.text.clone();
                let uses = Arc::new(Uses {
                    expanded,
                    sent: sent.to_string(),
                });
                Self::from_text(&text, quantize, Some(uses))
            }
            Err(d) => {
                let problems = vec![Problem::from_diagnostic(&d, sent, Located::Snippet(d.span))];
                Self {
                    source: sent.to_string(),
                    quantize: quantize.map(str::to_string),
                    parsed: Err(problems),
                    uses: None,
                }
            }
        }
    }

    /// The module files that were joined in.
    pub fn module_files(&self) -> Vec<std::path::PathBuf> {
        self.uses
            .iter()
            .flat_map(|u| u.expanded.files().map(std::path::Path::to_path_buf))
            .collect()
    }

    pub(super) fn from_text(source: &str, quantize: Option<&str>, uses: Option<Arc<Uses>>) -> Self {
        let parsed = (|| {
            let fallback = quantize.map(parse_quantum).transpose()?;
            let statements = analyze(source).map_err(|d| {
                let problems = vec![Problem::from_diagnostic(
                    &d,
                    source,
                    Located::Snippet(d.span),
                )];
                match &uses {
                    Some(u) => u.locate(problems),
                    None => problems,
                }
            })?;
            Ok((statements, fallback))
        })();
        Self {
            source: source.to_string(),
            quantize: quantize.map(str::to_string),
            parsed,
            uses,
        }
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
pub(super) const NEW_BASE: u64 = 1 << 62;

pub(super) fn is_new(id: u64) -> bool {
    id == 0 || id >= NEW_BASE
}

impl EvalJob {
    /// Compile. This is the slow step.
    pub fn compile(self) -> EvalDone {
        let result = self.prepare();
        EvalDone { job: self, result }
    }

    pub(super) fn prepare(&self) -> Result<Prepared, Vec<Problem>> {
        let locate = |problems| match &self.uses {
            Some(u) => u.locate(problems),
            None => problems,
        };
        // Validate the complete transaction before superseding anything.
        prepare_pending(
            &self.config,
            0,
            self.lands_at,
            &self.source,
            self.statements.clone(),
            &self.base,
            &self.playing,
        )
        .map_err(locate)?;
        let writes: Vec<&str> = self
            .statements
            .iter()
            .filter_map(|s| s.write_key.as_deref())
            .collect();
        let mut waiting = self.waiting.clone();
        for p in &mut waiting {
            p.statements.retain(|s| {
                s.write_key
                    .as_deref()
                    .is_none_or(|key| !writes.contains(&key))
            });
        }
        waiting.retain(|p| !p.statements.is_empty());
        waiting.push(Waiting {
            id: 0,
            group: None,
            lands_at: self.lands_at,
            snippet: self.source.clone(),
            statements: self.statements.clone(),
        });
        for (k, (after, statement)) in self.ramp.iter().enumerate() {
            waiting.push(Waiting {
                id: NEW_BASE + k as u64,
                group: None,
                lands_at: self.lands_at + after,
                snippet: self.source.clone(),
                statements: vec![statement.clone()],
            });
        }
        waiting.sort_by_key(|p| (p.lands_at, !is_new(p.id), p.id));

        let latest = waiting
            .iter()
            .map(|p| p.lands_at)
            .max()
            .unwrap_or(self.lands_at);
        let (mut project, mut playing) = (self.committed.clone(), self.committed_playing.clone());
        let mut queue = Vec::new();
        let mut deferred = Vec::new();
        let mut notices = Vec::new();
        for p in waiting {
            match prepare_next(&self.config, &p, p.lands_at, &mut project, &mut playing) {
                Ok(next) => queue.push(next),
                // A transaction can refer to definitions waiting for a later
                // boundary. Keep it together and move it after those definitions.
                Err(_) => deferred.push(p),
            }
        }
        for p in deferred {
            match prepare_next(&self.config, &p, latest, &mut project, &mut playing) {
                Ok(next) => queue.push(next),
                Err(problems) if is_new(p.id) => return Err(locate(problems)),
                Err(_) => notices.push(format!(
                    "change {} was dropped: a newer pending write invalidated it",
                    p.id
                )),
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
        // a step of a ramp goes with the rest of it
        let group = self.id.and_then(|id| {
            self.waiting
                .iter()
                .find(|p| p.id == id)
                .and_then(|p| p.group)
                .or(Some(id))
        });
        for p in &self.waiting {
            if self.id.is_none_or(|id| id == p.id) || (p.group.is_some() && p.group == group) {
                dropped += 1;
                continue;
            }
            match prepare_next(&self.config, p, p.lands_at, &mut project, &mut playing) {
                Ok(next) => queue.push(next),
                Err(_) => {
                    dropped += 1;
                    notices.push(format!(
                        "change {} was dropped: it depended on a change that was cancelled",
                        p.id
                    ));
                }
            }
        }
        CancelDone {
            job: self,
            prepared: Prepared { queue, notices },
            dropped,
        }
    }
}

/// Compile a waiting change on top of the running `project` and `playing`, which become what it leaves behind.
pub(super) fn prepare_next(
    cfg: &PrepareConfig,
    p: &Waiting,
    lands_at: u64,
    project: &mut Project,
    playing: &mut BTreeMap<String, Playing>,
) -> Result<Pending, Vec<Problem>> {
    let mut next = prepare_pending(
        cfg,
        p.id,
        lands_at,
        &p.snippet,
        p.statements.clone(),
        project,
        playing,
    )?;
    *project = next.project.clone();
    *playing = next.playing.clone();
    next.group = p.group;
    Ok(next)
}

/// Compile everything `statements` will need when they land, against the
/// project as it will be then (`base`, with `base_playing` running). This is
/// the slow part of an evaluation, and needs nothing from a running session.
pub(super) fn prepare_pending(
    cfg: &PrepareConfig,
    id: u64,
    lands_at: u64,
    snippet: &str,
    statements: Vec<Statement>,
    base: &Project,
    base_playing: &BTreeMap<String, Playing>,
) -> Result<Pending, Vec<Problem>> {
    let definitions: Vec<&Statement> = statements
        .iter()
        .filter(|s| s.kind() == StatementKind::Definition)
        .collect();
    let delta: Vec<Entry> = definitions
        .iter()
        .map(|s| Entry {
            key: s.key().expect("definitions have keys").to_string(),
            text: s.text.clone(),
        })
        .collect();
    let from_snippet: HashMap<String, usize> = definitions
        .iter()
        .map(|s| {
            (
                s.key().expect("definitions have keys").to_string(),
                s.offset,
            )
        })
        .collect();

    // `new` forgets everything sent before this snippet; the output layout stays, since it
    // can only be chosen when the server starts.
    let clears = statements.iter().any(|s| s.clears());
    let fresh = Project {
        entries: base
            .entries
            .iter()
            .filter(|e| e.key == "config")
            .cloned()
            .collect(),
    };
    let (base, base_playing) = if clears {
        (&fresh, &BTreeMap::new())
    } else {
        (base, base_playing)
    };

    let mut project = base.clone();
    project.apply(&delta);
    project.drop_broken(&from_snippet, |full| compile_with(full, &options(cfg.layout, &cfg.samples)).map(|_| ()));
    let check = |project: &Project, extra: &[Piece]| -> Result<Composition, Vec<Problem>> {
        let (full, map) = project.source(&from_snippet, extra);
        compile_with(&full, &options(cfg.layout, &cfg.samples)).map_err(|diags| {
            diags
                .iter()
                .map(|d| Problem::from_diagnostic(d, snippet, map.locate(d.span)))
                .collect::<Vec<_>>()
        })
    };

    let mut program = check(&project, &[])?;
    if program.master != cfg.layout {
        return Err(vec![Problem::plain(format!(
            "this session outputs {}, but the project says {}; the output layout can only be chosen when the server starts",
            cfg.layout, program.master
        ))]);
    }

    // The commands run in order, each seeing what the ones before it left playing.
    let mut playing = base_playing.clone();
    let mut actions = Vec::new();
    for s in statements
        .iter()
        .filter(|s| s.kind() != StatementKind::Definition)
    {
        for (s, edited) in join_edit(s, &playing, &project)? {
            let piece = Piece {
                text: s.text.clone(),
                origin: Origin::Snippet { offset: s.offset },
            };
            let program = check(&project, &[piece])?;
            // A command for later (`at +4bars chorus.start`) is not what is playing now.
            let later = program
                .performance
                .iter()
                .any(|p| p.at.to_seconds(program.tempo) > 0.0);
            if !later {
                apply_to_playing(&mut playing, &program, &s);
                if let Some(scene) = s.launch() {
                    apply_launch(&mut playing, &program, &project, scene);
                }
                if let Some((node, edited)) = edited {
                    playing.insert(node, edited);
                }
            }
            actions.push(Compiled {
                statement: s,
                program,
            });
        }
    }

    let changed: Vec<String> = definitions
        .iter()
        .filter_map(|s| s.binding().map(str::to_string))
        .collect();
    // Nodes that play something this change touches start it over, unless the commands
    // here already start them.
    let relaunched = actions.iter().any(|a| a.statement.launch().is_some());
    let mut replays = Vec::new();
    for (node, now) in playing.clone() {
        let started = relaunched
            || actions
                .iter()
                .any(|a| a.statement.plays().is_some_and(|p| p.node == node));
        let what = now.text();
        if started || !mentions_any(&what, &changed) {
            continue;
        }
        let text = format!("{node}.play({what})");
        let Some(statement) = analyze(&text).ok().and_then(|mut s| s.pop()) else {
            continue;
        };
        let piece = Piece {
            text: text.clone(),
            origin: Origin::Snippet { offset: 0 },
        };
        // If the new version no longer works for the node, leave it playing as it was.
        if let Ok(program) = check(&project, &[piece]) {
            replays.push(Compiled { statement, program });
        }
    }

    // A bare instrument used by a command becomes a persistent clip, so subsequent
    // evaluations keep its playback slot even when they only change a control.
    let mut implicit = Vec::new();
    for action in &actions {
        for node in action.program.nodes.iter().skip(1) {
            if program.nodes.iter().any(|known| known.name == node.name)
                || implicit.iter().any(|entry: &Entry| entry.key == format!("clip:{}", node.name))
            {
                continue;
            }
            if node.instrument.is_some_and(|i| action.program.instruments[i].name == node.name) {
                implicit.push(Entry {
                    key: format!("clip:{}", node.name),
                    text: format!("clip {} {{ instrument: {} }}", node.name, node.name),
                });
            }
        }
    }
    if !implicit.is_empty() {
        project.apply(&implicit);
        program = check(&project, &[])?;
    }
    let has_definitions = !definitions.is_empty() || !implicit.is_empty() || clears;
    Ok(Pending {
        id,
        group: None,
        lands_at,
        snippet: snippet.to_string(),
        summary: statements.iter().map(|s| first_line(&s.text)).collect(),
        statements,
        project,
        program: has_definitions.then(|| Arc::new(program)),
        actions,
        replays,
        playing,
        done: false,
    })
}

pub(super) fn options(layout: Layout, samples: &Samples) -> CompileOptions {
    CompileOptions {
        default_layout: layout,
        samples: samples.clone(),
        ..Default::default()
    }
}

/// `@ next 4bars` as text, for an override.
pub(super) fn parse_quantum(text: &str) -> Result<Quantize, Vec<Problem>> {
    analyze(&format!("hush @ {text}"))
        .ok()
        .and_then(|mut s| s.pop())
        .and_then(|s| s.quantize)
        .ok_or_else(|| {
            vec![Problem::plain(format!(
                "`{text}` isn't a quantum: use `now`, `beat`, `bar`, `cycle`, `next 4bars`, `in 2beats`..."
            ))]
        })
}

impl Live {
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
    /// [`EvalJob::compile`] needs no access to the session, so a server can do
    /// it without holding the render worker's control lock.
    pub fn begin_eval(&mut self, source: &str, quantize: Option<&str>) -> Begin {
        self.begin_parsed(ParsedEval::new(source, quantize))
    }

    pub fn begin_parsed(&mut self, input: ParsedEval) -> Begin {
        self.look();
        let ParsedEval {
            source,
            quantize,
            parsed,
            uses,
        } = input;
        // a compile job logs itself when it finishes; one settled here is logged now
        let settled = match parsed {
            Err(problems) => Err(problems),
            Ok((statements, _)) if statements.is_empty() => Ok(Accepted {
                id: None,
                lands_at: self.clock,
                position: self.grid().position(self.clock),
                changes: vec![],
            }),
            Ok((mut statements, fallback)) => match self.expand_ramp(&mut statements) {
                Ok(ramp) => {
                    return Begin::Job(self.job(source, uses, quantize, fallback, statements, ramp));
                }
                Err(problems) => Err(problems),
            },
        };
        self.log_eval(&source, quantize.as_deref());
        Begin::Done(settled)
    }

    /// An evaluation to compile against the session as it is now.
    pub(super) fn job(
        &self,
        source: String,
        uses: Option<Arc<Uses>>,
        quantize: Option<String>,
        fallback: Option<Quantize>,
        statements: Vec<Statement>,
        ramp: Vec<(u64, Statement)>,
    ) -> Box<EvalJob> {
        let (base, playing) = self.projected();
        Box::new(EvalJob {
            lands_at: self.landing(&statements, fallback),
            source,
            uses,
            quantize,
            fallback,
            statements,
            ramp,
            base,
            playing,
            version: self.version,
            config: self.prepare_config(),
            committed: self.project.clone(),
            committed_playing: self.playing.clone(),
            waiting: self.waiting(),
        })
    }

    /// The evaluations waiting to land, as a compile job sees them.
    pub(super) fn waiting(&self) -> Vec<Waiting> {
        self.pending
            .iter()
            .filter(|p| !p.done)
            .map(|p| Waiting {
                id: p.id,
                group: p.group,
                lands_at: p.lands_at,
                snippet: p.snippet.clone(),
                statements: p.statements.clone(),
            })
            .collect()
    }

    /// `tempo 124bpm @ next bar over 8bars` becomes a tempo for each quarter beat on the way:
    /// the first replaces the statement, and the rest are returned with how long after it each lands.
    pub(super) fn expand_ramp(
        &self,
        statements: &mut [Statement],
    ) -> Result<Vec<(u64, Statement)>, Vec<Problem>> {
        let Some(index) = statements.iter().position(|s| s.ramp().is_some()) else {
            return Ok(Vec::new());
        };
        let Some(ramp) = statements[index].take_ramp() else {
            return Ok(Vec::new());
        };
        let tempo = self.program.tempo;
        let beats = ramp.over.to_seconds(tempo) * tempo.bpm / 60.0;
        let steps = (beats * RAMP_STEPS_PER_BEAT).ceil();
        if !(1.0..=MAX_RAMP_STEPS).contains(&steps) {
            return Err(vec![Problem::plain(format!(
                "a tempo ramp takes from a quarter of a beat up to {} beats",
                MAX_RAMP_STEPS / RAMP_STEPS_PER_BEAT
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

    /// When each kind of change lands: the session's defaults, then what the running project's `config` says.
    pub(super) fn quantums(&self) -> QuantizeDefaults {
        self.defaults.with_project(&self.program.quantize)
    }

    /// What is waiting to land becomes `queue`, after the changes that have been made here already.
    pub(super) fn replace_pending(&mut self, queue: Vec<Pending>) {
        let mut kept: Vec<Pending> = std::mem::take(&mut self.pending).into_iter().filter(|p| p.done).collect();
        kept.extend(queue);
        self.pending = kept;
    }

    pub(super) fn landing(&self, statements: &[Statement], fallback: Option<Quantize>) -> u64 {
        let grid = self.grid();
        statements
            .iter()
            .map(|s| grid.landing(self.quantums().for_statement(s, fallback), self.clock))
            .max()
            .expect("an evaluation has statements")
    }

    /// Return a fresh compilation job when the project changed while compiling.
    /// Servers run this job outside their control lock.
    pub fn retry_eval(&self, done: &EvalDone) -> Option<Box<EvalJob>> {
        let job = &done.job;
        if job.version == self.version {
            return None;
        }
        Some(self.job(
            job.source.clone(),
            job.uses.clone(),
            job.quantize.clone(),
            job.fallback,
            job.statements.clone(),
            job.ramp.clone(),
        ))
    }

    /// Queue a compiled transaction. The synchronous API retries inline;
    /// servers use `retry_eval` to do that compilation outside their lock.
    pub fn finish_eval(&mut self, mut done: EvalDone) -> Result<Accepted, Vec<Problem>> {
        self.look();
        while let Some(job) = self.retry_eval(&done) {
            done = job.compile();
        }
        let EvalDone { job, result } = done;
        self.log_eval(&job.source, job.quantize.as_deref());
        let Prepared { mut queue, notices } = result?;
        let landing = self.landing(&job.statements, job.fallback);
        let pending = queue
            .iter_mut()
            .find(|p| p.id == 0)
            .expect("the new transaction is queued");
        // A compile that missed its grid waits for the next one. `now` lands
        // when compilation completes, rather than at its original clock sample.
        pending.lands_at = pending.lands_at.max(landing);
        let id = self.next_id;
        pending.id = id;
        self.next_id += 1;
        let first_at = pending.lands_at;
        let accepted = Accepted {
            id: Some(id),
            lands_at: pending.lands_at,
            position: self.grid().position(pending.lands_at),
            changes: pending.summary.clone(),
        };
        // Preserve chronological snapshots if a compile crossed a boundary.
        // Existing transactions already prepared against this one cannot land
        // before it. Their original order remains deterministic.
        // the steps of a tempo ramp follow the first by the spacing worked out when it was written
        for step in queue.iter_mut().filter(|p| p.id >= NEW_BASE) {
            step.lands_at = first_at + job.ramp[(step.id - NEW_BASE) as usize].0;
            step.id = self.next_id;
            step.group = accepted.id;
            self.next_id += 1;
        }
        if !job.ramp.is_empty()
            && let Some(first) = queue.iter_mut().find(|p| p.id == id)
        {
            first.group = accepted.id;
        }
        let new_index = queue
            .iter()
            .position(|p| p.id == id)
            .expect("the new transaction is queued");
        let mut at = queue[new_index].lands_at;
        for p in &mut queue[new_index + 1..] {
            p.lands_at = p.lands_at.max(at);
            at = p.lands_at;
        }
        self.replace_pending(queue);
        if accepted.lands_at > job.lands_at
            && self.clock > job.lands_at
            && job.statements.iter().any(|s| {
                self.quantums().for_statement(s, job.fallback).unit
                    != niminal_compose::QuantizeUnit::Now
            })
        {
            self.notify(format!(
                "change {} missed its boundary while compiling; moved to bar {} beat {:.2}",
                id, accepted.position.0, accepted.position.1
            ));
        }
        for notice in notices {
            self.notify(notice);
        }
        self.version += 1;
        self.advance();
        Ok(accepted)
    }

    pub(super) fn log_eval(&mut self, source: &str, quantize: Option<&str>) {
        self.log.push(LiveLogEntry {
            sample: self.clock,
            input: LiveInput::Eval {
                source: source.to_string(),
                quantize: quantize.map(str::to_string),
            },
        });
    }

    pub(super) fn prepare_config(&self) -> PrepareConfig {
        PrepareConfig {
            layout: self.layout,
            samples: self.samples.clone(),
        }
    }

    /// Cancel a waiting evaluation, or all of them. Returns how many. What is
    /// left is compiled again, since it may have depended on what was cancelled.
    pub fn cancel(&mut self, id: Option<u64>) -> usize {
        let done = self.begin_cancel(id).compile();
        self.finish_cancel(done)
    }

    pub fn begin_cancel(&self, id: Option<u64>) -> Box<CancelJob> {
        Box::new(CancelJob {
            id,
            version: self.version,
            config: self.prepare_config(),
            project: self.project.clone(),
            playing: self.playing.clone(),
            waiting: self.waiting(),
        })
    }

    pub fn retry_cancel(&self, done: &CancelDone) -> Option<Box<CancelJob>> {
        (done.job.version != self.version).then(|| self.begin_cancel(done.job.id))
    }

    pub fn finish_cancel(&mut self, mut done: CancelDone) -> usize {
        self.look();
        while let Some(job) = self.retry_cancel(&done) {
            done = job.compile();
        }
        self.log.push(LiveLogEntry {
            sample: self.clock,
            input: LiveInput::Cancel { id: done.job.id },
        });
        self.replace_pending(done.prepared.queue);
        for notice in done.prepared.notices {
            self.notify(notice);
        }
        self.version += 1;
        done.dropped
    }
}
