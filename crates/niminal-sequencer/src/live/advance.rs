use super::*;

impl Live {
    /// Know where the engine is: by asking it, or if it said recently, by counting the time since. The count
    /// starts when its answer arrived, so it never runs ahead of the engine.
    pub(super) fn tick(&mut self) -> bool {
        if let Some((at, sample)) = self.synced
            && at.elapsed() < self.clock_sync
        {
            let since = (at.elapsed().as_secs_f64() * f64::from(self.sample_rate)) as u64;
            self.clock = self.clock.max(sample + since);
            return true;
        }
        self.look()
    }

    /// Ask the engine where it is and take in what it has to say.
    pub(super) fn look(&mut self) -> bool {
        match self.link.request(EngineRequest::Clock {}) {
            Ok(EngineReply::Clock(info)) => {
                self.clock = self.clock.max(info.sample);
                self.synced = Some((Instant::now(), info.sample));
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
                    for client in params["routing"].as_array().into_iter().flatten() {
                        note_peaks(&mut self.node_peaks, &client["nodes"]);
                        note_peaks(&mut self.bus_peaks, &client["buses"]);
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
        if !self.tick() {
            return;
        }
        let horizon = horizon
            .unwrap_or(self.clock + self.lookahead)
            .max(self.clock);
        // the engine knows of every change before it is sent notes that depend on one
        self.sync_pending();
        loop {
            while let Some(i) = self
                .pending
                .iter()
                .position(|p| !p.done && p.lands_at <= horizon)
            {
                // what falls before the change is as the project was
                self.fetch_until(self.pending[i].lands_at);
                self.clock_out.emit(&self.grid(), self.pending[i].lands_at);
                self.land(i);
                self.sync_pending();
            }
            // a clip about to play its last pass queues what comes next, which may land at once
            if !self.arm_thens(horizon) {
                break;
            }
        }
        self.fetch_until(horizon);
        self.clock_out.emit(&self.grid(), horizon);
        // a clip that plays once and has finished is no longer running, and forgets what was done to it
        let at = self.seconds(horizon);
        let (schedule, nodes) = (&self.schedule, &self.program.nodes);
        self.playing.retain(|name, _| {
            nodes
                .iter()
                .position(|n| n.name == *name)
                .is_some_and(|i| schedule.node_state(i, at).clip.is_some())
        });
        let clock = self.clock;
        self.pending.retain(|p| !p.done || p.lands_at > clock);
        self.asked.retain(|a| a.lands_at > clock);
        self.written
            .retain(|w| w.start + w.dur + 1 > clock.saturating_sub(self.sample_rate as u64 * 64));
        self.sync_pending();
    }

    /// Queue the `then:` of every clip that is about to finish, to land where it does, and take back
    /// those of clips that were stopped or replaced first. Is there something new to land?
    fn arm_thens(&mut self, horizon: u64) -> bool {
        if self.arming {
            return false;
        }
        let finishes: Vec<(Finish, u64)> = self
            .schedule
            .finishes()
            .into_iter()
            .map(|f| {
                let at = self.samples(f.at.max(0.0));
                (f, at)
            })
            .collect();
        // clips that no longer finish where they were going to
        let mut taken_back = Vec::new();
        let (clock, second) = (self.clock, self.sample_rate as u64);
        self.armed.retain(|a| {
            let kept = finishes.iter().any(|(f, at)| {
                f.node == a.node && f.origin.to_bits() == a.origin.to_bits() && *at == a.at
            });
            if !kept && a.at > clock {
                taken_back.extend(a.ids.iter().copied());
            }
            kept && a.at + second > clock
        });
        for id in taken_back {
            self.cancel(Some(id));
        }
        let ahead = (f64::from(self.sample_rate) * ARM_AHEAD_SECONDS) as u64;
        let mut queued = false;
        for (f, at) in finishes {
            let known = self
                .armed
                .iter()
                .any(|a| a.node == f.node && a.origin.to_bits() == f.origin.to_bits());
            if known || at > horizon + ahead || at + self.sample_rate as u64 <= self.clock {
                continue;
            }
            self.arming = true;
            let ids: Vec<u64> = f
                .then
                .iter()
                .filter_map(|statement| self.eval_at(statement, at))
                .collect();
            self.arming = false;
            queued |= !ids.is_empty();
            self.armed.push(Armed {
                node: f.node,
                origin: f.origin,
                at,
                ids,
            });
        }
        queued
    }

    /// Evaluate `statement` so that it lands at sample `at`. Its id, if it was accepted.
    fn eval_at(&mut self, statement: &str, at: u64) -> Option<u64> {
        self.landing_at = Some(at);
        let result = self.eval(statement, Some("now"));
        self.landing_at = None;
        match result {
            Ok(accepted) => accepted.id,
            Err(problems) => {
                let why = problems
                    .first()
                    .map_or_else(String::new, |p| format!(": {}", p.message));
                self.notify(format!("`then: {statement}` could not run{why}"));
                None
            }
        }
    }

    /// Clips kept from before a program changed name their drums and nodes by number, and the new program
    /// may number them differently (a kit given its drums in another order): translate by name.
    fn renumber_history(&mut self, before: &Composition) {
        let (old, new) = (before, &self.program);
        if old.sample_names == new.sample_names
            && old
                .nodes
                .iter()
                .map(|n| &n.name)
                .eq(new.nodes.iter().map(|n| &n.name))
        {
            return;
        }
        let samples: Vec<Option<u32>> = old
            .sample_names
            .iter()
            .map(|name| {
                new.sample_names
                    .iter()
                    .position(|n| n == name)
                    .map(|i| i as u32)
            })
            .collect();
        let nodes: Vec<Option<usize>> = old
            .nodes
            .iter()
            .map(|node| new.nodes.iter().position(|n| n.name == node.name))
            .collect();
        for h in &mut self.history {
            if let NamedAction::Start { clip, .. } | NamedAction::Update { clip, .. } =
                &mut h.action
            {
                let (samples, nodes) = (samples.clone(), nodes.clone());
                clip.renumber(
                    move |id| samples.get(id as usize).copied().flatten(),
                    move |n| nodes.get(n).copied().flatten(),
                );
            }
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
            let before = std::mem::replace(&mut self.program, program);
            self.project = project;
            self.renumber_history(&before);
            // what was done to a clip that is gone is not done again to a new clip of its name
            let program = &self.program;
            self.history.retain(|h| match &h.action {
                NamedAction::Start { node, .. } | NamedAction::Update { node, .. } => {
                    program.node_index(node).is_some()
                }
                NamedAction::Stop(t)
                | NamedAction::Mute(t)
                | NamedAction::Unmute(t)
                | NamedAction::Solo(t) => program.node_index(t).is_some(),
                NamedAction::Unsolo(t) => {
                    t.as_ref().is_none_or(|t| program.node_index(t).is_some())
                }
                _ => true,
            });
        }
        let mut captures = Vec::new();
        let mut ops = Vec::new();
        for a in actions.iter().chain(&replays) {
            captures.extend(self.run_action(a, at, &mut ops));
        }
        self.send(ops);
        self.playing = playing;

        self.rebuild_schedule(at);
        self.send(vec![self.tempo_op(at, self.program.tempo, false)]);
        let mut changes = summary;
        for (bars, name) in captures {
            if let Some(notice) = self.capture_section(bars, &name, at) {
                changes.push(notice);
            }
        }
        let position = self.grid().position(at);
        if let Some(i) = self.followed.iter().position(|&f| f == id) {
            self.followed.remove(i);
            return;
        }
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
        // starting an arrangement puts its lines into the history, each marked with its name
        let arrangement = statement
            .target()
            .filter(|t| statement.starts() && program.arrangements.iter().any(|a| a.name == *t))
            .map(str::to_string);
        if let Some(name) = &arrangement {
            // its lines are placed from where the command was (`at +2bars song.start`), so the start is the
            // earliest of them, less where the arrangement itself begins
            let own = program
                .arrangements
                .iter()
                .find(|a| a.name == *name)
                .map_or(0.0, |a| {
                    a.actions
                        .iter()
                        .map(|s| s.at.to_seconds(tempo))
                        .chain(a.lanes.iter().map(|(_, _, t)| t.to_seconds(tempo)))
                        .fold(f64::INFINITY, f64::min)
                });
            let first = program
                .performance
                .iter()
                .filter(|s| !matches!(s.action, Action::Cancel { .. } | Action::Capture { .. }))
                .map(|s| s.at.to_seconds(tempo))
                .fold(f64::INFINITY, f64::min);
            if own.is_finite() && first.is_finite() {
                let begun = if statement.has_at() {
                    origin + first - own
                } else {
                    landing_seconds + first - own
                };
                self.arrangement_starts.insert(name.clone(), begun);
            }
        }
        for scheduled in &program.performance {
            if let Action::Capture { bars, name } = &scheduled.action {
                captures.push((*bars, name.clone()));
                continue;
            }
            if let Action::Cancel { arrangement } = &scheduled.action {
                self.arrangement_starts.remove(arrangement);
                self.history.retain(|h| {
                    h.arrangement.as_deref() != Some(arrangement) || h.at <= landing_seconds
                });
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
            if matches!(named, NamedAction::Reset) {
                self.clock_out.restart(self.samples(when));
            }
            self.history.push(HistoryEntry {
                at: when,
                action: named,
                arrangement: arrangement.clone(),
            });
        }
        captures
    }

    /// The event as it can play in the project as it is now, with its plan, or why it can't (said in a notice). A
    /// lane for a parameter its instrument no longer has is left out, so redefining an instrument does not
    /// silence the clips that play it.
    pub(super) fn playable(
        &mut self,
        what: &str,
        event: &Event,
    ) -> Option<(Event, niminal_lang::NotePlan)> {
        let mut event = event.clone();
        let mut ignored = Vec::new();
        let plan = loop {
            match self.program.plan(&event) {
                Ok(plan) => break plan,
                Err(e) => match e.unknown_parameter().map(str::to_owned) {
                    Some(name) if event.args.remove(&name).is_some() => ignored.push(name),
                    _ => {
                        let fix = e.missing_argument().map_or(String::new(), |name| {
                            format!(
                                " (give it a default in the instrument, as `{name}: ... = ...`, or set `{name}:` in the clip)"
                            )
                        });
                        self.notify(format!(
                            "{what} for `{}` could not play: {e}{fix}",
                            event.target
                        ));
                        return None;
                    }
                },
            }
        };
        if !ignored.is_empty() {
            self.notify(format!(
                "`{}` sets {}, which its instrument doesn't have any more: left out",
                event.target,
                ignored
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Some((event, plan))
    }

    /// A note for the engine, if it can play in the project as it is now.
    pub(super) fn note_op(&mut self, event: &Event, start: u64, dur: u64, tag: &str) -> Option<Op> {
        let (event, _) = self.playable("note", event)?;
        let event = &event;
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
                self.quantum_of(s, None).unit,
                QuantizeUnit::Bar | QuantizeUnit::Cycle
            )
        })
    }

    /// The schedule is made again from the project as it is at `at`, and what was sent for after that
    /// is taken back, to be fetched again.
    pub(super) fn rebuild_schedule(&mut self, at: u64) {
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
            self.land_control_changes(self.fetched_until);
            let mut piece = (self.fetched_until + self.sample_rate as u64 / 2).min(horizon);
            // a control that moves within the stretch splits it, so that what comes before is played as it was
            if let Some(&(at, ..)) = self.control_changes.first()
                && at > self.fetched_until
                && at < piece
            {
                piece = at;
            }
            self.fetch_piece(piece);
        }
    }

    /// Give the bank the control moves that are due by sample `until`.
    fn land_control_changes(&mut self, until: u64) {
        let due = self.control_changes.partition_point(|c| c.0 <= until);
        for (_, name, value) in self.control_changes.drain(..due) {
            self.program.bank.set(&name, value);
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
            ops.push(self.tempo_op(self.samples(origin.at), self.program.tempo, true));
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
        ops.extend(found.controls.iter().filter_map(|set| self.control_op(set)));
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

    /// The op that makes a control change from a clip's lanes, if the control exists.
    pub(super) fn control_op(&self, set: &niminal_compose::ControlSet) -> Option<Op> {
        let def = self.program.controls.iter().find(|c| c.name == set.name)?;
        let (node, param) = set.name.split_once("::")?;
        let value = match def.unit {
            niminal_lang::Unit::Hz => Value::Hz(set.target),
            // the engine takes a bare number as a linear gain
            niminal_lang::Unit::Db | niminal_lang::Unit::Num => Value::Num(set.target),
            niminal_lang::Unit::Time => Value::Seconds(set.target),
            niminal_lang::Unit::Angle => Value::Degrees(set.target),
            niminal_lang::Unit::Semitones => Value::Semitones(set.target),
        };
        Some(Op::Routing {
            at: At::Sample(self.samples(set.at)),
            edits: vec![RoutingEdit::Set {
                id: routing_id(node),
                args: [(param.to_string(), value)].into(),
                glide: set.glide,
            }],
            tag: Some(SCHEDULED.into()),
        })
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
                    let message =
                        format!("change {id} was refused by the engine: {}", describe(&e));
                    self.notify(message.clone());
                    self.refused.push((id, message));
                    self.pending.remove(index);
                    self.version += 1;
                    return;
                }
            }
        }
    }

    pub(super) fn definition_ops(
        &self,
        statements: &[Statement],
        program: Option<&Composition>,
        at: At,
    ) -> Vec<Op> {
        let mut ops = definition_ops(statements, program, at, self.sample_dirs.first());
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
                    fx: Some(id.clone()),
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
                fx_decls: nodes.clone(),
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
                complete: true,
                tag: None,
            });
        }
        if statements.iter().any(Statement::clears) {
            self.linked_sent.borrow_mut().clear();
        }
        if let Some(program) = program {
            // effect parameters that start at a length in beats follow the tempo; one already told, and perhaps
            // changed by hand since, is left as it is
            let mut sent = self.linked_sent.borrow_mut();
            for control in niminal_compose::beat_controls(&program.source) {
                let key = match &control.effect {
                    Some(effect) => format!("{effect}::{}", control.name),
                    None => control.name.clone(),
                };
                let told = (control.value, control.unit.to_string());
                if sent.get(&key) == Some(&told) {
                    continue;
                }
                sent.insert(key, told);
                ops.push(match &control.effect {
                    Some(effect) => Op::Routing {
                        at,
                        edits: vec![RoutingEdit::Set {
                            id: routing_id(effect),
                            args: [(
                                control.name.clone(),
                                if control.unit == "bars" {
                                    Value::Bars(control.value)
                                } else {
                                    Value::Beats(control.value)
                                },
                            )]
                            .into(),
                            glide: 0.0,
                        }],
                        tag: None,
                    },
                    None => Op::Control {
                        name: control.name.clone(),
                        at,
                        value: control.value,
                        unit: Some(control.unit.to_string()),
                        glide: Some(Time::Seconds(0.0)),
                        tag: None,
                    },
                });
            }
        }
        if ops.iter().any(|op| matches!(op, Op::Define { source, .. } | Op::Transaction { source, .. } if source.contains("clock.") || niminal_lang::reads_clock(source))) {
            let tempo = program.map_or(self.program.tempo, |p| p.tempo);
            let at = match at {
                At::Sample(n) => n,
                At::Now => self.clock,
            };
            ops.insert(0, self.tempo_op(at, tempo, false));
        }
        ops
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



pub(super) fn change_tag(id: u64) -> String {
    format!("change-{id}")
}

pub(super) fn tag_op(op: &mut Op, tag: &str) {
    match op {
        Op::Define { tag: t, .. }
        | Op::Forget { tag: t, .. }
        | Op::Routing { tag: t, .. }
        | Op::Transaction { tag: t, .. } => *t = Some(tag.to_string()),
        _ => {}
    }
}

/// What the engine said was wrong, in a sentence.
pub(super) fn describe(error: &EngineError) -> String {
    let problems = error.problems();
    if problems.is_empty() {
        format!("the engine: {}", error.message)
    } else {
        format!("the engine: {}", problems.join("; "))
    }
}

pub(crate) fn name_action(program: &Composition, action: &Action) -> Option<NamedAction> {
    let name = |t: &usize| program.nodes[*t].name.clone();
    Some(match action {
        Action::Start { node, clip } => NamedAction::Start {
            node: name(node),
            clip: clip.clone(),
        },
        Action::Update { node, clip } => NamedAction::Update {
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
        Action::Lane { control, lane } => NamedAction::Lane {
            control: control.clone(),
            lane: lane.clone(),
        },
        Action::Capture { .. } | Action::Cancel { .. } => return None,
    })
}

pub(crate) fn schedule_for(program: &Composition, history: &[HistoryEntry]) -> Schedule {
    let names = program.nodes.iter().map(|t| t.name.clone()).collect();
    let index = |name: &String| program.node_index(name);
    let commands = history
        .iter()
        .filter_map(|h| {
            let action = match &h.action {
                NamedAction::Start { node, clip } => Action::Start {
                    node: index(node)?,
                    clip: clip.clone(),
                },
                NamedAction::Update { node, clip } => Action::Update {
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
                NamedAction::Lane { control, lane } => Action::Lane {
                    control: control.clone(),
                    lane: lane.clone(),
                },
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
