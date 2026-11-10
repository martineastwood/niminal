use super::*;
use crate::denormals::flush;

// Fixed quadrature allpass networks. Squaring then rounding matches the raw biquad ports.
const I: [f64; 4] = [0.6923878, 0.9360654322959, 0.9882295226860, 0.9987488452737];
const Q: [f64; 4] = [
    0.4021921162426,
    0.8561710882420,
    0.9722909545651,
    0.9952884791278,
];
#[derive(Clone)]
pub struct FreqShifter {
    sample_rate: f64,
    i: [[f64; 2]; 4],
    q: [[f64; 2]; 4],
    delayed: f64,
    phase: f64,
}
impl Default for FreqShifter {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            i: [[0.0; 2]; 4],
            q: [[0.0; 2]; 4],
            delayed: 0.0,
            phase: 0.0,
        }
    }
}
fn network(mut x: f64, state: &mut [[f64; 2]; 4], coefficients: &[f64; 4]) -> f64 {
    for (s, a) in state.iter_mut().zip(coefficients) {
        let a = f64::from((a * a) as f32);
        let y = a * x + s[0];
        s[0] = s[1];
        s[1] = flush(-x + a * y);
        // Each original biquad has an f32 output, even inside a fused kernel.
        x = f64::from(y as f32);
    }
    x
}
impl FreqShifter {
    fn input_ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("shift", 100.0)];
        PORTS
    }
    fn fresh(&self) -> Self {
        Self::default()
    }
    fn clear(&mut self) {
        self.i = [[0.0; 2]; 4];
        self.q = [[0.0; 2]; 4];
        self.delayed = 0.0;
        self.phase = 0.0;
    }
    fn carry(&mut self, old: &mut Self) {
        self.i = old.i;
        self.q = old.q;
        self.delayed = old.delayed;
        self.phase = old.phase;
    }
    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 2] {
        let input = bounded(x[0], -16.0, 16.0);
        let i = self.delayed;
        self.delayed = network(input, &mut self.i, &I);
        let q = network(input, &mut self.q, &Q);
        let hz = bounded(x[1], -2000.0, 2000.0);
        let phase = f64::from(self.phase as f32) * std::f64::consts::TAU;
        self.phase = (self.phase + hz.abs() / self.sample_rate).fract();
        let (sin, cos) = phase.sin_cos();
        let sign = if hz > 0.0 {
            1.0
        } else if hz < 0.0 {
            -1.0
        } else {
            0.0
        };
        [
            (i * cos + q * sin * sign).clamp(-1.5, 1.5) as f32,
            (i * cos - q * sin * sign).clamp(-1.5, 1.5) as f32,
        ]
    }
}
block_opcode!(FreqShifter, "freq_shifter_dsp", 2, 2);
