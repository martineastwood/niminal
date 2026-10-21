//! Tube-style asymmetric distortion with tone control.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// Asymmetric tube-style distortion. `drive` controls how hard the signal
/// is pushed (1 = clean, higher = more harmonics); `tone` is a post-
/// distortion tilt EQ (0 = dark, 0.5 = neutral, 1 = bright); `mix`
/// blends wet with dry.
#[derive(Clone)]
pub struct Distortion {
    sample_rate: f32,
    /// One-pole state for the tone filter.
    tone_state: f64,
}

impl Default for Distortion {
    fn default() -> Self {
        Distortion {
            sample_rate: 48_000.0,
            tone_state: 0.0,
        }
    }
}

impl Opcode for Distortion {
    fn reset(&mut self) {
        self.tone_state = 0.0;
    }

    fn name(&self) -> &str {
        "distortion"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("drive", 4.0),
            Port::optional("tone", 0.5),
            Port::optional("mix", 1.0),
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
            self.tone_state = old.tone_state;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let dry = f64::from(inputs[0][i]);
            let drive = f64::from(inputs[1][i]).max(1.0);
            let tone = f64::from(inputs[2][i]).clamp(0.0, 1.0);
            let mix = f64::from(inputs[3][i]).clamp(0.0, 1.0);

            // asymmetric soft clipping: positive side clips harder
            let driven = dry * drive;
            let shaped = if driven >= 0.0 {
                1.0 - (-driven).exp()
            } else {
                -(1.0 - (driven * 0.7).exp()) / 0.7
            };

            // one-pole tone: tone 0..1 maps to lowpass cutoff 200Hz..20kHz
            let freq = 200.0 * (100.0_f64).powf(tone);
            let coeff = (-std::f64::consts::TAU * freq / sr).exp();
            self.tone_state = flush(shaped * (1.0 - coeff) + self.tone_state * coeff);

            *o = (dry * (1.0 - mix) + self.tone_state * mix) as f32;
        }
    }
}
