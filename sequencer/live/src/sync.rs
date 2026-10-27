//! MIDI clock: 24 ticks to a beat, with start, continue and stop. [`ClockOut`] turns the bar grid
//! into the messages to send; [`ClockIn`] turns the messages of another clock into a tempo and a
//! place for bar 1.

use std::collections::VecDeque;

use niminal_compose::{Quantize, QuantizeRelation, QuantizeUnit};

use crate::quantize::Grid;

const TICKS_PER_BEAT: f64 = 24.0;

/// The one-byte real-time messages of MIDI clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Realtime {
    Tick,
    Start,
    Continue,
    Stop,
}

impl Realtime {
    pub fn byte(self) -> u8 {
        match self {
            Realtime::Tick => 0xF8,
            Realtime::Start => 0xFA,
            Realtime::Continue => 0xFB,
            Realtime::Stop => 0xFC,
        }
    }

    pub fn from_byte(byte: u8) -> Option<Realtime> {
        match byte {
            0xF8 => Some(Realtime::Tick),
            0xFA => Some(Realtime::Start),
            0xFB => Some(Realtime::Continue),
            0xFC => Some(Realtime::Stop),
            _ => None,
        }
    }
}

/// A message to send when the output has played up to sample `at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockMessage {
    pub at: u64,
    pub message: Realtime,
}

/// What this session sends to others: a start on a bar line, then a tick for every 24th of a beat.
#[derive(Default)]
pub(crate) struct ClockOut {
    on: bool,
    /// Messages up to this sample have been made.
    emitted_to: u64,
    started: bool,
    queue: Vec<ClockMessage>,
}

impl ClockOut {
    /// Send from sample `from`, beginning at the next bar line.
    pub fn enable(&mut self, from: u64) {
        *self = ClockOut {
            on: true,
            emitted_to: from,
            ..ClockOut::default()
        };
    }

    pub fn disable(&mut self, at: u64) {
        if self.on && self.started {
            self.queue.push(ClockMessage {
                at: at.max(self.emitted_to),
                message: Realtime::Stop,
            });
        }
        self.on = false;
        self.started = false;
    }

    /// The bars were counted again from sample `at`: stop, and start again on the next bar line.
    pub fn restart(&mut self, at: u64) {
        if self.on && self.started {
            self.queue.push(ClockMessage {
                at: at.max(self.emitted_to),
                message: Realtime::Stop,
            });
        }
        self.started = false;
    }

    /// Make the messages that fall before sample `upto`, on `grid`.
    pub fn emit(&mut self, grid: &Grid, upto: u64) {
        let from = self.emitted_to;
        if !self.on || upto <= from {
            return;
        }
        self.emitted_to = upto;
        let mut first = from;
        if !self.started {
            let bar = Quantize {
                relation: QuantizeRelation::OnGrid,
                count: 1.0,
                unit: QuantizeUnit::Bar,
            };
            let start = grid.landing(bar, from);
            if start >= upto {
                return;
            }
            self.queue.push(ClockMessage {
                at: start,
                message: Realtime::Start,
            });
            self.started = true;
            first = start;
        }
        let mut k = (grid.beats(first) * TICKS_PER_BEAT - 1e-6).ceil();
        loop {
            let at = grid.sample_at(k / TICKS_PER_BEAT).round() as i64;
            if at >= upto as i64 {
                break;
            }
            if at >= first as i64 {
                self.queue.push(ClockMessage {
                    at: at as u64,
                    message: Realtime::Tick,
                });
            }
            k += 1.0;
        }
    }

    pub fn take(&mut self) -> Vec<ClockMessage> {
        std::mem::take(&mut self.queue)
    }
}

/// How many ticks the tempo is worked out from.
const WINDOW_TICKS: usize = 48;
/// A tempo is only changed for a difference of at least this many beats per minute.
const DEADBAND_BPM: f64 = 0.05;
/// A phase error is worked off over this many beats.
const CORRECTION_BEATS: f64 = 4.0;
/// More than this far out of phase, in beats, the bars are moved instead.
const SLIP_BEATS: f64 = 0.25;
/// A tempo this far out, as a fraction, is taken as it is measured, without regard to the phase.
const FAR: f64 = 0.02;
const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 300.0;

/// What following the other clock asks of the session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Follow {
    /// Bar 1 started at this sample.
    Origin(f64),
    Tempo(f64),
}

/// The clock of another device or program, followed.
#[derive(Default)]
pub(crate) struct ClockIn {
    on: bool,
    running: bool,
    /// The next tick is the one the bars are lined up with.
    rebase: bool,
    /// Ticks received since the last start.
    ticks: u64,
    recent: VecDeque<f64>,
    /// No tempo is asked for until this many ticks have been received, so that a change has landed.
    quiet_until: u64,
}

