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

/// The next number of a seeded stream (splitmix64).
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A number from 0 up to (not including) 1.
fn random_unit(state: &mut u64) -> f64 {
    (next_random(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// A stream for `seed` and `salt` that is the same every time.
fn seeded(seed: f32, salt: u64) -> u64 {
    let mut state = (seed.round() as i64 as u64) ^ salt.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    next_random(&mut state);
    state
}

const MAX_SLICES: usize = 32;

/// Cuts the last `length` of the sound into `slices` and plays them back shuffled while `hold` is above zero.
/// Each pass through the pieces is a new shuffle, and the same `seed` gives the same shuffles every time.
/// Each slice then gets a roll of the dice: `reverse` is the chance (0 to 1) that it plays backwards, `repeat`
/// that it is swapped for a slice picked at random (so some come twice and some not at all), `drop` that it is
/// silent, and `detune` is how many semitones, up to, it may be shifted either way.
#[derive(Clone)]
pub struct Scatter {
    ring: Ring,
    hold: Hold,
    sample_rate: f32,
    frozen: usize,
    phase: f64,
    size: f64,
    count: usize,
    pass: u64,
    seed: f32,
    reverse: f32,
    repeat: f32,
    drop: f32,
    detune: f32,
    order: [u8; MAX_SLICES],
    flipped: [bool; MAX_SLICES],
    dropped: [bool; MAX_SLICES],
    speed: [f64; MAX_SLICES],
}

impl Scatter {
    pub fn new(max_seconds: f32) -> Self {
        Scatter {
            ring: Ring::new(max_seconds),
            hold: Hold::default(),
            sample_rate: 48_000.0,
            frozen: 0,
            phase: 0.0,
            size: 1.0,
            count: 2,
            pass: 0,
            seed: 0.0,
            reverse: 0.0,
            repeat: 0.0,
            drop: 0.0,
            detune: 0.0,
            order: [0; MAX_SLICES],
            flipped: [false; MAX_SLICES],
            dropped: [false; MAX_SLICES],
            speed: [1.0; MAX_SLICES],
        }
    }

    /// Deal the slices out in a new order for this pass.
    fn shuffle(&mut self) {
        let mut state = seeded(self.seed, self.pass);
        for i in 0..self.count {
            self.order[i] = i as u8;
        }
        for i in (1..self.count).rev() {
            let j = (random_unit(&mut state) * (i + 1) as f64) as usize;
            self.order.swap(i, j);
        }
        for i in 0..self.count {
            if random_unit(&mut state) < f64::from(self.repeat) {
                self.order[i] = (random_unit(&mut state) * self.count as f64) as u8;
            }
            self.flipped[i] = random_unit(&mut state) < f64::from(self.reverse);
            self.dropped[i] = random_unit(&mut state) < f64::from(self.drop);
            let shift = (random_unit(&mut state) * 2.0 - 1.0) * f64::from(self.detune);
            self.speed[i] = (shift * 0.057_762_265).exp();
        }
    }
}

impl Opcode for Scatter {
    fn name(&self) -> &str {
        "scatter"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("hold"),
            Port::required("length"),
            Port::optional("slices", 8.0),
            Port::optional("seed", 0.0),
            Port::optional("reverse", 0.0),
            Port::optional("repeat", 0.0),
            Port::optional("drop", 0.0),
            Port::optional("detune", 0.0),
            Port::optional("blend", 0.003),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = Scatter::new(self.ring.max_seconds);
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
        self.size = 1.0;
        self.count = 2;
        self.pass = 0;
        self.order = [0; MAX_SLICES];
        self.flipped = [false; MAX_SLICES];
        self.dropped = [false; MAX_SLICES];
        self.speed = [1.0; MAX_SLICES];
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
            self.count = old.count;
            self.pass = old.pass;
            self.seed = old.seed;
            self.reverse = old.reverse;
            self.repeat = old.repeat;
            self.drop = old.drop;
            self.detune = old.detune;
            self.order = old.order;
            self.flipped = old.flipped;
            self.dropped = old.dropped;
            self.speed = old.speed;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        let seam = 0.001 * sr;
        let longest = (self.ring.buf.len() - 1) as f64;
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let (held, rising, mix) = self.hold.step(inputs[1][i], inputs[9][i], self.sample_rate);
            if rising {
                self.frozen = self.ring.pos;
                self.phase = 0.0;
                self.pass = 0;
                self.count = (inputs[3][i].round().max(2.0) as usize).min(MAX_SLICES);
                self.size = (f64::from(inputs[2][i]) * sr).clamp(self.count as f64 * 4.0 * seam, longest.max(1.0));
                self.seed = inputs[4][i];
                self.reverse = inputs[5][i];
                self.repeat = inputs[6][i];
                self.drop = inputs[7][i];
                self.detune = inputs[8][i];
                self.shuffle();
            }
            let mut wet = x;
            if held || mix > 0.0 {
                let slice = self.size / self.count as f64;
                let index = ((self.phase / slice) as usize).min(self.count - 1);
                let within = self.phase - index as f64 * slice;
                let base = self.frozen as f64 - self.size + f64::from(self.order[index]) * slice;
                let along = within * self.speed[index];
                let at = if self.flipped[index] { base + slice - along } else { base + along };
                // a slice shifted up must not read on past the end of what was held
                let at = at.min(self.frozen as f64 - 1.0);
                wet = if self.dropped[index] { 0.0 } else { self.ring.at(at) * edge(within, slice, seam) };
                self.phase += 1.0;
                if self.phase >= self.size {
                    self.phase -= self.size;
                    self.pass += 1;
                    self.shuffle();
                }
            }
            if !held {
                self.ring.push(x);
            }
            *o = x + (wet - x) * mix;
        }
    }
}

const MAX_GRAINS: usize = 24;

#[derive(Clone, Copy, Default)]
struct Grain {
    read: f64,
    speed: f64,
    age: f64,
    size: f64,
}

/// Holds the last `length` of the sound as a cloud of short grains while `hold` is above zero: a grain of
/// `grain` seconds is started `density` times per grain length, each at a place picked around `position`
/// (0 the start of the held piece, 1 its end) by up to `spread` and pitched by `pitch` (semitones) plus up to
/// `detune` either way. The same `seed` gives the same cloud every time.
#[derive(Clone)]
pub struct Freeze {
    ring: Ring,
    hold: Hold,
    sample_rate: f32,
    frozen: usize,
    region: f64,
    until_next: f64,
    random: u64,
    grains: [Grain; MAX_GRAINS],
}

impl Freeze {
    pub fn new(max_seconds: f32) -> Self {
        Freeze {
            ring: Ring::new(max_seconds),
            hold: Hold::default(),
            sample_rate: 48_000.0,
            frozen: 0,
            region: 1.0,
            until_next: 0.0,
            random: 0,
            grains: [Grain::default(); MAX_GRAINS],
        }
    }
}

impl Opcode for Freeze {
    fn name(&self) -> &str {
        "freeze"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("hold"),
            Port::optional("length", 0.25),
            Port::optional("grain", 0.08),
            Port::optional("density", 4.0),
            Port::optional("position", 1.0),
            Port::optional("spread", 0.5),
            Port::optional("pitch", 0.0),
            Port::optional("detune", 0.0),
            Port::optional("seed", 0.0),
            Port::optional("blend", 0.01),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut c = Freeze::new(self.ring.max_seconds);
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
        self.region = 1.0;
        self.until_next = 0.0;
        self.random = 0;
        self.grains = [Grain::default(); MAX_GRAINS];
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.sample_rate == old.sample_rate
        {
            self.ring.carry(&mut old.ring);
            self.hold = old.hold.clone();
            self.frozen = old.frozen;
            self.region = old.region;
            self.until_next = old.until_next;
            self.random = old.random;
            self.grains = old.grains;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        let longest = (self.ring.buf.len() - 1) as f64;
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let (held, rising, mix) = self.hold.step(inputs[1][i], inputs[10][i], self.sample_rate);
            if rising {
                self.frozen = self.ring.pos;
                self.region = (f64::from(inputs[2][i]) * sr).clamp(64.0, longest.max(64.0));
                self.until_next = 0.0;
                self.random = seeded(inputs[9][i], 1);
                self.grains = [Grain::default(); MAX_GRAINS];
            }
            let mut wet = x;
            if held || mix > 0.0 {
                let density = f64::from(inputs[4][i]).clamp(1.0, 16.0);
                let size = (f64::from(inputs[3][i]) * sr).clamp(64.0, longest.max(64.0));
                if held {
                    self.until_next -= 1.0;
                    if self.until_next <= 0.0 {
                        self.until_next += size / density;
                        if let Some(slot) = self.grains.iter_mut().find(|g| g.age >= g.size) {
                            let detune = (random_unit(&mut self.random) * 2.0 - 1.0) * f64::from(inputs[8][i]);
                            let speed = ((f64::from(inputs[7][i]) + detune) * 0.057_762_265).exp();
                            let span = (size * speed).min(self.region);
                            let place = f64::from(inputs[5][i])
                                + (random_unit(&mut self.random) * 2.0 - 1.0) * f64::from(inputs[6][i]);
                            let back = span + (1.0 - place.clamp(0.0, 1.0)) * (self.region - span);
                            *slot = Grain { read: self.frozen as f64 - back, speed, age: 0.0, size };
                        }
                    }
                }
                let mut cloud = 0.0;
                for g in self.grains.iter_mut().filter(|g| g.age < g.size) {
                    let t = g.age / g.size;
                    let window = 0.5 - 0.5 * (std::f64::consts::TAU * t).cos();
                    cloud += self.ring.at(g.read) * window as f32;
                    g.read += g.speed;
                    g.age += 1.0;
                }
                // grains start at random places, so they add up in power rather than in level
                wet = cloud / (0.375 * density as f32).sqrt();
            }
            if !held {
                self.ring.push(x);
            }
            *o = x + (wet - x) * mix;
        }
    }
}
