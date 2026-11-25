//! Validation, definition queueing, and planning of incoming event batches.

use super::super::*;
use super::{Plan, in_seconds, problem_at};

impl Session {
    pub(super) fn validate_ops(&self, ops: &[Op]) -> Result<(), Vec<Problem>> {
        let mut invalid = Vec::new();
        for (index, op) in ops.iter().enumerate() {
            let (duration, args) = match op {
                Op::Note { dur, args, .. } => (*dur, Some(args)),
                Op::Set { glide, args, .. } => (*glide, Some(args)),
                Op::Control { glide, unit, .. } => {
                    if unit.as_deref().is_some_and(|u| u == "bpm") {
                        invalid.push(problem_at(index, "`bpm` is not a unit a control takes"));
                    }
                    (*glide, None)
                }
                Op::Array { glide, values, .. } => {
                    if values
                        .iter()
                        .any(|v| matches!(v, Value::Beats(_) | Value::Bars(_)))
                    {
                        invalid.push(problem_at(index, "resolve musical units in the client"));
                    }
                    (*glide, None)
                }
                _ => (None, None),
            };
            if duration.is_some_and(|t| !matches!(t, Time::Seconds(n) if n.is_finite() && n >= 0.0))
                || args.is_some_and(|a| {
                    a.values()
                        .any(|v| matches!(v, Value::Beats(_) | Value::Bars(_)))
                })
            {
                invalid.push(problem_at(
                    index,
                    "durations must be finite, nonnegative seconds; resolve musical time in the client",
                ));
            }
        }
        if !invalid.is_empty() {
            return Err(invalid);
        }
        if self.activities.len() + ops.iter().filter(|o| matches!(o, Op::Note { .. })).count()
            > 4096
        {
            return Err(vec![Problem::plain(
                "note receipt capacity reached; drain voice updates",
            )]);
        }
        Ok(())
    }

