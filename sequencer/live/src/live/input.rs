//! Things that arrive from outside while the sequencer runs: controls moved by MIDI and OSC, and keys.

use super::*;

/// How far a keyboard's pitch bend goes each way, in semitones.
const BEND_RANGE: f64 = 2.0;
/// How far an MPE note's own pitch bend goes, each way, in semitones.
const MPE_BEND_RANGE: f64 = 48.0;

/// What arrived from outside to move a bound `ctl`. MIDI channels count from 1, and
/// an OSC value is 0 to 1 (outside is clamped).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlSignal<'a> {
    MidiCc {
        cc: u8,
        channel: u8,
        value: u8,
    },
    /// A key pressed (`velocity` from 1) or let go (`velocity` 0), on a MIDI channel from 1.
    MidiNote {
        note: u8,
        channel: u8,
        velocity: u8,
    },
    /// Pitch bend on a channel, 0 to 16383 with 8192 in the middle.
    MidiBend {
        channel: u8,
        value: u16,
    },
    /// Pressure on a channel, or on one key (`note`), 0 to 127.
    MidiPressure {
        channel: u8,
        note: Option<u8>,
        value: u8,
    },
    Osc {
        address: &'a str,
        value: f64,
    },
}

impl Live {
    /// Move a `ctl` now, to `value` in `unit` (the declaration's own if none), over `smooth_ms` (the
    /// control's own smoothing if none).
    pub fn set_control(
        &mut self,
        name: &str,
        value: f64,
        unit: Option<&str>,
        smooth_ms: Option<f64>,
    ) -> Result<(), Vec<Problem>> {
        let op = Op::Control {
            name: name.into(),
            at: At::Now,
            value,
            unit: unit.map(str::to_owned),
            glide: smooth_ms.map(|ms| Time::Seconds(ms / 1000.0)),
            tag: None,
        };
        self.link
            .request(EngineRequest::EventsSend { events: vec![op] })
            .map(|_| ())
            .map_err(|e| {
                e.problems()
                    .into_iter()
                    .map(Problem::plain)
                    .chain(std::iter::once(Problem::plain(e.message)))
                    .take(1)
                    .collect()
            })
    }

    /// A message from a MIDI device or OSC sender. Every control bound to it moves. Returns how many
    /// controls took it.
    pub fn control_input(&mut self, signal: &ControlSignal) -> usize {
        let ran = self.fire_triggers(signal);
        ran + self.drive_controls(signal)
    }

    /// Run the commands of every `on` this signal sets off. A controller or OSC value sets it off as it
    /// goes down (64 or more, or above one half), not again until it has come back up. Returns how many
    /// went off.
    fn fire_triggers(&mut self, signal: &ControlSignal) -> usize {
        let program = self.program.clone();
        let mut went_off = 0;
        for trigger in &program.triggers {
            let key = trigger.source.key();
            let hit = match (&trigger.source, signal) {
                (
                    InputSource::MidiNote { note, channel },
                    ControlSignal::MidiNote {
                        note: got,
                        channel: on,
                        velocity,
                    },
                ) => note == got && channel.is_none_or(|c| c == *on) && *velocity > 0,
                (
                    InputSource::MidiCc { cc, channel },
                    ControlSignal::MidiCc {
                        cc: got,
                        channel: on,
                        value,
                    },
                ) if cc == got && channel.is_none_or(|c| c == *on) => {
                    self.goes_down(&key, *value >= 64)
                }
                (
                    InputSource::Osc(address),
                    ControlSignal::Osc {
                        address: got,
                        value,
                    },
                ) if address == got => self.goes_down(&key, *value > 0.5),
                _ => false,
            };
            if !hit {
                continue;
            }
            went_off += 1;
            for command in &trigger.commands {
                if let Err(problems) = self.eval(command, None) {
                    let why = problems
                        .first()
                        .map_or_else(String::new, |p| format!(": {}", p.message));
                    self.notify(format!("`on {key}` could not run `{command}`{why}"));
                }
            }
        }
        went_off
    }

    /// Is `down` a change from up to down for the trigger `key`?
    fn goes_down(&mut self, key: &str, down: bool) -> bool {
        if down {
            self.triggers_down.insert(key.to_string())
        } else {
            self.triggers_down.remove(key);
            false
        }
    }

