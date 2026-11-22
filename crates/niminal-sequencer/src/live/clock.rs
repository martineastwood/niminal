use super::*;

impl Live {
    /// The tempo and meter the project has now.
    pub fn tempo(&self) -> Tempo {
        self.program.tempo
    }

    /// Send MIDI clock from the next bar line, or stop sending it.
    pub fn set_clock_out(&mut self, on: bool) {
        if on {
            self.clock_out.enable(self.clock);
        } else {
            self.clock_out.disable(self.clock);
        }
    }

    /// Follow the tempo of a MIDI clock that [`Live::midi_clock`] is given, or stop.
    pub fn set_clock_in(&mut self, on: bool) {
        self.clock_in.set(on);
    }

    /// The clock messages to send since this was last called, in order, each for the sample at which
    /// the output plays it.
    pub fn take_clock_out(&mut self) -> Vec<ClockMessage> {
        self.clock_out.take()
    }

    /// The Unix time in microseconds at which the output plays `sample`, once the engine has said.
    pub fn wall_micros_of(&self, sample: u64) -> Option<u64> {
        let info = self.engine.as_ref()?;
        let wall = info.wall_micros? as f64;
        let seconds = (sample as f64 - info.sample as f64) / f64::from(self.sample_rate);
        Some((wall + (seconds + info.output_latency) * 1e6).max(0.0) as u64)
    }

    /// A message of the clock being followed arrived at Unix time `wall_micros`.
    pub fn midi_clock(&mut self, message: Realtime, wall_micros: u64) {
        let Some(info) = &self.engine else { return };
        let Some(wall) = info.wall_micros else { return };
        let seconds = (wall_micros as f64 - wall as f64) / 1e6 - info.output_latency;
        let sample = info.sample as f64 + seconds * f64::from(self.sample_rate);
        self.midi_clock_at(message, sample);
    }

    /// Like [`Live::midi_clock`], for a message that arrived as the output was at `sample`.
    pub fn midi_clock_at(&mut self, message: Realtime, sample: f64) {
        match self.clock_in.message(message, sample, &self.grid()) {
            Some(Follow::Origin(origin)) => self.rebase_origin(origin),
            Some(Follow::Tempo(bpm)) => self.follow_tempo(bpm),
            None => {}
        }
    }

    /// Bars count from `sample` now.
    fn rebase_origin(&mut self, sample: f64) {
        let at = sample / f64::from(self.sample_rate);
        let i = self.history.partition_point(|h| h.at <= at);
        self.history.insert(
            i,
            HistoryEntry {
                at,
                action: NamedAction::Origin,
                arrangement: None,
            },
        );
        let tempo = self.program.tempo;
        self.send(vec![self.tempo_op(self.samples(at), tempo, true)]);
    }

    /// Ask for a tempo, to land a little ahead of what has been sent, without saying so.
    fn follow_tempo(&mut self, bpm: f64) {
        let at = self.clock.max(self.fetched_until);
        self.landing_at = Some(at);
        let result = self.eval(&format!("tempo {bpm:.2}bpm"), Some("now"));
        self.landing_at = None;
        if let Ok(accepted) = result
            && let Some(id) = accepted.id
        {
            self.followed.push(id);
        }
    }

    fn clock_origin(&self, at: u64) -> f64 {
        self.history
            .iter()
            .rev()
            .find(|h| {
                self.samples(h.at) <= at
                    && matches!(h.action, NamedAction::Reset | NamedAction::Origin)
            })
            .map_or(0.0, |h| h.at)
    }

    /// Tell the engine the tempo from `at`: this connection's `clock.beat` carries on through a change, and
    /// counts from 0 at `at` when `restart` says the bars start there. The first time, it counts from the
    /// origin the project has.
    pub(super) fn tempo_op(&self, at: u64, tempo: Tempo, restart: bool) -> Op {
        let beat = if restart {
            Some(0.0)
        } else if !self.tempo_sent.get() {
            Some((self.seconds(at) - self.clock_origin(at)) * tempo.bpm / 60.0)
        } else {
            None
        };
        self.tempo_sent.set(true);
        Op::Tempo {
            at: At::Sample(at),
            bpm: tempo.bpm,
            meter: Some(tempo.beats_per_bar),
            beat,
            tag: Some(SCHEDULED.into()),
        }
    }
}

/// Where a change that waits for a boundary lands once the tempo or meter has changed at `now`: the same
/// number of bars (or beats) on. Changes that are themselves a tempo or meter keep their time, as the steps
/// of a ramp are spaced by the tempo they pass through.
pub(super) fn rescale_landing(lands_at: u64, now: u64, factor: f64) -> u64 {
    now + (lands_at.saturating_sub(now) as f64 * factor).round() as u64
}

/// How much longer a bar (`by_bar`) or a beat gets when the tempo or meter changes from `old` to `new`.
pub(super) fn landing_factor(by_bar: bool, old: Tempo, new: Tempo) -> f64 {
    if by_bar {
        (new.beats_per_bar / new.bpm) / (old.beats_per_bar / old.bpm)
    } else {
        old.bpm / new.bpm
    }
}

fn is_timing(p: &Pending) -> bool {
    p.statements
        .iter()
        .any(|s| matches!(s.key(), Some("tempo" | "meter")))
}

pub(super) fn rescale_pending(
    pending: &mut [Pending],
    now: u64,
    old: Tempo,
    new: Tempo,
    by_bar: impl Fn(&Pending) -> bool,
) {
    if old == new {
        return;
    }
    for p in pending.iter_mut().filter(|p| !p.done && !is_timing(p)) {
        p.lands_at = rescale_landing(p.lands_at, now, landing_factor(by_bar(p), old, new));
    }
    pending.sort_by_key(|p| p.lands_at);
}

/// A tempo or meter change keeps every clip where it is in its bar: what has happened is stretched, and
/// what is still to come, `at bar 9` for one, is stretched the same way around `now`. So are the bar
/// lines, which keeps the bar and beat that `now` is at.
pub(super) fn retime(history: &mut [HistoryEntry], now: f64, old: Tempo, new: Tempo) {
    let (was, is) = (old.beats_per_bar / old.bpm, new.beats_per_bar / new.bpm);
    if was == is {
        return;
    }
    let ratio = is / was;
    for entry in history {
        entry.at = now + (entry.at - now) * ratio;
    }
}

/// When bar 1 is, in session seconds: where the last `reset` landed.
pub(crate) fn origin_of(history: &[HistoryEntry]) -> f64 {
    history
        .iter()
        .rev()
        .find(|h| matches!(h.action, NamedAction::Reset | NamedAction::Origin))
        .map_or(0.0, |h| h.at)
}
