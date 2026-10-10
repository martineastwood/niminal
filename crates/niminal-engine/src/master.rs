//! The always-on safety stage at the end of the signal path: a DC blocker and
//! a lookahead peak limiter.
//!
//! The limiter delays the signal by [`Master::latency`] samples so it can
//! start turning the gain down before a peak arrives. Gain is the sliding
//! minimum of the required gain over the lookahead window, smoothed by a moving
//! average of the same length, which guarantees the output never exceeds the
//! ceiling. Gain then recovers slowly so bass notes aren't distorted.
//!
//! The ceiling applies to true peaks: each stretch between two samples is
//! looked at at 4x its sample rate, so a signal that would swing over the
//! ceiling between samples in a converter is caught too. That needs a few
//! samples of the future, which is why the signal is delayed by a few samples
//! more than the lookahead.

const LOOKAHEAD_SECONDS: f32 = 0.001;
const RELEASE_SECONDS: f64 = 0.05;
const DC_CUTOFF_HZ: f64 = 5.0;

/// The interpolator that finds peaks between samples looks at this many samples, centred on the
/// stretch between the two `HALF` and `HALF - 1` samples ago.
const TAPS: usize = 24;
const HALF: usize = TAPS / 2;
/// Where between two samples to look: a quarter, a half and three quarters of the way.
const PHASES: [f64; 3] = [0.25, 0.5, 0.75];

pub struct Master {
    channels: Vec<ChannelState>,
    dc_r: f64,
    ceiling: f64,
    window: usize,
    pos: usize,
    required: Vec<f64>,
    windowed_min: Vec<f64>,
    min_sum: f64,
    release_coeff: f64,
    gain: f64,
    /// The weights that find the signal between two samples, for each of `PHASES`.
    between: [[f64; TAPS]; PHASES.len()],
}

/// The per-channel parts: DC blocker memory and the lookahead delay line.
struct ChannelState {
    dc_x1: f64,
    dc_y1: f64,
    delay: Vec<f64>,
    /// The latest `TAPS` samples after the DC blocker, newest first.
    recent: [f64; TAPS],
}

impl Master {
    /// A one-channel master. `ceiling` is a linear level, for example 0.966
    /// for -0.3db.
    pub fn new(sample_rate: f32, ceiling: f32) -> Self {
        Master::with_channels(sample_rate, ceiling, 1)
    }

    /// A master for `channels` channels. The limiter looks at all of them
    /// together and applies one gain to all, so reducing a loud peak on one
    /// channel never shifts the stereo or surround image.
    pub fn with_channels(sample_rate: f32, ceiling: f32, channels: usize) -> Self {
        let sr = f64::from(sample_rate);
        let window = ((LOOKAHEAD_SECONDS * sample_rate).round() as usize).max(2);
        Master {
            channels: (0..channels)
                .map(|_| ChannelState { dc_x1: 0.0, dc_y1: 0.0, delay: vec![0.0; window], recent: [0.0; TAPS] })
                .collect(),
            dc_r: 1.0 - std::f64::consts::TAU * DC_CUTOFF_HZ / sr,
            ceiling: f64::from(ceiling),
            window,
            pos: 0,
            required: vec![1.0; window],
            windowed_min: vec![1.0; window],
            min_sum: window as f64,
            release_coeff: 1.0 - (-1.0 / (RELEASE_SECONDS * sr)).exp(),
            gain: 1.0,
            between: std::array::from_fn(|i| interpolation_weights(PHASES[i])),
        }
    }

    /// Clear lookahead and DC memory on panic so pre-panic audio cannot leak.
    pub fn clear(&mut self) {
        for channel in &mut self.channels {
            channel.dc_x1 = 0.0;
            channel.dc_y1 = 0.0;
            channel.delay.fill(0.0);
            channel.recent = [0.0; TAPS];
        }
        self.required.fill(1.0);
        self.windowed_min.fill(1.0);
        self.min_sum = self.window as f64;
        self.gain = 1.0;
        self.pos = 0;
    }

    /// Nothing is in the delay line, no gain reduction is in progress and the DC blocker has
    /// settled, so silence in gives exactly silence out and the state does not change.
    fn at_rest(&self) -> bool {
        self.gain == 1.0
            && self.channels.iter().all(|c| {
                c.dc_x1 == 0.0 && c.dc_y1.abs() < 1e-18 && c.delay.iter().all(|d| *d == 0.0) && c.recent.iter().all(|r| *r == 0.0)
            })
            && self.required.iter().all(|r| *r == 1.0)
            && self.windowed_min.iter().all(|m| *m == 1.0)
    }

