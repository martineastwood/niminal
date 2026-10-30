//! Streaming, block-independent STFT with normalized analysis/synthesis windows.
use crate::opcode::TAIL_LEVEL;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex, num_complex::Complex};
use std::sync::Arc;

pub(super) struct SpectralFrames {
    pub size: usize,
    pub hop: usize,
    forward: Arc<dyn RealToComplex<f64>>,
    inverse: Arc<dyn ComplexToReal<f64>>,
    input: Vec<f64>,
    output: Vec<f64>,
    window: Vec<f64>,
    synthesis: Vec<f64>,
    time: Vec<f64>,
    spectrum: Vec<Complex<f64>>,
    scratch_forward: Vec<Complex<f64>>,
    scratch_inverse: Vec<Complex<f64>>,
    pos: usize,
    quiet: usize,
}

impl SpectralFrames {
    pub fn new(size: usize, overlap: usize) -> Self {
        let hop = size / overlap;
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let window: Vec<_> = (0..size)
            .map(|i| 0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / size as f64).cos()))
            .collect();
        let synthesis = if overlap == 2 {
            vec![1.0; size]
        } else {
            (0..size)
                .map(|i| {
                    let weight: f64 = (0..size)
                        .step_by(hop)
                        .map(|j| window[(i + j) % size].powi(2))
                        .sum();
                    window[i] / weight
                })
                .collect()
        };
        Self {
            size,
            hop,
            scratch_forward: forward.make_scratch_vec(),
            scratch_inverse: inverse.make_scratch_vec(),
            spectrum: forward.make_output_vec(),
            forward,
            inverse,
            input: vec![0.0; size],
            output: vec![0.0; size],
            window,
            synthesis,
            time: vec![0.0; size],
            pos: 0,
            quiet: 0,
        }
    }
    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.pos = 0;
        self.quiet = 0;
    }
    pub fn active(&self) -> bool {
        self.quiet < self.size * 2
    }
    pub fn run(
        &mut self,
        input: &[f32],
        out: &mut [f32],
        mut transform: impl FnMut(&mut [Complex<f64>], usize),
    ) {
        for (i, (&x, y)) in input.iter().zip(out).enumerate() {
            *y = std::mem::take(&mut self.output[self.pos]) as f32;
            self.quiet = if y.abs() < TAIL_LEVEL {
                self.quiet.saturating_add(1)
            } else {
                0
            };
            self.input[self.pos] = x as f64;
            self.pos = (self.pos + 1) % self.size;
            if !self.pos.is_multiple_of(self.hop) {
                continue;
            }
            for k in 0..self.size {
                self.time[k] = self.input[(self.pos + k) % self.size] * self.window[k];
            }
            self.forward
                .process_with_scratch(
                    &mut self.time,
                    &mut self.spectrum,
                    &mut self.scratch_forward,
                )
                .expect("spectral analysis");
            transform(&mut self.spectrum, i);
            self.spectrum[0].im = 0.0;
            self.spectrum[self.size / 2].im = 0.0;
            self.inverse
                .process_with_scratch(
                    &mut self.spectrum,
                    &mut self.time,
                    &mut self.scratch_inverse,
                )
                .expect("spectral synthesis");
            for k in 0..self.size {
                self.output[(self.pos + k) % self.size] +=
                    self.time[k] * self.synthesis[k] / self.size as f64;
            }
        }
    }
}
