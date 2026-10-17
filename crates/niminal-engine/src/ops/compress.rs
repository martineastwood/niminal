//! Soft-knee feed-forward compressor with optional sidechain.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// A compressor. `threshold` and `makeup` are linear gains (the language
/// converts db). `ratio` is how hard it squeezes above the threshold;
/// `attack` and `release` are times in seconds. `sidechain` replaces the
/// detector input when wired; leave it at 0 to key off the main signal.
#[derive(Clone)]
pub struct Compressor {
    envelope: f32,
    sample_rate: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self { envelope: 0.0, sample_rate: 48_000.0 }
    }
}

impl Opcode for Compressor {
    fn reset(&mut self) {
        self.envelope = 0.0;
    }

    fn name(&self) -> &str {
        "compress"
    }

    fn ports(&self) -> &[Port] {
        // threshold default ≈ -12db as linear; makeup 1; ratio 4; attack 5ms; release 50ms.
        // sidechain defaults to NaN: "not wired, key off x". A wired sidechain replaces the detector.
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("threshold", 0.251_188_64), // -12db
            Port::optional("ratio", 4.0),
            Port::optional("attack", 0.005),
            Port::optional("release", 0.05),
            Port::optional("makeup", 1.0),
            Port::optional("sidechain", f32::NAN),
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
        let sr = self.sample_rate.max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let threshold = inputs[1][i].abs().max(1e-8);
            let ratio = inputs[2][i].max(1.0);
            let attack = inputs[3][i].max(0.0);
            let release = inputs[4][i].max(0.0);
            let makeup = inputs[5][i].max(0.0);
            let side = inputs[6][i];
            let detect = if side.is_nan() { x } else { side }.abs();

            let coeff = if detect > self.envelope {
                (-1.0 / (attack * sr).max(1.0)).exp()
            } else {
                (-1.0 / (release * sr).max(1.0)).exp()
            };
            self.envelope = detect + coeff * (self.envelope - detect);

            let gain = if self.envelope <= threshold {
                1.0
            } else {
                // linear: desired = thresh * (env/thresh)^(1/ratio); gain = desired/env
                let over = self.envelope / threshold;
                over.powf(1.0 / ratio - 1.0)
            };
            *o = x * gain * makeup;
        }
    }
}
