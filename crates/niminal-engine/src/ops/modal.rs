//! A bank of resonators at fixed ratios of one frequency.

use std::sync::Arc;

use crate::Table;
use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// One two-pole resonator for each of `ratios` times `freq`, in parallel and summed: modal synthesis of
/// bars, bells and drums. `decay` is how long every mode takes to fall by 60db, in seconds, and
/// each mode is about as loud as the input at its own frequency. Modes above half the sample rate are left
/// out.
#[derive(Clone)]
pub struct Modal {
    ratios: Arc<Table>,
    sample_rate: f32,
    s1: Vec<f64>,
    s2: Vec<f64>,
    cached_for: (f32, f32),
    /// Per mode: the gain on the input and the two feedback coefficients.
    coef: Vec<[f64; 3]>,
}

impl Modal {
    pub fn new(ratios: Arc<Table>) -> Self {
        let n = ratios.data().len();
        Modal {
            ratios,
            sample_rate: 48_000.0,
            s1: vec![0.0; n],
            s2: vec![0.0; n],
            cached_for: (f32::NAN, f32::NAN),
            coef: vec![[0.0; 3]; n],
        }
    }

    fn update(&mut self, freq: f32, decay: f32) {
        if (freq, decay) == self.cached_for {
            return;
        }
        self.cached_for = (freq, decay);
        let sr = f64::from(self.sample_rate);
        // the pole radius that falls by 60db in `decay` seconds
        let r = (-6.907_755_278_982_137 / (f64::from(decay).max(0.001) * sr)).exp();
        for (c, ratio) in self.coef.iter_mut().zip(self.ratios.data()) {
            let hz = f64::from(freq) * f64::from(*ratio);
            *c = if hz > 0.0 && hz < sr * 0.5 {
                let w = std::f64::consts::TAU * hz / sr;
                [2.0 * (1.0 - r) * w.sin(), 2.0 * r * w.cos(), r * r]
            } else {
                [0.0; 3]
            };
        }
    }
}

impl Opcode for Modal {
    fn name(&self) -> &str {
        "modal"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("freq"),
            Port::optional("decay", 0.3),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = Modal::new(self.ratios.clone());
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached_for = (f32::NAN, f32::NAN);
    }

    fn reset(&mut self) {
        self.s1.fill(0.0);
        self.s2.fill(0.0);
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && old.s1.len() == self.s1.len()
        {
            std::mem::swap(&mut self.s1, &mut old.s1);
            std::mem::swap(&mut self.s2, &mut old.s2);
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            self.update(inputs[1][i], inputs[2][i]);
            let x = f64::from(inputs[0][i]);
            let mut sum = 0.0;
            for ((c, s1), s2) in self.coef.iter().zip(&mut self.s1).zip(&mut self.s2) {
                let y = flush(x * c[0] + c[1] * *s1 - c[2] * *s2);
                *s2 = *s1;
                *s1 = y;
                sum += y;
            }
            *o = sum as f32;
        }
    }
}
