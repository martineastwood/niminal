use crate::opcode::{Opcode, Port, ProcessCtx};

/// How a segment travels between its two levels.
///
/// Curves are described by the shape of value against time, whichever way the
/// segment is heading: `Exp` is convex (a rising exp starts slowly and speeds
/// up; a falling exp drops fast and levels off, like a natural decay), `Log`
/// is the concave mirror image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Linear,
    Exp,
    Log,
    /// Positive bends like `Exp`, negative like `Log`, zero is linear.
    Custom(f32),
}

impl Curve {
    pub fn bend(self) -> f64 {
        match self {
            Curve::Linear => 0.0,
            Curve::Exp => 5.0,
            Curve::Log => -5.0,
            Curve::Custom(k) => f64::from(k),
        }
    }
}

/// Move to `target` over `dur` seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub dur: f32,
    pub target: f32,
    pub curve: Curve,
}

impl Segment {
    pub fn new(dur: f32, target: f32, curve: Curve) -> Self {
        Segment { dur, target, curve }
    }

    pub fn linear(dur: f32, target: f32) -> Self {
        Segment::new(dur, target, Curve::Linear)
    }
}

/// Position `t` (0..1) along a segment from `a` to `b`.
pub fn shape(a: f32, b: f32, t: f64, k: f64) -> f32 {
    if k.abs() < 1e-6 {
        return a + (b - a) * t as f32;
    }
    let inv_denom = 1.0 / (k.exp() - 1.0);
    shape_fast(a, b, t, k, inv_denom)
}

fn shape_fast(a: f32, b: f32, t: f64, k: f64, inv_denom: f64) -> f32 {
    if k.abs() < 1e-6 {
        return a + (b - a) * t as f32;
    }
    let c = |x: f64| ((k * x).exp() - 1.0) * inv_denom;
    let s = if b >= a { c(t) } else { c(1.0 - t) };
    if b >= a {
        a + (b - a) * s as f32
    } else {
        b + (a - b) * s as f32
    }
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Running { seg: usize, pos: u32, from: f32 },
    Hold,
    Done,
}

/// Multi-segment envelope: `env[0 5ms 1 120ms 0.4 | 600ms 0]`.
///
/// `sustain_at` is the number of segments before the `|`. With a sustain point
/// the envelope holds there until the note is released, then runs the
/// remaining segments from wherever it was (even mid-segment). Without one it
/// is a one-shot that plays through and ignores the gate.
#[derive(Clone)]
pub struct Env {
    start: f32,
    segments: Vec<Segment>,
    sustain_at: Option<usize>,
    /// Starts over from `start` when the last segment ends.
    looped: bool,
    lengths: Vec<u32>,
    level: f32,
    phase: Phase,
    released: bool,
    /// Values read from the inputs when the note starts, one per port.
    fields: Vec<EnvField>,
    ports: Vec<Port>,
    sample_rate: f32,
    /// Whether the inputs have been read for this note.
    read: bool,
    /// Cached curve parameters for the current segment, avoiding per-sample exp().
    seg_k: f64,
    seg_inv_denom: f64,
    seg_exp: f64,
    seg_mul: f64,
    seg_rising: bool,
}

/// A part of an envelope that can come from an input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnvField {
    Start,
    /// The duration of a segment, in seconds.
    Dur(usize),
    /// The level a segment ends at.
    Level(usize),
}

/// Milliseconds with one decimal, for messages.
pub(crate) fn ms(seconds: f32) -> String {
    format!("{:.1}ms", seconds * 1000.0)
}

impl Env {
    pub fn new(start: f32, segments: Vec<Segment>, sustain_at: Option<usize>) -> Self {
        if let Some(s) = sustain_at {
            assert!(s <= segments.len(), "sustain point past the last segment");
        }
        let mut env = Env {
            start,
            lengths: vec![0; segments.len()],
            segments,
            sustain_at,
            looped: false,
            level: start,
            phase: Phase::Done,
            released: false,
            fields: Vec::new(),
            ports: Vec::new(),
            sample_rate: 48_000.0,
            read: true,
            seg_k: 0.0,
            seg_inv_denom: 0.0,
            seg_exp: 1.0,
            seg_mul: 1.0,
            seg_rising: true,
        };
        env.reset(48_000.0);
        env
    }

