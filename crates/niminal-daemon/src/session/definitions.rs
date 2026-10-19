//! Changes of definitions, waiting for the sample they land at.
//!
//! Each is compiled when it is sent, on top of the changes waiting before it, and the mixer for the
//! program it makes is built then too, so that landing only has to move things into place. A change that
//! is taken back, or put in front of others, makes the ones after it compile again.

use super::*;

/// What a change sent to the engine is allowed to say.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum What {
    /// DSP definitions only.
    Definitions,
    /// `new`.
    Forget,
}

/// Who sent a change, and what it was told.
#[derive(Clone)]
pub(super) struct Meta {
    pub client: ClientId,
    pub tag: Option<String>,
}

/// A change that has compiled and is waiting to land.
pub(super) struct Pending {
    pub id: u64,
    pub applied: Arc<AtomicU64>,
    pub lands_at: u64,
    /// What was sent, with the modules it uses joined in, so that it can be compiled again.
    pub snippet: String,
    pub statements: Vec<Statement>,
    /// The project once this has landed.
    pub project: Project,
    pub definitions: Arc<Program>,
    pub routing: crate::routing::Routing,
    pub edits: Vec<crate::RoutingEdit>,
    /// The materialized program and mixer, prepared before rendering.
    pub program: Arc<Program>,
    pub mixer: Mixer,
    pub meta: Meta,
}

impl Pending {
    /// Copy a prepared change for batch validation, retaining the original for rollback.
    pub(super) fn copy_prepared(&self, sample_rate: f32) -> Self {
        Self {
            id: self.id,
            applied: self.applied.clone(),
            lands_at: self.lands_at,
            snippet: self.snippet.clone(),
            statements: self.statements.clone(),
            project: self.project.clone(),
            definitions: self.definitions.clone(),
            routing: self.routing.clone(),
            edits: self.edits.clone(),
            program: self.program.clone(),
            mixer: self.program.mixer(sample_rate),
            meta: self.meta.clone(),
        }
    }
}

/// The modules joined into a change, and the code as it was sent, so a problem in a module can be pointed
/// at the line that uses it.
struct Uses {
    expanded: Expanded,
    sent: String,
}

impl Uses {
    fn locate(&self, problems: Vec<Problem>) -> Vec<Problem> {
        problems
            .into_iter()
            .map(|p| p.in_module(&self.expanded, &self.sent))
            .collect()
    }
}

impl Session {
    pub(super) fn queue_routing(
        &mut self,
        edits: Vec<crate::RoutingEdit>,
        at: u64,
        meta: Meta,
    ) -> Result<u64, Vec<Problem>> {
        self.queue_transaction("", None, at, What::Definitions, edits, meta)
    }

    pub(super) fn queue_transaction(
        &mut self,
        source: &str,
        dir: Option<&str>,
        at: u64,
        what: What,
        edits: Vec<crate::RoutingEdit>,
        meta: Meta,
    ) -> Result<u64, Vec<Problem>> {
        let (snippet, statements, uses) = if source.trim().is_empty() {
            (String::new(), Vec::new(), None)
        } else {
            self.read_change(source, dir, what)?
        };
        let at = at.max(self.clock);
        let index = self.pending.partition_point(|p| p.lands_at <= at);
        let (base, definitions, routing) = index.checked_sub(1).map_or_else(
            || {
                (
                    self.project.clone(),
                    self.definitions.clone(),
                    self.routing.clone(),
                )
            },
            |i| {
                let p = &self.pending[i];
                (p.project.clone(), p.definitions.clone(), p.routing.clone())
            },
        );
        let id = self.next_id;
        let pending = self
            .prepare(
                id,
                at,
                snippet,
                statements,
                &base,
                &definitions,
                &routing,
                edits,
                meta,
            )
            .map_err(|problems| match uses {
                Some(u) => u.locate(problems),
                None => problems,
            })?;
        self.next_id += 1;
        self.pending.insert(index, pending);
        self.recompile_from(index + 1);
        self.version += 1;
        Ok(id)
    }