impl ClockIn {
    pub fn set(&mut self, on: bool) {
        *self = ClockIn {
            on,
            ..ClockIn::default()
        };
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// A message arrived when the output was at `sample`.
    pub fn message(&mut self, message: Realtime, sample: f64, grid: &Grid) -> Option<Follow> {
        if !self.on {
            return None;
        }
        match message {
            Realtime::Start => {
                self.running = true;
                self.rebase = true;
                self.ticks = 0;
                self.recent.clear();
                None
            }
            Realtime::Continue => {
                self.running = true;
                self.rebase = true;
                self.recent.clear();
                None
            }
            Realtime::Stop => {
                self.running = false;
                self.recent.clear();
                None
            }
            Realtime::Tick => self.tick(sample, grid),
        }
    }

    fn tick(&mut self, sample: f64, grid: &Grid) -> Option<Follow> {
        let beat_samples = grid.beat_seconds() * grid.sample_rate;
        if self.rebase {
            self.rebase = false;
            self.recent.push_back(sample);
            self.ticks += 1;
            let origin = sample - (self.ticks - 1) as f64 / TICKS_PER_BEAT * beat_samples;
            return Some(Follow::Origin(origin));
        }
        self.recent.push_back(sample);
        if self.recent.len() > WINDOW_TICKS + 1 {
            self.recent.pop_front();
        }
        self.ticks += 1;
        if self.recent.len() < WINDOW_TICKS + 1 || self.ticks < self.quiet_until {
            return None;
        }
        let (first, last) = (self.recent[0], self.recent[self.recent.len() - 1]);
        let per_tick = (last - first) / (self.recent.len() - 1) as f64;
        let measured = 60.0 * grid.sample_rate / (per_tick * TICKS_PER_BEAT);
        if !(MIN_BPM..=MAX_BPM).contains(&measured) {
            return None;
        }
        let mut bpm = measured;
        // a tempo far from ours first; the phase is worked on once the tempo has landed
        let far = (measured - grid.bpm).abs() > measured * FAR;
        if self.running && !far {
            let expected = (self.ticks - 1) as f64 / TICKS_PER_BEAT;
            let ahead = (sample - grid.origin as f64) / beat_samples - expected;
            if ahead.abs() > SLIP_BEATS {
                let origin = sample - expected * beat_samples;
                self.recent.clear();
                self.quiet_until = self.ticks + TICKS_PER_BEAT as u64;
                return Some(Follow::Origin(origin));
            }
            bpm *= 1.0 - ahead / CORRECTION_BEATS;
        }
        if (bpm - grid.bpm).abs() < DEADBAND_BPM {
            return None;
        }
        self.quiet_until = self.ticks + TICKS_PER_BEAT as u64;
        Some(Follow::Tempo(bpm.clamp(MIN_BPM, MAX_BPM)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(bpm: f64) -> Grid {
        Grid {
            sample_rate: 48_000.0,
            bpm,
            beats_per_bar: 4.0,
            origin: 0,
        }
    }

    #[test]
    fn ticks_are_twenty_four_to_the_beat_and_start_on_a_bar_line() {
        // 120bpm: a tick every 1000 samples, a bar of 96000
        let mut out = ClockOut::default();
        out.enable(10_000);
        out.emit(&grid(120.0), 100_000);
        let sent = out.take();
        assert_eq!(
            sent[0],
            ClockMessage {
                at: 96_000,
                message: Realtime::Start
            }
        );
        assert_eq!(
            sent[1],
            ClockMessage {
                at: 96_000,
                message: Realtime::Tick
            }
        );
        let ticks: Vec<u64> = sent[1..].iter().map(|m| m.at).collect();
        assert_eq!(ticks, vec![96_000, 97_000, 98_000, 99_000]);
    }

    #[test]
    fn windows_join_without_a_gap_or_a_repeat() {
        let mut one = ClockOut::default();
        one.enable(0);
        one.emit(&grid(123.4), 400_000);
        let whole = one.take();
        let mut two = ClockOut::default();
        two.enable(0);
        for upto in [50_001, 137_777, 250_000, 400_000] {
            two.emit(&grid(123.4), upto);
        }
        assert_eq!(two.take(), whole);
        assert!(whole.len() > 90);
    }

    #[test]
    fn a_tempo_change_keeps_the_ticks_coming_in_time() {
        let mut out = ClockOut::default();
        out.enable(0);
        out.emit(&grid(120.0), 96_000 + 5_000);
        let _ = out.take();
        // 100 samples after beat 4 and 24 ticks the tempo halves; the beat count stays
        let at = 102_000;
        let slow = Grid {
            origin: at as i64 - at as i64 * 2,
            ..grid(60.0)
        };
        out.emit(&slow, 110_000);
        let ticks: Vec<u64> = out.take().iter().map(|m| m.at).collect();
        assert!(ticks.windows(2).all(|w| w[1] - w[0] >= 1_900 && w[1] - w[0] <= 2_100));
    }

    #[test]
    fn disabling_stops_and_a_restart_starts_on_the_next_bar() {
        let mut out = ClockOut::default();
        out.enable(0);
        out.emit(&grid(120.0), 10_000);
        let _ = out.take();
        out.restart(10_000);
        out.emit(&grid(120.0), 200_000);
        let sent = out.take();
        assert_eq!(sent[0].message, Realtime::Stop);
        assert_eq!(
            sent.iter().find(|m| m.message == Realtime::Start).unwrap().at,
            96_000
        );
        out.disable(200_000);
        assert_eq!(out.take()[0].message, Realtime::Stop);
        out.emit(&grid(120.0), 300_000);
        assert!(out.take().is_empty());
    }

    /// Ticks of a clock at `bpm`, one every `per_tick` samples from `first`.
    fn feed(
        follower: &mut ClockIn,
        grid: &mut Grid,
        from: f64,
        n: usize,
        per_tick: f64,
    ) -> Vec<Follow> {
        let mut asked = Vec::new();
        for i in 0..n {
            let at = from + i as f64 * per_tick;
            if let Some(f) = follower.message(Realtime::Tick, at, grid) {
                match f {
                    Follow::Origin(o) => grid.origin = o.round() as i64,
                    // a tempo change keeps the beat the clock is at
                    Follow::Tempo(bpm) => {
                        let beats = (at - grid.origin as f64) / (grid.beat_seconds() * grid.sample_rate);
                        grid.bpm = bpm;
                        grid.origin = (at - beats * grid.beat_seconds() * grid.sample_rate).round() as i64;
                    }
                }
                asked.push(f);
            }
        }
        asked
    }

    #[test]
    fn a_start_puts_bar_one_on_the_first_tick() {
        let mut follower = ClockIn::default();
        follower.set(true);
        let g = grid(120.0);
        follower.message(Realtime::Start, 4_900.0, &g);
        let first = follower.message(Realtime::Tick, 5_000.0, &g);
        assert_eq!(first, Some(Follow::Origin(5_000.0)));
    }

    #[test]
    fn it_settles_on_the_other_clocks_tempo() {
        let mut follower = ClockIn::default();
        follower.set(true);
        let mut g = grid(120.0);
        follower.message(Realtime::Start, 0.0, &g);
        // 100bpm: 60 * 48000 / (100 * 24) = 1200 samples a tick
        let asked = feed(&mut follower, &mut g, 5_000.0, 200, 1_200.0);
        assert!(matches!(asked[0], Follow::Origin(_)));
        assert!((g.bpm - 100.0).abs() < 0.1, "bpm was {}", g.bpm);
        // and the bars are where the other clock's are: tick 192 (beat 8) is on a bar line
        let beats = (5_000.0 + 192.0 * 1_200.0 - g.origin as f64) / (g.beat_seconds() * 48_000.0);
        assert!((beats - 8.0).abs() < 0.05, "beats {beats}");
    }

    #[test]
    fn a_clock_a_little_fast_is_not_left_to_drift() {
        let mut follower = ClockIn::default();
        follower.set(true);
        let mut g = grid(100.0);
        follower.message(Realtime::Start, 0.0, &g);
        feed(&mut follower, &mut g, 0.0, 1, 1_200.0);
        // 100.5bpm for a few minutes
        let per_tick = 60.0 * 48_000.0 / (100.5 * 24.0);
        let n = 4_000;
        feed(&mut follower, &mut g, per_tick, n, per_tick);
        let at = (n + 1) as f64 * per_tick;
        let beats = (at - g.origin as f64) / (g.beat_seconds() * 48_000.0);
        let want = (n + 1) as f64 / 24.0;
        assert!((beats - want).abs() < 0.1, "{beats} against {want}");
        assert!((g.bpm - 100.5).abs() < 0.2, "bpm was {}", g.bpm);
    }

    #[test]
    fn without_a_start_only_the_tempo_is_followed() {
        let mut follower = ClockIn::default();
        follower.set(true);
        let mut g = grid(120.0);
        let asked = feed(&mut follower, &mut g, 0.0, 120, 1_200.0);
        assert!(asked.iter().all(|f| matches!(f, Follow::Tempo(_))));
        assert!((g.bpm - 100.0).abs() < 0.1);
    }

    #[test]
    fn it_does_nothing_unless_switched_on() {
        let mut follower = ClockIn::default();
        let mut g = grid(120.0);
        assert!(feed(&mut follower, &mut g, 0.0, 200, 1_200.0).is_empty());
    }

    #[test]
    fn bytes_round_trip() {
        for m in [Realtime::Tick, Realtime::Start, Realtime::Continue, Realtime::Stop] {
            assert_eq!(Realtime::from_byte(m.byte()), Some(m));
        }
        assert_eq!(Realtime::from_byte(0xFE), None);
    }
}
