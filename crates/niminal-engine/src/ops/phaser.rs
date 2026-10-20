//! A phaser: a chain of allpass stages whose frequency is swept by an LFO.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

const NUM_STAGES: usize = 6;

/// A phaser: `rate` is the sweep LFO in hz, `depth` scales the sweep
/// range, `feedback` feeds the output back into the chain, and `mix`
/// blends wet with dry. The sweep range spans from `center` down and up
/// by `depth` octaves.
#[derive(Clone)]
pub struct Phaser {
    sample_rate: f32,
    phase: f64,
    stages: [f64; NUM_STAGES],
    feedback_state: f64,
}

impl Default for Phaser {
    fn default() -> Self {
        Phaser {
            sample_rate: 48_000.0,
            phase: 0.0,
            stages: [0.0; NUM_STAGES],
            feedback_state: 0.0,
        }
    }
}

impl Opcode for Phaser {
    fn reset(&mut self) {
        self.phase = 0.0;
        self.stages = [0.0; NUM_STAGES];
        self.feedback_state = 0.0;
    }

    fn name(&self) -> &str { "phaser" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("rate", 0.3),
            Port::optional("depth", 0.5),
            Port::optional("center", 1000.0),
            Port::optional("feedback", 0.5),
            Port::optional("mix", 0.5),
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
            self.phase = old.phase;
            self.stages = old.stages;
            self.feedback_state = old.feedback_state;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let dry = f64::from(inputs[0][i]);
            let rate = f64::from(inputs[1][i]).max(0.0);
            let depth = f64::from(inputs[2][i]).clamp(0.0, 1.0);
            let center = f64::from(inputs[3][i]).clamp(20.0, sr * 0.45);
            let feedback = f64::from(inputs[4][i]).clamp(-0.95, 0.95);
            let mix = f64::from(inputs[5][i]).clamp(0.0, 1.0);

            // LFO: triangle for smoother sweep
            let lfo = (self.phase * 4.0 - 1.0).abs() * 2.0 - 1.0; // -1..1 triangle
            self.phase = (self.phase + rate / sr).fract();

            // sweep the allpass frequency across a range centered on `center`
            let octaves = depth * 3.0; // up to 3 octaves of sweep
            let freq = (center * (2.0_f64).powf(lfo * octaves)).clamp(20.0, sr * 0.45);

            // first-order allpass coefficient from frequency
            let w = std::f64::consts::PI * freq / sr;
            let coeff = (w.tan() - 1.0) / (w.tan() + 1.0);

            // run through allpass chain
            let mut x = dry + self.feedback_state * feedback;
            for stage in &mut self.stages {
                let y = coeff * x + *stage;
                *stage = flush(x - coeff * y);
                x = y;
            }
            self.feedback_state = flush(x);

            *o = (dry * (1.0 - mix) + x * mix) as f32;
        }
    }
}
