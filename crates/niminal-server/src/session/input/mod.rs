//! Things that arrive from outside while the engine runs: batches of events and their cancellation.

mod apply;
mod cancel;
mod plan;

use super::*;

pub(super) enum Plan {
    NodeBind {
        id: String,
        param: String,
        resource: u64,
        bank: niminal_engine::BufferBank,
        count: usize,
        at: u64,
        tag: Option<String>,
    },
    ChainMap(ChainMap),
    Bind {
        id: u64,
        param: String,
        resource: u64,
        bank: niminal_engine::BufferBank,
        count: usize,
        at: u64,
        tag: Option<String>,
    },
    Buffer {
        set: BufferSet,
        start: u64,
    },
    Map {
        id: u64,
        start: u64,
        param: String,
        source: Option<String>,
        channel: usize,
        scale: f32,
        offset: f32,
        tag: Option<String>,
    },
    Note {
        op: usize,
        event: Event,
        instrument: String,
        start: u64,
        dur: u64,
        id: Option<u64>,
        tag: Option<String>,
    },
    Control {
        name: String,
        value: f32,
        frames: u64,
        at: u64,
        op: usize,
        tag: Option<String>,
        /// Sent for "now" rather than for a sample.
        now: bool,
    },
    Node {
        key: String,
        value: f32,
        frames: u64,
        at: u64,
        tag: Option<String>,
    },
    Panic {
        at: u64,
        tag: Option<String>,
    },
    Set {
        id: u64,
        args: Vec<(String, Value, f32)>,
        frames: u64,
        at: u64,
        tag: Option<String>,
    },
    Off {
        id: u64,
        at: u64,
        tag: Option<String>,
    },
}

impl Session {
    /// A note handed to the session directly. If it falls in a stretch the planner has prepared already,
    /// the planner puts it in without redoing that stretch.
    pub(super) fn add_explicit(
        &mut self,
        start: u64,
        dur: u64,
        event: Event,
        key: Option<u32>,
        owner: Option<(ClientId, Option<String>)>,
        activity: Option<Arc<niminal_engine::Activity>>,
    ) {
        if self.live.is_some() {
            self.fresh.push(realtime::Fresh::Note {
                event: event.clone(),
                start,
                dur,
                key,
                activity: activity.clone(),
            });
        }
        let (client, tag) = owner.map_or((None, None), |(c, t)| (Some(c), t));
        self.insert_upcoming(Upcoming {
            start,
            dur,
            event,
            key,
            client,
            tag,
            activity,
        });
    }

