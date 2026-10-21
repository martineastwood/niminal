//! Envelope follower: tracks the amplitude of a signal.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Tracks the amplitude envelope of a signal with adjustable attack and
/// release times. The output is always positive: the smoothed absolute
/// value of the input.
#[derive(Clone)]
pub struct Follow {
    envelope: f64,
    sample_rate: f32,
}

impl Default for Follow {
    fn default() -> Self {
        Follow {
            envelope: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

impl Opcode for Follow {
    fn reset(&mut self) {
        self.envelope = 0.0;
    }

    fn name(&self) -> &str {
        "follow"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("attack", 0.01),
            Port::optional("release", 0.1),
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
            self.envelope = old.envelope;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate).max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            let level = f64::from(inputs[0][i]).abs();
            let coeff = if level > self.envelope {
                let attack = f64::from(inputs[1][i]).max(0.0);
                (-1.0 / (attack * sr).max(1.0)).exp()
            } else {
                let release = f64::from(inputs[2][i]).max(0.0);
                (-1.0 / (release * sr).max(1.0)).exp()
            };
            self.envelope = level + coeff * (self.envelope - level);
            *o = self.envelope as f32;
        }
    }
}
