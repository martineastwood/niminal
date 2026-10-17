use std::sync::Arc;

use crate::opcode::{Opcode, Port, ProcessCtx};
use crate::table::Table;

/// Frames over which a slice fades in and out.
const SLICE_FADE: f64 = 16.0;

/// How a sample loops when it reaches its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    None,
    Forward,
    /// Ping-pong: reverse direction at each end.
    Bidirectional,
}

/// Plays one channel of a table, once or looped, from the start of the note.
///
/// Pitch is `freq / root`, then `pitch` semitones, then `rate`, so a note at
/// the sample's root plays it as recorded. The sample is read with cubic
/// interpolation at whatever rate the engine runs. The gate is ignored: a
/// sample plays to its end unless the voice is stopped (choked, or panicked),
/// unless it loops.
///
/// A player given a `range` (a slice of the sample) fades in and out at its
/// edges so the cut doesn't click (one-shot only).
///
/// In a kit, `member` is the sample's place in the kit and the player stays
/// silent unless the `sample` port selects it.
#[derive(Clone)]
pub struct Sampler {
    data: Arc<Table>,
    root_hz: f32,
    member: Option<usize>,
    /// The part of the sample this player covers, in frames.
    range: (usize, usize),
    /// A fixed playback speed on top of the pitch, to fit a loop to a tempo.
    speed: f64,
    loop_mode: LoopMode,
    /// Playback direction: +1 forward, -1 reverse (bidirectional loops).
    direction: f64,
    position: f64,
    started: bool,
    done: bool,
    sample_rate: f32,
}

impl Sampler {
    pub fn new(data: Arc<Table>, root_hz: f32, member: Option<usize>) -> Self {
        let range = (0, data.len());
        Sampler {
            data,
            root_hz,
            member,
            range,
            speed: 1.0,
            loop_mode: LoopMode::None,
            direction: 1.0,
            position: 0.0,
            started: false,
            done: false,
            sample_rate: 48_000.0,
        }
    }

    /// Play `speed` times as fast, whatever the pitch asked for.
    pub fn with_speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    pub fn with_loop(mut self, mode: LoopMode) -> Self {
        self.loop_mode = mode;
        self
    }

    /// Play only frames `start..end` of the sample.
    pub fn slice(mut self, start: usize, end: usize) -> Self {
        self.range = (start, end.min(self.data.len()));
        self
    }

    fn at(&self, i: i64) -> f32 {
        let buffer = self.data.data();
        let (first, last) = (self.range.0 as i64, self.range.1 as i64);
        if i < first || i >= last {
            0.0
        } else {
            buffer.get(i as usize).copied().unwrap_or(0.0)
        }
    }

    fn read(&self) -> f32 {
        let i = self.position.floor();
        let t = (self.position - i) as f32;
        let i = i as i64;
        let (a, b, c, d) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        // Catmull-Rom spline through b and c.
        b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
    }

    fn source_rate(&self) -> f32 {
        self.data.sample_rate_or(self.sample_rate)
    }
}

impl Opcode for Sampler {
    fn reset(&mut self) {
        self.position = 0.0;
        self.direction = 1.0;
        self.started = false;
        self.done = false;
    }
    fn name(&self) -> &'static str {
        "sample"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::optional("freq", 0.0),
            Port::optional("pitch", 0.0),
            Port::optional("rate", 1.0),
            Port::optional("start", 0.0),
            Port::optional("sample", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Sampler {
            position: 0.0,
            direction: 1.0,
            started: false,
            done: false,
            ..self.clone()
        })
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if !self.started {
            self.started = true;
            let selected = self.member.is_none_or(|m| inputs[4][0].round() as i64 == m as i64);
            if selected && !self.data.is_empty() {
                self.position = self.range.0 as f64
                    + f64::from(inputs[3][0].max(0.0)) * f64::from(self.source_rate());
            } else {
                self.done = true;
            }
        }
        if self.done {
            out.fill(0.0);
            return;
        }

