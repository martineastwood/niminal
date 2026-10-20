//! DC blocker: removes the constant offset from a signal.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// A DC blocker (one-pole highpass at ~5 Hz). Removes the constant offset
/// that accumulates in feedback loops and asymmetric waveshaping, without
/// touching anything audible.
#[derive(Clone, Default)]
pub struct DcBlock {
    x1: f64,
    y1: f64,
}

impl Opcode for DcBlock {
    fn reset(&mut self) { self.x1 = 0.0; self.y1 = 0.0; }

    fn name(&self) -> &str { "dc_block" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.x1 = old.x1;
            self.y1 = old.y1;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        // y[n] = x[n] - x[n-1] + R * y[n-1], R close to 1
        const R: f64 = 0.995;
        for (i, o) in out.iter_mut().enumerate() {
            let x = f64::from(inputs[0][i]);
            self.y1 = flush(x - self.x1 + R * self.y1);
            self.x1 = x;
            *o = self.y1 as f32;
        }
    }
}
