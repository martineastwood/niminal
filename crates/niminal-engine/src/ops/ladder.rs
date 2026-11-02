//! Four saturating trapezoidal one-poles with same-sample resonance feedback.
//! Runs at the session rate; oversampling is an explicit choice in the language.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// Four one-pole stages in a row, each saturating, with the output fed back to the input. `res` is how much
/// is fed back: 0 is a plain low-pass, and near 1 it rings and then sings at the cutoff. `drive` scales the
/// input, so loud sounds saturate the stages harder.
#[derive(Clone)]
pub struct Ladder {
    sample_rate: f32,
    /// Trapezoidal integrator memories.
    s: [f64; 4],
    /// Last solved stage outputs, used to start the next Newton solve.
    y: [f64; 4],
    /// The coefficient for the last cutoff.
    cached: (f32, f64),
}

impl Default for Ladder {
    fn default() -> Self {
        Ladder {
            sample_rate: 48_000.0,
            s: [0.0; 4],
            y: [0.0; 4],
            cached: (f32::NAN, 0.0),
        }
    }
}

impl Ladder {
    #[inline(always)]
    fn step(&mut self, x: f32, cutoff: f32, res: f32, drive: f32) -> f32 {
        let cutoff = if cutoff.is_finite() { cutoff } else { 0.0 };
        if cutoff != self.cached.0 {
            let fc = f64::from(cutoff).clamp(0.0, f64::from(self.sample_rate) * 0.45);
            let g = (std::f64::consts::PI * fc / f64::from(self.sample_rate)).tan();
            self.cached = (cutoff, g);
        }
        let g = self.cached.1;
        let k = 4.0 * f64::from(res).clamp(0.0, 1.0);
        let driven = f64::from(x) * f64::from(drive);
        if !driven.is_finite() || !k.is_finite() {
            self.s.fill(0.0);
            self.y.fill(0.0);
            return 0.0;
        }
        // Nonlinear trapezoidal integrators:
        // y[i] = s[i] + g * (tanh(input[i]) - tanh(y[i])).
        // Solve all four stages and the resonance loop together. The Jacobian
        // is a chain with one feedback edge, so elimination needs no matrix.
        let residual = |y: [f64; 4]| {
            let t = y.map(f64::tanh);
            let input = (driven - k * y[3]).tanh();
            let previous = [input, t[0], t[1], t[2]];
            let error: [f64; 4] =
                std::array::from_fn(|i| y[i] - self.s[i] - g * (previous[i] - t[i]));
            (error, t, input)
        };
        let mut y = self.y;
        for _ in 0..8 {
            let (error, t, input) = residual(y);
            let norm = error.iter().fold(0.0_f64, |m, e| m.max(e.abs()));
            if norm < 1e-9 {
                break;
            }
            let d = t.map(|t| 1.0 - t * t);
            let denominator = d.map(|d| 1.0 + g * d);
            let a: [f64; 4] = std::array::from_fn(|i| {
                if i == 0 {
                    -k * g * (1.0 - input * input) / denominator[0]
                } else {
                    g * d[i - 1] / denominator[i]
                }
            });
            let b: [f64; 4] = std::array::from_fn(|i| -error[i] / denominator[i]);
            let chain = a[3] * a[2] * a[1];
            let last =
                (chain * b[0] + a[3] * a[2] * b[1] + a[3] * b[2] + b[3]) / (1.0 - chain * a[0]);
            let mut delta = [0.0; 4];
            delta[0] = a[0] * last + b[0];
            for i in 1..4 {
                delta[i] = a[i] * delta[i - 1] + b[i];
            }
            // Backtracking prevents an extreme drive/control jump from throwing
            // Newton into the flat part of the opposite saturator.
            let mut scale = 1.0;
            for _ in 0..8 {
                let candidate = std::array::from_fn(|i| y[i] + scale * delta[i]);
                let (next, _, _) = residual(candidate);
                if next.iter().all(|e| e.abs() <= norm) {
                    y = candidate;
                    break;
                }
                scale *= 0.5;
            }
        }
        for (state, output) in self.s.iter_mut().zip(y) {
            *state = flush(2.0 * output - *state);
        }
        self.y = y;
        y[3] as f32
    }
}

impl Opcode for Ladder {
    fn name(&self) -> &str {
        "ladder"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("cutoff"),
            Port::optional("res", 0.3),
            Port::optional("drive", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = Ladder::default();
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached = (f32::NAN, 0.0);
    }

    fn reset(&mut self) {
        self.s = [0.0; 4];
        self.y = [0.0; 4];
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.s = old.s;
            self.y = old.y;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.step(inputs[0][i], inputs[1][i], inputs[2][i], inputs[3][i]);
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(inputs[0], inputs[1], inputs[2], inputs[3])
    }
}
