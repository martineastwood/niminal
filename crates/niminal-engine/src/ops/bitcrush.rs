//! Sample-rate and bit-depth reduction.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Holds samples and quantizes amplitude. `rate` is how many times a second a
/// new sample is taken (defaults to the audio rate, so no reduction); `bits`
/// is the bit depth (defaults to 16).
#[derive(Clone)]
pub struct Bitcrush {
    held: f32,
    phase: f32,
    sample_rate: f32,
}

impl Default for Bitcrush {
    fn default() -> Self {
        Self { held: 0.0, phase: 0.0, sample_rate: 48_000.0 }
    }
}

impl Opcode for Bitcrush {
    fn reset(&mut self) {
        self.held = 0.0;
        self.phase = 0.0;
    }

    fn name(&self) -> &str {
        "bitcrush"
    }

    fn ports(&self) -> &[Port] {
        // rate in hz; bits as a plain number
        // rate defaults above any practical sample rate so the hold never triggers
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("rate", 1_000_000.0),
            Port::optional("bits", 16.0),
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
            self.held = old.held;
            self.phase = old.phase;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = self.sample_rate.max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            let rate = inputs[1][i].clamp(1.0, sr);
            let bits = inputs[2][i].clamp(1.0, 32.0);
            let step = rate / sr;
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.held = inputs[0][i];
            }
            let levels = (2.0f32).powf(bits) - 1.0;
            // quantize the held sample into the bit grid
            let q = (self.held.clamp(-1.0, 1.0) * 0.5 + 0.5) * levels;
            *o = (q.round() / levels) * 2.0 - 1.0;
        }
    }
}
