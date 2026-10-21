//! Sample-and-hold: freeze a signal's value on each trigger.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Captures the value of `x` on each positive edge of `trigger` and holds
/// it until the next trigger. Classic for stepped random melodies
/// (`noise().sample_hold(impulse(4hz))`), staircase LFOs, and quantised
/// control signals.
#[derive(Clone, Default)]
pub struct SampleHold {
    value: f32,
    prev_trig: f32,
}

impl Opcode for SampleHold {
    fn reset(&mut self) {
        self.value = 0.0;
        self.prev_trig = 0.0;
    }

    fn name(&self) -> &str {
        "sample_hold"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::required("trigger")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.value = old.value;
            self.prev_trig = old.prev_trig;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let trig = inputs[1][i];
            if trig > 0.0 && self.prev_trig <= 0.0 {
                self.value = inputs[0][i];
            }
            self.prev_trig = trig;
            *o = self.value;
        }
    }
}
