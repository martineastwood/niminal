use std::sync::{Arc, LazyLock};

use crate::opcode::{Opcode, Port, ProcessCtx};
use crate::table::Table;

const PORTS: &[Port] = &[
    Port::optional("freq", 0.0),
    Port::optional("pitch", 0.0),
    Port::optional("rate", 1.0),
    Port::optional("start", 0.0),
    Port::optional("sample", 0.0),
    Port::optional("begin", 0.0),
];

/// Frames over which a slice fades in and out.
const SLICE_FADE: f64 = 16.0;

/// Sample playback quality. Cubic is the inexpensive default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SampleInterp {
    #[default]
    Cubic,
    Sinc,
}

// Sixteen-lobe Blackman-windowed sinc, tabulated off the audio thread.
const SINC_RADIUS: f64 = 16.0;
const SINC_DENSITY: f64 = 1024.0;
static SINC: LazyLock<Vec<f64>> = LazyLock::new(|| {
    (0..=(SINC_RADIUS * SINC_DENSITY) as usize)
        .map(|i| {
            let x = i as f64 / SINC_DENSITY;
            let sinc = if i == 0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let phase = std::f64::consts::PI * x / SINC_RADIUS;
            sinc * (0.42 + 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos())
        })
        .collect()
});

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
/// interpolation by default, or rate-aware windowed sinc when selected.
/// The gate is ignored: a
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
    interp: SampleInterp,
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
            interp: SampleInterp::Cubic,
            direction: 1.0,
            position: 0.0,
            started: false,
            done: false,
            sample_rate: 48_000.0,
        }
    }

    pub fn with_interp(mut self, interp: SampleInterp) -> Self {
        if interp == SampleInterp::Sinc {
            LazyLock::force(&SINC);
        }
        self.interp = interp;
        self
    }

    fn read_sinc(&self, step: f64) -> f32 {
        let cutoff = 1.0 / step.max(1.0);
        // Bound realtime work at extreme playback rates (up to 512 taps).
        let radius = (SINC_RADIUS / cutoff).min(255.5);
        let first = self.range.0 as i64;
        let len = (self.range.1 - self.range.0) as i64;
        if len == 0 {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut weight = 0.0;
        for i in (self.position - radius).ceil() as i64..=(self.position + radius).floor() as i64 {
            let x = (i as f64 - self.position).abs() * cutoff * SINC_DENSITY;
            let k = x as usize;
            if k + 1 >= SINC.len() {
                continue;
            }
            let w = SINC[k] + (SINC[k + 1] - SINC[k]) * (x - k as f64);
            let index = match self.loop_mode {
                LoopMode::None => i,
                LoopMode::Forward => first + (i - first).rem_euclid(len),
                LoopMode::Bidirectional => {
                    let p = (i - first).rem_euclid(2 * len);
                    first + if p < len { p } else { 2 * len - 1 - p }
                }
            };
            sum += f64::from(self.at(index)) * w;
            weight += w;
        }
        if weight.abs() < 1e-12 {
            0.0
        } else {
            (sum / weight) as f32
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
            let selected = self
                .member
                .is_none_or(|m| inputs[4][0].round() as i64 == m as i64);
            if selected && !self.data.is_empty() {
                let length = (self.range.1 - self.range.0) as f32 / self.source_rate();
                let begin = inputs.get(5).map_or(0.0, |input| input[0]);
                let start = inputs[3][0] + begin * length;
                let offset = f64::from(start.max(0.0)) * f64::from(self.source_rate());
                // A negative `rate` as the note starts plays the sample backwards, from its end.
                if inputs[2][0] < 0.0 {
                    self.direction = -1.0;
                    self.position = self.range.1 as f64 - 1.0 - offset;
                } else {
                    self.position = self.range.0 as f64 + offset;
                }
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
            let pitch = f64::from(inputs[1][n]) / 12.0;
            let ratio = f64::from(inputs[0][n]) / f64::from(self.root_hz)
                * if pitch == 0.0 { 1.0 } else { pitch.exp2() }
                * f64::from(inputs[2][n]).abs()
                * self.speed;
            let step = ratio.max(0.0) * source_rate / f64::from(self.sample_rate);
            *y = match self.interp {
                SampleInterp::Cubic => self.read(),
                SampleInterp::Sinc => self.read_sinc(step),
            };
            if sliced {
                let edge = (self.position - first).min(last - self.position) / SLICE_FADE;
                *y *= edge.clamp(0.0, 1.0) as f32;
            }
            self.position += self.direction * step;
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

/// A kit channel selects one player at note start. Unselected members never run DSP or allocate
/// per-voice buffers. Missing members are silent (another channel-layout group handles them).
#[derive(Clone)]
pub struct KitSampler {
    players: Arc<[Option<Sampler>]>,
    selected: Option<Sampler>,
    started: bool,
    sample_rate: f32,
}

impl KitSampler {
    pub fn new(players: Vec<Option<Sampler>>) -> Self {
        Self {
            players: players.into(),
            selected: None,
            started: false,
            sample_rate: 48_000.0,
        }
    }
}

impl Opcode for KitSampler {
    fn name(&self) -> &str {
        "sample"
    }
    fn ports(&self) -> &'static [Port] {
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self {
            selected: None,
            started: false,
            ..self.clone()
        })
    }
    fn reset(&mut self) {
        self.selected = None;
        self.started = false;
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if !self.started {
            self.started = true;
            let member = inputs[4][0].round() as i64;
            self.selected = usize::try_from(member)
                .ok()
                .and_then(|i| self.players.get(i))
                .and_then(Option::as_ref)
                .cloned();
            if let Some(player) = &mut self.selected {
                player.prepare(self.sample_rate);
            }
        }
        if let Some(player) = &mut self.selected {
            player.process(ctx, inputs, out);
        } else {
            out.fill(0.0);
        }
    }
    fn is_active(&self) -> bool {
        !self.started || self.selected.as_ref().is_some_and(Sampler::is_active)
    }
    fn is_envelope(&self) -> bool {
        self.players.iter().flatten().any(Sampler::is_envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ProcessCtx {
        ProcessCtx::new(48_000.0, true)
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
    fn sinc_preserves_unity_and_loop_boundaries() {
        let mut values = vec![9.0; 80];
        values[20..60].fill(1.0);
        let data = Arc::new(Table::from_f32(values, Some(48_000.0)));
        for mode in [LoopMode::Forward, LoopMode::Bidirectional] {
            let mut s = sampler(data.clone(), None)
                .slice(20, 60)
                .with_loop(mode)
                .with_interp(SampleInterp::Sinc);
            for pos in [20.0, 20.25, 39.5, 59.75] {
                s.position = pos;
                for step in [0.5, 1.0, 2.0, 8.0] {
                    assert!((s.read_sinc(step) - 1.0).abs() < 1e-6);
                }
            }
        }
        let mut s = sampler(ramp(100, 48_000.0), None).with_interp(SampleInterp::Sinc);
        s.position = 40.0;
        assert!((s.read_sinc(1.0) - 40.0).abs() < 1e-5);
    }

    #[test]
    fn sinc_preserves_passband_at_fractional_positions() {
        let data = Arc::new(Table::from_f32(
            (0..1024)
                .map(|i| (std::f64::consts::TAU * 0.05 * i as f64).sin() as f32)
                .collect::<Vec<_>>(),
            Some(48_000.0),
        ));
        let mut s = sampler(data, None).with_interp(SampleInterp::Sinc);
        for step in [0.5, 1.0, 2.0, 4.0] {
            for i in 300..400 {
                s.position = i as f64 + 0.37;
                let expected = (std::f64::consts::TAU * 0.05 * s.position).sin() as f32;
                assert!((s.read_sinc(step) - expected).abs() < 0.001);
            }
        }
    }

    #[test]
    fn sinc_filters_aliases_when_playback_is_faster() {
        let data = Arc::new(Table::from_f32(
            (0..4096)
                .map(|i| (std::f64::consts::TAU * 0.375 * i as f64).sin() as f32)
                .collect::<Vec<_>>(),
            Some(48_000.0),
        ));
        let mut s = sampler(data, None).with_interp(SampleInterp::Sinc);
        let freq = [440.0; 256];
        let pitch = [0.0; 256];
        let rate = [2.0; 256];
        let start = [512.25 / 48_000.0; 256];
        let select = [0.0; 256];
        let mut out = [0.0; 256];
        s.process(&ctx(), &[&freq, &pitch, &rate, &start, &select], &mut out);
        assert!(out.iter().all(|x| x.abs() < 0.001));
        s.interp = SampleInterp::Cubic;
        s.reset();
        s.process(&ctx(), &[&freq, &pitch, &rate, &start, &select], &mut out);
        assert!(out.iter().any(|x| x.abs() > 0.5));
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
    fn a_negative_rate_at_the_start_plays_backwards_from_the_end() {
        let mut s = sampler(ramp(100, 48_000.0), None);
        let (freq, pitch, start, sample) = ([440.0f32; 8], [0.0f32; 8], [0.0f32; 8], [0.0f32; 8]);
        let mut out = [0.0f32; 8];
        s.process(&ctx(), &[&freq, &pitch, &[-1.0; 8], &start, &sample], &mut out);
        assert!((out[0] - 99.0).abs() < 0.01 && (out[7] - 92.0).abs() < 0.1, "{out:?}");
        // an offset counts from the end too, and running out of sample ends the note
        let mut s = sampler(ramp(100, 48_000.0), None);
        s.process(&ctx(), &[&freq, &pitch, &[-1.0; 8], &[10.0 / 48_000.0; 8], &sample], &mut out);
        assert!((out[0] - 89.0).abs() < 0.01, "{out:?}");
        let mut s = sampler(ramp(10, 48_000.0), None);
        s.process(&ctx(), &[&freq, &pitch, &[-1.0; 8], &start, &sample], &mut out);
        s.process(&ctx(), &[&freq, &pitch, &[-1.0; 8], &start, &sample], &mut out);
        assert!(out[3..].iter().all(|&x| x == 0.0) && !s.is_active(), "{out:?}");
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
        let data = Arc::new(Table::from_f32(
            vec![1.0; 1000].into_boxed_slice(),
            Some(48_000.0),
        ));
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
