//! Impulse train generator.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// A single-sample impulse at a given rate: `impulse(4hz)` emits 1.0 on
/// one sample every quarter-second and 0.0 everywhere else. The first
/// sample is always a trigger. Use it to drive `sample_hold`, `reset_on`,
/// or any trigger input.
#[derive(Clone, Default)]
pub struct Impulse {
    phase: f64,
    sample_rate: f32,
}

impl Opcode for Impulse {
    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn name(&self) -> &str {
        "impulse"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("freq")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.phase = old.phase;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let freq = f64::from(inputs[0][i]).max(0.0);
            let inc = freq / sr;
            let prev = self.phase;
            self.phase += inc;
            // trigger when phase wraps past 1.0, or on the very first sample
            if self.phase >= 1.0 || (prev == 0.0 && i == 0) {
                self.phase -= self.phase.floor();
                *o = 1.0;
            } else {
                *o = 0.0;
            }
        }
    }
}
