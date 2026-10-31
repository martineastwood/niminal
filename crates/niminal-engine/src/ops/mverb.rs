//! Stereo figure-of-eight reverb with input diffusion and early reflections.

use crate::denormals::flush32 as flush;
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

const PORTS: &[Port] = &[
    Port::required("x"),
    Port::optional("right", f32::NAN),
    Port::optional("damp", 0.0),
    Port::optional("density", 0.5),
    Port::optional("bandwidth", 1.0),
    Port::optional("decay", 0.5),
    Port::optional("predelay", 0.0),
    Port::optional("size", 0.5),
    Port::optional("gain", 1.0),
    Port::optional("mix", 1.0),
    Port::optional("early_mix", 0.75),
];

/// A delay whose read taps advance together. Tap positions are forward offsets
/// from the write head, matching the tank's output tap geometry.
#[derive(Clone)]
struct Delay<const N: usize> {
    buffer: Vec<f32>,
    indices: [usize; N],
    length: usize,
    peak: f32,
}

impl<const N: usize> Delay<N> {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity.max(1)],
            indices: [0; N],
            length: 1,
            peak: 0.0,
        }
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.indices.fill(0);
        self.peak = 0.0;
    }

    fn configure(&mut self, length: usize, taps: [usize; N]) {
        self.clear();
        self.length = length.clamp(1, self.buffer.len());
        self.indices = taps.map(|i| i.min(self.length - 1));
    }

    fn tap(&self, index: usize) -> f32 {
        self.buffer[self.indices[index]]
    }

    fn step(&mut self, input: f32, feedback: f32) -> f32 {
        let delayed = self.tap(0);
        let out = delayed - input * feedback;
        let stored = flush(input + out * feedback);
        self.buffer[self.indices[0]] = stored;
        self.peak = stored.abs();
        for index in &mut self.indices {
            *index += 1;
            if *index >= self.length {
                *index = 0;
            }
        }
        out
    }
}

/// Four oversampled state-variable lowpass steps, with critical damping.
#[derive(Clone, Default)]
struct Lowpass {
    low: f32,
    band: f32,
    coefficient: f32,
}

impl Lowpass {
    fn tune(&mut self, hz: f32, sr: f32) {
        // Keep the original filter stable at low device sample rates too.
        self.coefficient = 2.0 * (std::f32::consts::PI * hz.min(sr * 0.49) / (sr * 4.0)).sin();
    }

    fn step(&mut self, input: f32) -> f32 {
        for _ in 0..4 {
            self.low += self.coefficient * self.band;
            let high = input - self.low - 2.0 * self.band;
            self.band += self.coefficient * high;
        }
        self.low = flush(self.low);
        self.band = flush(self.band);
        self.low
    }
}

/// A single-pole lowpass for the feedback loop, as in the FDN reverb.
#[derive(Clone, Default)]
struct Damping {
    low: f32,
    pole: f32,
}

impl Damping {
    fn tune(&mut self, hz: f32, sr: f32) {
        self.pole = (-std::f32::consts::TAU * hz.min(sr * 0.45) / sr).exp();
    }

    fn step(&mut self, input: f32) -> f32 {
        self.low = flush(self.low * self.pole + input * (1.0 - self.pole));
        self.low
    }
}

/// One shared stereo tank. Mono input excites both sides; output is always stereo.
#[derive(Clone)]
pub struct Mverb {
    sr: f32,
    diffusers: [Delay<1>; 4],
    allpasses: [Delay<4>; 4],
    tank: [Delay<4>; 4],
    early: [Delay<8>; 2],
    predelay: Delay<1>,
    bandwidth: [Lowpass; 2],
    damping: [Damping; 2],
    previous: [f32; 2],
    size: f32,
    // mix, late fraction, bandwidth Hz, damping Hz, predelay samples, decay, density, gain.
    smooth: [f32; 8],
    smoothing: f32,
    initialized: bool,
    control_counter: usize,
    quiet: usize,
    tail_window: usize,
}

