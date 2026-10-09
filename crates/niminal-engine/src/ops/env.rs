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
    fn bend(self) -> f64 {
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
fn shape(a: f32, b: f32, t: f64, k: f64) -> f32 {
    if k.abs() < 1e-6 {
        return a + (b - a) * t as f32;
    }
    // Rises 0..1, convex for k > 0 and concave for k < 0.
    let c = |x: f64| ((k * x).exp() - 1.0) / (k.exp() - 1.0);
    let s = if b >= a { c(t) } else { c(1.0 - t) };
    if b >= a { a + (b - a) * s as f32 } else { b + (a - b) * s as f32 }
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
    lengths: Vec<u32>,
    level: f32,
    phase: Phase,
    released: bool,
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
            level: start,
            phase: Phase::Done,
            released: false,
        };
        env.reset(48_000.0);
        env
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
        self.lengths = self
            .segments
            .iter()
            .map(|s| (s.dur.max(0.0) * sample_rate).round() as u32)
            .collect();
        self.level = self.start;
        self.released = false;
        self.enter(0, self.start);
    }

    /// Begin segment `seg` from level `from`, skipping zero-length segments and
    /// stopping at the sustain point.
    fn enter(&mut self, mut seg: usize, mut from: f32) {
        loop {
            if seg >= self.segments.len() {
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
            self.phase = Phase::Running { seg, pos: 0, from };
            return;
        }
    }

    fn release(&mut self) {
        self.released = true;
        let Some(sustain) = self.sustain_at else { return };
        let before_sustain = match self.phase {
            Phase::Hold => true,
            Phase::Running { seg, .. } => seg < sustain,
            Phase::Done => false,
        };
        if before_sustain {
            self.enter(sustain, self.level);
        }
    }

    fn step(&mut self) {
        if let Phase::Running { seg, pos, from } = self.phase {
            let s = self.segments[seg];
            let pos = pos + 1;
            if pos >= self.lengths[seg] {
                self.level = s.target;
                self.enter(seg + 1, s.target);
            } else {
                let t = f64::from(pos) / f64::from(self.lengths[seg]);
                self.level = shape(from, s.target, t, s.curve.bend());
                self.phase = Phase::Running { seg, pos, from };
            }
        }
    }
}

impl Opcode for Env {
    fn name(&self) -> &'static str {
        "env"
    }

    fn ports(&self) -> &'static [Port] {
        &[]
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut e = self.clone();
        e.reset(48_000.0);
        Box::new(e)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.reset(sample_rate);
    }

    fn process(&mut self, ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        if !ctx.gate && !self.released {
            self.release();
        }
        for o in out.iter_mut() {
            *o = self.level;
            self.step();
        }
    }

    fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Done)
    }
}