    /// Take a waiting change back. What was waiting after it is compiled again.
    pub(super) fn cancel_change(&mut self, id: u64) -> bool {
        let Some(index) = self.pending.iter().position(|p| p.id == id) else {
            return false;
        };
        self.cancel_transaction(id);
        self.pending.remove(index);
        self.recompile_from(index);
        self.reconcile_transactions();
        self.version += 1;
        true
    }

    /// Read the source: join in the modules it uses, split it into statements and check it says only what
    /// `what` allows.
    fn read_change(
        &self,
        source: &str,
        dir: Option<&str>,
        what: What,
    ) -> Result<(String, Vec<Statement>, Option<Uses>), Vec<Problem>> {
        let (text, uses) =
            match niminal_lang::expand(source, std::path::Path::new(dir.unwrap_or("."))) {
                Ok(expanded) => (
                    expanded.text.clone(),
                    Some(Uses {
                        expanded,
                        sent: source.to_string(),
                    }),
                ),
                Err(d) => {
                    return Err(vec![Problem::from_diagnostic(
                        &d,
                        source,
                        Located::Snippet(d.span),
                    )]);
                }
            };
        if what == What::Definitions {
            niminal_lang::validate_dsp(&text).map_err(|d| {
                let problems = vec![Problem::from_diagnostic(
                    &d,
                    &text,
                    Located::Snippet(d.span),
                )];
                match &uses {
                    Some(u) => u.locate(problems),
                    None => problems,
                }
            })?;
        }
        let statements = analyze(&text).map_err(|d| {
            let problems = vec![Problem::from_diagnostic(
                &d,
                &text,
                Located::Snippet(d.span),
            )];
            match &uses {
                Some(u) => u.locate(problems),
                None => problems,
            }
        })?;
        if statements.is_empty() {
            return Err(vec![Problem::plain("there is nothing to define")]);
        }
        for statement in &statements {
            if statement
                .key()
                .and_then(|key| key.split_once(':'))
                .is_some_and(|(_, name)| name.starts_with("__routing_"))
            {
                return Err(vec![Problem::plain(
                    "names starting with __routing_ are reserved for private routing resources",
                )]);
            }
            let kind = statement.key().map(|k| k.split(':').next().unwrap_or(k));
            let allowed = match what {
                What::Definitions => kind.is_some_and(|k| {
                    matches!(
                        k,
                        "instr"
                            | "fn"
                            | "const"
                            | "ctl"
                            | "bus"
                            | "buffer"
                            | "track"
                            | "sample"
                            | "kit"
                            | "config"
                    )
                }),
                What::Forget => statement.clears(),
            };
            if !allowed {
                let mut problem = Problem::plain(format!(
                    "`{}` can't be sent here: `define` takes definitions such as instruments, tracks, effects, buses and controls, musical structure and timing belong to the client",
                    first_line(&statement.text)
                ));
                problem.span = Some(niminal_lang::Span::new(
                    statement.offset,
                    statement.offset + statement.text.len(),
                ));
                return Err(match &uses {
                    Some(u) => u.locate(vec![problem]),
                    None => vec![problem],
                });
            }
        }
        Ok((text, statements, uses))
    }

