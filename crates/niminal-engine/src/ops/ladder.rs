//! A four-pole low-pass with a `tanh` in each stage, like a transistor ladder.

use crate::denormals::flush32;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// Four one-pole stages in a row, each saturating, with the output fed back to the input. `res` is how much
/// is fed back: 0 is a plain low-pass, and near 1 it rings and then sings at the cutoff. `drive` scales the
/// input, so loud sounds saturate the stages harder.
#[derive(Clone)]
pub struct Ladder {
    sample_rate: f32,
    /// The four stage outputs, and the `tanh` of each, kept so each is worked out once.
    s: [f32; 4],
    t: [f32; 4],
    /// The coefficient for the last cutoff.
    cached: (f32, f32),
}

impl Default for Ladder {
    fn default() -> Self {
        Ladder {
            sample_rate: 48_000.0,
            s: [0.0; 4],
            t: [0.0; 4],
            cached: (f32::NAN, 0.0),
        }
    }
}

impl Ladder {
    #[inline(always)]
    fn step(&mut self, x: f32, cutoff: f32, res: f32, drive: f32) -> f32 {
        if cutoff != self.cached.0 {
            let w = std::f32::consts::TAU * cutoff.max(0.0) / self.sample_rate;
            self.cached = (cutoff, 1.0 - (-w).exp());
        }
        let g = self.cached.1;
        let mut input = (x * drive - 4.0 * res * self.s[3]).tanh();
        for i in 0..4 {
            self.s[i] = flush32(self.s[i] + g * (input - self.t[i]));
            self.t[i] = self.s[i].tanh();
            input = self.t[i];
        }
        self.s[3]
    }
}

impl Opcode for Ladder {
    fn name(&self) -> &str {
        "ladder"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("cutoff"),
            Port::optional("res", 0.3),
            Port::optional("drive", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = Ladder::default();
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached = (f32::NAN, 0.0);
    }

    fn reset(&mut self) {
        self.s = [0.0; 4];
        self.t = [0.0; 4];
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.s = old.s;
            self.t = old.t;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.step(inputs[0][i], inputs[1][i], inputs[2][i], inputs[3][i]);
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(inputs[0], inputs[1], inputs[2], inputs[3])
    }
}