    pub(super) fn queue_definitions(
        &mut self,
        client: ClientId,
        ops: &[Op],
        clock_state: &niminal_lang::ClockState,
        linked: &mut HashMap<String, Option<Link>>,
    ) -> Result<Vec<(crate::events::TransactionReceipt, Arc<AtomicU64>)>, Vec<Problem>> {
        let mut early = Vec::new();
        let mut accepted = Vec::new();
        for (n, op) in ops.iter().enumerate() {
            let (source, dir, at, what, edits, tag) = match op {
                Op::Routing { edits, .. } if crate::routing::Routing::is_control_edit(edits) => {
                    continue;
                }
                Op::Routing { at, edits, tag } => (
                    String::new(),
                    None,
                    self.resolve(*at),
                    What::Definitions,
                    in_seconds(edits, client, clock_state, linked),
                    tag.clone(),
                ),
                Op::Define {
                    source,
                    at,
                    dir,
                    tag,
                } => {
                    if source.trim().is_empty() {
                        early.push(problem_at(n, "there is nothing to define"));
                        break;
                    }

                    (
                        source.clone(),
                        dir.as_deref(),
                        self.resolve(*at),
                        What::Definitions,
                        Vec::new(),
                        tag.clone(),
                    )
                }
                Op::Forget { at, tag } => (
                    "new".to_string(),
                    None,
                    self.resolve(*at),
                    What::Forget,
                    Vec::new(),
                    tag.clone(),
                ),
                Op::Transaction {
                    source,
                    dir,
                    at,
                    edits,
                    complete,
                    tag,
                } => (
                    source.clone(),
                    dir.as_deref(),
                    self.resolve(*at),
                    if *complete {
                        What::Complete
                    } else {
                        What::Definitions
                    },
                    in_seconds(edits, client, clock_state, linked),
                    tag.clone(),
                ),
                _ => continue,
            };
            if let Some(dir) = dir {
                self.set_client_sample_dir(client, std::path::Path::new(dir));
            }
            match self.queue_transaction(&source, dir, at, what, edits, Meta { client, tag }) {
                Ok(id) => {
                    let pending = self
                        .pending
                        .iter()
                        .find(|p| p.id == id)
                        .expect("a queued change is waiting to land");
                    accepted.push((
                        crate::events::TransactionReceipt {
                            id,
                            at: pending.lands_at,
                        },
                        pending.applied.clone(),
                    ));
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
        Ok(accepted)
    }

    pub(super) fn plan_ops(
        &self,
        client: ClientId,
        ops: &mut [Op],
        sent_now: &[bool],
        clock: u64,
        clock_state: &mut niminal_lang::ClockState,
        linked: &mut HashMap<String, Option<Link>>,
    ) -> Result<Vec<Plan>, Vec<Problem>> {
        let rate = f64::from(self.sample_rate);
        // the notes this client holds, as the batch goes on opening and releasing them
        let mut open: Vec<(u64, Arc<niminal_lang::Instrument>)> = self
            .voices
            .iter()
            .filter(|v| v.client == client)
            .map(|v| (v.id, v.schema.clone()))
            .collect();
        let mut plans = Vec::new();
        let mut problems = Vec::new();
        let mut controls = self.control_changes.len();
        for (n, op) in ops.iter_mut().enumerate() {
            let mut fail = |message: String| problems.push(problem_at(n, message));
            if let Op::Array {
                id,
                at,
                param,
                values,
                glide,
                tag,
            } = op
            {
                let Some((_, schema)) = open.iter().find(|(open, _)| open == id) else {
                    fail(format!("no note with id {id} is sounding"));
                    continue;
                };
                let Some(paths) = schema.arrays.get(param) else {
                    fail(format!("`{param}` is not a live array parameter"));
                    continue;
                };
                if paths.len() != values.len() {
                    fail(format!("array `{param}` needs {} values", paths.len()));
                    continue;
                }
                *op = Op::Set {
                    id: *id,
                    at: *at,
                    args: paths.iter().cloned().zip(std::mem::take(values)).collect(),
                    glide: *glide,
                    tag: tag.take(),
                };
            }
            let op = &*op;
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
                    } else if self
                        .routing_at(start)
                        .has_node(client, definitions::local(client, target).unwrap_or(target))
                    {
                        crate::routing::node_name(
                            client,
                            definitions::local(client, target).unwrap_or(target),
                        )
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
                                open.push((
                                    *id,
                                    Arc::new(program.instruments[plan.instrument].clone()),
                                ));
                            }
                            plans.push(Plan::Note {
                                op: n,
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
                    let now = sent_now.get(n).copied().unwrap_or(false);
                    let at = self.resolve(*at);

                    // a control the definitions waiting to land make is set once they have
                    let at = if self.program_at(at).controls.iter().any(|c| c.name == *name) {
                        at
                    } else {
                        self.pending
                            .iter()
                            .find(|p| {
                                p.lands_at > at
                                    && p.program.controls.iter().any(|c| c.name == *name)
                            })
                            .map_or(at, |p| p.lands_at)
                    };
                    let program = self.program_at(at);
                    let Some(def) = program.controls.iter().find(|c| c.name == *name) else {
                        fail(format!("unknown control `{name}`"));
                        continue;
                    };
                    // a length in beats or bars is kept to them: it follows the tempo from here on
                    let musical = unit
                        .as_deref()
                        .is_some_and(|u| matches!(u, "beat" | "beats" | "bar" | "bars"));
                    let tempo = niminal_score::Tempo {
                        bpm: clock_state.bpm,
                        beats_per_bar: clock_state.beats_per_bar,
                        ..program.tempo
                    };
                    let converted = match def.convert(*value, unit.as_deref(), tempo) {
                        Ok(v) => v,
                        Err(e) => {
                            fail(e);
                            continue;
                        }
                    };
                    linked.insert(
                        name.clone(),
                        musical.then(|| {
                            let per_unit = if unit.as_deref().is_some_and(|u| u.starts_with("bar"))
                            {
                                clock_state.beats_per_bar
                            } else {
                                1.0
                            };
                            Link {
                                client,
                                beats: value * per_unit,
                                node: false,
                            }
                        }),
                    );
                    let seconds = glide.map_or(def.smooth_seconds, |g| g.to_seconds(program.tempo));
                    if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                        fail("`glide` must be between 0 and 60sec".into());
                        continue;
                    }
                    controls += 1;
                    plans.push(Plan::Control {
                        name: name.clone(),
                        value: converted,
                        frames: (seconds * rate).round() as u64,
                        at,
                        op: n,
                        tag: tag.clone(),
                        now,
                    });
                }
                Op::Tempo {
                    at,
                    bpm,
                    meter,
                    beat,
                    tag,
                } => {
                    let valid = |v: f64| v.is_finite() && v > 0.0;
                    if !valid(*bpm)
                        || meter.is_some_and(|m| !valid(m))
                        || beat.is_some_and(|b| !b.is_finite())
                    {
                        fail("`bpm` and `meter` must be positive numbers".into());
                        continue;
                    }
                    let start = self.resolve(*at);

                    let old = *clock_state;
                    let new = old.changed(
                        start as f64 / f64::from(self.sample_rate),
                        *bpm,
                        meter.unwrap_or(old.beats_per_bar),
                        *beat,
                    );
                    *clock_state = new;
                    let program = self.program_at(start);
                    for (name, value, unit) in [
                        (niminal_lang::CLOCK_BPM, new.bpm, None),
                        (niminal_lang::CLOCK_ORIGIN, new.origin, Some("sec")),
                        (niminal_lang::CLOCK_METER, new.beats_per_bar, None),
                    ] {
                        let name = definitions::scoped(client, name);
                        let Some(def) = program.controls.iter().find(|c| c.name == name) else {
                            continue;
                        };
                        match def.convert(value, unit, program.tempo) {
                            Ok(value) => {
                                controls += 1;
                                plans.push(Plan::Control {
                                    name,
                                    value,
                                    frames: 0,
                                    at: start,
                                    op: n,
                                    tag: tag.clone(),
                                    now: false,
                                });
                            }
                            Err(e) => fail(e),
                        }
                    }
                    // what is kept to a number of beats is as many of them at the new tempo
                    let mut kept: HashMap<&String, Link> = self
                        .linked
                        .iter()
                        .filter(|(_, link)| link.client == client)
                        .map(|(name, link)| (name, *link))
                        .collect();
                    for (name, link) in linked.iter() {
                        match link {
                            Some(link) => kept.insert(name, *link),
                            None => kept.remove(name),
                        };
                    }
                    let tempo = niminal_score::Tempo {
                        bpm: new.bpm,
                        beats_per_bar: new.beats_per_bar,
                        ..program.tempo
                    };
                    let mut kept: Vec<_> = kept.into_iter().collect();
                    kept.sort_by(|a, b| a.0.cmp(b.0));
                    for (name, link) in kept {
                        let Some(def) = program.controls.iter().find(|c| {
                            if link.node {
                                c.key == *name
                            } else {
                                c.name == *name
                            }
                        }) else {
                            continue;
                        };
                        let Ok(value) = def.convert(link.beats, Some("beats"), tempo) else {
                            continue;
                        };
                        let frames = (def.smooth_seconds * rate).round() as u64;
                        if link.node {
                            plans.push(Plan::Node {
                                key: name.clone(),
                                value,
                                frames,
                                at: start,
                                tag: tag.clone(),
                            });
                        } else {
                            controls += 1;
                            plans.push(Plan::Control {
                                name: name.clone(),
                                value,
                                frames,
                                at: start,
                                op: n,
                                tag: tag.clone(),
                                now: false,
                            });
                        }
                    }
                }
                Op::Routing { at, edits, tag } => {
                    let start = self.resolve(*at);
                    for edit in edits {
                        if let crate::RoutingEdit::BindBuffer { id, param, buffer } = edit {
                            let result = (|| -> Result<_, String> {
                                let routing = self.routing_at(start);
                                let program = self.program_at(start);
                                let (resource, count, width) =
                                    routing.buffer_binding(client, id, param)?;
                                let (_, data) = program
                                    .buffers
                                    .iter()
                                    .find(|(n, _)| n == buffer && !self.buffer_is_hidden(n))
                                    .ok_or("no buffer")?;
                                if data.channels() < width {
                                    return Err("buffer has too few channels for the fx".into());
                                }
                                Ok(Plan::NodeBind {
                                    id: id.clone(),
                                    param: param.clone(),
                                    resource,
                                    count,
                                    bank: Arc::new(vec![data.clone()]),
                                    at: start,
                                    tag: tag.clone(),
                                })
                            })();
                            match result {
                                Ok(plan) => plans.push(plan),
                                Err(error) => fail(error),
                            }
                        }
                        if let crate::RoutingEdit::Map {
                            id,
                            param,
                            source,
                            channel,
                            scale,
                            offset,
                        } = edit
                        {
                            let result = (|| -> Result<ChainMap, String> {
                                let routing = self.routing_at(start);
                                let program = self.program_at(start);
                                let key = match routing.parameter(client, id, param) {
                                    Ok(key) => key,
                                    Err(_) if source.is_none() => {
                                        self.routing.parameter(client, id, param)?
                                    }
                                    Err(error) => return Err(error),
                                };
                                let name = source
                                    .as_ref()
                                    .map(|s| routing.source(client, s))
                                    .transpose()?;
                                super::super::parameters::validate_mapping(
                                    program,
                                    name.as_deref(),
                                    *channel,
                                    *scale,
                                    *offset,
                                )?;
                                Ok(ChainMap {
                                    client,
                                    node: id.clone(),
                                    control_key: key,
                                    parameter_name: param.clone(),
                                    source: name,
                                    channel: channel - 1,
                                    scale: *scale,
                                    offset: *offset,
                                    at: start,
                                    tag: tag.clone(),
                                    done: false,
                                })
                            })();
                            match result {
                                Ok(map) => plans.push(Plan::ChainMap(map)),
                                Err(error) => fail(error),
                            }
                        }
                    }
                    if crate::routing::Routing::is_control_edit(edits) {
                        // an effect that the changes waiting to land make is set once they have
                        let ids: Vec<&String> = edits
                            .iter()
                            .filter_map(|e| match e {
                                crate::RoutingEdit::Set { id, .. } => Some(id),
                                _ => None,
                            })
                            .collect();
                        let start = if ids
                            .iter()
                            .all(|id| self.routing_at(start).has_node(client, id))
                        {
                            start
                        } else {
                            self.pending
                                .iter()
                                .find(|p| {
                                    p.lands_at > start
                                        && ids.iter().all(|id| p.routing.has_node(client, id))
                                })
                                .map_or(start, |p| p.lands_at)
                        };
                        // a length in beats or bars is kept to them, as it is for a control
                        let edits = in_seconds(edits, client, clock_state, linked);
                        match self
                            .routing_at(start)
                            .updates(client, &edits, self.program_at(start))
                        {
                            Ok(updates) => {
                                for (key, value, glide) in updates {
                                    plans.push(Plan::Node {
                                        key,
                                        value,
                                        frames: (glide * rate).round() as u64,
                                        at: start,
                                        tag: tag.clone(),
                                    });
                                }
                            }
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
                    let Some((_, schema)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
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
                    if let Err(error) = super::super::parameters::validate_mapping(
                        program,
                        source.as_deref(),
                        *channel,
                        *scale,
                        *offset,
                    ) {
                        fail(error);
                        continue;
                    }
                    if self.voice_maps.len() + plans.len() >= 4096 {
                        fail("the modulation mapping queue is full".into());
                        continue;
                    }

                    plans.push(Plan::Map {
                        id: *id,
                        start,
                        param: param.clone(),
                        source: source.clone(),
                        channel: *channel,
                        scale: *scale,
                        offset: *offset,
                        tag: tag.clone(),
                    });
                }
                Op::Set {
                    id,
                    at,
                    args,
                    glide,
                    tag,
                } => {
                    let at = self.resolve(*at);

                    let Some((_, instrument)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    let program = self.program_at(at);
                    let seconds = glide.map_or(0.0, |g| g.to_seconds(program.tempo));
                    if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                        fail("`glide` must be between 0 and 60sec".into());
                        continue;
                    }
                    let mut converted = Vec::new();
                    for (name, value) in args {
                        // a note's pitch is `freq` to the score, and whatever `pitch = ...` named it to the instrument
                        let actual = if name == niminal_lang::PITCH_PARAM {
                            &instrument.pitch
                        } else {
                            name
                        };
                        match instrument
                            .param(actual)
                            .and_then(|(_, p)| instrument.param_value(p, *value, program.tempo))
                        {
                            Ok(v) => converted.push((actual.clone(), *value, v)),
                            Err(e) => fail(e.to_string()),
                        }
                    }
                    plans.push(Plan::Set {
                        id: *id,
                        args: converted,
                        frames: (seconds * rate).round() as u64,
                        at,
                        tag: tag.clone(),
                    });
                }
                Op::Off { id, at, tag } => {
                    let at = self.resolve(*at);

                    let Some(i) = open.iter().position(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    open.remove(i);
                    plans.push(Plan::Off {
                        id: *id,
                        at,
                        tag: tag.clone(),
                    });
                }
                Op::BufferWrite {
                    name,
                    at,
                    offset,
                    values,
                    tag,
                } => {
                    let start = self.resolve(*at);

                    let program = self.program_at(start);
                    let Some((_, buffer)) = program.buffers.iter().find(|(n, _)| n == name) else {
                        fail(format!("no buffer `{name}`"));
                        continue;
                    };
                    if *offset > buffer.len()
                        || values.len() > (buffer.len() - offset) * buffer.channels()
                        || values.len() % buffer.channels() != 0
                    {
                        fail("buffer write is outside its length".into());
                        continue;
                    }
                    if values.len() > 65536 || values.iter().any(|v| !v.is_finite()) {
                        fail("a buffer write has at most 65536 finite samples".into());
                        continue;
                    }
                    if self.buffer_sets.len() + plans.len() >= 4096 {
                        fail("the buffer write queue is full".into());
                        continue;
                    }
                    plans.push(Plan::Buffer {
                        start,
                        set: BufferSet {
                            name: name.clone(),
                            offset: *offset,
                            values: values.clone().into(),
                            at: start.max(clock),
                            client,
                            tag: tag.clone(),
                            done: false,
                        },
                    });
                }
                Op::Panic { at, tag } => {
                    let start = self.resolve(*at);

                    plans.push(Plan::Panic {
                        at: start,
                        tag: tag.clone(),
                    });
                }
                Op::BindBuffer {
                    id,
                    at,
                    param,
                    buffer,
                    tag,
                } => {
                    let at = self.resolve(*at);
                    let program = self.program_at(at);
                    let Some((_, schema)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id}"));
                        continue;
                    };
                    let Some(&resource) = schema.resource_keys.get(param) else {
                        fail(format!("`{param}` is not a buffer parameter"));
                        continue;
                    };
                    let Some((_, data)) = program
                        .buffers
                        .iter()
                        .find(|(n, _)| n == buffer && !self.buffer_is_hidden(n))
                    else {
                        fail(format!("no buffer `{buffer}`"));
                        continue;
                    };
                    if data.channels() < schema.graph.buffer_channels(resource) {
                        fail("buffer has too few channels for this voice".into());
                        continue;
                    }
                    plans.push(Plan::Bind {
                        id: *id,
                        param: param.clone(),
                        resource,
                        bank: Arc::new(vec![data.clone()]),
                        count: schema.graph.buffer_bindings(resource),
                        at,
                        tag: tag.clone(),
                    });
                }
                Op::Array { .. } => unreachable!("parameter events expanded"),
                Op::Define { .. } | Op::Forget { .. } | Op::Transaction { .. } => {}
            }
        }
        if problems.is_empty() && controls > crate::controls::CAPACITY {
            problems.push(Problem::plain("the control update queue is full"));
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(plans)
    }
}