impl Default for Mverb {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl Mverb {
    fn new(sr: f32) -> Self {
        let rate = f64::from(sr);
        let samples = |seconds: f64| (seconds * rate) as usize + 1;
        let mut this = Self {
            sr,
            diffusers: [0.0048, 0.0036, 0.0127, 0.0093].map(|t| Delay::new(samples(t))),
            allpasses: [0.020, 0.060, 0.030, 0.089].map(|t| Delay::new(samples(t))),
            tank: [0.15, 0.12, 0.14, 0.11].map(|t| Delay::new(samples(t))),
            early: [0.089, 0.069].map(|t| Delay::new(samples(t))),
            predelay: Delay::new(samples(0.2)),
            bandwidth: std::array::from_fn(|_| Lowpass::default()),
            damping: std::array::from_fn(|_| Damping::default()),
            previous: [0.0; 2],
            size: -1.0,
            smooth: [0.0; 8],
            // Like lag(time: 10ms): 60 dB convergence in ten milliseconds.
            smoothing: 1.0 - (-6.907_755 / (0.01 * sr)).exp(),
            initialized: false,
            control_counter: 0,
            quiet: 0,
            tail_window: samples(1.5),
        };
        for (delay, time) in this
            .diffusers
            .iter_mut()
            .zip([0.0048, 0.0036, 0.0127, 0.0093])
        {
            delay.configure((time * rate) as usize, [0]);
        }
        this.early[0].configure(
            (0.089 * rate) as usize,
            [0.0, 0.0199, 0.0219, 0.0354, 0.0389, 0.0414, 0.0692, 0.0].map(|t| (t * rate) as usize),
        );
        this.early[1].configure(
            (0.069 * rate) as usize,
            [0.0, 0.0099, 0.011, 0.0182, 0.0189, 0.0213, 0.0431, 0.0].map(|t| (t * rate) as usize),
        );
        this.set_size(0.5);
        this.quiet = this.tail_window;
        this
    }

    fn set_size(&mut self, size: f32) {
        self.size = size;
        // Preserve the original truncation to integer delay lengths, including
        // its double-precision calculation before storing normalized size as f32.
        let internal_size = (0.95 * f64::from(size) + 0.05) as f32;
        let samples = |t: f64| (t * f64::from(self.sr) * f64::from(internal_size)) as usize;
        for (i, (time, taps)) in [
            (0.020, [0.0, 0.0, 0.0, 0.0]),
            (0.060, [0.0, 0.006, 0.041, 0.0]),
            (0.030, [0.0, 0.0, 0.0, 0.0]),
            (0.089, [0.0, 0.031, 0.011, 0.0]),
        ]
        .into_iter()
        .enumerate()
        {
            self.allpasses[i].configure(samples(time), taps.map(samples));
        }
        for (i, (time, taps)) in [
            (0.15, [0.0, 0.067, 0.011, 0.121]),
            (0.12, [0.0, 0.036, 0.089, 0.0]),
            (0.14, [0.0, 0.0089, 0.099, 0.0]),
            (0.11, [0.0, 0.067, 0.0041, 0.0]),
        ]
        .into_iter()
        .enumerate()
        {
            self.tank[i].configure(samples(time), taps.map(samples));
        }
    }

