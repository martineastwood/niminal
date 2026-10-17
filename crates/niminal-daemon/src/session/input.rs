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
        musical: Option<bool>,
    ) {
        if self.live.is_some() {
            self.fresh.push(realtime::Fresh::Note { event: event.clone(), start, dur, key, musical: musical.is_some() });
        }
        let (client, tag) = owner.map_or((None, None), |(c, t)| (Some(c), t));
        self.insert_upcoming(Upcoming { start, dur, event, key, client, tag, musical });
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
        let revision = match self.pending.iter().rev().find(|p| p.lands_at <= at && p.program.is_some()) {
            Some(p) if p.statements.iter().any(|s| s.key().and_then(|k| k.strip_prefix("ctl:")) == Some(name)) => p.id,
            _ => self.controls.iter().find(|c| c.key == def.key).map_or(0, |c| c.revision),
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

    /// A time as the log keeps it: a sample, except a position in beats or bars, which is kept so that
    /// it moves with the tempo again when the log is played back.
    fn logged(&self, at: At) -> At {
        match at {
            At::Time(Time::Beats(_) | Time::Bars(_)) => at,
            _ => At::Sample(self.resolve(at)),
        }
    }

    /// The sample a time stands for: a sample as it is, `now` as the clock, and a position on the
    /// transport counted from the bar origin.
    fn resolve(&self, at: At) -> u64 {
        match at {
            At::Sample(n) => n,
            At::Now => self.clock,
            At::Time(t) => {
                let grid = self.grid();
                let seconds = t.to_seconds(self.program.tempo);
                (grid.origin as f64 + seconds * f64::from(self.sample_rate)).round().max(0.0) as u64
            }
        }
    }

    /// Do what a batch of events from `client` says. All of it is checked first: if any is wrong none of it is
    /// done. Times come out as samples in the log, so that replaying it gives the same sound.
    pub fn send_events(&mut self, client: ClientId, ops: Vec<Op>) -> Result<Sent, Vec<Problem>> {
        self.sync_realtime();
        let rate = f64::from(self.sample_rate);
        let clock = self.clock;
        let musical = |at: &At| match at {
            At::Time(Time::Beats(_)) => Some(false),
            At::Time(Time::Bars(_)) => Some(true),
            _ => None,
        };

        // Changes of definitions and of the transport go first: they are compiled, which can fail, and the
        // rest of the batch is checked against the program they leave. If anything later is wrong they are
        // taken back.
        let mut resolved: Vec<Option<Op>> = vec![None; ops.len()];
        let mut queued: Vec<u64> = Vec::new();
        let mut early = Vec::new();
        for (n, op) in ops.iter().enumerate() {
            let (source, dir, at, what, meta) = match op {
                Op::Define { source, at, dir, tag } => {
                    let meta = Meta { client: Some(client), tag: tag.clone(), musical: musical(at), origin: false };
                    resolved[n] = Some(Op::Define { source: source.clone(), at: self.logged(*at), dir: dir.clone(), tag: tag.clone() });
                    (source.clone(), dir.as_deref(), self.resolve(*at), What::Definitions, meta)
                }
                Op::Transport { at, bpm, meter, origin, tag } => {
                    let mut lines = Vec::new();
                    if let Some(bpm) = bpm {
                        lines.push(format!("tempo {bpm}bpm"));
                    }
                    if let Some(meter) = meter {
                        lines.push(format!("meter {meter}"));
                    }
                    let is_origin = origin.unwrap_or(false);
                    if lines.is_empty() && !is_origin {
                        early.push(problem_at(n, "a `transport` needs a `bpm`, a `meter` or `origin`"));
                        continue;
                    }
                    let meta = Meta { client: Some(client), tag: tag.clone(), musical: musical(at), origin: is_origin };
                    resolved[n] = Some(Op::Transport { at: self.logged(*at), bpm: *bpm, meter: meter.clone(), origin: *origin, tag: tag.clone() });
                    (lines.join("\n"), None, self.resolve(*at), What::Timing, meta)
                }
                Op::Forget { at, tag } => {
                    let meta = Meta { client: Some(client), tag: tag.clone(), musical: musical(at), origin: false };
                    resolved[n] = Some(Op::Forget { at: self.logged(*at), tag: tag.clone() });
                    ("new".to_string(), None, self.resolve(*at), What::Forget, meta)
                }
                _ => continue,
            };
            if let Some(dir) = dir {
                self.set_sample_dir(std::path::Path::new(dir));
            }
            let result = if source.is_empty() {
                // only a point that is bar 1
                self.queue_origin(at, meta)
            } else {
                self.queue_change(&source, dir, at, what, meta)
            };
            match result {
                Ok(id) => queued.push(id),
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
            self.take_back(&queued);
            return Err(early);
        }

        enum Plan {
            Note { event: Event, instrument: String, start: u64, dur: u64, id: Option<u64>, tag: Option<String>, musical: Option<bool> },
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
                Op::Note { target, at, args, dur, id, tag } => {
                    let start = self.resolve(*at);
                    resolved[n] = Some(Op::Note { target: target.clone(), at: self.logged(*at), args: args.clone(), dur: *dur, id: *id, tag: tag.clone() });
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
                    let event = Event { target: target.clone(), at: Time::Seconds(0.0), dur: dur.unwrap_or(Time::Seconds(0.0)), args: args.clone() };
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
                                musical: musical(at),
                            });
                        }
                        Err(e) => fail(format!("note for `{target}`: {e}")),
                    }
                }
                Op::Control { name, at, value, unit, glide, tag } => {
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
                    resolved[n] = Some(Op::Track { name: name.clone(), at: self.logged(*raw), args: args.clone(), glide: *glide, tag: tag.clone() });
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
                Op::Set { id, at, args, glide, tag } => {
                    let raw = at;
                    let at = self.resolve(*at);
                    resolved[n] = Some(Op::Set { id: *id, at: self.logged(*raw), args: args.clone(), glide: *glide, tag: tag.clone() });
                    let Some((_, instrument)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    let program = self.program_at(at);
                    let Some(instrument) = program.instruments.iter().find(|i| i.name == *instrument) else {
                        fail(format!("the instrument of note {id} is not defined any more"));
                        continue;
                    };
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
                Op::Panic { at, tag } => {
                    let start = self.resolve(*at);
                    resolved[n] = Some(Op::Panic { at: self.logged(*at), tag: tag.clone() });
                    plans.push(Plan::Panic { at: start, tag: tag.clone() });
                }
                Op::Define { .. } | Op::Transport { .. } | Op::Forget { .. } => {}
            }
        }
        if problems.is_empty() && controls > crate::controls::CAPACITY {
            problems.push(Problem::plain("the control update queue is full"));
        }
        if !problems.is_empty() {
            self.take_back(&queued);
            return Err(problems);
        }

        let log_index = self.log.len();
        self.log.push(LogEntry {
            sample: clock,
            input: LogInput::Send { client, ops: resolved.into_iter().flatten().collect() },
        });
        let mut sent = Sent { accepted: ops.len(), late: 0, first: None };
        // definitions and the transport, which are queued already
        for op in &ops {
            let at = match op {
                Op::Define { at, .. } | Op::Transport { at, .. } | Op::Forget { at, .. } => self.resolve(*at),
                _ => continue,
            };
            sent.late += usize::from(at < clock);
            sent.first = Some(sent.first.map_or(at, |f| f.min(at)));
        }
        for plan in plans {
            let at = match plan {
                Plan::Note { event, instrument, start, dur, id, tag, musical } => {
                    let key = id.map(|_| self.new_key());
                    if let (Some(id), Some(key)) = (id, key) {
                        self.voices.push(VoiceEntry { client, id, key, instrument, tag: tag.clone() });
                    }
                    self.add_explicit(start.max(clock), dur, event, key, Some((client, tag)), musical);
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
        self.land_due();
        if self.pending.len() != waiting {
            self.invalidate_live();
        } else if !queued.is_empty() {
            self.invalidate_windows();
        }
        self.wake_planner();
        Ok(sent)
    }

    /// Take back changes of definitions or the transport queued by a batch that then failed.
    fn take_back(&mut self, ids: &[u64]) {
        for &id in ids {
            self.cancel_change(id);
        }
    }

    /// Change parameters of the note a client holds by `id`: now, if it is sounding, or as it starts.
    fn set_voice(&mut self, client: ClientId, id: u64, args: Vec<(String, Value, f32)>, frames: u64, at: u64, tag: Option<String>) {
        let Some(entry) = self.voices.iter().find(|v| v.client == client && v.id == id) else {
            return;
        };
        let (key, instrument) = (entry.key, entry.instrument.clone());
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
            let set = VoiceSet { key, instrument: instrument.clone(), name, value, frames, at, done: false, client, tag: tag.clone() };
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

        // changes of definitions or the transport that are still to land
        let landing: Vec<u64> = self
            .pending
            .iter()
            .filter(|p| p.lands_at >= from && mine(p.meta.client, &p.meta.tag))
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
        self.sync_realtime();
        let held: Vec<u32> = self.voices.iter().filter(|v| v.client == client).map(|v| v.key).collect();
        self.voices.retain(|v| v.client != client);
        for key in held {
            self.release_at(key, self.clock, None);
        }
        let before = self.upcoming.len() + self.track_sets.len();
        let clock = self.clock;
        self.upcoming.retain(|u| u.client != Some(client) || u.start < clock || u.key.is_some());
        self.track_sets.retain(|s| s.client != Some(client) || s.at < clock);
        if self.upcoming.len() + self.track_sets.len() != before {
            self.invalidate_windows();
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
