//! Two-pole, two-zero filter sections.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiquadMode {
    /// The five coefficients are inputs: `biquad(x, b0, b1, b2, a1, a2)`, with `a0` already divided out.
    Raw,
    Low,
    High,
    Band,
    Notch,
    All,
}

/// One biquad section. The named modes work out their coefficients from `cutoff` and `q` (the standard
/// cookbook shapes) only when those change, using a trapezoidal SVF realization
/// for modulation. `Raw` takes the coefficients as they are.
#[derive(Clone)]
pub struct Biquad {
    mode: BiquadMode,
    sample_rate: f32,
    // Raw: transposed direct form II. Named: trapezoidal integrator memories.
    z1: f64,
    z2: f64,
    cached_for: (f32, f32),
    c: [f64; 5],
    // Named sections use a TPT state variable topology for cutoff modulation.
    svf: [f64; 4],
}

impl Biquad {
    pub fn new(mode: BiquadMode) -> Self {
        Biquad {
            mode,
            sample_rate: 48_000.0,
            z1: 0.0,
            z2: 0.0,
            cached_for: (f32::NAN, f32::NAN),
            c: [1.0, 0.0, 0.0, 0.0, 0.0],
            svf: [0.0; 4],
        }
    }

    fn update(&mut self, cutoff: f32, q: f32) {
        if (cutoff, q) == self.cached_for {
            return;
        }
        self.cached_for = (cutoff, q);
        let sr = f64::from(self.sample_rate);
        let fc = f64::from(cutoff).clamp(1.0, sr * 0.49);
        let g = (std::f64::consts::PI * fc / sr).tan();
        let k = 1.0 / f64::from(q).max(0.01);
        let a1 = 1.0 / (1.0 + g * (g + k));
        self.svf = [a1, g * a1, g * g * a1, k];
    }

    #[inline(always)]
    fn run(&mut self, x: f64) -> f32 {
        let [b0, b1, b2, a1, a2] = self.c;
        let y = b0 * x + self.z1;
        self.z1 = flush(b1 * x - a1 * y + self.z2);
        self.z2 = flush(b2 * x - a2 * y);
        y as f32
    }

    #[inline(always)]
    fn step(&mut self, inputs: &[f32]) -> f32 {
        if self.mode == BiquadMode::Raw {
            self.c = [
                f64::from(inputs[1]),
                f64::from(inputs[2]),
                f64::from(inputs[3]),
                f64::from(inputs[4]),
                f64::from(inputs[5]),
            ];
        } else {
            self.update(inputs[1], inputs[2]);
            let [a1, a2, a3, k] = self.svf;
            let x = f64::from(inputs[0]);
            let v3 = x - self.z2;
            let band = a1 * self.z1 + a2 * v3;
            let low = self.z2 + a2 * self.z1 + a3 * v3;
            self.z1 = flush(2.0 * band - self.z1);
            self.z2 = flush(2.0 * low - self.z2);
            return match self.mode {
                BiquadMode::Low => low,
                BiquadMode::High => x - k * band - low,
                BiquadMode::Band => k * band,
                BiquadMode::Notch => x - k * band,
                BiquadMode::All => x - 2.0 * k * band,
                BiquadMode::Raw => unreachable!(),
            } as f32;
        }
        self.run(f64::from(inputs[0]))
    }
}

impl Opcode for Biquad {
    fn name(&self) -> &str {
        match self.mode {
            BiquadMode::Raw => "biquad",
            BiquadMode::Low => "biquad_lp",
            BiquadMode::High => "biquad_hp",
            BiquadMode::Band => "biquad_bp",
            BiquadMode::Notch => "biquad_notch",
            BiquadMode::All => "biquad_allpass",
        }
    }

    fn ports(&self) -> &[Port] {
        const RAW: &[Port] = &[
            Port::required("x"),
            Port::required("b0"),
            Port::required("b1"),
            Port::required("b2"),
            Port::required("a1"),
            Port::required("a2"),
        ];
        const NAMED: &[Port] = &[
            Port::required("x"),
            Port::required("cutoff"),
            Port::optional("q", std::f32::consts::FRAC_1_SQRT_2),
        ];
        if self.mode == BiquadMode::Raw {
            RAW
        } else {
            NAMED
        }
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = Biquad::new(self.mode);
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached_for = (f32::NAN, f32::NAN);
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && old.mode == self.mode
        {
            self.z1 = old.z1;
            self.z2 = old.z2;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let mut one = [0.0f32; 6];
        for (i, o) in out.iter_mut().enumerate() {
            for (slot, input) in one.iter_mut().zip(inputs) {
                *slot = input[i];
            }
            *o = self.step(&one);
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(inputs)
    }
}
