//! Glitches that grab the last piece of the sound: repeat it, play it backwards, or wind the sound down
//! like a stopping tape. Each is switched on by a `hold` input above zero and fades in and out over `blend`.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Seconds of sound kept unless `max` says otherwise.
pub const DEFAULT_MAX_SECONDS: f32 = 2.0;

/// The fade between the live sound and the glitch, and the edges of `hold`.
#[derive(Clone, Default)]
struct Hold {
    prev: f32,
    mix: f32,
}

impl Hold {
    /// Take this sample's `hold` and `blend` (seconds): whether it is held, whether it just began, and how
    /// far the fade has got.
    fn step(&mut self, hold: f32, blend: f32, sample_rate: f32) -> (bool, bool, f32) {
        let held = hold > 0.0;
        let rising = held && self.prev <= 0.0;
        self.prev = hold;
        let step = 1.0 / (blend * sample_rate).max(1.0);
        let target = if held { 1.0 } else { 0.0 };
        self.mix += (target - self.mix).clamp(-step, step);
        (held, rising, self.mix)
    }
}

/// The ring of recent sound, read with linear interpolation and wrapping.
#[derive(Clone)]
struct Ring {
    max_seconds: f32,
    buf: Vec<f32>,
    pos: usize,
}

impl Ring {
    fn new(max_seconds: f32) -> Self {
        Ring {
            max_seconds,
            buf: vec![0.0; 1],
            pos: 0,
        }
    }

