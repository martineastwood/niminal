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
}

/// The per-channel parts: DC blocker memory and the lookahead delay line.
struct ChannelState {
    dc_x1: f64,
    dc_y1: f64,
    delay: Vec<f64>,
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
                .map(|_| ChannelState { dc_x1: 0.0, dc_y1: 0.0, delay: vec![0.0; window] })
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
        }
    }

    /// Clear lookahead and DC memory on panic so pre-panic audio cannot leak.
    pub fn clear(&mut self) {
        for channel in &mut self.channels {
            channel.dc_x1 = 0.0;
            channel.dc_y1 = 0.0;
            channel.delay.fill(0.0);
        }
        self.required.fill(1.0);
        self.windowed_min.fill(1.0);
        self.min_sum = self.window as f64;
        self.gain = 1.0;
        self.pos = 0;
    }

    /// Samples of delay between input and output.
    pub fn latency(&self) -> usize {
        self.window - 1
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
            state.delay[p] = x;
            peak = peak.max(x.abs());
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
