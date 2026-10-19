//! Things that arrive from outside while the engine runs: batches of events and their cancellation.

use super::*;

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
    ) {
        if self.live.is_some() {
            self.fresh.push(realtime::Fresh::Note { event: event.clone(), start, dur, key });
        }
        let (client, tag) = owner.map_or((None, None), |(c, t)| (Some(c), t));
        self.insert_upcoming(Upcoming { start, dur, event, key, client, tag });
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
                    .any(|s| s.key().and_then(|k| k.strip_prefix("ctl:")) == Some(name)) =>
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
            retract: false,
        };
        if let Some(planner) = &mut self.live {
            planner
                .push_control(change)
                .map_err(|_| "the control update queue is full")?;
        }
        self.next_control += 1;
        self.control_changes.push(change);
        self.control_changes
            .sort_unstable_by_key(|c| (c.at, c.serial));
        Ok(())
    }

    fn logged(&self, at: At) -> At { At::Sample(self.resolve(at)) }

    pub(super) fn resolve(&self, at: At) -> u64 {
        match at { At::Sample(n) => n, At::Now => self.clock }
    }

    /// Do what a batch of events from `client` says. All of it is checked first: if any is wrong none of it is
    /// done. Times come out as samples in the log, so that replaying it gives the same sound.
    pub fn send_events(&mut self, client: ClientId, ops: Vec<Op>) -> Result<Sent, Vec<Problem>> {
        self.sync_realtime();
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
                self.sample_dirs.clone(),
            )
        });
        let result = self.send_events_inner(client, ops, has_changes);
        if result.is_err()
            && let Some((pending, notices, recent, samples, dirs)) = backup
        {
            self.pending = pending;
            self.notices = notices;
            self.recent_notices = recent;
            self.samples = samples;
            self.sample_dirs = dirs;
        }
        result
    }

    fn send_events_inner(
        &mut self,
        client: ClientId,
        ops: Vec<Op>,
        has_changes: bool,
    ) -> Result<Sent, Vec<Problem>> {
        let mut invalid = Vec::new();
        for (index, op) in ops.iter().enumerate() {
            let (duration, args) = match op {
                Op::Note { dur, args, .. } => (*dur, Some(args)),
                Op::Set { glide, args, .. } | Op::Track { glide, args, .. } => (*glide, Some(args)),
                Op::Control { glide, unit, .. } => {
                    if unit.as_deref().is_some_and(|u| matches!(u, "beat" | "beats" | "bar" | "bars" | "bpm")) {
                        invalid.push(problem_at(index, "resolve musical units in the client"));
                    }
                    (*glide, None)
                }
                Op::Array { glide, values, .. } => {
                    if values.iter().any(|v| matches!(v, Value::Beats(_) | Value::Bars(_))) {
                        invalid.push(problem_at(index, "resolve musical units in the client"));
                    }
                    (*glide, None)
                }
                _ => (None, None),
            };
            if duration.is_some_and(|t| !matches!(t, Time::Seconds(n) if n.is_finite() && n >= 0.0)) ||
                args.is_some_and(|a| a.values().any(|v| matches!(v, Value::Beats(_) | Value::Bars(_)))) {
                invalid.push(problem_at(index, "durations must be finite, nonnegative seconds; resolve musical time in the client"));
            }
        }
        if !invalid.is_empty() { return Err(invalid); }
        let rate = f64::from(self.sample_rate);
        let clock = self.clock;


        // Changes of definitions go first: they are compiled, which can fail, and the
        // rest of the batch is checked against the program they leave. If anything later is wrong they are
        // taken back.
        let mut resolved: Vec<Option<Op>> = vec![None; ops.len()];
        let mut early = Vec::new();
        let mut accepted = Vec::new();
        for (n, op) in ops.iter().enumerate() {
            if let Op::Routing { edits, .. } = op && crate::routing::Routing::is_control_edit(edits) { continue; }
            if let Op::Routing { at, edits, tag } = op {
                let sample = self.resolve(*at);
                resolved[n] = Some(Op::Routing {
                    at: At::Sample(sample),
                    edits: edits.clone(),
                    tag: tag.clone(),
                });
                match self.queue_routing(
                    edits.clone(),
                    sample,
                    Meta {
                        client,
                        tag: tag.clone(),
                    },
                ) {
                    Ok(id) => {
                        let pending=self.pending.iter().find(|p|p.id==id).unwrap();
                        accepted.push((crate::events::TransactionReceipt { id, at:pending.lands_at }, pending.applied.clone()));
                    }
                    Err(mut problems) => {
                        for p in &mut problems {
                            p.event = Some(n);
                        }
                        early.extend(problems);
                        break;
                    }
                }
                continue;
            }
            let (source, dir, at, what, edits, meta) = match op {
                Op::Define { source, at, dir, tag } => {
                    if source.trim().is_empty() { early.push(problem_at(n,"there is nothing to define")); break; }
                    let meta = Meta { client, tag: tag.clone() };
                    resolved[n] = Some(Op::Define { source: source.clone(), at: self.logged(*at), dir: dir.clone(), tag: tag.clone() });
                    (source.clone(), dir.as_deref(), self.resolve(*at), What::Definitions, Vec::new(), meta)
                }
                Op::Forget { at, tag } => {
                    let meta = Meta { client, tag: tag.clone() };
                    resolved[n] = Some(Op::Forget { at: self.logged(*at), tag: tag.clone() });
                    ("new".to_string(), None, self.resolve(*at), What::Forget, Vec::new(), meta)
                }
                Op::Transaction { source, dir, at, edits, tag } => {
                    resolved[n] = Some(Op::Transaction { source:source.clone(), dir:dir.clone(), at:self.logged(*at), edits:edits.clone(), tag:tag.clone() });
                    (source.clone(),dir.as_deref(),self.resolve(*at),What::Definitions,edits.clone(),Meta {client,tag:tag.clone()})
                }
                _ => continue,
            };
            if let Some(dir) = dir {
                self.set_sample_dir(std::path::Path::new(dir));
            }
            let result = self.queue_transaction(&source, dir, at, what, edits, meta);
            match result {
                Ok(id) => {
                    let pending=self.pending.iter().find(|p|p.id==id).unwrap();
                    accepted.push((crate::events::TransactionReceipt {id,at:pending.lands_at}, pending.applied.clone()));
                }
                Err(mut problems) => {
                    for p in &mut problems {
                        p.event = Some(n);
                    }
                    early.extend(problems);
                    break;
                }
            }
        }
        if !early.is_empty() {
            return Err(early);
        }

        let ops = self.expand_parameter_events(client, ops)?;
        enum Plan {
            Buffer { set: BufferSet, start: u64 },
            Map { id: u64, start: u64, param: String, source: Option<String>, channel: usize, scale: f32, offset: f32, tag: Option<String> },
            Note { event: Event, instrument: String, start: u64, dur: u64, id: Option<u64>, tag: Option<String> },
            Control { name: String, value: f32, frames: u64, at: u64, op: usize, tag: Option<String> },
            Track { key: String, value: f32, frames: u64, at: u64, tag: Option<String> },
            Panic { at: u64, tag: Option<String> },
            Set { id: u64, args: Vec<(String, Value, f32)>, frames: u64, at: u64, tag: Option<String> },
            Off { id: u64, at: u64, tag: Option<String> },
        }
        // the notes this client holds, as the batch goes on opening and releasing them
        let mut open: Vec<(u64, String)> = self
            .voices
            .iter()
            .filter(|v| v.client == client)
            .map(|v| (v.id, v.instrument.clone()))
            .collect();
        let mut plans = Vec::new();
        let mut problems = Vec::new();
        let mut controls = self.control_changes.len();
        for (n, op) in ops.iter().enumerate() {
            let mut fail = |message: String| problems.push(problem_at(n, message));
            match op {
                Op::Note {
                    target,
                    to,
                    at,
                    args,
                    dur,
                    id,
                    tag,
                } => {
                    let start = self.resolve(*at);
                    resolved[n] = Some(Op::Note {
                        to: to.clone(),
                        target: target.clone(),
                        at: self.logged(*at),
                        args: args.clone(),
                        dur: *dur,
                        id: *id,
                        tag: tag.clone(),
                    });
                    if dur.is_none() && id.is_none() {
                        fail("a note needs a `dur`, or an `id` to be released by an `off`".into());
                        continue;
                    }
                    if let Some(id) = id
                        && open.iter().any(|(open, _)| open == id)
                    {
                        fail(format!("note id {id} is already sounding"));
                        continue;
                    }
                    let program = self.program_at(start).clone();
                    if target.contains('@') || target.starts_with("__routing_") {
                        fail("use a local node ID, rather than an internal routing name".into());
                        continue;
                    }
                    let target = if let Some(destination) = to {
                        if !self.routing_at(start).has_node(client, destination) {
                            fail(format!("no node `{destination}`"));
                            continue;
                        }
                        format!(
                            "{target}@{}",
                            crate::routing::node_name(client, destination)
                        )
                    } else if self.routing_at(start).has_node(client, target) {
                        crate::routing::node_name(client, target)
                    } else {
                        target.clone()
                    };
                    let event = Event {
                        target: target.clone(),
                        at: Time::Seconds(0.0),
                        dur: dur.unwrap_or(Time::Seconds(0.0)),
                        args: args.clone(),
                    };
                    match program.plan(&event) {
                        Ok(plan) => {
                            let instrument = program.instruments[plan.instrument].name.clone();
                            if let Some(id) = id {
                                open.push((*id, instrument.clone()));
                            }
                            plans.push(Plan::Note {
                                start,
                                dur: dur.map_or(0, |d| self.samples(d.to_seconds(program.tempo))),
                                event,
                                instrument,
                                id: *id,
                                tag: tag.clone(),
                            });
                        }
                        Err(e) => fail(format!("note for `{target}`: {e}")),
                    }
                }
                Op::Control {
                    name,
                    at,
                    value,
                    unit,
                    glide,
                    tag,
                } => {
                    if name.starts_with("__routing_") {
                        fail("use routing edits to change private instance controls".into());
                        continue;
                    }
                    let raw = at;
                    let at = self.resolve(*at);
                    resolved[n] = Some(Op::Control { name: name.clone(), at: self.logged(*raw), value: *value, unit: unit.clone(), glide: *glide, tag: tag.clone() });
                    let program = self.program_at(at);
                    let Some(def) = program.controls.iter().find(|c| c.name == *name) else {
                        fail(format!("unknown control `{name}`"));
                        continue;
                    };
                    let converted = match def.convert(*value, unit.as_deref(), program.tempo) {
                        Ok(v) => v,
                        Err(e) => {
                            fail(e);
                            continue;
                        }
                    };
                    let seconds = glide.map_or(def.smooth_seconds, |g| g.to_seconds(program.tempo));
                    if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                        fail("`glide` must be between 0 and 60sec".into());
                        continue;
                    }
                    controls += 1;
                    plans.push(Plan::Control { name: name.clone(), value: converted, frames: (seconds * rate).round() as u64, at, op: n, tag: tag.clone() });
                }
                Op::Track { name, at, args, glide, tag } => {
                    let raw = at;
                    let at = self.resolve(*at);
                    resolved[n] = Some(Op::Track {
                        name: name.clone(),
                        at: self.logged(*raw),
                        args: args.clone(),
                        glide: *glide,
                        tag: tag.clone(),
                    });
                    if name.starts_with("__routing_") {
                        fail("use a local node ID".into());
                        continue;
                    }
                    let local_name = if self.routing_at(at).has_node(client, name) {
                        crate::routing::node_name(client, name)
                    } else {
                        name.clone()
                    };
                    let name = &local_name;
                    let program = self.program_at(at);
                    for (param, value) in args {
                        let full = format!("{name}::{param}");
                        let Some(def) = program.controls.iter().find(|c| c.name == full) else {
                            fail(format!("`{name}` has no parameter `{param}`"));
                            continue;
                        };
                        let seconds = glide.map_or(def.smooth_seconds, |g| g.to_seconds(program.tempo));
                        if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                            fail("`glide` must be between 0 and 60sec".into());
                            continue;
                        }
                        match def.from_value(*value, program.tempo) {
                            Ok(v) => {
                                plans.push(Plan::Track { key: def.key.clone(), value: v, frames: (seconds * rate).round() as u64, at, tag: tag.clone() });
                            }
                            Err(e) => fail(format!("parameter `{param}`: {e}")),
                        }
                    }
                }
                Op::Routing { at, edits, tag } => {
                    let start=self.resolve(*at);
                    if crate::routing::Routing::is_control_edit(edits) {
                        resolved[n]=Some(Op::Routing { at:self.logged(*at), edits:edits.clone(), tag:tag.clone() });
                        match self.routing_at(start).updates(client,edits,self.program_at(start)) {
                            Ok(updates) => for (key,value,glide) in updates { plans.push(Plan::Track {key,value,frames:(glide*rate).round() as u64,at:start,tag:tag.clone()}); },
                            Err(e) => fail(e),
                        }
                    }
                }
                Op::Map {
                    id,
                    at,
                    param,
                    source,
                    channel,
                    scale,
                    offset,
                    tag,
                } => {
                    if source.as_ref().is_some_and(|n| n.starts_with("__routing_")) {
                        fail("private routing buses cannot be addressed by internal names".into());
                        continue;
                    }
                    let start = self.resolve(*at);
                    let program = self.program_at(start);
                    let Some((_, name)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    let schema = self
                        .voices
                        .iter()
                        .find(|v| v.client == client && v.id == *id)
                        .map(|v| v.schema.as_ref())
                        .or_else(|| program.instrument(name));
                    let Some(schema) = schema else {
                        fail("the instrument is no longer defined".into());
                        continue;
                    };
                    if let Err(error) = schema.param(param) {
                        fail(error.to_string());
                        continue;
                    }
                    if schema.resources.contains_key(param) {
                        fail("use bind_buffer to select a buffer resource".into());
                        continue;
                    }
                    if !scale.is_finite() || !offset.is_finite() || *channel == 0 {
                        fail(
                            "mapping scale and offset must be finite, and channel is 1-based"
                                .into(),
                        );
                        continue;
                    }
                    if let Some(source) = source {
                        if let Some(bus) = program.buses.iter().position(|n| n == source) {
                            if *channel > program.bus_layouts[bus].channels() { fail("mapping channel is outside the bus".into()); continue; }
                        } else if !program.controls.iter().any(|c| c.name == *source) || *channel != 1 { fail(format!("no control or bus `{source}` with that channel")); continue; }
                    }
                    if self.voice_maps.len() + plans.len() >= 4096 { fail("the modulation mapping queue is full".into()); continue; }
                    resolved[n] = Some(Op::Map { id: *id, at: self.logged(*at), param: param.clone(), source: source.clone(), channel: *channel, scale: *scale, offset: *offset, tag: tag.clone() });
                    plans.push(Plan::Map { id: *id, start, param: param.clone(), source: source.clone(), channel: *channel, scale: *scale, offset: *offset, tag: tag.clone() });
                }
                Op::Set { id, at, args, glide, tag } => {
                    let raw = at;
                    let at = self.resolve(*at);
                    resolved[n] = Some(Op::Set { id: *id, at: self.logged(*raw), args: args.clone(), glide: *glide, tag: tag.clone() });
                    let Some((_, instrument)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    let program = self.program_at(at);
                    let schema = self.voices.iter().find(|v| v.client == client && v.id == *id).map(|v| v.schema.clone())
                        .or_else(|| program.instrument(instrument).map(|i| Arc::new(i.clone())));
                    let Some(instrument) = schema else { fail(format!("the instrument of note {id} is not defined any more")); continue; };
                    let seconds = glide.map_or(0.0, |g| g.to_seconds(program.tempo));
                    if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                        fail("`glide` must be between 0 and 60sec".into());
                        continue;
                    }
                    let mut converted = Vec::new();
                    for (name, value) in args {
                        // a note's pitch is `freq` to the score, and whatever `with(pitch: ...)` named it to the instrument
                        let actual = if name == niminal_lang::PITCH_PARAM { &instrument.pitch } else { name };
                        match instrument.param(actual).and_then(|(_, p)| instrument.param_value(p, *value, program.tempo)) {
                            Ok(v) => converted.push((actual.clone(), *value, v)),
                            Err(e) => fail(e.to_string()),
                        }
                    }
                    plans.push(Plan::Set { id: *id, args: converted, frames: (seconds * rate).round() as u64, at, tag: tag.clone() });
                }
                Op::Off { id, at, tag } => {
                    let raw = at;
                    let at = self.resolve(*at);
                    resolved[n] = Some(Op::Off { id: *id, at: self.logged(*raw), tag: tag.clone() });
                    let Some(i) = open.iter().position(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    open.remove(i);
                    plans.push(Plan::Off { id: *id, at, tag: tag.clone() });
                }
                Op::BufferWrite { name, at, offset, values, tag } => {
                    let start = self.resolve(*at);
                    resolved[n] = Some(Op::BufferWrite { name: name.clone(), at: self.logged(*at), offset: *offset, values: values.clone(), tag: tag.clone() });
                    let program = self.program_at(start);
                    let Some((_, buffer)) = program.buffers.iter().find(|(n, _)| n == name) else {
                        fail(format!("no buffer `{name}`")); continue;
                    };
                    if *offset > buffer.len() || values.len() > buffer.len() - offset {
                        fail("buffer write is outside its length".into()); continue;
                    }
                    if values.len() > 65536 || values.iter().any(|v| !v.is_finite()) {
                        fail("a buffer write has at most 65536 finite samples".into()); continue;
                    }
                    if self.buffer_sets.len() + plans.len() >= 4096 {
                        fail("the buffer write queue is full".into()); continue;
                    }
                    plans.push(Plan::Buffer { start, set: BufferSet { name: name.clone(), offset: *offset, values: values.clone().into(), at: start.max(clock), client, tag: tag.clone(), done: false } });
                }
                Op::Panic { at, tag } => {
                    let start = self.resolve(*at);
                    resolved[n] = Some(Op::Panic { at: self.logged(*at), tag: tag.clone() });
                    plans.push(Plan::Panic { at: start, tag: tag.clone() });
                }
                Op::Array { .. } | Op::BindBuffer { .. } => unreachable!("parameter events expanded"),
                Op::Define { .. } | Op::Forget { .. } | Op::Transaction { .. } => {}
            }
        }
        if problems.is_empty() && controls > crate::controls::CAPACITY {
            problems.push(Problem::plain("the control update queue is full"));
        }
        if !problems.is_empty() {
            return Err(problems);
        }

        let log_index = self.log.len();
        self.log.push(LogEntry {
            sample: clock,
            input: LogInput::Send { client, ops: resolved.into_iter().flatten().collect() },
        });
        self.accept_transactions(&accepted,client);
        let mut sent = Sent { transactions:accepted.into_iter().map(|(r,_)|r).collect(), accepted: ops.len(), late: 0, first: None };
        // definitions, which are queued already
        for op in &ops {
            let at = match op {
                Op::Define { at, .. } | Op::Forget { at, .. } | Op::Transaction { at, .. } => {
                    self.resolve(*at)
                }
                Op::Routing {at,edits,..} if !crate::routing::Routing::is_control_edit(edits) => self.resolve(*at),
                _ => continue,
            };
            sent.late += usize::from(at < clock);
            sent.first = Some(sent.first.map_or(at, |f| f.min(at)));
        }
        for plan in plans {
            let at = match plan {
                Plan::Map { id, start, param, source, channel, scale, offset, tag } => {
                    if let Some(entry) = self.voices.iter().find(|v| v.client == client && v.id == id) {
                        let range = entry.schema.param(&param).ok().and_then(|(_,p)| p.range).map(|(lo,hi)| (lo as f32,hi as f32));
                        let set = VoiceMap { range, key: entry.key, param, source, channel: channel-1, scale, offset, at: start.max(clock), client, tag, done: false };
                        if self.live.is_some() { self.fresh.push(realtime::Fresh::Map(set.clone())); }
                        self.voice_maps.push(set);
                    }
                    start
                }
                Plan::Buffer { set, start } => {
                    if self.live.is_some() { self.fresh.push(realtime::Fresh::Buffer(set.clone())); }
                    self.buffer_sets.push(set);
                    start
                }
                Plan::Note { event, instrument, start, dur, id, tag } => {
                    let key = id.map(|_| self.new_key());
                    if let (Some(id), Some(key)) = (id, key) {
                        let schema = Arc::new(
                            self.program_at(start)
                                .instrument(&instrument)
                                .expect("validated instrument")
                                .clone(),
                        );
                        let destination = event
                            .target
                            .split_once('@')
                            .map(|(_, node)| node.to_string())
                            .or_else(|| {
                                event
                                    .target
                                    .starts_with("__routing_node_")
                                    .then(|| event.target.clone())
                            });
                        self.voices.push(VoiceEntry {
                            destination,
                            client,
                            id,
                            key,
                            instrument,
                            schema,
                            tag: tag.clone(),
                        });
                    }
                    self.add_explicit(start.max(clock), dur, event, key, Some((client, tag)));
                    start
                }
                Plan::Control { name, value, frames, at, op, tag } => {
                    let serial = self.next_control;
                    match self.queue_control(&name, value, frames, at.max(clock), (log_index, op)) {
                        Ok(()) => self.control_owners.push(ControlOwner { serial, client, tag }),
                        Err(e) => self.notify(e.into()),
                    }
                    at
                }
                Plan::Track { key, value, frames, at, tag } => {
                    let set = TrackSet { key, value, frames, at: at.max(clock), client: Some(client), tag, done: false };
                    if self.live.is_some() {
                        self.fresh.push(realtime::Fresh::Track(set.clone()));
                    }
                    let place = self.track_sets.partition_point(|s| s.at <= set.at);
                    self.track_sets.insert(place, set);
                    at
                }
                Plan::Set { id, args, frames, at, tag } => {
                    self.set_voice(client, id, args, frames, at.max(clock), tag);
                    at
                }
                Plan::Panic { at, tag } => {
                    self.timed_panics.push(TimedPanic { at: at.max(clock), client, tag, done: false });
                    self.invalidate_windows();
                    at
                }
                Plan::Off { id, at, tag } => {
                    if let Some(i) = self.voices.iter().position(|v| v.client == client && v.id == id) {
                        let entry = self.voices.remove(i);
                        self.release_at(entry.key, at.max(clock), Some((client, tag)));
                        self.closed.push(entry);
                    }
                    at
                }
            };
            if at < clock {
                sent.late += 1;
            }
            sent.first = Some(sent.first.map_or(at, |f| f.min(at)));
        }
        // a change that was asked for at once happens at once, and the renderer is told
        let waiting = self.pending.len();
        self.land_due(if self.detached { self.clock } else { self.clock.saturating_add(1) });
        if self.pending.len() != waiting {
            self.invalidate_live();
        } else if has_changes {
            self.invalidate_windows();
        }
        self.wake_planner();
        Ok(sent)
    }

    /// Change parameters of the note a client holds by `id`: now, if it is sounding, or as it starts.
    fn set_voice(&mut self, client: ClientId, id: u64, args: Vec<(String, Value, f32)>, frames: u64, at: u64, tag: Option<String>) {
        let Some(entry) = self.voices.iter().find(|v| v.client == client && v.id == id) else {
            return;
        };
        let key = entry.key;
        // a note that has not started by then starts with the value
        let waiting = self.upcoming.iter().any(|u| u.key == Some(key) && u.start >= at);
        if waiting {
            for note in self.upcoming.iter_mut().filter(|u| u.key == Some(key)) {
                for (name, value, _) in &args {
                    note.event.args.insert(name.clone(), *value);
                }
            }
            // it may be prepared with the old value
            self.invalidate_live();
            return;
        }
        for (name, _, value) in args {
            let set = VoiceSet { key, name, value, frames, at, done: false, client, tag: tag.clone() };
            if self.live.is_some() {
                self.fresh.push(realtime::Fresh::Set(set.clone()));
            }
            self.voice_sets.push(set);
        }
    }

    /// Let go of the note a key holds, at sample `at`. A note that is not to start before then is played as a
    /// blip instead, long enough to hear.
    pub(super) fn release_at(&mut self, key: u32, at: u64, owner: Option<(ClientId, Option<String>)>) {
        let stale = self.clock.saturating_sub(self.samples(2.0));
        self.key_offs.retain(|k| k.at > stale);
        let offs = &self.key_offs;
        self.closed.retain(|c| offs.iter().any(|k| k.key == c.key));
        let (client, tag) = owner.map_or((None, None), |(c, t)| (Some(c), t));
        self.key_offs.push(KeyOff { key, at, client, tag });
        let blip = self.samples(0.01);
        let mut waiting = false;
        for note in self.upcoming.iter_mut().filter(|u| u.key == Some(key) && u.start >= at) {
            note.key = None;
            note.dur = blip;
            waiting = true;
        }
        if waiting {
            // the note may be prepared already as a held one
            self.invalidate_live();
        } else {
            if self.live.is_some() {
                self.fresh.push(realtime::Fresh::Release { key, at });
            }
            self.wake_planner();
        }
    }

    /// Take back this client's events that are still waiting, those with `tag` if one is given, at or after
    /// `from`. With `release`, held notes of the same tag that are already sounding are let go of at `from`.
    pub fn cancel_events(&mut self, client: ClientId, tag: Option<&str>, from: At, release: bool) -> Cancelled {
        self.sync_realtime();
        let from = self.resolve(from);
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::CancelEvents { client, tag: tag.map(str::to_owned), from, release },
        });
        let mut done = Cancelled::default();
        let mut replan = false;
        let mine = |owner: Option<ClientId>, t: &Option<String>| owner == Some(client) && tag.is_none_or(|x| t.as_deref() == Some(x));

        // notes still to start
        let mut dropped_keys = Vec::new();
        self.upcoming.retain(|u| {
            let drop = u.start >= from && mine(u.client, &u.tag);
            if drop {
                done.cancelled += 1;
                dropped_keys.extend(u.key);
            }
            !drop
        });
        replan |= done.cancelled > 0;
        self.voices.retain(|v| !dropped_keys.contains(&v.key));

        // changes of a note's parameters, and releases, still to come
        let before = done.cancelled;
        let mut sets = std::mem::take(&mut self.voice_sets);
        sets.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.voice_sets = sets;
        let mut reopened = Vec::new();
        self.key_offs.retain(|k| {
            let drop = k.at >= from && k.at >= self.clock && mine(k.client, &k.tag);
            done.cancelled += usize::from(drop);
            if drop {
                reopened.push(k.key);
            }
            !drop
        });
        // a note whose release was taken back is held again
        for key in reopened {
            if let Some(i) = self.closed.iter().position(|c| c.key == key) {
                let entry = self.closed.remove(i);
                if !self.voices.iter().any(|v| v.client == entry.client && v.id == entry.id) {
                    self.voices.push(entry);
                }
            }
        }
        self.timed_panics.retain(|p| {
            let drop = !p.done && p.at >= from && mine(Some(p.client), &p.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        let mut tracks = std::mem::take(&mut self.track_sets);
        tracks.retain(|s| {
            let drop = !s.done && s.at >= from && mine(s.client, &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.voice_maps.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client),&s.tag);
            done.cancelled += usize::from(drop); !drop
        });
        self.buffer_sets.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.track_sets = tracks;
        replan |= done.cancelled > before;

        // control changes the renderer has not carried out
        let clock = self.clock;
        let detached = self.detached;
        let mut withdrawn = Vec::new();
        let owners = std::mem::take(&mut self.control_owners);
        let (mine_owned, rest): (Vec<_>, Vec<_>) = owners.into_iter().partition(|o| {
            tag.is_none_or(|x| o.tag.as_deref() == Some(x)) && o.client == client
        });
        self.control_owners = rest;
        for owner in mine_owned {
            let Some(change) = self.control_changes.iter().find(|c| c.serial == owner.serial) else {
                continue; // it has happened
            };
            let waiting = if detached { !change.applied } else { change.at > clock };
            if change.at >= from && waiting {
                withdrawn.push(owner.serial);
            } else {
                self.control_owners.push(owner);
            }
        }
        for serial in withdrawn {
            self.control_changes.retain(|c| c.serial != serial);
            if let Some(planner) = &mut self.live {
                let retract = crate::controls::Change {
                    index: 0,
                    unit: niminal_lang::Unit::Num,
                    revision: 0,
                    at: 0,
                    value: 0.0,
                    frames: 0,
                    serial,
                    log_index: 0,
                    log_op: 0,
                    applied: false,
                    retract: true,
                };
                let _ = planner.push_control(retract);
            }
            done.cancelled += 1;
        }

        // changes of definitions that are still to land
        let landing: Vec<u64> = self
            .pending
            .iter()
            .filter(|p| p.lands_at >= from && mine(Some(p.meta.client), &p.meta.tag))
            .map(|p| p.id)
            .collect();
        for id in landing {
            let gone = self.cancel_change(id);
            done.cancelled += usize::from(gone);
            replan |= gone;
        }

        if release {
            let keys: Vec<u32> = self
                .voices
                .iter()
                .filter(|v| v.client == client && tag.is_none_or(|x| v.tag.as_deref() == Some(x)))
                .map(|v| v.key)
                .collect();
            self.voices.retain(|v| !keys.contains(&v.key));
            for key in keys {
                self.release_at(key, from.max(self.clock), None);
                done.released += 1;
            }
        }
        if replan {
            self.invalidate_windows();
        } else {
            self.wake_planner();
        }
        done
    }

    /// A client has gone: its held notes are let go of and its notes still waiting never sound.
    pub fn drop_client(&mut self, client: ClientId) {
        self.cancel_events(client, None, At::Now, true);
        if !self.routing.is_empty(client)
            && let Err(problems) = self.send_events(
                client,
                vec![Op::Routing {
                    at: At::Now,
                    edits: vec![crate::RoutingEdit::Clear],
                    tag: None,
                }],
            )
        {
            self.notify(format!("routing cleanup failed: {}", problems[0].message));
        }
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

fn problem_at(event: usize, message: impl Into<String>) -> Problem {
    let mut p = Problem::plain(message);
    p.event = Some(event);
    p
}