    fn prepare(&mut self, sample_rate: f32) {
        let len = (self.max_seconds * sample_rate).round().max(1.0) as usize;
        self.buf = vec![0.0; len];
        self.pos = 0;
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    fn push(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
    }

    fn at(&self, pos: f64) -> f32 {
        if !pos.is_finite() {
            return self.buf[0];
        }
        let len = self.buf.len() as i64;
        let base = pos.floor();
        let t = (pos - base) as f32;
        let a = self.buf[(base as i64).rem_euclid(len) as usize];
        let b = self.buf[(base as i64 + 1).rem_euclid(len) as usize];
        a + (b - a) * t
    }

    fn carry(&mut self, old: &mut Ring) {
        if self.buf.len() == old.buf.len() {
            std::mem::swap(&mut self.buf, &mut old.buf);
            self.pos = old.pos;
        }
    }
}

/// How much of a piece's edges are faded so a repeat starts and ends without a click.
fn edge(phase: f64, size: f64, seam: f64) -> f32 {
    (phase.min(size - phase).min(seam) / seam) as f32
}

/// Repeats the last `length` of the sound while `hold` is above zero. Each repeat is `fade` times the level
/// of the one before and `shrink` times the length; `pitch` (semitones) changes the speed it is read at. The
/// `length` is read once at the start of each repeat, so a length that moves gives a run of clean repeats
/// instead of a smeared one. `gate` is how much of each repeat sounds (the rest is silence) and a `reverse`
/// above zero plays each repeat backwards. `shape` gives each repeat a level curve: above zero it dies away
/// (1 is a straight fade to nothing), below zero it swells in. `damp` darkens each repeat more than the one
/// before, like a tape echo. `attack` is how much of the repeat (0 to 1) it takes to rise to full level,
/// `bend` turns the straight `shape` curve into an exponential one (a pluck), and `accent` makes every second
/// repeat that much quieter, so a roll has a beat in it.
#[derive(Clone)]
pub struct RepeatLast {
    ring: Ring,
    hold: Hold,
    sample_rate: f32,
    frozen: usize,
    phase: f64,
    size: f64,
    level: f32,
    scale: f64,
    repeats: i32,
    lp: f32,
}

impl RepeatLast {
    pub fn new(max_seconds: f32) -> Self {
        RepeatLast {
            ring: Ring::new(max_seconds),
            hold: Hold::default(),
            sample_rate: 48_000.0,
            frozen: 0,
            phase: 0.0,
            size: 0.0,
            level: 1.0,
            scale: 1.0,
            repeats: 0,
            lp: 0.0,
        }
    }
}

impl Opcode for RepeatLast {
    fn name(&self) -> &str {
        "repeat_last"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("hold"),
            Port::required("length"),
            Port::optional("pitch", 0.0),
            Port::optional("fade", 1.0),
            Port::optional("shrink", 1.0),
            Port::optional("blend", 0.003),
            Port::optional("gate", 1.0),
            Port::optional("reverse", 0.0),
            Port::optional("shape", 0.0),
            Port::optional("damp", 0.0),
            Port::optional("attack", 0.0),
            Port::optional("bend", 0.0),
            Port::optional("accent", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = RepeatLast::new(self.ring.max_seconds);
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.ring.prepare(sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.ring.clear();
        self.hold = Hold::default();
        self.frozen = 0;
        self.phase = 0.0;
        self.size = 0.0;
        self.level = 1.0;
        self.scale = 1.0;
        self.repeats = 0;
        self.lp = 0.0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.sample_rate == old.sample_rate
        {
            self.ring.carry(&mut old.ring);
            self.hold = old.hold.clone();
            self.frozen = old.frozen;
            self.phase = old.phase;
            self.size = old.size;
            self.repeats = old.repeats;
            self.lp = old.lp;
            self.level = old.level;
            self.scale = old.scale;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        let seam = 0.001 * sr;
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let (held, rising, mix) = self.hold.step(inputs[1][i], inputs[6][i], self.sample_rate);
            let length = f64::from(inputs[2][i]) * sr;
            if rising {
                self.frozen = self.ring.pos;
                self.phase = 0.0;
                self.level = 1.0;
                self.scale = 1.0;
                self.repeats = 0;
                self.size = length.max(4.0 * seam);
            }
            let size = self.size;
            let mut wet = x;
            if held || mix > 0.0 {
                let open = size * f64::from(inputs[7][i]).clamp(0.05, 1.0);
                let at = if inputs[8][i] > 0.0 { self.frozen as f64 - self.phase } else { self.frozen as f64 - size + self.phase };
                let window = if self.phase < open { edge(self.phase, open, seam) } else { 0.0 };
                let t = (self.phase / open.max(1.0)).min(1.0) as f32;
                let shape = inputs[9][i].clamp(-1.0, 1.0);
                let curve = if shape >= 0.0 { 1.0 - shape * t } else { 1.0 + shape * (1.0 - t) };
                let curve = curve.powf(1.0 + 4.0 * inputs[12][i].clamp(0.0, 1.0));
                let attack = inputs[11][i].clamp(0.0, 1.0);
                let rise = if attack > 0.0 { (t / attack).min(1.0) } else { 1.0 };
                let accent = if self.repeats % 2 == 1 { 1.0 - inputs[13][i].clamp(0.0, 1.0) } else { 1.0 };
                let a = (1.0 - inputs[10][i].clamp(0.0, 0.95)).powi(self.repeats).max(0.02);
                self.lp += a * (self.ring.at(at) - self.lp);
                wet = self.lp * self.level * window * curve * rise * accent;
                self.phase += (f64::from(inputs[3][i]) * 0.057_762_265).exp();
                if self.phase >= size {
                    self.phase -= size;
                    self.level *= inputs[4][i];
                    self.scale *= f64::from(inputs[5][i]);
                    self.repeats += 1;
                    self.size = (length * self.scale).max(4.0 * seam);
                }
            }
            if !held {
                self.ring.push(x);
            }
            *o = x + (wet - x) * mix;
        }
    }
}

/// Plays the last `length` of the sound backwards, over and over, while `hold` is above zero.
#[derive(Clone)]
pub struct RewindLast {
    ring: Ring,
    hold: Hold,
    sample_rate: f32,
    frozen: usize,
    phase: f64,
}

impl RewindLast {
    pub fn new(max_seconds: f32) -> Self {
        RewindLast {
            ring: Ring::new(max_seconds),
            hold: Hold::default(),
            sample_rate: 48_000.0,
            frozen: 0,
            phase: 0.0,
        }
    }
}

impl Opcode for RewindLast {
    fn name(&self) -> &str {
        "rewind_last"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("hold"),
            Port::required("length"),
            Port::optional("blend", 0.003),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = RewindLast::new(self.ring.max_seconds);
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.ring.prepare(sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.ring.clear();
        self.hold = Hold::default();
        self.frozen = 0;
        self.phase = 0.0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.sample_rate == old.sample_rate
        {
            self.ring.carry(&mut old.ring);
            self.hold = old.hold.clone();
            self.frozen = old.frozen;
            self.phase = old.phase;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        let seam = 0.001 * sr;
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let (held, rising, mix) = self.hold.step(inputs[1][i], inputs[3][i], self.sample_rate);
            if rising {
                self.frozen = self.ring.pos;
                self.phase = 0.0;
            }
            let size = f64::from(inputs[2][i]) * sr;
            let mut wet = x;
            if held || mix > 0.0 {
                wet = self.ring.at(self.frozen as f64 - self.phase) * edge(self.phase, size, seam);
                self.phase += 1.0;
                if self.phase >= size {
                    self.phase -= size;
                }
            }
            if !held {
                self.ring.push(x);
            }
            *o = x + (wet - x) * mix;
        }
    }
}

/// Slows the sound down to a stop over `time` while `hold` is above zero, like a tape machine switched off.
#[derive(Clone)]
pub struct SlowDown {
    ring: Ring,
    hold: Hold,
    sample_rate: f32,
    read: f64,
    elapsed: f64,
}

impl SlowDown {
    pub fn new(max_seconds: f32) -> Self {
        SlowDown {
            ring: Ring::new(max_seconds),
            hold: Hold::default(),
            sample_rate: 48_000.0,
            read: 0.0,
            elapsed: 0.0,
        }
    }
}

impl Opcode for SlowDown {
    fn name(&self) -> &str {
        "slow_down"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("hold"),
            Port::required("time"),
            Port::optional("blend", 0.003),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = SlowDown::new(self.ring.max_seconds);
        c.prepare(self.sample_rate);
        Box::new(c)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.ring.prepare(sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.ring.clear();
        self.hold = Hold::default();
        self.read = 0.0;
        self.elapsed = 0.0;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.sample_rate == old.sample_rate
        {
            self.ring.carry(&mut old.ring);
            self.hold = old.hold.clone();
            self.read = old.read;
            self.elapsed = old.elapsed;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            self.ring.push(x);
            let (held, rising, mix) = self.hold.step(inputs[1][i], inputs[3][i], self.sample_rate);
            if rising {
                self.read = self.ring.pos as f64 - 1.0;
                self.elapsed = 0.0;
            }
            let size = f64::from(inputs[2][i]) * sr;
            let mut wet = x;
            if held || mix > 0.0 {
                let speed = if self.elapsed < size {
                    1.0 - self.elapsed / size
                } else {
                    0.0
                };
                wet = self.ring.at(self.read) * (speed * 8.0).min(1.0) as f32;
                self.read += speed;
                self.elapsed += 1.0;
            }
            *o = x + (wet - x) * mix;
        }
    }
}