        let (first, last) = (self.range.0 as f64, self.range.1 as f64);
        let sliced = self.range != (0, self.data.len()) && self.loop_mode == LoopMode::None;
        let source_rate = f64::from(self.source_rate());
        for (n, y) in out.iter_mut().enumerate() {
            if self.position >= last || self.position < first {
                match self.loop_mode {
                    LoopMode::None => {
                        self.done = true;
                        *y = 0.0;
                        continue;
                    }
                    LoopMode::Forward => {
                        self.position = first + (self.position - first).rem_euclid(last - first);
                    }
                    LoopMode::Bidirectional => {
                        self.direction = -self.direction;
                        self.position = self.position.clamp(first, last - f64::EPSILON);
                    }
                }
            }
            *y = self.read();
            if sliced {
                let edge = (self.position - first).min(last - self.position) / SLICE_FADE;
                *y *= edge.clamp(0.0, 1.0) as f32;
            }
            let pitch = f64::from(inputs[1][n]) / 12.0;
            let ratio = f64::from(inputs[0][n]) / f64::from(self.root_hz)
                * pitch.exp2()
                * f64::from(inputs[2][n])
                * self.speed;
            self.position +=
                self.direction * ratio.max(0.0) * source_rate / f64::from(self.sample_rate);
        }
    }

    fn is_active(&self) -> bool {
        !self.done
    }

    /// A sample plays out, so like an envelope it decides when its voice ends.
    fn is_envelope(&self) -> bool {
        self.loop_mode == LoopMode::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ProcessCtx {
        ProcessCtx {
            sample_rate: 48_000.0,
            gate: true,
        }
    }

    fn ramp(len: usize, rate: f32) -> Arc<Table> {
        Arc::new(Table::from_f32(
            (0..len).map(|i| i as f32).collect::<Box<[f32]>>(),
            Some(rate),
        ))
    }

    fn sampler(data: Arc<Table>, member: Option<usize>) -> Sampler {
        let mut s = Sampler::new(data, 440.0, member);
        s.prepare(48_000.0);
        s
    }

    #[test]
    fn plays_at_unity_when_freq_matches_root() {
        let data = ramp(100, 48_000.0);
        let mut s = sampler(data, None);
        let freq = vec![440.0f32; 8];
        let pitch = vec![0.0f32; 8];
        let rate = vec![1.0f32; 8];
        let start = vec![0.0f32; 8];
        let sample = vec![0.0f32; 8];
        let mut out = vec![0.0f32; 8];
        s.process(&ctx(), &[&freq, &pitch, &rate, &start, &sample], &mut out);
        assert!((out[0] - 0.0).abs() < 0.01);
        assert!((out[7] - 7.0).abs() < 0.1);
    }

    #[test]
    fn kit_member_stays_silent_when_not_selected() {
        let data = ramp(100, 48_000.0);
        let mut s = sampler(data, Some(1));
        let freq = vec![440.0f32; 4];
        let pitch = vec![0.0f32; 4];
        let rate = vec![1.0f32; 4];
        let start = vec![0.0f32; 4];
        let sample = vec![0.0f32; 4]; // selects member 0
        let mut out = vec![1.0f32; 4];
        s.process(&ctx(), &[&freq, &pitch, &rate, &start, &sample], &mut out);
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn slice_fades_at_edges() {
        let data = Arc::new(Table::from_f32(vec![1.0; 1000].into_boxed_slice(), Some(48_000.0)));
        let mut s = sampler(data, None).slice(100, 200);
        let freq = vec![440.0f32; 1];
        let pitch = vec![0.0f32; 1];
        let rate = vec![1.0f32; 1];
        let start = vec![0.0f32; 1];
        let sample = vec![0.0f32; 1];
        let mut out = vec![0.0f32; 1];
        s.process(&ctx(), &[&freq, &pitch, &rate, &start, &sample], &mut out);
        // first sample of a slice is near the fade-in edge
        assert!(out[0] < 0.2, "{}", out[0]);
    }
}