    /// Take these parts of the envelope from inputs, read once when the note starts. The ports are
    /// named `v0`, `v1`... in the order given.
    pub fn with_inputs(mut self, fields: Vec<EnvField>) -> Self {
        self.ports = (0..fields.len())
            .map(|i| Port::named(format!("v{i}"), None))
            .collect();
        self.read = fields.is_empty();
        self.fields = fields;
        self
    }

    /// Start over from the first level each time the last segment ends, for as long as the note is
    /// held: after a release the pass in progress is the last. Has no sustain point.
    pub fn looped(mut self) -> Self {
        assert!(
            self.sustain_at.is_none(),
            "a looping envelope has no sustain point"
        );
        self.looped = true;
        self
    }

    /// Attack, decay, sustain, release.
    pub fn adsr(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Env::new(
            0.0,
            vec![
                Segment::linear(attack, 1.0),
                Segment::new(decay, sustain, Curve::Exp),
                Segment::new(release, 0.0, Curve::Exp),
            ],
            Some(2),
        )
    }

    fn reset(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.read = self.fields.is_empty();
        for (length, segment) in self.lengths.iter_mut().zip(&self.segments) {
            *length = (segment.dur.max(0.0) * sample_rate).round() as u32;
        }
        self.level = self.start;
        self.released = false;
        self.enter(0, self.start);
    }

    /// Begin segment `seg` from level `from`, skipping zero-length segments and
    /// stopping at the sustain point.
    fn enter(&mut self, mut seg: usize, mut from: f32) {
        loop {
            if seg >= self.segments.len() {
                // a loop starts again, unless nothing in it takes any time
                if self.looped && !self.released && self.lengths.iter().any(|&l| l > 0) {
                    seg = 0;
                    from = self.start;
                    self.level = from;
                    continue;
                }
                self.phase = Phase::Done;
                return;
            }
            if self.sustain_at == Some(seg) && !self.released {
                self.phase = Phase::Hold;
                return;
            }
            if self.lengths[seg] == 0 {
                from = self.segments[seg].target;
                self.level = from;
                seg += 1;
                continue;
            }
            let seg_len = f64::from(self.lengths[seg]);
            self.seg_k = self.segments[seg].curve.bend();
            self.seg_rising = self.segments[seg].target >= from;
            if self.seg_k.abs() < 1e-6 {
                self.seg_inv_denom = 0.0;
                self.seg_exp = 1.0;
                self.seg_mul = 1.0;
            } else {
                let exp_k = self.seg_k.exp();
                self.seg_inv_denom = 1.0 / (exp_k - 1.0);
                if self.seg_rising {
                    self.seg_exp = 1.0;
                    self.seg_mul = (self.seg_k / seg_len).exp();
                } else {
                    self.seg_exp = exp_k;
                    self.seg_mul = (-self.seg_k / seg_len).exp();
                }
            }
            self.phase = Phase::Running { seg, pos: 0, from };
            return;
        }
    }

    fn release(&mut self) {
        self.released = true;
        let Some(sustain) = self.sustain_at else {
            return;
        };
        let before_sustain = match self.phase {
            Phase::Hold => true,
            Phase::Running { seg, .. } => seg < sustain,
            Phase::Done => false,
        };
        if before_sustain {
            self.enter(sustain, self.level);
        }
    }

    fn read_inputs(&mut self, input: impl Fn(usize) -> f32) {
        for (i, field) in self.fields.iter().enumerate() {
            let v = input(i);
            match *field {
                EnvField::Start => self.start = v,
                EnvField::Dur(k) => self.segments[k].dur = v,
                EnvField::Level(k) => self.segments[k].target = v,
            }
        }
        let fields = std::mem::take(&mut self.fields);
        self.reset(self.sample_rate);
        self.fields = fields;
        self.read = true;
    }

    fn step(&mut self) {
        if let Phase::Running { seg, pos, from } = self.phase {
            let s = self.segments[seg];
            let pos = pos + 1;
            let len = self.lengths[seg];
            if pos >= len {
                self.level = s.target;
                self.enter(seg + 1, s.target);
            } else {
                if self.seg_k.abs() < 1e-6 {
                    let t = f64::from(pos) / f64::from(len);
                    self.level = from + (s.target - from) * t as f32;
                } else {
                    self.seg_exp *= self.seg_mul;
                    let sv = ((self.seg_exp - 1.0) * self.seg_inv_denom) as f32;
                    self.level = if self.seg_rising {
                        from + (s.target - from) * sv
                    } else {
                        s.target + (from - s.target) * sv
                    };
                }
                self.phase = Phase::Running { seg, pos, from };
            }
        }
    }
}

