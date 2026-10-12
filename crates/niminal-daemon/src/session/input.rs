//! Things that arrive from outside while the session runs: events, control changes and keys.

use super::*;



impl Session {
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
            self.insert_upcoming(Upcoming { start, dur, event, key: None, scheduled: false });
        }
        Ok(())
    }

    /// Queue a typed control update at an absolute sample, or at the render
    /// clock when no timestamp is given. No parsing or graph compilation occurs.
    pub fn set_control(&mut self, name: &str, value: f64, unit: Option<&str>, at: Option<u64>, smooth_ms: Option<f64>) -> Result<u64, Vec<Problem>> {
        self.sync_realtime();
        let error = |message: String| vec![Problem::plain(message)];
        let index = self.program.controls.iter().position(|c| c.name == name)
            .ok_or_else(|| error(format!("unknown control `{name}`")))?;
        let def = &self.program.controls[index];
        let converted = def.convert(value, unit, self.program.tempo).map_err(error)?;
        let at = at.unwrap_or(self.clock);
        if at < self.clock { return Err(error("a control timestamp cannot be in the past".into())); }
        let seconds = smooth_ms.map_or(def.smooth_seconds, |ms| ms / 1000.0);
        if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
            return Err(error("control smoothing must be between 0ms and 60sec".into()));
        }
        if self.control_changes.len() >= crate::controls::CAPACITY {
            return Err(error("the control update queue is full".into()));
        }
        let change = crate::controls::Change { index, unit: def.unit, revision: self.controls[index].revision, at, value: converted,
            frames: (seconds * f64::from(self.sample_rate)).round() as u64, serial: self.next_control, log_index: self.log.len(), applied: !self.detached };
        if let Some(planner) = &mut self.live {
            planner.push_control(change).map_err(|_| error("the control update queue is full".into()))?;
        }
        self.next_control += 1;
        self.control_changes.push(change);
        self.control_changes.sort_unstable_by_key(|c| (c.at, c.serial));
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Control {
            name: name.into(), value, unit: unit.map(str::to_owned), at, smooth_ms,
        } });
        Ok(at)
    }

    /// A message from a MIDI device or OSC sender. Every control bound to it moves,
    /// exactly as if `control.set` had been called, so it is logged and replays.
    /// Returns how many controls took it.
    pub fn control_input(&mut self, signal: &ControlSignal) -> usize {
        match signal {
            ControlSignal::MidiNote { note, channel, velocity } => return self.key_input(*channel, *note, *velocity),
            // a bend of 2 semitones each way, or 48 for a note of its own on an MPE keyboard
            ControlSignal::MidiBend { channel, value } => {
                let range = if *channel != 1 && self.plays_mpe() { MPE_BEND_RANGE } else { BEND_RANGE };
                let semitones = (f64::from(*value) - 8192.0) / 8192.0 * range;
                return self.on_channels(*channel, |s, c| s.expression_input(c, Expression::Bend, semitones, None));
            }
            ControlSignal::MidiPressure { channel, note, value } =>
                return self.on_channels(*channel, |s, c| s.expression_input(c, Expression::Pressure, f64::from(*value) / 127.0, *note)),
            ControlSignal::MidiCc { cc: 74, channel, value } if self.plays_mpe() =>
                return self.on_channels(*channel, |s, c| s.expression_input(c, Expression::Slide, f64::from(*value) / 127.0, None)),
            ControlSignal::MidiCc { cc: 64, channel, value } => {
                self.on_channels(*channel, |s, c| s.expression_input(c, Expression::Sustain, f64::from(*value) / 127.0, None));
            }
            _ => {}
        }
        self.learn(signal);
        let targets: Vec<(String, f64, Option<&'static str>)> = self.program.controls.iter().filter_map(|def| {
            let binding = def.input.as_ref()?;
            // a learning control listens to the control change it was given
            let learned = self.learned.get(&def.name).map(|&cc| InputSource::MidiCc { cc, channel: None });
            let source = if binding.source == InputSource::MidiLearn { learned.as_ref()? } else { &binding.source };
            let t = match (source, signal) {
                (InputSource::MidiCc { cc, channel }, ControlSignal::MidiCc { cc: got, channel: on, value }) =>
                    (cc == got && channel.is_none_or(|c| c == *on)).then(|| f64::from(*value) / 127.0)?,
                (InputSource::Osc(address), ControlSignal::Osc { address: got, value }) =>
                    (address == got && value.is_finite()).then_some(*value)?,
                _ => return None,
            };
            Some((def.name.clone(), def.map_input(t)?, def.unit.suffix()))
        }).collect();
        targets.iter().filter(|(name, value, unit)| self.set_control(name, *value, *unit, None, None).is_ok()).count()
    }

    /// Is a clip played from an MPE keyboard playing?
    pub(super) fn plays_mpe(&self) -> bool {
        self.schedule.plays_mpe(self.seconds(self.clock))
    }

    /// Run `f` for `channel`. On an MPE keyboard the master channel (1) speaks for every note's channel.
    pub(super) fn on_channels(&mut self, channel: u8, mut f: impl FnMut(&mut Self, u8) -> usize) -> usize {
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
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Key { channel, note, velocity } });
        self.key_event(channel, note, velocity)
    }

    /// A keyboard's pitch bend, pressure or sustain pedal. A held key's note gets a `bend` (an interval) or
    /// `pressure` (0 to 1) parameter changed if its instrument has one; keys played later start with the
    /// channel's current value. The pedal holds the notes of keys that come up until it does. It is logged.
    /// Returns how many notes it reached.
    pub fn expression_input(&mut self, channel: u8, what: Expression, value: f64, note: Option<u8>) -> usize {
        self.sync_realtime();
        self.log.push(LogEntry { sample: self.clock, input: LogInput::Expression { channel, what, value, note } });
        match what {
            Expression::Sustain => {
                let released = self.keyboard.pedal(channel, value >= 0.5);
                for &key in &released {
                    self.release_key(key);
                }
                released.len()
            }
            Expression::Bend | Expression::Pressure | Expression::Slide => {
                let name = match what { Expression::Bend => "bend", Expression::Pressure => "pressure", _ => "slide" };
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
        self.invalidate_live();
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
            .filter_map(|track| self.schedule.key_event(track, now, channel, i32::from(note), gain))
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
                    event.args.insert(name.into(), if name == "bend" { Value::Semitones(level) } else { Value::Num(level) });
                }
            }
            let key = self.keyboard.new_key();
            let start = self.samples(event.at.to_seconds(self.program.tempo)).max(self.clock);
            self.insert_upcoming(Upcoming { start, dur: 0, event, key: Some(key), scheduled: false });
            keys.push(key);
        }
        self.invalidate_live();
        let taken = keys.len();
        self.keyboard.hold(channel, note, keys);
        taken
    }

    /// Let go of the note a key started: now, if it is sounding, or as soon as it starts.
    pub(super) fn release_key(&mut self, key: u32) {
        let stale = self.clock.saturating_sub(self.samples(2.0));
        self.key_offs.retain(|&(_, at)| at > stale);
        self.key_offs.push((key, self.clock));
        self.sounding.release_key(key, self.clock);
        // a note that has not started yet is played as a blip, long enough to hear
        let blip = self.samples(0.01);
        for note in self.upcoming.iter_mut().filter(|u| u.key == Some(key)) {
            note.key = None;
            note.dur = blip;
        }
        self.invalidate_live();
    }

    /// The first control waiting on `midi.learn` takes the control change that just arrived.
    pub(super) fn learn(&mut self, signal: &ControlSignal) {
        let ControlSignal::MidiCc { cc, .. } = signal else { return };
        let waiting = self.program.controls.iter()
            .find(|d| d.input.as_ref().is_some_and(|i| i.source == InputSource::MidiLearn) && !self.learned.contains_key(&d.name))
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
        while let Some(index) = self.control_changes.iter().position(|c| c.at <= self.clock && c.applied) {
            let change = self.control_changes.remove(index);
            if self.program.controls.get(change.index).is_some_and(|d| d.unit == change.unit)
                && let Some(state) = self.controls.get_mut(change.index)
            {
                state.set(change.at, change.value, change.frames);
                if !self.detached {
                    self.mixer.set_control(&state.key, state.start, state.target, state.frames, self.clock.saturating_sub(state.at));
                }
            } else { self.notify("a queued control update was dropped after its unit changed".into()); }
        }
    }
}

