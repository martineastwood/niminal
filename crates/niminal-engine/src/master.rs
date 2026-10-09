//! The always-on safety stage at the end of the signal path: a DC blocker and
//! a lookahead peak limiter.
//!
//! The limiter delays the signal by [`Master::latency`] samples so it can
//! start turning the gain down before a peak arrives. Gain is the sliding
//! minimum of the required gain over the lookahead window, smoothed by a moving
//! average of the same length, which guarantees the output never exceeds the
//! ceiling. Gain then recovers slowly so bass notes aren't distorted.
//!
//! The ceiling applies to sample peaks. Inter-sample (true) peaks can exceed it
//! by a fraction of a decibel.

const LOOKAHEAD_SECONDS: f32 = 0.001;
const RELEASE_SECONDS: f64 = 0.05;
const DC_CUTOFF_HZ: f64 = 5.0;

pub struct Master {
    // DC blocker
    dc_r: f64,
    dc_x1: f64,
    dc_y1: f64,

    ceiling: f64,
    window: usize,
    pos: usize,
    delay: Vec<f64>,
    required: Vec<f64>,
    windowed_min: Vec<f64>,
    min_sum: f64,
    release_coeff: f64,
    gain: f64,
}

impl Master {
    /// `ceiling` is a linear level, for example 0.966 for -0.3db.
    pub fn new(sample_rate: f32, ceiling: f32) -> Self {
        let sr = f64::from(sample_rate);
        let window = ((LOOKAHEAD_SECONDS * sample_rate).round() as usize).max(2);
        Master {
            dc_r: 1.0 - std::f64::consts::TAU * DC_CUTOFF_HZ / sr,
            dc_x1: 0.0,
            dc_y1: 0.0,
            ceiling: f64::from(ceiling),
            window,
            pos: 0,
            delay: vec![0.0; window],
            required: vec![1.0; window],
            windowed_min: vec![1.0; window],
            min_sum: window as f64,
            release_coeff: 1.0 - (-1.0 / (RELEASE_SECONDS * sr)).exp(),
            gain: 1.0,
        }
    }

    /// Samples of delay between input and output.
    pub fn latency(&self) -> usize {
        self.window - 1
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let n = self.window;
        for sample in buf.iter_mut() {
            // Remove DC, then clean up anything that isn't a number.
            let input = f64::from(*sample);
            let input = if input.is_finite() { input } else { 0.0 };
            let x = input - self.dc_x1 + self.dc_r * self.dc_y1;
            self.dc_x1 = input;
            self.dc_y1 = x;

            let p = self.pos;
            self.delay[p] = x;
            let peak = x.abs();
            self.required[p] = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };

            let min = self.required.iter().fold(1.0f64, |m, r| m.min(*r));
            self.min_sum += min - self.windowed_min[p];
            self.windowed_min[p] = min;
            let smoothed = (self.min_sum / n as f64).min(1.0);

            self.gain = if smoothed < self.gain {
                smoothed
            } else {
                self.gain + (smoothed - self.gain) * self.release_coeff
            };

            // The oldest entry in the ring is the sample from `latency` ago.
            *sample = (self.delay[(p + 1) % n] * self.gain) as f32;
            self.pos = (p + 1) % n;
        }
    }
}