    fn drive_controls(&mut self, signal: &ControlSignal) -> usize {
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
                let binding = self.program.input(&def.name)?;
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
                Some((
                    def.name.clone(),
                    binding.map_input(def, t),
                    def.unit.suffix(),
                ))
            })
            .collect();
        targets
            .iter()
            .filter(|(name, value, unit)| self.set_control(name, *value, *unit, None).is_ok())
            .count()
    }

    /// Is a clip played from an MPE keyboard playing?
    fn plays_mpe(&self) -> bool {
        self.schedule.plays_mpe(self.seconds(self.clock))
    }

    /// Run `f` for `channel`. On an MPE keyboard the master channel (1) speaks for every note's channel.
    fn on_channels(&mut self, channel: u8, mut f: impl FnMut(&mut Self, u8) -> usize) -> usize {
        if channel == 1 && self.plays_mpe() {
            (2..=16).map(|c| f(self, c)).sum()
        } else {
            f(self, channel)
        }
    }

    /// A key pressed (`velocity` from 1) or let go (0) on a MIDI keyboard. Every part whose clip is
    /// played from keys on this channel (`keys: midi.keys(channel: 1)`) plays the note until the key
    /// comes up. Returns how many parts took it.
    pub fn key_input(&mut self, channel: u8, note: u8, velocity: u8) -> usize {
        self.look();
        self.log.push(LiveLogEntry {
            sample: self.clock,
            input: LiveInput::Key {
                channel,
                note,
                velocity,
            },
        });
        self.key_event(channel, note, velocity)
    }

    /// A keyboard's pitch bend, pressure or sustain pedal. A held key's note gets a `bend` (an interval) or
    /// `pressure` (0 to 1) parameter changed if its instrument has one; keys played later start with the
    /// channel's current value. The pedal holds the notes of keys that come up until it does.
    /// Returns how many notes it reached.
    pub fn expression_input(
        &mut self,
        channel: u8,
        what: Expression,
        value: f64,
        note: Option<u8>,
    ) -> usize {
        self.look();
        self.log.push(LiveLogEntry {
            sample: self.clock,
            input: LiveInput::Expression {
                channel,
                what,
                value,
                note,
            },
        });
        match what {
            Expression::Sustain => {
                let released = self.keyboard.pedal(channel, value >= 0.5);
                let mut ops = Vec::new();
                for &key in &released {
                    ops.extend(self.release_key(key));
                }
                self.send(ops);
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
                let mut ops = Vec::new();
                for &key in &keys {
                    let id = u64::from(key);
                    let has = self
                        .held
                        .get(&id)
                        .and_then(|name| self.program.instruments.iter().find(|i| i.name == *name))
                        .is_some_and(|i| i.params.iter().any(|p| p.name == name));
                    if has {
                        let value = if what == Expression::Bend {
                            Value::Semitones(value)
                        } else {
                            Value::Num(value)
                        };
                        ops.push(Op::Set {
                            id,
                            at: At::Now,
                            args: [(name.to_string(), value)].into(),
                            glide: None,
                            tag: None,
                        });
                    }
                }
                self.send(ops);
                keys.len()
            }
        }
    }

    fn key_event(&mut self, channel: u8, note: u8, velocity: u8) -> usize {
        let mut ops = Vec::new();
        if velocity == 0 {
            let (released, total) = self.keyboard.lift(channel, note);
            for key in released {
                ops.extend(self.release_key(key));
            }
            self.send(ops);
            return total;
        }
        // pressing a key that is still down, or held by the pedal, lets the first one go
        for key in self.keyboard.restrike(channel, note) {
            ops.extend(self.release_key(key));
        }
        let now = self.seconds(self.clock);
        let gain = -30.0 + 30.0 * f64::from(velocity.min(127)) / 127.0;
        let events: Vec<Event> = (1..self.program.nodes.len())
            .filter_map(|node| {
                self.schedule
                    .key_event(node, now, channel, i32::from(note), gain)
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
            let instrument = &self.program.instruments[plan.instrument];
            for (name, level) in self.keyboard.starting(channel) {
                if let Some(level) =
                    level.filter(|_| instrument.params.iter().any(|p| p.name == name))
                {
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
            let instrument = instrument.name.clone();
            let key = self.keyboard.new_key();
            let start = self
                .samples(event.at.to_seconds(self.program.tempo))
                .max(self.clock);
            let id = u64::from(key);
            self.held.insert(id, instrument);
            ops.push(Op::Note {
                to: None,
                target: if self.program.node_index(&event.target).is_some() {
                    super::routing_id(&event.target)
                } else {
                    event.target.clone()
                },
                at: At::Sample(start),
                args: event
                    .args
                    .iter()
                    .map(|(n, v)| (n.clone(), niminal_lang::dsp_value(*v, self.program.tempo)))
                    .collect(),
                dur: None,
                id: Some(id),
                tag: None,
            });
            keys.push(key);
        }
        self.send(ops);
        let taken = keys.len();
        self.keyboard.hold(channel, note, keys);
        taken
    }

    /// Let go of the note a key started: now, if it is sounding, or as soon as it starts.
    fn release_key(&mut self, key: u32) -> Option<Op> {
        let id = u64::from(key);
        self.held.remove(&id)?;
        Some(Op::Off {
            id,
            at: At::Now,
            tag: None,
        })
    }

    /// The first control waiting on `midi.learn` takes the control change that just arrived.
    fn learn(&mut self, signal: &ControlSignal) {
        let ControlSignal::MidiCc { cc, .. } = signal else {
            return;
        };
        let waiting = self
            .program
            .controls
            .iter()
            .find(|d| {
                self.program
                    .input(&d.name)
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

    /// Stop everything at once, including effect tails, and forget what was playing.
    pub fn panic(&mut self) {
        self.look();
        self.log.push(LiveLogEntry {
            sample: self.clock,
            input: LiveInput::Panic,
        });
        if let Err(e) = self.link.request(EngineRequest::Panic {}) {
            self.notify(describe(&e));
        }
        self.take_back(SCHEDULED, 0);
        self.take_back(WRITTEN, 0);
        self.held.clear();
        let at = self.seconds(self.clock);
        self.history.push(HistoryEntry {
            at,
            action: NamedAction::Panic,
            arrangement: None,
        });
        self.playing.clear();
        self.rebuild_schedule(self.clock);
        // the panic just made is done
        self.next_panic = self
            .schedule
            .panics()
            .iter()
            .take_while(|&&t| self.samples(t) <= self.clock)
            .count();
        self.advance();
    }

    /// Stop every clip, leaving what is sounding to ring out.
    pub fn hush(&mut self) -> Result<Accepted, Vec<Problem>> {
        self.eval("hush", Some("now"))
    }
}
