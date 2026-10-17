//! A simple chorus / flanger: modulated delay mixed with the dry signal.

use crate::denormals::flush32;
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

const MAX_DELAY_MS: f32 = 40.0;

/// Chorus: a short delay whose time is swept by a sine LFO, mixed with dry.
/// Small `depth` and longer `time` read as chorus; more depth and less time
/// as a flanger. `rate` is the LFO in hz; `mix` blends wet (1) with dry (0).
#[derive(Clone)]
pub struct Chorus {
    buf: Vec<f32>,
    write: usize,
    sample_rate: f32,
    phase: f64,
    quiet_run: usize,
}

impl Default for Chorus {
    fn default() -> Self {
        let mut c = Chorus {
            buf: Vec::new(),
            write: 0,
            sample_rate: 48_000.0,
            phase: 0.0,
            quiet_run: 0,
        };
        c.prepare(48_000.0);
        c
    }
}

impl Opcode for Chorus {
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.phase = 0.0;
        self.quiet_run = 0;
    }

    fn name(&self) -> &str {
        "chorus"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("rate", 0.5),
            Port::optional("depth", 0.5),
            Port::optional("time", 0.007),
            Port::optional("mix", 0.5),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let max = ((MAX_DELAY_MS / 1000.0) * sample_rate).ceil() as usize + 4;
        self.buf = vec![0.0; max.max(8)];
        self.write = 0;
        self.quiet_run = 0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.buf.len() == old.buf.len()
            && self.sample_rate == old.sample_rate
        {
            std::mem::swap(&mut self.buf, &mut old.buf);
            self.write = old.write;
            self.phase = old.phase;
            self.quiet_run = old.quiet_run;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let len = self.buf.len();
        let sr = f64::from(self.sample_rate);
        let max_delay = (len - 3) as f64;
        for (i, o) in out.iter_mut().enumerate() {
            let rate = f64::from(inputs[1][i]).max(0.0);
            let depth = f64::from(inputs[2][i]).clamp(0.0, 1.0);
            let base = (f64::from(inputs[3][i]) * sr).clamp(1.0, max_delay * 0.75);
            let mix = inputs[4][i].clamp(0.0, 1.0);
            let dry = inputs[0][i];

            let lfo = (self.phase * std::f64::consts::TAU).sin() * 0.5 + 0.5;
            self.phase = (self.phase + rate / sr).fract();
            let samples = (base * (1.0 + depth * (lfo * 2.0 - 1.0) * 0.9)).clamp(1.0, max_delay);
            let whole = samples.floor() as usize;
            let frac = (samples - samples.floor()) as f32;
            let newer = self.buf[(self.write + len - whole) % len];
            let older = self.buf[(self.write + len - whole - 1) % len];
            let wet = newer + (older - newer) * frac;

            self.buf[self.write] = flush32(dry);
            self.write = (self.write + 1) % len;
            let mixed = dry * (1.0 - mix) + wet * mix;
            self.quiet_run = if mixed.abs() < TAIL_LEVEL {
                self.quiet_run.saturating_add(1)
            } else {
                0
            };
            *o = mixed;
        }
    }

    fn is_active(&self) -> bool {
        self.quiet_run < self.buf.len()
    }
}
