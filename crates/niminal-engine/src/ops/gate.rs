//! Noise gate.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// A noise gate: silences the signal when its level drops below
/// `threshold` (linear; the language converts dB). `attack` is how fast
/// the gate opens, `release` how fast it closes, and `range` is the
/// attenuation in the closed state (0 = full silence, 1 = no gating).
#[derive(Clone)]
pub struct Gate {
    envelope: f64,
    gain: f64,
    sample_rate: f32,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            envelope: 0.0,
            gain: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

impl Opcode for Gate {
    fn reset(&mut self) {
        self.envelope = 0.0;
        self.gain = 0.0;
    }

    fn name(&self) -> &str {
        "gate"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("threshold", 0.01),
            Port::optional("attack", 0.001),
            Port::optional("release", 0.05),
            Port::optional("range", 0.0),
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
            self.gain = old.gain;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate).max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let threshold = f64::from(inputs[1][i]).abs().max(1e-10);
            let attack = f64::from(inputs[2][i]).max(0.0);
            let release = f64::from(inputs[3][i]).max(0.0);
            let range = f64::from(inputs[4][i]).clamp(0.0, 1.0);

            let level = f64::from(x).abs();
            let det_coeff = if level > self.envelope {
                (-1.0 / (0.001 * sr).max(1.0)).exp()
            } else {
                (-1.0 / (0.01 * sr).max(1.0)).exp()
            };
            self.envelope = level + det_coeff * (self.envelope - level);

            let target = if self.envelope > threshold {
                1.0
            } else {
                range
            };
            let coeff = if target > self.gain {
                (-1.0 / (attack * sr).max(1.0)).exp()
            } else {
                (-1.0 / (release * sr).max(1.0)).exp()
            };
            self.gain = target + coeff * (self.gain - target);

            *o = x * self.gain as f32;
        }
    }
}
