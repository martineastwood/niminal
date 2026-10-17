//! Things that arrive from outside while the session runs: events, control changes and keys.

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
        self.insert_upcoming(Upcoming { start, dur, event, key, scheduled: false, client, tag });
    }

    /// Queue a typed control update at an absolute sample, or at the render
    /// clock when no timestamp is given. No parsing or graph compilation occurs.
    pub fn set_control(
        &mut self,
        name: &str,
        value: f64,
        unit: Option<&str>,
        at: Option<u64>,
        smooth_ms: Option<f64>,
    ) -> Result<u64, Vec<Problem>> {
        self.sync_realtime();
        let at = at.unwrap_or(self.clock);
        if at < self.clock {
            return Err(vec![Problem::plain("a control timestamp cannot be in the past")]);
        }
        if let Some(ms) = smooth_ms
            && (!ms.is_finite() || !(0.0..=60_000.0).contains(&ms))
        {
            return Err(vec![Problem::plain("control smoothing must be between 0ms and 60sec")]);
        }
        self.send_events(
            0,
            vec![Op::Control {
                name: name.into(),
                at: At::Sample(at),
                value,
                unit: unit.map(str::to_owned),
                glide: smooth_ms.map(|ms| Time::Seconds(ms / 1000.0)),
                tag: None,
            }],
        )?;
        Ok(at)
    }

    /// Move control `index` to `value` (in the engine's unit) at sample `at`, over `frames`. `log` says which
    /// event of which log entry it is, so that the sample it really happens at can be written back.
    fn queue_control(
        &mut self,
        index: usize,
        value: f32,
        frames: u64,
        at: u64,
        log: (usize, usize),
    ) -> Result<(), &'static str> {
        if self.control_changes.len() >= crate::controls::CAPACITY {
            return Err("the control update queue is full");
        }
        let def = &self.program.controls[index];
        let change = crate::controls::Change {
            index,
            unit: def.unit,
            revision: self.controls[index].revision,
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
        enum Plan {
            Note { event: Event, start: u64, dur: u64, id: Option<u64>, tag: Option<String> },
            Control { index: usize, value: f32, frames: u64, at: u64, op: usize, tag: Option<String> },
            Set { id: u64, args: Vec<(String, Value, f32)>, frames: u64, at: u64, tag: Option<String> },
            Off { id: u64, at: u64, tag: Option<String> },
        }
        let tempo = self.program.tempo;
        let rate = f64::from(self.sample_rate);
        // the notes this client holds, as the batch goes on opening and releasing them
        let mut open: Vec<(u64, String)> = self
            .voices
            .iter()
            .filter(|v| v.client == client)
            .map(|v| (v.id, v.instrument.clone()))
            .collect();
        let mut plans = Vec::new();
        let mut problems = Vec::new();
        let mut resolved = Vec::new();
        let mut controls = self.control_changes.len();
        for (n, op) in ops.iter().enumerate() {
            let mut fail = |message: String| {
                let mut p = Problem::plain(message);
                p.event = Some(n);
                problems.push(p);
            };
            match op {
                Op::Note { target, at, args, dur, id, tag } => {
                    let start = self.resolve(*at);
                    resolved.push(Op::Note { target: target.clone(), at: At::Sample(start), args: args.clone(), dur: *dur, id: *id, tag: tag.clone() });
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
                    let event = Event { target: target.clone(), at: Time::Seconds(0.0), dur: dur.unwrap_or(Time::Seconds(0.0)), args: args.clone() };
                    match self.program.plan(&event) {
                        Ok(plan) => {
                            if let Some(id) = id {
                                open.push((*id, self.program.instruments[plan.instrument].name.clone()));
                            }
                            plans.push(Plan::Note {
                                start,
                                dur: dur.map_or(0, |d| self.samples(d.to_seconds(tempo))),
                                event,
                                id: *id,
                                tag: tag.clone(),
                            });
                        }
                        Err(e) => fail(format!("note for `{target}`: {e}")),
                    }
                }
                Op::Control { name, at, value, unit, glide, tag } => {
                    let at = self.resolve(*at);
                    resolved.push(Op::Control { name: name.clone(), at: At::Sample(at), value: *value, unit: unit.clone(), glide: *glide, tag: tag.clone() });
                    let Some(index) = self.program.controls.iter().position(|c| c.name == *name) else {
                        fail(format!("unknown control `{name}`"));
                        continue;
                    };
                    let def = &self.program.controls[index];
                    let converted = match def.convert(*value, unit.as_deref(), tempo) {
                        Ok(v) => v,
                        Err(e) => {
                            fail(e);
                            continue;
                        }
                    };
                    let seconds = glide.map_or(def.smooth_seconds, |g| g.to_seconds(tempo));
                    if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                        fail("`glide` must be between 0 and 60sec".into());
                        continue;
                    }
                    controls += 1;
                    plans.push(Plan::Control { index, value: converted, frames: (seconds * rate).round() as u64, at, op: n, tag: tag.clone() });
                }
                Op::Track { name, at, args, tag } => {
                    let at = self.resolve(*at);
                    resolved.push(Op::Track { name: name.clone(), at: At::Sample(at), args: args.clone(), tag: tag.clone() });
                    for (param, value) in args {
                        let full = format!("{name}::{param}");
                        let Some(index) = self.program.controls.iter().position(|c| c.name == full) else {
                            fail(format!("`{name}` has no parameter `{param}`"));
                            continue;
                        };
                        let def = &self.program.controls[index];
                        match def.from_value(*value, tempo) {
                            Ok(v) => {
                                controls += 1;
                                plans.push(Plan::Control { index, value: v, frames: (def.smooth_seconds * rate).round() as u64, at, op: n, tag: tag.clone() });
                            }
                            Err(e) => fail(format!("parameter `{param}`: {e}")),
                        }
                    }
                }
                Op::Set { id, at, args, glide, tag } => {
                    let at = self.resolve(*at);
                    resolved.push(Op::Set { id: *id, at: At::Sample(at), args: args.clone(), glide: *glide, tag: tag.clone() });
                    let Some((_, instrument)) = open.iter().find(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    let Some(instrument) = self.program.instruments.iter().find(|i| i.name == *instrument) else {
                        fail(format!("the instrument of note {id} is not defined any more"));
                        continue;
                    };
                    let seconds = glide.map_or(0.0, |g| g.to_seconds(tempo));
                    if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                        fail("`glide` must be between 0 and 60sec".into());
                        continue;
                    }
                    let mut converted = Vec::new();
                    for (name, value) in args {
                        // a note's pitch is `freq` to the score, and whatever `with(pitch: ...)` named it to the instrument
                        let actual = if name == niminal_lang::PITCH_PARAM { &instrument.pitch } else { name };
                        match instrument.param(actual).and_then(|(_, p)| instrument.param_value(p, *value, tempo)) {
                            Ok(v) => converted.push((actual.clone(), *value, v)),
                            Err(e) => fail(e.to_string()),
                        }
                    }
                    plans.push(Plan::Set { id: *id, args: converted, frames: (seconds * rate).round() as u64, at, tag: tag.clone() });
                }
                Op::Off { id, at, tag } => {
                    let at = self.resolve(*at);
                    resolved.push(Op::Off { id: *id, at: At::Sample(at), tag: tag.clone() });
                    let Some(i) = open.iter().position(|(open, _)| open == id) else {
                        fail(format!("no note with id {id} is sounding"));
                        continue;
                    };
                    open.remove(i);
                    plans.push(Plan::Off { id: *id, at, tag: tag.clone() });
                }
            }
        }
        if problems.is_empty() && controls > crate::controls::CAPACITY {
            problems.push(Problem::plain("the control update queue is full"));
        }
        if !problems.is_empty() {
            return Err(problems);
        }

        let clock = self.clock;
        let log_index = self.log.len();
        self.log.push(LogEntry { sample: clock, input: LogInput::Send { client, ops: resolved } });
        let mut sent = Sent { accepted: ops.len(), late: 0, first: None };
        for plan in plans {
            let at = match plan {
                Plan::Note { event, start, dur, id, tag } => {
                    let key = id.map(|_| self.keyboard.new_key());
                    if let (Some(id), Some(key)) = (id, key) {
                        let instrument = self.program.plan(&event).map(|p| self.program.instruments[p.instrument].name.clone()).unwrap_or_default();
                        self.voices.push(VoiceEntry { client, id, key, instrument, tag: tag.clone() });
                    }
                    self.add_explicit(start.max(clock), dur, event, key, Some((client, tag)));
                    start
                }
                Plan::Control { index, value, frames, at, op, tag } => {
                    let serial = self.next_control;
                    match self.queue_control(index, value, frames, at.max(clock), (log_index, op)) {
                        Ok(()) => self.control_owners.push(ControlOwner { serial, client, tag }),
                        Err(e) => self.notify(e.into()),
                    }
                    at
                }
                Plan::Set { id, args, frames, at, tag } => {
                    self.set_voice(client, id, args, frames, at.max(clock), tag);
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
        self.wake_planner();
        Ok(sent)
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
        let mut done = Cancelled::default();
        let mut replan = false;
        let mine = |owner: Option<ClientId>, t: &Option<String>| owner == Some(client) && tag.is_none_or(|x| t.as_deref() == Some(x));

        // notes still to start
        let mut dropped_keys = Vec::new();
        self.upcoming.retain(|u| {
            let drop = !u.scheduled && u.start >= from && mine(u.client, &u.tag);
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
        let before = self.upcoming.len();
        let clock = self.clock;
        self.upcoming.retain(|u| u.client != Some(client) || u.start < clock || u.key.is_some());
        if self.upcoming.len() != before {
            self.invalidate_windows();
        }
    }

    /// A message from a MIDI device or OSC sender. Every control bound to it moves,
    /// exactly as if `control.set` had been called, so it is logged and replays.
    /// Returns how many controls took it.
    pub fn control_input(&mut self, signal: &ControlSignal) -> usize {
        match signal {
            ControlSignal::MidiNote {
                note,
                channel,
                velocity,
            } => return self.key_input(*channel, *note, *velocity),
            // a bend of 2 semitones each way, or 48 for a note of its own on an MPE keyboard
            ControlSignal::MidiBend { channel, value } => {
                let range = if *channel != 1 && self.plays_mpe() {
                    MPE_BEND_RANGE
                } else {
                    BEND_RANGE
                };
                let semitones = (f64::from(*value) - 8192.0) / 8192.0 * range;
                return self.on_channels(*channel, |s, c| {
                    s.expression_input(c, Expression::Bend, semitones, None)
                });
            }
            ControlSignal::MidiPressure {
                channel,
                note,
                value,
            } => {
                return self.on_channels(*channel, |s, c| {
                    s.expression_input(c, Expression::Pressure, f64::from(*value) / 127.0, *note)
                });
            }
            ControlSignal::MidiCc {
                cc: 74,
                channel,
                value,
            } if self.plays_mpe() => {
                return self.on_channels(*channel, |s, c| {
                    s.expression_input(c, Expression::Slide, f64::from(*value) / 127.0, None)
                });
            }
            ControlSignal::MidiCc {
                cc: 64,
                channel,
                value,
            } => {
                self.on_channels(*channel, |s, c| {
                    s.expression_input(c, Expression::Sustain, f64::from(*value) / 127.0, None)
                });
            }
            _ => {}
        }
        self.learn(signal);
        let targets: Vec<(String, f64, Option<&'static str>)> = self
            .program
            .controls
            .iter()
            .filter_map(|def| {
                let binding = def.input.as_ref()?;
                // a learning control listens to the control change it was given
                let learned = self
                    .learned
                    .get(&def.name)
                    .map(|&cc| InputSource::MidiCc { cc, channel: None });
                let source = if binding.source == InputSource::MidiLearn {
                    learned.as_ref()?
                } else {
                    &binding.source
                };
                let t = match (source, signal) {
                    (
                        InputSource::MidiCc { cc, channel },
                        ControlSignal::MidiCc {
                            cc: got,
                            channel: on,
                            value,
                        },
                    ) => (cc == got && channel.is_none_or(|c| c == *on))
                        .then(|| f64::from(*value) / 127.0)?,
                    (
                        InputSource::Osc(address),
                        ControlSignal::Osc {
                            address: got,
                            value,
                        },
                    ) => (address == got && value.is_finite()).then_some(*value)?,
                    _ => return None,
                };
                Some((def.name.clone(), def.map_input(t)?, def.unit.suffix()))
            })
            .collect();
        targets
            .iter()
            .filter(|(name, value, unit)| self.set_control(name, *value, *unit, None, None).is_ok())
            .count()
    }

    /// Is a clip played from an MPE keyboard playing?
    pub(super) fn plays_mpe(&self) -> bool {
        self.schedule.plays_mpe(self.seconds(self.clock))
    }

    /// Run `f` for `channel`. On an MPE keyboard the master channel (1) speaks for every note's channel.
    pub(super) fn on_channels(
        &mut self,
        channel: u8,
        mut f: impl FnMut(&mut Self, u8) -> usize,
    ) -> usize {
        if channel == 1 && self.plays_mpe() {
            (2..=16).map(|c| f(self, c)).sum()
        } else {
            f(self, channel)
        }
    }

    /// A key pressed (`velocity` from 1) or let go (0) on a MIDI keyboard. Every part whose clip is
    /// played from keys on this channel (`keys: midi.keys(channel: 1)`) plays the note until the key
    /// comes up. Returns how many parts took it. It is logged, so a set replays.
    pub fn key_input(&mut self, channel: u8, note: u8, velocity: u8) -> usize {
        self.sync_realtime();
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::Key {
                channel,
                note,
                velocity,
            },
        });
        self.key_event(channel, note, velocity)
    }

    /// A keyboard's pitch bend, pressure or sustain pedal. A held key's note gets a `bend` (an interval) or
    /// `pressure` (0 to 1) parameter changed if its instrument has one; keys played later start with the
    /// channel's current value. The pedal holds the notes of keys that come up until it does. It is logged.
    /// Returns how many notes it reached.
    pub fn expression_input(
        &mut self,
        channel: u8,
        what: Expression,
        value: f64,
        note: Option<u8>,
    ) -> usize {
        self.sync_realtime();
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::Expression {
                channel,
                what,
                value,
                note,
            },
        });
        match what {
            Expression::Sustain => {
                let released = self.keyboard.pedal(channel, value >= 0.5);
                for &key in &released {
                    self.release_key(key);
                }
                released.len()
            }
            Expression::Bend | Expression::Pressure | Expression::Slide => {
                let name = match what {
                    Expression::Bend => "bend",
                    Expression::Pressure => "pressure",
                    _ => "slide",
                };
                if note.is_none() {
                    self.keyboard.set_level(what, channel, value);
                }
                let keys = self.keyboard.keys_of(channel, note);
                for &key in &keys {
                    self.key_param(key, name, value as f32);
                }
                keys.len()
            }
        }
    }

    /// Set a parameter of the note a key holds, now.
    pub(super) fn key_param(&mut self, key: u32, name: &'static str, value: f32) {
        let stale = self.clock.saturating_sub(self.samples(2.0));
        self.key_params.retain(|p| p.3 > stale);
        self.key_params.push((key, name, value, self.clock));
        for held in self.sounding.held.iter().filter(|h| h.2 == key) {
            self.mixer.set_voice_param(held.1, name, value);
        }
        if self.live.is_some() {
            self.fresh.push(realtime::Fresh::Param { key, name, value, at: self.clock });
        }
        self.wake_planner();
    }

    pub(super) fn key_event(&mut self, channel: u8, note: u8, velocity: u8) -> usize {
        if velocity == 0 {
            let (released, total) = self.keyboard.lift(channel, note);
            for key in released {
                self.release_key(key);
            }
            return total;
        }
        // pressing a key that is still down, or held by the pedal, lets the first one go
        for key in self.keyboard.restrike(channel, note) {
            self.release_key(key);
        }
        let now = self.seconds(self.clock);
        let gain = -30.0 + 30.0 * f64::from(velocity.min(127)) / 127.0;
        let events: Vec<Event> = (1..self.program.tracks.len())
            .filter_map(|track| {
                self.schedule
                    .key_event(track, now, channel, i32::from(note), gain)
            })
            .collect();
        let mut keys = Vec::new();
        for mut event in events {
            let plan = match self.program.plan(&event) {
                Ok(plan) => plan,
                Err(e) => {
                    self.notify(format!("key for `{}` could not play: {e}", event.target));
                    continue;
                }
            };
            // a note starts with the bend and pressure the channel is at
            let params = &self.program.instruments[plan.instrument].params;
            for (name, level) in self.keyboard.starting(channel) {
                if let Some(level) = level.filter(|_| params.iter().any(|p| p.name == name)) {
                    event.args.insert(
                        name.into(),
                        if name == "bend" {
                            Value::Semitones(level)
                        } else {
                            Value::Num(level)
                        },
                    );
                }
            }
            let key = self.keyboard.new_key();
            let start = self
                .samples(event.at.to_seconds(self.program.tempo))
                .max(self.clock);
            self.add_explicit(start, 0, event, Some(key), None);
            keys.push(key);
        }
        self.wake_planner();
        let taken = keys.len();
        self.keyboard.hold(channel, note, keys);
        taken
    }

    /// Let go of the note a key started: now, if it is sounding, or as soon as it starts.
    pub(super) fn release_key(&mut self, key: u32) {
        self.sounding.release_key(key, self.clock);
        self.release_at(key, self.clock, None);
    }

    /// The first control waiting on `midi.learn` takes the control change that just arrived.
    pub(super) fn learn(&mut self, signal: &ControlSignal) {
        let ControlSignal::MidiCc { cc, .. } = signal else {
            return;
        };
        let waiting = self
            .program
            .controls
            .iter()
            .find(|d| {
                d.input
                    .as_ref()
                    .is_some_and(|i| i.source == InputSource::MidiLearn)
                    && !self.learned.contains_key(&d.name)
            })
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
