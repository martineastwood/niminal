//! Schroeder allpass: delay-based allpass filter for diffusion and phasing.

use crate::denormals::flush32;
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

/// A Schroeder allpass delay: `y[n] = -g*x[n] + x[n-d] + g*y[n-d]`.
/// Passes all frequencies at equal level but scrambles their phase — the
/// building block of reverb diffusion networks and phasers. `time` sets
/// the delay, `feedback` is the allpass coefficient g (±0.995).
#[derive(Clone)]
pub struct Allpass {
    max_seconds: f32,
    buf: Vec<f32>,
    write: usize,
    sample_rate: f32,
    quiet_run: usize,
    in_flight: usize,
}

impl Allpass {
    pub fn new(max_seconds: f32) -> Self {
        let mut a = Allpass {
            max_seconds,
            buf: Vec::new(),
            write: 0,
            sample_rate: 48_000.0,
            quiet_run: 0,
            in_flight: 0,
        };
        a.prepare(48_000.0);
        a
    }
}

impl Opcode for Allpass {
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.quiet_run = 0;
        self.in_flight = 0;
    }

    fn name(&self) -> &str {
        "allpass"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("time"),
            Port::optional("feedback", 0.5),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Allpass::new(self.max_seconds))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let max = (self.max_seconds * sample_rate).round().max(1.0);
        self.buf = vec![0.0; max as usize + 3];
        self.write = 0;
        self.quiet_run = 0;
        self.in_flight = 0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.buf.len() == old.buf.len()
            && self.sample_rate == old.sample_rate
        {
            std::mem::swap(&mut self.buf, &mut old.buf);
            self.write = old.write;
            self.quiet_run = old.quiet_run;
            self.in_flight = old.in_flight;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let len = self.buf.len();
        let max_samples = (len - 3) as f64;
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let exact = f64::from(inputs[1][i]) * sr;
            let snapped = exact.round();
            let samples = if (exact - snapped).abs() < 1e-3 {
                snapped
            } else {
                exact
            }
            .clamp(1.0, max_samples);
            let whole = samples.floor() as usize;
            let frac = (samples - samples.floor()) as f32;

            let newer = self.buf[(self.write + len - whole) % len];
            let older = self.buf[(self.write + len - whole - 1) % len];
            let delayed = newer + (older - newer) * frac;

            let g = inputs[2][i].clamp(-0.995, 0.995);

            // Schroeder allpass: y = -g*x + delayed + g*(delayed fed back)
            let x = inputs[0][i];
            let y = -g * x + delayed;
            let written = flush32(x + g * delayed);
            self.buf[self.write] = written;
            self.write = (self.write + 1) % len;
            self.in_flight = samples.ceil() as usize + 1;
            self.quiet_run = if y.abs() < TAIL_LEVEL {
                self.quiet_run.saturating_add(1)
            } else {
                0
            };
            *o = y;
        }
    }

    fn is_active(&self) -> bool {
        self.quiet_run < self.in_flight
    }
}
