//! Brickwall lookahead limiter.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Seconds of lookahead: the time over which the gain eases down before a peak arrives.
const LOOKAHEAD: f64 = 0.005;

/// A lookahead brickwall limiter. `ceiling` is the maximum output level (linear; the language
/// converts dB) and `release` how fast the gain recovers once the peak has passed.
///
/// The gain each sample needs to stay under the ceiling is held at its lowest over the lookahead
/// window, then smoothed by a moving average over the same window. That eases the gain down
/// before a peak arrives, and means the smoothed gain at every sample is no higher than the gain
/// the loudest sample within reach of it needs, so the output never passes the ceiling, whatever
/// the release. The signal is delayed by [`Opcode::latency`] samples to make room for the lookahead.
#[derive(Clone)]
pub struct Limiter {
    /// The input, one lookahead window long: its oldest entry is the sample that leaves next.
    delay: Vec<f32>,
    /// The lowest required gain over the window, for each of its last `window` samples.
    lowest: Vec<f64>,
    /// How many entries of `lowest` are below 1.
    reduced: usize,
    /// The sum of `lowest`.
    sum: f64,
    /// The sliding minimum of the required gain: values that can still become the lowest, rising
    /// from the front, with the sample count each was seen at. A ring of `window + 1` entries.
    ranked: Vec<(u64, f64)>,
    front: usize,
    held: usize,
    count: u64,
    pos: usize,
    gain: f64,
    window: usize,
    sample_rate: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        let mut limiter = Limiter {
            delay: Vec::new(),
            lowest: Vec::new(),
            reduced: 0,
            sum: 0.0,
            ranked: Vec::new(),
            front: 0,
            held: 0,
            count: 0,
            pos: 0,
            gain: 1.0,
            window: 0,
            sample_rate: 48_000.0,
        };
        limiter.prepare(48_000.0);
        limiter
    }
}

impl Limiter {
    /// The lowest required gain among the last `window` samples, with `required` the newest.
    fn lowest_with(&mut self, required: f64) -> f64 {
        let size = self.ranked.len();
        // anything not lower than the new value can never be the lowest again
        while self.held > 0 && self.ranked[(self.front + self.held - 1) % size].1 >= required {
            self.held -= 1;
        }
        self.ranked[(self.front + self.held) % size] = (self.count, required);
        self.held += 1;
        // and what has left the window no longer counts
        while self.ranked[self.front].0 + self.window as u64 <= self.count {
            self.front = (self.front + 1) % size;
            self.held -= 1;
        }
        let lowest = self.ranked[self.front].1;
        self.count += 1;
        lowest
    }
}

