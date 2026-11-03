//! Parametric EQ: low shelf, high shelf, and peaking bell.
//!
//! RBJ cookbook responses realized as trapezoidal state variable filters,
//! so cutoff and gain can be modulated without direct-form state transients.

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
    ic1: f64,
    ic2: f64,
    cached_for: (f32, f32, f32),
    a1: f64,
    a2: f64,
    a3: f64,
    mix: [f64; 3],
}

impl Eq {
    pub fn new(mode: EqMode) -> Self {
        let mut eq = Eq {
            mode,
            sample_rate: 48_000.0,
            ic1: 0.0,
            ic2: 0.0,
            cached_for: (f32::NAN, f32::NAN, f32::NAN),
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            mix: [1.0, 0.0, 0.0],
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
        let mut g = (std::f64::consts::PI * fc / sr).tan();
        let q = f64::from(q).max(0.1);
        let k = match self.mode {
            EqMode::Peak => 1.0 / (q * a),
            _ => 1.0 / q,
        };
        self.mix = match self.mode {
            EqMode::LowShelf => {
                g /= a.sqrt();
                [1.0, k * (a - 1.0), a * a - 1.0]
            }
            EqMode::HighShelf => {
                g *= a.sqrt();
                [a * a, k * (1.0 - a) * a, 1.0 - a * a]
            }
            EqMode::Peak => [1.0, k * (a * a - 1.0), 0.0],
        };
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
}

impl Opcode for Eq {
    fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
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
            self.ic1 = old.ic1;
            self.ic2 = old.ic2;
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
            let v3 = x - self.ic2;
            let band = self.a1 * self.ic1 + self.a2 * v3;
            let low = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
            self.ic1 = flush(2.0 * band - self.ic1);
            self.ic2 = flush(2.0 * low - self.ic2);
            let y = self.mix[0] * x + self.mix[1] * band + self.mix[2] * low;
            *o = y as f32;
        }
    }
}
