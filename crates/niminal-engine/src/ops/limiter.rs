//! Brickwall lookahead limiter.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// A lookahead brickwall limiter. `ceiling` is the maximum output level
/// (linear; the language converts dB). The attack lookahead is fixed at
/// 5ms (240 samples at 48kHz) for transparent transient preservation;
/// `release` controls how fast the gain recovers.
#[derive(Clone)]
pub struct Limiter {
    delay_buf: Vec<f32>,
    write: usize,
    lookahead: usize,
    envelope: f64,
    sample_rate: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        let mut l = Limiter {
            delay_buf: Vec::new(),
            write: 0,
            lookahead: 240,
            envelope: 0.0,
            sample_rate: 48_000.0,
        };
        l.prepare(48_000.0);
        l
    }
}

impl Opcode for Limiter {
    fn reset(&mut self) {
        self.delay_buf.fill(0.0);
        self.write = 0;
        self.envelope = 0.0;
    }

    fn name(&self) -> &str { "limiter" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("ceiling", 1.0),
            Port::optional("release", 0.1),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.lookahead = (0.005 * sample_rate as f64).round() as usize;
        self.delay_buf = vec![0.0; self.lookahead + 1];
        self.write = 0;
        self.envelope = 0.0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.delay_buf.len() == old.delay_buf.len()
            && self.sample_rate == old.sample_rate
        {
            std::mem::swap(&mut self.delay_buf, &mut old.delay_buf);
            self.write = old.write;
            self.envelope = old.envelope;
        }
    }

    fn latency(&self) -> usize {
        self.lookahead
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let len = self.delay_buf.len();
        let sr = f64::from(self.sample_rate).max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let ceiling = f64::from(inputs[1][i].abs().max(1e-6));
            let release_time = f64::from(inputs[2][i]).max(0.001);
            let release_coeff = (-1.0 / (release_time * sr)).exp();

            let level = f64::from(x).abs();
            // instant attack, smooth release
            if level > self.envelope {
                self.envelope = level;
            } else {
                self.envelope = level + release_coeff * (self.envelope - level);
            }

            // read the delayed signal
            let read_pos = (self.write + len - self.lookahead) % len;
            let delayed = self.delay_buf[read_pos];
            self.delay_buf[self.write] = x;
            self.write = (self.write + 1) % len;

            let gain = if self.envelope > ceiling {
                (ceiling / self.envelope) as f32
            } else {
                1.0
            };
            *o = delayed * gain;
        }
    }
}
