//! Comb filter: feedforward (FIR) and feedback (IIR) modes.

use crate::denormals::flush32;
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

/// A comb filter. In feedback mode (`feedback` wired) the output is the
/// delayed signal fed back on itself — tuned resonance, Karplus-Strong
/// plucks. In feedforward mode (`feedback` at 0) it is a simple comb
/// notch / peak pattern. `time` sets the comb spacing; `feedback` or
/// `feedforward` scale the delayed copy. `damp` is a one-pole lowpass on
/// the feedback path (0 = none, 1 = heavily damped).
#[derive(Clone)]
pub struct Comb {
    max_seconds: f32,
    buf: Vec<f32>,
    write: usize,
    sample_rate: f32,
    quiet_run: usize,
    in_flight: usize,
    /// One-pole state for the damping filter on the feedback path.
    damp_state: f64,
}

impl Comb {
    pub fn new(max_seconds: f32) -> Self {
        let mut c = Comb {
            max_seconds,
            buf: Vec::new(),
            write: 0,
            sample_rate: 48_000.0,
            quiet_run: 0,
            in_flight: 0,
            damp_state: 0.0,
        };
        c.prepare(48_000.0);
        c
    }
}

impl Opcode for Comb {
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.quiet_run = 0;
        self.in_flight = 0;
        self.damp_state = 0.0;
    }

    fn name(&self) -> &str { "comb" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("time"),
            Port::optional("feedback", 0.0),
            Port::optional("feedforward", 0.0),
            Port::optional("damp", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Comb::new(self.max_seconds))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let max = (self.max_seconds * sample_rate).round().max(1.0);
        self.buf = vec![0.0; max as usize + 3];
        self.write = 0;
        self.quiet_run = 0;
        self.in_flight = 0;
        self.damp_state = 0.0;
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
            self.damp_state = old.damp_state;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let len = self.buf.len();
        let max_samples = (len - 3) as f64;
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let exact = f64::from(inputs[1][i]) * sr;
            let snapped = exact.round();
            let samples = if (exact - snapped).abs() < 1e-3 { snapped } else { exact }.clamp(1.0, max_samples);
            let whole = samples.floor() as usize;
            let frac = (samples - samples.floor()) as f32;

            let newer = self.buf[(self.write + len - whole) % len];
            let older = self.buf[(self.write + len - whole - 1) % len];
            let delayed = newer + (older - newer) * frac;

            let fb = inputs[2][i].clamp(-0.995, 0.995);
            let ff = inputs[3][i].clamp(-1.0, 1.0);
            let damp = f64::from(inputs[4][i].clamp(0.0, 1.0));

            // one-pole lowpass on the feedback path
            let fb_signal = f64::from(delayed) * f64::from(fb);
            self.damp_state = fb_signal * (1.0 - damp) + self.damp_state * damp;
            let fb_damped = crate::denormals::flush(self.damp_state) as f32;

            let written = flush32(inputs[0][i] + fb_damped);
            self.buf[self.write] = written;
            self.write = (self.write + 1) % len;
            self.in_flight = samples.ceil() as usize + 1;

            let result = inputs[0][i] + delayed * ff + fb_damped;
            self.quiet_run = if result.abs() < TAIL_LEVEL { self.quiet_run.saturating_add(1) } else { 0 };
            *o = result;
        }
    }

    fn is_active(&self) -> bool {
        self.quiet_run < self.in_flight
    }
}