    /// Samples of delay between input and output.
    pub fn latency(&self) -> usize {
        self.window - 1 + HALF
    }

    /// Process a one-channel master in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        assert_eq!(self.channels.len(), 1, "this master has {} channels; use process_linked", self.channels.len());
        for sample in buf.iter_mut() {
            let mut frame = [*sample];
            self.process_frame(&mut frame);
            *sample = frame[0];
        }
    }

    /// Process the first `frames` samples of every channel's block in place,
    /// without allocating, for the real-time path.
    pub fn process_block(&mut self, block: &mut [[f32; crate::opcode::BLOCK]], frames: usize) {
        assert_eq!(block.len(), self.channels.len(), "one block per channel");
        // Silence into a master that has nothing left to say comes out as silence: skip the work.
        if self.at_rest() && block.iter().all(|b| b[..frames].iter().all(|s| *s == 0.0)) {
            return;
        }
        let mut frame = [0.0f32; crate::mixer::MAX_CHANNELS];
        let n = block.len();
        for i in 0..frames {
            for (f, b) in frame.iter_mut().zip(block.iter()) {
                *f = b[i];
            }
            self.process_frame(&mut frame[..n]);
            for (f, b) in frame.iter().zip(block.iter_mut()) {
                b[i] = *f;
            }
        }
    }

    /// Process all channels in place; every buffer must be the same length.
    pub fn process_linked(&mut self, bufs: &mut [&mut [f32]]) {
        assert_eq!(bufs.len(), self.channels.len(), "one buffer per channel");
        let len = bufs.first().map_or(0, |b| b.len());
        let mut frame = [0.0f32; crate::mixer::MAX_CHANNELS];
        for i in 0..len {
            for (f, b) in frame.iter_mut().zip(bufs.iter()) {
                *f = b[i];
            }
            self.process_frame(&mut frame[..bufs.len()]);
            for (f, b) in frame.iter().zip(bufs.iter_mut()) {
                b[i] = *f;
            }
        }
    }

    fn process_frame(&mut self, frame: &mut [f32]) {
        let n = self.window;
        let p = self.pos;

        let mut peak = 0.0f64;
        for (state, sample) in self.channels.iter_mut().zip(frame.iter()) {
            // Remove DC, then clean up anything that isn't a number.
            let input = f64::from(*sample);
            let input = if input.is_finite() { input } else { 0.0 };
            let x = input - state.dc_x1 + self.dc_r * state.dc_y1;
            state.dc_x1 = input;
            state.dc_y1 = x;
            state.recent.rotate_right(1);
            state.recent[0] = x;
            // The sample from `HALF` ago is the one going into the lookahead, so the stretch after it
            // can be looked at with samples from before and after.
            let sample = state.recent[HALF];
            state.delay[p] = sample;
            let between = self.between.iter().map(|w| w.iter().zip(&state.recent).map(|(w, r)| w * r).sum::<f64>().abs());
            peak = between.fold(peak.max(sample.abs()), f64::max);
        }
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

        // The oldest entry in each ring is the sample from `latency` ago.
        for (state, sample) in self.channels.iter().zip(frame.iter_mut()) {
            *sample = (state.delay[(p + 1) % n] * self.gain) as f32;
        }
        self.pos = (p + 1) % n;
    }
}

/// Weights for the signal a fraction `phase` of the way from the sample `HALF` ago to the next one,
/// from the latest `TAPS` samples: a sinc narrowed by a Hann window.
fn interpolation_weights(phase: f64) -> [f64; TAPS] {
    let mut weights = [0.0; TAPS];
    for (k, weight) in weights.iter_mut().enumerate() {
        // samples are `k` back; the point looked for is `HALF - phase` back
        let distance = k as f64 - (HALF as f64 - phase);
        let sinc = if distance == 0.0 { 1.0 } else { (std::f64::consts::PI * distance).sin() / (std::f64::consts::PI * distance) };
        let window = 0.5 * (1.0 + (std::f64::consts::PI * distance / HALF as f64).cos());
        *weight = if distance.abs() < HALF as f64 { sinc * window } else { 0.0 };
    }
    let sum: f64 = weights.iter().sum();
    weights.iter_mut().for_each(|w| *w /= sum);
    weights
}