    /// Move the control `name` to `value` (in the engine's unit) at sample `at`, over `frames`, in the program
    /// that is running then. `log` says which event of which log entry it is, so that the sample it really
    /// happens at can be written back.
    fn queue_control(
        &mut self,
        name: &str,
        value: f32,
        frames: u64,
        at: u64,
        log: (usize, usize),
        now: bool,
    ) -> Result<(), &'static str> {
        if self.control_changes.len() >= crate::controls::CAPACITY {
            return Err("the control update queue is full");
        }
        let program = self.program_at(at).clone();
        let Some(index) = program.controls.iter().position(|c| c.name == name) else {
            return Err("the control is not defined any more");
        };
        let def = &program.controls[index];
        // a control that a change still to land declares is not there for the renderer until it lands
        let revision = match self.pending.iter().rev().find(|p| p.lands_at <= at) {
            Some(p)
                if p.statements
                    .iter()
                    .any(|s| s.key.strip_prefix("ctl:") == Some(name)) =>
            {
                p.id
            }
            _ => self
                .controls
                .iter()
                .find(|c| c.key == def.key)
                .map_or(0, |c| c.revision),
        };
        let change = crate::controls::Change {
            index,
            unit: def.unit,
            revision,
            at,
            value,
            frames,
            serial: self.next_control,
            log_index: log.0,
            log_op: log.1,
            applied: !self.detached,
            now,
        };
        if let Some(planner) = &mut self.live {
            planner
                .push_control(crate::controls::ControlMessage::Set(change))
                .map_err(|_| "the control update queue is full")?;
        }
        self.next_control += 1;
        self.control_changes.push(change);
        self.control_changes
            .sort_unstable_by_key(|c| (c.at, c.serial));
        Ok(())
    }

    pub(super) fn resolve(&self, at: At) -> u64 {
        match at {
            At::Sample(n) => n,
            At::Now => self.clock,
        }
    }

    /// Do what a batch of events from `client` says. All of it is checked first: if any is wrong none of it is
    /// done. Times come out as samples in the log, so that replaying it gives the same sound.
    pub fn send_events(
        &mut self,
        client: ClientId,
        mut ops: Vec<Op>,
    ) -> Result<Sent, Vec<Problem>> {
        self.sync_realtime();
        for op in &mut ops {
            match op {
                Op::Note { target, to, at, .. } => {
                    if to.is_some() || !self.routing_at(self.resolve(*at)).has_node(client, target)
                    {
                        *target = definitions::scoped(client, target);
                    }
                }
                Op::Control { name, .. } | Op::BufferWrite { name, .. } => {
                    *name = definitions::scoped(client, name)
                }
                Op::BindBuffer { buffer, .. } => *buffer = definitions::scoped(client, buffer),
                Op::Map {
                    source: Some(name), ..
                } => *name = definitions::scoped(client, name),
                Op::Routing { edits, .. } | Op::Transaction { edits, .. } => {
                    for edit in edits {
                        match edit {
                            crate::RoutingEdit::BindBuffer { buffer, .. } => {
                                *buffer = definitions::scoped(client, buffer)
                            }
                            crate::RoutingEdit::Import { node, .. } => {
                                *node = definitions::scoped(client, node)
                            }
                            crate::RoutingEdit::Bus {
                                source: Some(source),
                                ..
                            } => *source = definitions::scoped(client, source),
                            crate::RoutingEdit::Node {
                                instrument: Some(instrument),
                                ..
                            } => *instrument = definitions::scoped(client, instrument),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let has_changes = ops.iter().any(|op| {
            matches!(
                op,
                Op::Define { .. } | Op::Forget { .. } | Op::Transaction { .. }
            ) || matches!(op, Op::Routing { edits, .. } if !crate::routing::Routing::is_control_edit(edits))
        });
        let backup = has_changes.then(|| {
            let pending = std::mem::take(&mut self.pending);
            self.pending = pending
                .iter()
                .map(|p| p.copy_prepared(self.sample_rate))
                .collect();
            (
                pending,
                self.notices.clone(),
                self.recent_notices.clone(),
                self.samples.clone(),
                self.client_samples.clone(),
            )
        });
        let result = self.send_events_inner(client, ops, has_changes);
        if result.is_err()
            && let Some((pending, notices, recent, samples, client_samples)) = backup
        {
            self.pending = pending;
            self.notices = notices;
            self.recent_notices = recent;
            self.samples = samples;
            self.client_samples = client_samples;
        }
        result
    }

    fn send_events_inner(
        &mut self,
        client: ClientId,
        mut ops: Vec<Op>,
        has_changes: bool,
    ) -> Result<Sent, Vec<Problem>> {
        self.validate_ops(&ops)?;
        let clock = self.clock;

        // Changes of definitions go first: they are compiled, which can fail, and the
        // rest of the batch is checked against the program they leave. If anything later is wrong they are
        // taken back.
        // which were sent for "now" (a knob, a fader), before every time is pinned to a sample for the log
        let sent_now: Vec<bool> = ops.iter().map(|op| matches!(op.at(), At::Now)).collect();
        for op in &mut ops {
            *op.at_mut() = At::Sample(self.resolve(op.at()));
        }
        let mut clock_state = self.clocks.get(&client).copied().unwrap_or_default();
        // controls that are now kept to a number of beats (`Some`) or let go of (`None`), once the batch is accepted
        let mut linked: HashMap<String, Option<Link>> = HashMap::new();
        let accepted = self.queue_definitions(client, &ops, &clock_state, &mut linked)?;
        let plans = self.plan_ops(
            client,
            &mut ops,
            &sent_now,
            clock,
            &mut clock_state,
            &mut linked,
        )?;

        self.clocks.insert(client, clock_state);
        for (name, link) in linked {
            match link {
                Some(link) => self.linked.insert(name, link),
                None => self.linked.remove(&name),
            };
        }
        let sent = self.apply_plans(client, ops, accepted, plans, clock, has_changes);
        Ok(sent)
    }

    pub(super) fn apply_controls(&mut self) {
        while let Some(index) = self
            .control_changes
            .iter()
            .position(|c| c.at <= self.clock && c.applied)
        {
            let change = self.control_changes.remove(index);
            if self
                .program
                .controls
                .get(change.index)
                .is_some_and(|d| d.unit == change.unit)
                && let Some(state) = self.controls.get_mut(change.index)
            {
                state.set(change.at, change.value, change.frames);
                if !self.detached {
                    self.mixer.set_control(
                        &state.key,
                        state.start,
                        state.target,
                        state.frames,
                        self.clock.saturating_sub(state.at),
                    );
                }
            } else {
                self.notify("a queued control update was dropped after its unit changed".into());
            }
        }
    }
}

pub(super) fn problem_at(event: usize, message: impl Into<String>) -> Problem {
    let mut p = Problem::plain(message);
    p.event = Some(event);
    p
}

/// `edits` with the lengths in beats and bars of the arguments set in seconds, at the tempo of `clock`; `linked`
/// is told which of the controls they are for are now kept to beats, and which are set to something else.
pub(super) fn in_seconds(
    edits: &[crate::RoutingEdit],
    client: ClientId,
    clock: &niminal_lang::ClockState,
    linked: &mut HashMap<String, Option<Link>>,
) -> Vec<crate::RoutingEdit> {
    let mut edits = edits.to_vec();
    for edit in &mut edits {
        let (crate::RoutingEdit::Node { id, args, .. }
        | crate::RoutingEdit::Replace { id, args, .. }
        | crate::RoutingEdit::Set { id, args, .. }) = edit
        else {
            continue;
        };
        for (param, value) in args.iter_mut() {
            let beats = match *value {
                Value::Beats(b) => Some(b),
                Value::Bars(b) => Some(b * clock.beats_per_bar),
                _ => None,
            };
            if let Some(beats) = beats {
                *value = Value::Seconds(beats * 60.0 / clock.bpm);
            }
            linked.insert(
                crate::routing::param_key(client, id, param),
                beats.map(|beats| Link {
                    client,
                    beats,
                    node: true,
                }),
            );
        }
    }
    edits
}
