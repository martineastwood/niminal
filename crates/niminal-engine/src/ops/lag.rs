//! One-pole exponential smoothing for control signals.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// Exponential smoothing: `lag(x, time: 10ms)` slides toward `x` with a
/// 60dB convergence time of `time` seconds. Use it to stop parameter
/// changes from clicking.
#[derive(Clone)]
pub struct Lag {
    y: f64,
    sample_rate: f32,
}

impl Default for Lag {
    fn default() -> Self {
        Lag { y: 0.0, sample_rate: 48_000.0 }
    }
}

impl Opcode for Lag {
    fn reset(&mut self) { self.y = 0.0; }

    fn name(&self) -> &str { "lag" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("time"),
        ];
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
            self.y = old.y;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let x = f64::from(inputs[0][i]);
            let time = f64::from(inputs[1][i]).max(0.0);
            // coefficient for 60dB convergence in `time` seconds
            let coeff = if time * sr < 1.0 { 0.0 } else { (-6.907_755_278_982_137 / (time * sr)).exp() };
            self.y = flush(x + coeff * (self.y - x));
            *o = self.y as f32;
        }
    }
}