    /// Compile `statements` on top of the project `base`, and build a mixer for the result.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &self,
        id: u64,
        lands_at: u64,
        snippet: String,
        statements: Vec<Statement>,
        base: &Project,
        base_definitions: &Arc<Program>,
        routing: &crate::routing::Routing,
        edits: Vec<crate::RoutingEdit>,
        meta: Meta,
    ) -> Result<Pending, Vec<Problem>> {
        let clears = statements.iter().any(|s| s.clears());
        let (project, definitions) = if statements.is_empty() {
            (base.clone(), base_definitions.clone())
        } else {
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

            // `new` forgets everything sent before this snippet; the output layout stays, since it can only be
            // chosen when the daemon starts.
            let fresh = Project {
                entries: base
                    .entries
                    .iter()
                    .filter(|e| e.key == "config")
                    .cloned()
                    .collect(),
            };
            let base = if clears { &fresh } else { base };
            let mut project = base.clone();
            project.apply(&delta);

            let mut compile_options = options(self.layout, &self.samples);
            if !clears {
                compile_options.buffers = base_definitions.buffers.iter().cloned().collect();
            }
            project.drop_broken(&from_snippet, |full| {
                compile_with(full, &compile_options).map(|_| ())
            });
            let (full, map) = project.source(&from_snippet, &[]);
            let program = compile_with(&full, &compile_options).map_err(|diags| {
                diags
                    .iter()
                    .map(|d| Problem::from_diagnostic(d, &snippet, map.locate(d.span)))
                    .collect::<Vec<_>>()
            })?;
            if program.master != self.layout {
                return Err(vec![Problem::plain(format!(
                    "this engine outputs {}, but the project says {}; the output layout can only be chosen when the daemon starts",
                    self.layout, program.master
                ))]);
            }
            (project, Arc::new(program))
        };
        let mut routing = if clears {
            crate::routing::Routing::default()
        } else {
            routing.clone()
        };
        routing
            .apply(
                meta.client,
                &edits,
                &definitions,
                &options(self.layout, &self.samples),
                lands_at,
                self.sample_rate,
            )
            .map_err(|e| vec![Problem::plain(e)])?;
        let program = routing
            .materialize(&definitions)
            .map_err(|e| vec![Problem::plain(e)])?;
        let mixer = program.mixer(self.sample_rate);
        Ok(Pending {
            applied: Arc::new(AtomicU64::new(super::transactions::WAITING)),
            id,
            lands_at,
            snippet,
            statements,
            project,
            definitions,
            routing,
            edits,
            program: Arc::new(program),
            mixer,
            meta,
        })
    }

    /// Compile the waiting changes from `index` on again, each on the one before. One that no longer
    /// compiles because of what came before it is dropped, with a notice.
    fn recompile_from(&mut self, index: usize) {
        let mut base = match index.checked_sub(1) {
            Some(i) => self.pending[i].project.clone(),
            None => self.project.clone(),
        };
        let mut routing = index
            .checked_sub(1)
            .map_or_else(|| self.routing.clone(), |i| self.pending[i].routing.clone());
        let mut definitions = index.checked_sub(1).map_or_else(
            || self.definitions.clone(),
            |i| self.pending[i].definitions.clone(),
        );
        let rest: Vec<Pending> = self.pending.drain(index..).collect();
        for p in rest {
            let Pending {
                id,
                lands_at,
                snippet,
                statements,
                meta,
                edits,
                applied,
                ..
            } = p;
            let result = self.prepare(
                id,
                lands_at,
                snippet,
                statements,
                &base,
                &definitions,
                &routing,
                edits,
                meta,
            );
            match result {
                Ok(mut next) => {
                    next.applied = applied;
                    base = next.project.clone();
                    routing = next.routing.clone();
                    definitions = next.definitions.clone();
                    self.pending.push(next);
                }
                Err(problems) => {
                    let why = problems
                        .first()
                        .map_or("it no longer compiles", |p| p.message.as_str());
                    self.notify(format!("change {id} was dropped: {why}"));
                }
            }
        }
    }
}

/// How code is compiled here.
pub(super) fn options(layout: Layout, samples: &Samples) -> CompileOptions {
    CompileOptions {
        defer_latency: true,
        default_layout: layout,
        samples: samples.clone(),
        ..Default::default()
    }
}

pub(super) fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        format!("{}...", line.chars().take(57).collect::<String>())
    } else {
        line.to_string()
    }
}