impl Opcode for Limiter {
    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.lowest.fill(1.0);
        self.reduced = 0;
        self.sum = self.window as f64;
        self.front = 0;
        self.held = 0;
        self.count = 0;
        self.pos = 0;
        self.gain = 1.0;
    }

    fn name(&self) -> &str {
        "limiter"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("ceiling", 1.0),
            Port::optional("release", 0.1),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut fresh = Self::default();
        fresh.prepare(self.sample_rate);
        Box::new(fresh)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.window = ((LOOKAHEAD * f64::from(sample_rate)).round() as usize).max(2);
        self.delay = vec![0.0; self.window];
        self.lowest = vec![1.0; self.window];
        self.ranked = vec![(0, 1.0); self.window + 1];
        self.reset();
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.window == old.window
        {
            std::mem::swap(&mut self.delay, &mut old.delay);
            std::mem::swap(&mut self.lowest, &mut old.lowest);
            std::mem::swap(&mut self.ranked, &mut old.ranked);
            self.reduced = old.reduced;
            self.sum = old.sum;
            self.front = old.front;
            self.held = old.held;
            self.count = old.count;
            self.pos = old.pos;
            self.gain = old.gain;
        }
    }

    /// The lookahead: samples of delay between the input and the output.
    fn latency(&self) -> usize {
        self.window - 1
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let sr = f64::from(self.sample_rate).max(1.0);
        let release = f64::from(inputs[2][0]).max(0.001);
        let recovery = 1.0 - (-1.0 / (release * sr)).exp();
        let n = self.window;
        for (i, o) in out.iter_mut().enumerate() {
            let x = if inputs[0][i].is_finite() {
                inputs[0][i]
            } else {
                0.0
            };
            let ceiling = f64::from(inputs[1][i].abs().max(1e-6));
            let level = f64::from(x).abs();
            let required = if level > ceiling {
                ceiling / level
            } else {
                1.0
            };

            let lowest = self.lowest_with(required);
            let p = self.pos;
            self.reduced =
                self.reduced + usize::from(lowest < 1.0) - usize::from(self.lowest[p] < 1.0);
            // the running sum drifts a little with every change, so when nothing is reduced it is set exactly
            self.sum = if self.reduced == 0 {
                n as f64
            } else {
                self.sum + lowest - self.lowest[p]
            };
            self.lowest[p] = lowest;
            let smoothed = (self.sum / n as f64).min(1.0);

            // down at once, so no peak gets through; back up at the release
            self.gain = if smoothed < self.gain {
                smoothed
            } else {
                let gain = self.gain + (smoothed - self.gain) * recovery;
                if smoothed == 1.0 && 1.0 - gain < 1e-12 {
                    1.0
                } else {
                    gain
                }
            };

            self.delay[p] = x;
            // the oldest entry is the sample from `latency` ago
            *o = (f64::from(self.delay[(p + 1) % n]) * self.gain) as f32;
            self.pos = (p + 1) % n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(limiter: &mut Limiter, input: &[f32], ceiling: f32, release: f32) -> Vec<f32> {
        let ctx = ProcessCtx::new(48_000.0, true);
        let ceilings = vec![ceiling; input.len()];
        let releases = vec![release; input.len()];
        let mut out = vec![0.0; input.len()];
        limiter.process(&ctx, &[input, &ceilings, &releases], &mut out);
        out
    }

    /// A deterministic burst of loud noise between quiet stretches.
    fn burst(len: usize) -> Vec<f32> {
        let mut state = 12345u32;
        (0..len)
            .map(|i| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                if (2000..16_000).contains(&i) {
                    noise * 2.0
                } else {
                    noise * 0.1
                }
            })
            .collect()
    }

    #[test]
    fn the_output_never_passes_the_ceiling_whatever_the_release() {
        for release in [0.0001, 0.001, 0.01, 0.1, 0.5] {
            let mut limiter = Limiter::default();
            limiter.prepare(48_000.0);
            let out = run(&mut limiter, &burst(30_000), 0.5, release);
            let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(peak <= 0.5 + 1e-6, "release {release}: peak {peak}");
            assert!(
                peak > 0.45,
                "release {release}: the loud part is limited, not silenced ({peak})"
            );
        }
    }

    #[test]
    fn a_signal_under_the_ceiling_passes_unchanged_after_the_lookahead() {
        let mut limiter = Limiter::default();
        limiter.prepare(48_000.0);
        let input: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
        let out = run(&mut limiter, &input, 0.9, 0.1);
        let delay = limiter.latency();
        assert_eq!(delay, 239);
        assert!(out[..delay].iter().all(|v| *v == 0.0));
        for i in 0..input.len() - delay {
            assert_eq!(out[i + delay], input[i], "sample {i}");
        }
    }

    #[test]
    fn the_gain_comes_back_after_a_peak() {
        let mut limiter = Limiter::default();
        limiter.prepare(48_000.0);
        let mut input = vec![0.2f32; 48_000];
        input[1000] = 4.0;
        let out = run(&mut limiter, &input, 1.0, 0.05);
        assert!(out[1000 + limiter.latency()] <= 1.0 + 1e-6);
        assert!(
            (out[47_000] - 0.2).abs() < 1e-3,
            "recovered to {}",
            out[47_000]
        );
    }

    #[test]
    fn something_that_is_not_a_number_is_silence() {
        let mut limiter = Limiter::default();
        limiter.prepare(48_000.0);
        let mut input = vec![0.3f32; 2000];
        input[500] = f32::NAN;
        input[501] = f32::INFINITY;
        let out = run(&mut limiter, &input, 1.0, 0.1);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
