//! Parametric EQ: low shelf, high shelf, and peaking bell.
//!
//! All three use the RBJ (Audio EQ Cookbook) biquad topology with per-sample
//! coefficient update so cutoff and gain can be modulated.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqMode {
    LowShelf,
    HighShelf,
    Peak,
}

/// A biquad parametric EQ band. `cutoff` is the centre or corner frequency;
/// `gain` is the boost/cut as a linear factor (the language converts dB);
/// `q` shapes the bandwidth (default 0.707 for shelves, 1.0 for a bell).
#[derive(Clone)]
pub struct Eq {
    mode: EqMode,
    sample_rate: f32,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    cached_for: (f32, f32, f32),
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Eq {
    pub fn new(mode: EqMode) -> Self {
        let mut eq = Eq {
            mode,
            sample_rate: 48_000.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            cached_for: (f32::NAN, f32::NAN, f32::NAN),
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        };
        eq.update(1000.0, 1.0, eq.default_q());
        eq
    }

    fn default_q(&self) -> f32 {
        match self.mode {
            EqMode::Peak => 1.0,
            _ => std::f32::consts::FRAC_1_SQRT_2,
        }
    }

    fn update(&mut self, cutoff: f32, gain: f32, q: f32) {
        if (cutoff, gain, q) == self.cached_for {
            return;
        }
        self.cached_for = (cutoff, gain, q);

        let sr = f64::from(self.sample_rate);
        let fc = f64::from(cutoff).clamp(10.0, sr * 0.45);
        let a = f64::from(gain.max(1e-6)).sqrt();
        let w0 = std::f64::consts::TAU * fc / sr;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * f64::from(q).max(0.1));

        let (b0, b1, b2, a0, a1, a2) = match self.mode {
            EqMode::LowShelf => {
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
                let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
                let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
                let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
                let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
                let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
                let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqMode::HighShelf => {
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
                let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
                let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
                let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
                let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
                let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
                let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqMode::Peak => {
                let b0 = 1.0 + alpha * a;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0 - alpha * a;
                let a0 = 1.0 + alpha / a;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha / a;
                (b0, b1, b2, a0, a1, a2)
            }
        };

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }
}

impl Opcode for Eq {
    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    fn name(&self) -> &'static str {
        match self.mode {
            EqMode::LowShelf => "low_shelf",
            EqMode::HighShelf => "high_shelf",
            EqMode::Peak => "peak_eq",
        }
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("cutoff"),
            Port::optional("gain", 1.0),
            Port::optional("q", f32::NAN), // NaN = use mode default
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Eq::new(self.mode))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached_for = (f32::NAN, f32::NAN, f32::NAN);
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.x1 = old.x1;
            self.x2 = old.x2;
            self.y1 = old.y1;
            self.y2 = old.y2;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let default_q = self.default_q();
        for (i, o) in out.iter_mut().enumerate() {
            let q = if inputs[3][i].is_nan() {
                default_q
            } else {
                inputs[3][i]
            };
            self.update(inputs[1][i], inputs[2][i], q);
            let x = f64::from(inputs[0][i]);
            let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
                - self.a1 * self.y1
                - self.a2 * self.y2;
            self.x2 = self.x1;
            self.x1 = x;
            self.y2 = flush(self.y1);
            self.y1 = flush(y);
            *o = y as f32;
        }
    }
}