    fn frame(&mut self, p: &[f32]) -> [f32; 2] {
        let left = p[0];
        let right = if p[1].is_nan() { left } else { p[1] };
        let size = p[7].clamp(0.0, 1.0);
        if size != self.size {
            self.set_size(size);
        }
        let target = [
            p[9].clamp(0.0, 1.0),
            p[10].clamp(0.0, 1.0),
            p[4].clamp(0.0, 1.0) * 18400.0 + 100.0,
            (1.0 - p[2].clamp(0.0, 1.0)) * 18400.0 + 100.0,
            p[6].clamp(0.0, 0.2) * self.sr,
            0.7995 * p[5].clamp(0.0, 1.0) + 0.005,
            p[3].clamp(0.0, 1.0),
            p[8].max(0.0),
        ];
        if !self.initialized {
            self.smooth = target;
            self.initialized = true;
            self.control_counter = crate::opcode::control_step();
        } else {
            for (state, target) in self.smooth.iter_mut().zip(target) {
                *state += self.smoothing * (target - *state);
            }
        }
        if self.control_counter >= crate::opcode::control_step() {
            for f in &mut self.bandwidth {
                f.tune(self.smooth[2], self.sr);
            }
            for f in &mut self.damping {
                f.tune(self.smooth[3], self.sr);
            }
            self.control_counter = 0;
        }
        self.control_counter += 1;
        self.predelay.length = (self.smooth[4] as usize).clamp(1, self.predelay.buffer.len());
        let b = [self.bandwidth[0].step(left), self.bandwidth[1].step(right)];
        let mut early = [0.0; 2];
        for (side, out) in early.iter_mut().enumerate() {
            let (own, other) = (b[side], b[1 - side]);
            let line = &mut self.early[side];
            *out = line.step(own * 0.5 + other * 0.3, 0.0)
                + line.tap(2) * 0.6
                + line.tap(3) * 0.4
                + line.tap(4) * 0.3
                + line.tap(5) * 0.3
                + line.tap(6) * 0.1
                + line.tap(7) * 0.1
                + (own * 0.4 + other * 0.2) * 0.5;
        }
        let mut input = self.predelay.step((b[0] + b[1]) * 0.5, 0.0);
        for (line, feedback) in self.diffusers.iter_mut().zip([0.75, 0.75, 0.625, 0.625]) {
            input = line.step(input, feedback);
        }
        let density2 = (self.smooth[5] + 0.15).clamp(0.25, 0.5);
        let mut tanks = [0.0; 2];
        for (side, out) in tanks.iter_mut().enumerate() {
            let i = side * 2;
            let a = self.allpasses[i].step(input + self.previous[1 - side], self.smooth[6]);
            let d = self.tank[i].step(a, 0.0);
            let f = self.damping[side].step(d);
            let a = self.allpasses[i + 1].step(f, density2);
            *out = self.tank[i + 1].step(a, 0.0);
        }
        self.previous = tanks.map(|x| flush(x * self.smooth[5]));
        let late = [
            0.6 * self.tank[2].tap(1) + 0.6 * self.tank[2].tap(2) - 0.6 * self.allpasses[3].tap(1)
                + 0.6 * self.tank[3].tap(1)
                - 0.6 * self.tank[0].tap(1)
                - 0.6 * self.allpasses[1].tap(1)
                - 0.6 * self.tank[1].tap(1),
            0.6 * self.tank[0].tap(2) + 0.6 * self.tank[0].tap(3) - 0.6 * self.allpasses[1].tap(2)
                + 0.6 * self.tank[1].tap(2)
                - 0.6 * self.tank[2].tap(3)
                - 0.6 * self.allpasses[3].tap(2)
                - 0.6 * self.tank[3].tap(2),
        ];
        let peak = self
            .diffusers
            .iter()
            .map(|d| d.peak)
            .chain(self.allpasses.iter().map(|d| d.peak))
            .chain(self.tank.iter().map(|d| d.peak))
            .chain(self.early.iter().map(|d| d.peak))
            .chain([self.predelay.peak, left.abs(), right.abs()])
            .chain(
                self.bandwidth
                    .iter()
                    .flat_map(|f| [f.low.abs(), f.band.abs()])
                    .chain(self.damping.iter().map(|f| f.low.abs())),
            )
            .fold(0.0_f32, f32::max);
        self.quiet = if peak * self.smooth[7].max(1.0) < TAIL_LEVEL / 64.0 {
            self.quiet.saturating_add(1)
        } else {
            0
        };
        std::array::from_fn(|i| {
            let dry = [left, right][i];
            let wet = late[i] * self.smooth[1] + early[i] * (1.0 - self.smooth[1]);
            (dry + self.smooth[0] * (wet - dry)) * self.smooth[7]
        })
    }
}

impl Opcode for Mverb {
    fn name(&self) -> &str {
        "mverb"
    }
    fn ports(&self) -> &[Port] {
        PORTS
    }
    fn outputs(&self) -> usize {
        2
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.sr))
    }
    fn prepare(&mut self, sr: f32) {
        *self = Self::new(sr.max(1.0));
    }
    fn reset(&mut self) {
        for d in &mut self.diffusers {
            d.clear();
        }
        for d in &mut self.allpasses {
            d.clear();
        }
        for d in &mut self.tank {
            d.clear();
        }
        for d in &mut self.early {
            d.clear();
        }
        self.predelay.clear();
        self.bandwidth.fill(Lowpass::default());
        self.damping.fill(Damping::default());
        self.previous = [0.0; 2];
        self.initialized = false;
        self.control_counter = 0;
        // Restore the output tap positions, which clear() rewinds.
        self.set_size(self.size);
        let rate = f64::from(self.sr);
        self.early[0].indices =
            [0.0, 0.0199, 0.0219, 0.0354, 0.0389, 0.0414, 0.0692, 0.0].map(|t| (t * rate) as usize);
        self.early[1].indices =
            [0.0, 0.0099, 0.011, 0.0182, 0.0189, 0.0213, 0.0431, 0.0].map(|t| (t * rate) as usize);
        self.quiet = self.tail_window;
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.sr == old.sr
        {
            std::mem::swap(self, old);
        }
    }
    fn is_active(&self) -> bool {
        self.quiet < self.tail_window
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let mut p = [0.0; PORTS.len()];
        for (i, out) in out.iter_mut().enumerate() {
            for (v, input) in p.iter_mut().zip(inputs) {
                *v = input[i];
            }
            *out = self.frame(&p)[0];
        }
    }
    fn process_outputs(
        &mut self,
        _ctx: &ProcessCtx,
        inputs: &[&[f32]],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        let mut p = [0.0; PORTS.len()];
        for i in 0..frames {
            for (v, input) in p.iter_mut().zip(inputs) {
                *v = input[i];
            }
            let sample = self.frame(&p);
            for (out, sample) in outs.iter_mut().zip(sample) {
                out[i] = sample;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls() -> [f32; 11] {
        [0.0, 0.0, 0.3, 0.5, 0.8, 0.7, 0.02, 0.5, 1.0, 1.0, 0.75]
    }

    #[test]
    fn impulse_matches_the_one_pole_tank_reference_at_different_rates_and_sizes() {
        for (sr, size, expected) in [
            (
                44100.0,
                0.25,
                include_str!("../../tests/fixtures/mverb_44100.txt"),
            ),
            (
                48000.0,
                0.5,
                include_str!("../../tests/fixtures/mverb_48000.txt"),
            ),
            (
                96000.0,
                0.9,
                include_str!("../../tests/fixtures/mverb_96000.txt"),
            ),
        ] {
            let mut samples = expected
                .split_whitespace()
                .map(|s| s.parse::<f32>().unwrap());
            let mut r = Mverb::new(sr);
            let mut p = controls();
            p[7] = size;
            for _ in 0..6400 {
                r.frame(&p);
            }
            let frames = (sr as usize * 2 / 64) * 64;
            let stride = frames / 128 + 1;
            for i in 0..frames {
                p[0] = if i == 0 { 1.0 } else { 0.0 };
                p[1] = if i == 0 { -0.3 } else { 0.0 };
                let out = r.frame(&p);
                if i % stride == 0 {
                    for value in out {
                        let expected = samples.next().unwrap();
                        assert!(
                            (value - expected).abs() < 1e-6,
                            "{sr} Hz, sample {i}: {value} != {expected}"
                        );
                    }
                }
            }
            assert!(samples.next().is_none());
        }
    }

    #[test]
    fn stereo_tail_drains_and_reset_restores_the_impulse() {
        let mut r = Mverb::default();
        assert!(!r.is_active());
        let mut p = controls();
        p[0] = 1.0;
        let first = r.frame(&p);
        let mut response = vec![first];
        assert!(r.is_active());
        p[0] = 0.0;
        let mut different = false;
        let mut late_energy = 0.0;
        for i in 0..480_000 {
            let out = r.frame(&p);
            if i < 12000 {
                response.push(out);
            }
            assert!(out.iter().all(|x| x.is_finite()));
            different |= (out[0] - out[1]).abs() > 1e-5;
            if (4800..48000).contains(&i) {
                late_energy += out[0] * out[0];
            }
        }
        assert!(different && late_energy > 1e-4);
        assert!(!r.is_active());
        r.reset();
        p[0] = 1.0;
        assert_eq!(first, r.frame(&p));
        p[0] = 0.0;
        for expected in &response[1..] {
            assert_eq!(*expected, r.frame(&p));
        }
        let mut carried = Mverb::default();
        let mut expected = r.clone();
        carried.carry_state(&mut r);
        for _ in 0..1000 {
            assert_eq!(carried.frame(&p), expected.frame(&p));
        }
    }

    #[test]
    fn dry_stereo_and_mono_inputs_preserve_their_channels() {
        let mut p = controls();
        p[0] = 0.75;
        p[1] = -0.25;
        p[9] = 0.0;
        let mut r = Mverb::default();
        assert_eq!(r.frame(&p), [0.75, -0.25]);
        p[1] = f32::NAN;
        assert_eq!(r.frame(&p), [0.75; 2]);
    }

    #[test]
    fn damping_reduces_high_frequency_energy_in_the_late_tail() {
        let energy = |damp| {
            let mut r = Mverb::default();
            let mut p = controls();
            p[2] = damp;
            p[10] = 1.0;
            let mut last = [0.0; 2];
            let mut high = 0.0;
            for i in 0..96000 {
                p[0] = if i == 0 { 1.0 } else { 0.0 };
                let out = r.frame(&p);
                if i > 24000 {
                    for side in 0..2 {
                        high += (out[side] - last[side]).powi(2);
                    }
                }
                last = out;
            }
            high
        };
        let (dark, bright) = (energy(1.0), energy(0.0));
        assert!(dark < bright * 0.1, "dark: {dark}, bright: {bright}");
    }

    #[test]
    fn extreme_controls_remain_finite_at_device_sample_rates() {
        for sr in [8000.0, 44100.0, 48000.0, 96000.0, 192000.0] {
            for extreme in [0.0, 1.0] {
                let mut r = Mverb::new(sr);
                let mut p = controls();
                p[2] = extreme;
                p[3] = extreme;
                p[4] = 1.0;
                p[5] = extreme;
                p[6] = 0.2;
                p[7] = extreme;
                p[0] = 1.0;
                for i in 0..sr as usize {
                    if i > 0 {
                        p[0] = 0.0;
                    }
                    assert!(r.frame(&p).iter().all(|x| x.is_finite() && x.abs() < 10.0));
                }
            }
        }
    }
}
