//! Tuned resonator for physical modelling (bells, bars, drums).

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// A tuned resonant bandpass: rings at `freq` with a decay time set by
/// `decay` in seconds (time to -60dB). Use several in parallel for
/// modal synthesis of bells, bars, and membranes.
#[derive(Clone)]
pub struct Resonator {
    sample_rate: f32,
    y1: f64,
    y2: f64,
    cached_for: (f32, f32),
    b0: f64,
    a1: f64,
    a2: f64,
}

impl Default for Resonator {
    fn default() -> Self {
        let mut r = Resonator {
            sample_rate: 48_000.0,
            y1: 0.0,
            y2: 0.0,
            cached_for: (f32::NAN, f32::NAN),
            b0: 0.0,
            a1: 0.0,
            a2: 0.0,
        };
        r.update(440.0, 1.0);
        r
    }
}

impl Resonator {
    fn update(&mut self, freq: f32, decay: f32) {
        if (freq, decay) == self.cached_for {
            return;
        }
        self.cached_for = (freq, decay);

        let sr = f64::from(self.sample_rate);
        let f = f64::from(freq).clamp(10.0, sr * 0.45);
        let d = f64::from(decay).max(0.001);

        // two-pole resonator: r controls decay, theta controls frequency
        let r = (-6.907_755_278_982_137 / (d * sr)).exp(); // exp(-pi/(decay*sr)) for 60dB decay
        let theta = std::f64::consts::TAU * f / sr;

        self.a1 = -2.0 * r * theta.cos();
        self.a2 = r * r;
        // normalise so peak gain is 1
        self.b0 = (1.0 - self.a2) * 0.5;
    }
}

impl Opcode for Resonator {
    fn reset(&mut self) {
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    fn name(&self) -> &str {
        "resonator"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("freq"),
            Port::optional("decay", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached_for = (f32::NAN, f32::NAN);
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.y1 = old.y1;
            self.y2 = old.y2;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            self.update(inputs[1][i], inputs[2][i]);
            let x = f64::from(inputs[0][i]);
            let y = self.b0 * x - self.a1 * self.y1 - self.a2 * self.y2;
            self.y2 = flush(self.y1);
            self.y1 = flush(y);
            *o = y as f32;
        }
    }

    fn is_active(&self) -> bool {
        self.y1.abs() > f64::from(crate::opcode::TAIL_LEVEL)
            || self.y2.abs() > f64::from(crate::opcode::TAIL_LEVEL)
    }
}