impl Opcode for Env {
    fn input_is_init(&self, _index: usize) -> bool {
        true
    }
    fn reset(&mut self) {
        self.reset(self.sample_rate);
    }
    fn name(&self) -> &'static str {
        "env"
    }

    fn ports(&self) -> &[Port] {
        &self.ports
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut e = self.clone();
        e.reset(48_000.0);
        Box::new(e)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.reset(sample_rate);
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.start == old.start
            && self.segments == old.segments
            && self.sustain_at == old.sustain_at
            && self.looped == old.looped
            && self.lengths == old.lengths
        {
            self.level = old.level;
            self.phase = old.phase;
            self.released = old.released;
            self.seg_k = old.seg_k;
            self.seg_inv_denom = old.seg_inv_denom;
            self.seg_exp = old.seg_exp;
            self.seg_mul = old.seg_mul;
            self.seg_rising = old.seg_rising;
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        if !self.read {
            self.read_inputs(|i| inputs[i][0]);
        }
        if !ctx.gate && !self.released {
            self.release();
        }
        let mut idx = 0;
        let total = out.len();
        while idx < total {
            match self.phase {
                Phase::Hold | Phase::Done => {
                    out[idx..].fill(self.level);
                    return;
                }
                Phase::Running { seg, mut pos, from } => {
                    let s = self.segments[seg];
                    let len = self.lengths[seg];
                    let chunk = ((len - pos) as usize).min(total - idx);
                    let end_idx = idx + chunk;
                    if self.seg_k.abs() < 1e-6 {
                        let inv_len = 1.0 / f64::from(len);
                        let delta = s.target - from;
                        let mut level = self.level;
                        for o in &mut out[idx..end_idx] {
                            *o = level;
                            pos += 1;
                            level = from + delta * (f64::from(pos) * inv_len) as f32;
                        }
                        self.level = level;
                    } else {
                        let inv_denom = self.seg_inv_denom;
                        let mul = self.seg_mul;
                        let mut exp_state = self.seg_exp;
                        let mut level = self.level;
                        if self.seg_rising {
                            let delta = s.target - from;
                            for o in &mut out[idx..end_idx] {
                                *o = level;
                                pos += 1;
                                exp_state *= mul;
                                level = from + delta * ((exp_state - 1.0) * inv_denom) as f32;
                            }
                        } else {
                            let target = s.target;
                            let delta = from - target;
                            for o in &mut out[idx..end_idx] {
                                *o = level;
                                pos += 1;
                                exp_state *= mul;
                                level = target + delta * ((exp_state - 1.0) * inv_denom) as f32;
                            }
                        }
                        self.seg_exp = exp_state;
                        self.level = level;
                    }
                    idx = end_idx;
                    if pos >= len {
                        self.level = s.target;
                        self.enter(seg + 1, s.target);
                    } else {
                        self.phase = Phase::Running { seg, pos, from };
                    }
                }
            }
        }
    }

    fn has_process_one(&self) -> bool {
        // Parameterized envelopes avoid assembling input slices for each kernel sample.
        // With no ports, the existing block adapter is cheaper on the measured workloads.
        !self.fields.is_empty()
    }

    fn process_one(&mut self, ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        if !self.read {
            self.read_inputs(|i| inputs[i]);
        }
        if !ctx.gate && !self.released {
            self.release();
        }
        let out = self.level;
        self.step();
        out
    }

    fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Done)
    }

    fn is_envelope(&self) -> bool {
        true
    }

    fn step_warnings(&self, step: f32) -> Vec<String> {
        self.segments
            .iter()
            .filter(|s| s.dur > 0.0 && s.dur < step)
            .map(|s| {
                format!(
                    "an envelope segment of {} is shorter than one control step ({}), so it is a single jump",
                    ms(s.dur),
                    ms(step)
                )
            })
            .collect()
    }
}
