use std::sync::{Arc, LazyLock};

use crate::opcode::{Opcode, Port, ProcessCtx};
use crate::table::Table;

pub(super) const PORTS: &[Port] = &[
    Port::optional("freq", 0.0),
    Port::optional("pitch", 0.0),
    Port::optional("rate", 1.0),
    Port::optional("start", 0.0),
    Port::optional("sample", 0.0),
    Port::optional("begin", 0.0),
    Port::optional("stretch", 1.0),
    // 0: granular, 1: WSOLA, 2: transient-aware WSOLA.
    Port::optional("stretch_mode", 0.0),
    Port::optional("loop_start", 0.0),
    Port::optional("loop_end", 1.0),
    Port::optional("loop_crossfade", 0.0),
    // Target duration of the whole source, in seconds; zero means unfitted playback.
    Port::optional("duration", 0.0),
    // How hard the note is played, 0 to 1, which a multisample uses to choose its layers.
    Port::optional("vel", 1.0),
];

/// Frames over which a slice fades in and out.
const SLICE_FADE: f64 = 16.0;

/// Sample playback quality. Cubic is the default; linear is cheaper and rougher, sinc dearer and cleaner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SampleInterp {
    Linear,
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
    /// Shared analysis channel: every channel of a sample makes identical WSOLA decisions.
    stretch_reference: Arc<Table>,
    transients: Arc<[usize]>,
    root_hz: f32,
    member: Option<usize>,
    /// The part of the sample this player covers, in frames.
    range: (usize, usize),
    repitch: bool,
    loop_mode: LoopMode,
    loop_range: (usize, usize),
    /// Loop points in frames, for a loop the file fixes rather than the `loop_start` and `loop_end` ports.
    fixed_loop: Option<(usize, usize)>,
    looped: bool,
    interp: SampleInterp,
    /// Playback direction: +1 forward, -1 reverse (bidirectional loops).
    direction: f64,
    position: f64,
    grain_phase: f64,
    grain_offsets: [f64; 2],
    read_bounds: Option<(f64, f64)>,
    started: bool,
    done: bool,
    sample_rate: f32,
}

impl Sampler {
    pub fn new(data: Arc<Table>, root_hz: f32, member: Option<usize>) -> Self {
        let range = (0, data.len());
        Sampler {
            stretch_reference: data.clone(),
            transients: Arc::from([]),
            data,
            root_hz,
            member,
            range,
            repitch: false,
            loop_mode: LoopMode::None,
            loop_range: range,
            fixed_loop: None,
            looped: false,
            interp: SampleInterp::Cubic,
            direction: 1.0,
            position: 0.0,
            grain_phase: 0.0,
            grain_offsets: [0.0; 2],
            read_bounds: None,
            started: false,
            done: false,
            sample_rate: 48_000.0,
        }
    }

    /// Use the same reference on every channel to preserve the stereo image.
    pub fn with_stretch_reference(mut self, reference: Arc<Table>) -> Self {
        self.stretch_reference = reference;
        self
    }

    pub fn with_transients(mut self, transients: Arc<[usize]>) -> Self {
        self.transients = transients;
        self
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
        if self.range.0 >= self.range.1 {
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
            let index = self.loop_position(i as f64) as i64;
            sum += f64::from(self.at(index)) * w;
            weight += w;
        }
        if weight.abs() < 1e-12 {
            0.0
        } else {
            (sum / weight) as f32
        }
    }

    /// Fit duration by changing pitch as well as speed, instead of stretching.
    pub fn with_repitch(mut self, repitch: bool) -> Self {
        self.repitch = repitch;
        self
    }

    pub fn with_loop(mut self, mode: LoopMode) -> Self {
        self.loop_mode = mode;
        self
    }

    /// The pitch at which the sample plays as recorded.
    pub fn root_hz(&self) -> f32 {
        self.root_hz
    }

    /// Loop frames `start..end` of the sample, whatever the loop ports say.
    pub fn with_loop_frames(mut self, mode: LoopMode, start: usize, end: usize) -> Self {
        self.loop_mode = mode;
        self.fixed_loop = (start < end && end <= self.data.len()).then_some((start, end));
        self
    }

    /// Play only frames `start..end` of the sample.
    pub fn slice(mut self, start: usize, end: usize) -> Self {
        self.range = (start, end.min(self.data.len()));
        self.loop_range = self.range;
        self
    }

    fn at(&self, i: i64) -> f32 {
        let i = self.loop_position(i as f64) as i64;
        if self
            .read_bounds
            .is_some_and(|(first, last)| (i as f64) < first || i as f64 >= last)
        {
            return 0.0;
        }
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

    /// A straight line between the two frames around the position.
    fn read_linear(&self) -> f32 {
        let i = self.position.floor();
        let t = (self.position - i) as f32;
        let i = i as i64;
        let (a, b) = (self.at(i), self.at(i + 1));
        a + t * (b - a)
    }

    fn loop_position(&self, position: f64) -> f64 {
        let first = self.loop_range.0 as f64;
        let length = (self.loop_range.1 - self.loop_range.0) as f64;
        if !self.looped
            && self.loop_range != self.range
            && ((self.direction > 0.0 && position < first)
                || (self.direction < 0.0 && position >= first + length))
        {
            return position;
        }
        match self.loop_mode {
            LoopMode::None => position,
            LoopMode::Forward => first + (position - first).rem_euclid(length),
            LoopMode::Bidirectional => {
                let span = length - 1.0;
                if span <= 0.0 {
                    return first;
                }
                let p = (position - first).rem_euclid(2.0 * span);
                first + if p <= span { p } else { 2.0 * span - p }
            }
        }
    }

    fn analysis_at(&self, position: f64) -> f64 {
        let position = self.loop_position(position);
        let i = position.floor();
        let at = |index: i64| {
            let index = self.loop_position(index as f64) as i64;
            if index < self.range.0 as i64 || index >= self.range.1 as i64 {
                0.0
            } else {
                f64::from(
                    self.stretch_reference
                        .data()
                        .get(index as usize)
                        .copied()
                        .unwrap_or(0.0),
                )
            }
        };
        let a = at(i as i64);
        a + (at(i as i64 + 1) - a) * (position - i)
    }

    /// Align a new grain near the transport with the continuing grain. Search is
    /// bounded: 65 coarse candidates, then a small local refinement at each scale.
    /// Only a fixed stack buffer is needed, even for long samples and many voices.
    fn aligned_offset(
        &self,
        transport: f64,
        continuing: f64,
        step: f64,
        bounds: Option<(f64, f64)>,
    ) -> f64 {
        const POINTS: usize = 128;
        if step <= 0.0 || !step.is_finite() {
            return 0.0;
        }
        let stride = self.direction * step;
        let mut reference = [0.0; POINTS];
        let mut energy = 0.0;
        for (i, value) in reference.iter_mut().enumerate() {
            *value = self.analysis_at(transport + continuing + i as f64 * stride);
            energy += *value * *value;
        }
        if energy < 1e-12 {
            return 0.0;
        }
        let radius = (f64::from(self.source_rate()) * 0.01)
            .round()
            .clamp(1.0, 2048.0) as i32;
        let score = |offset: i32| {
            let start = transport + f64::from(offset);
            let end = start + (POINTS - 1) as f64 * stride;
            if bounds
                .is_some_and(|(first, last)| start.min(end) < first || start.max(end) >= last - 1.0)
            {
                return f64::NEG_INFINITY;
            }
            if self.loop_mode == LoopMode::None
                && (start.min(end) < self.range.0 as f64
                    || start.max(end) >= self.range.1 as f64 - 1.0)
            {
                return f64::NEG_INFINITY;
            }
            let mut dot = 0.0;
            let mut candidate_energy = 0.0;
            for (i, value) in reference.iter().enumerate() {
                let candidate = self.analysis_at(start + i as f64 * stride);
                dot += value * candidate;
                candidate_energy += candidate * candidate;
            }
            if candidate_energy < 1e-12 {
                f64::NEG_INFINITY
            } else {
                dot / (energy * candidate_energy).sqrt()
            }
        };
        let mut best: i32 = 0;
        let mut best_score = score(0);
        let mut spacing = (radius + 31) / 32;
        let consider = |offset: i32, best: &mut i32, best_score: &mut f64| {
            if offset.abs() > radius {
                return;
            }
            let value = score(offset);
            // Prefer the nominal position when the signal is constant or silent.
            if value > *best_score + 1e-9
                || ((value - *best_score).abs() <= 1e-9 && offset.abs() < best.abs())
            {
                *best = offset;
                *best_score = value;
            }
        };
        for i in -32..=32 {
            consider(i * spacing, &mut best, &mut best_score);
        }
        while spacing > 1 {
            let center = best;
            let next = (spacing / 4).max(1);
            for offset in (center - spacing..=center + spacing).step_by(next as usize) {
                consider(offset, &mut best, &mut best_score);
            }
            spacing = next;
        }
        f64::from(best)
    }

    /// A piecewise time map: attacks play at native pitch speed, and the body
    /// absorbs the duration change. Every detected onset retains its target time.
    /// The virtual transport still determines duration and loop boundaries.
    fn transient_position(&self, position: f64, stretch: f64) -> (f64, bool, (f64, f64)) {
        let loop_region = self.loop_mode != LoopMode::None
            && (self.looped || position >= self.loop_range.0 as f64);
        let (first, last) = if loop_region {
            self.loop_range
        } else if self.loop_mode != LoopMode::None {
            (self.range.0, self.loop_range.0)
        } else {
            self.range
        };
        let next = self.transients.partition_point(|&at| at as f64 <= position);
        let start = next
            .checked_sub(1)
            .map_or(first, |i| self.transients[i].max(first));
        let end = self.transients.get(next).copied().unwrap_or(last).min(last);
        let gap = end.saturating_sub(start) as f64;
        if gap <= 1.0 {
            return (position, false, (first as f64, last as f64));
        }
        // Reserve room for a body even at large compression ratios / short pieces.
        let protected = (f64::from(self.source_rate()) * 0.015)
            .min(gap * 0.25)
            .min(gap * stretch * 0.5);
        let attack_end = start as f64 + protected;
        let distance = (position - start as f64).clamp(0.0, gap);
        let attack_time = protected / stretch;
        if distance < attack_time {
            (
                start as f64 + distance * stretch,
                true,
                (start as f64, end as f64),
            )
        } else {
            let body_speed = (gap - protected) / (gap - attack_time);
            (
                attack_end + (distance - attack_time) * body_speed,
                false,
                (attack_end, end as f64),
            )
        }
    }

    fn requested_loop(&self, inputs: &[&[f32]], n: usize) -> (usize, usize) {
        if let Some(fixed) = self.fixed_loop {
            return fixed;
        }
        let frames = self.range.1.saturating_sub(self.range.0);
        let point = |port: usize, default: f32| {
            let value = inputs.get(port).map_or(default, |input| input[n]);
            let value = if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                default
            };
            self.range.0 + (f64::from(value) * frames as f64).round() as usize
        };
        let (from, to) = (point(8, 0.0), point(9, 1.0));
        if from < to { (from, to) } else { self.range }
    }

    fn crossfade_frames(&self, inputs: &[&[f32]], n: usize) -> f64 {
        let seconds = inputs.get(10).map_or(0.0, |input| input[n]);
        if !seconds.is_finite() {
            return 0.0;
        }
        let next = self.requested_loop(inputs, n);
        (f64::from(seconds.max(0.0)) * f64::from(self.source_rate()))
            .min(self.loop_range.1.saturating_sub(self.loop_range.0 + 1) as f64 * 0.5)
            .min(next.1.saturating_sub(next.0 + 1) as f64 * 0.5)
    }

    fn read_loop(&mut self, step: f64, fade: f64, next_range: (usize, usize)) -> f32 {
        let read = |s: &Self| match s.interp {
            SampleInterp::Linear => s.read_linear(),
            SampleInterp::Cubic => s.read(),
            SampleInterp::Sinc => s.read_sinc(step),
        };
        let original = read(self);
        if self.loop_mode == LoopMode::None || fade <= 0.0 {
            return original;
        }
        let (from, to) = (self.loop_range.0 as f64, self.loop_range.1 as f64);
        let boundary = if self.direction > 0.0 {
            to - if self.loop_mode == LoopMode::Bidirectional {
                1.0
            } else {
                0.0
            }
        } else {
            from
        };
        let distance = (boundary - self.position) * self.direction;
        if distance < 0.0 || distance >= fade {
            return original;
        }
        let position = self.position;
        let range = self.loop_range;
        let bounds = self.read_bounds;
        self.loop_range = next_range;
        self.read_bounds = None;
        let (next_from, next_to) = (next_range.0 as f64, next_range.1 as f64);
        self.position = if self.loop_mode == LoopMode::Bidirectional {
            let next_boundary = if self.direction > 0.0 {
                next_to - 1.0
            } else {
                next_from
            };
            next_boundary - self.direction * (fade - distance)
        } else if self.direction > 0.0 {
            next_from + fade - distance
        } else {
            next_to - fade + distance
        };
        let incoming = read(self);
        self.position = position;
        self.loop_range = range;
        self.read_bounds = bounds;
        let weight = (1.0 - distance / fade) as f32;
        original * (1.0 - weight) + incoming * weight
    }

    fn source_rate(&self) -> f32 {
        self.data.sample_rate_or(self.sample_rate)
    }
}

impl Opcode for Sampler {
    fn reset(&mut self) {
        self.position = 0.0;
        self.loop_range = self.range;
        self.looped = false;
        self.grain_phase = 0.0;
        self.grain_offsets = [0.0; 2];
        self.read_bounds = None;
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
            loop_range: self.range,
            looped: false,
            grain_phase: 0.0,
            grain_offsets: [0.0; 2],
            read_bounds: None,
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
                let frames = self.range.1.saturating_sub(self.range.0);
                self.loop_range = self.requested_loop(inputs, 0);
                if frames == 0 {
                    self.done = true;
                    out.fill(0.0);
                    return;
                }
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
            if self.loop_mode == LoopMode::None {
                if self.position >= last || self.position < first {
                    self.done = true;
                    *y = 0.0;
                    continue;
                }
            } else {
                let (from, to) = (self.loop_range.0 as f64, self.loop_range.1 as f64);
                let crossed = if self.direction > 0.0 {
                    self.position >= to || (self.looped && self.position < from)
                } else {
                    self.position < from || (self.looped && self.position >= to)
                };
                if crossed {
                    let old_range = self.loop_range;
                    let fade = self.crossfade_frames(inputs, n);
                    self.loop_range = self.requested_loop(inputs, n);
                    let (from, to) = (self.loop_range.0 as f64, self.loop_range.1 as f64);
                    let overshoot = if self.direction > 0.0 {
                        self.position - old_range.1 as f64
                    } else {
                        old_range.0 as f64 - self.position
                    };
                    self.looped = true;
                    match self.loop_mode {
                        LoopMode::Forward => {
                            let offset = fade + overshoot.rem_euclid(to - from - fade);
                            self.position = if self.direction > 0.0 {
                                from + offset
                            } else {
                                from + (to - from - offset).rem_euclid(to - from)
                            };
                        }
                        LoopMode::Bidirectional => {
                            if old_range != self.loop_range {
                                self.position = if self.direction > 0.0 {
                                    to + overshoot
                                } else {
                                    from - overshoot
                                };
                            }
                            let span = to - from - 1.0;
                            if span <= 0.0 {
                                self.position = from;
                            } else {
                                let p = (self.position - from).rem_euclid(2.0 * span);
                                self.position = from + if p <= span { p } else { 2.0 * span - p };
                                if p > span {
                                    self.direction = -self.direction;
                                }
                                self.position =
                                    (self.position + self.direction * fade).clamp(from, to - 1.0);
                            }
                        }
                        LoopMode::None => unreachable!(),
                    }
                    self.grain_offsets = [0.0; 2];
                }
            }
            let pitch = f64::from(inputs[1][n]) / 12.0;
            let ratio = f64::from(inputs[0][n]) / f64::from(self.root_hz)
                * if pitch == 0.0 { 1.0 } else { pitch.exp2() }
                * f64::from(inputs[2][n]).abs();
            let mut step = ratio.max(0.0) * source_rate / f64::from(self.sample_rate);
            let stretch = inputs.get(6).map_or(1.0, |input| input[n]);
            let stretch = if stretch.is_finite() {
                stretch.clamp(0.25, 4.0)
            } else {
                1.0
            };
            let mut advance = step / f64::from(stretch);
            let duration = inputs.get(11).map_or(0.0, |input| input[n]);
            if duration.is_finite() && duration > 0.0 {
                let fitted =
                    self.data.len() as f64 / (f64::from(duration) * f64::from(self.sample_rate));
                if self.repitch {
                    step *= fitted / (source_rate / f64::from(self.sample_rate));
                    advance = step / f64::from(stretch);
                } else {
                    advance = if step > 0.0 {
                        fitted / f64::from(stretch)
                    } else {
                        0.0
                    };
                }
            }
            // The grain readers follow pitch; only the transport follows fitted time.
            let stretch = if advance > 0.0 { step / advance } else { 1.0 };
            let mode = inputs.get(7).map_or(0.0, |input| input[n]);
            // Reverse grains use ordinary WSOLA; attack protection is forward-only.
            let transient = mode >= 1.5 && self.direction > 0.0 && stretch != 1.0;
            let (mapped, attack, bounds) = if transient {
                self.transient_position(self.position, stretch)
            } else {
                (self.position, false, (first, last))
            };
            self.read_bounds = transient.then_some(bounds);
            let mapped_advance = if transient {
                let next = self.transient_position(self.position + advance, stretch).0;
                next - mapped
            } else {
                advance
            };

            let fade = self.crossfade_frames(inputs, n);
            if stretch == 1.0 || attack {
                let transport = self.position;
                self.position = mapped;
                *y = self.read_loop(step, fade, self.requested_loop(inputs, n));
                self.position = transport;
                self.grain_offsets = [0.0; 2];
                if attack {
                    self.grain_phase = 0.0;
                }
            } else {
                // Two Hann-windowed grains, 50% overlapped. Their read heads follow
                // pitch while the transport follows duration. No audio-thread allocation.
                let transport = self.position;
                let weight = (std::f64::consts::PI * self.grain_phase).sin().powi(2);
                *y = 0.0;
                for (head, weight) in [weight, 1.0 - weight].into_iter().enumerate() {
                    self.position = mapped + self.grain_offsets[head];
                    if transient {
                        self.position = self
                            .position
                            .clamp(bounds.0, (bounds.1 - 1.0).max(bounds.0));
                    }
                    self.position = self.loop_position(self.position);
                    *y +=
                        weight as f32 * self.read_loop(step, fade, self.requested_loop(inputs, n));
                    self.grain_offsets[head] += self.direction * (step - mapped_advance);
                }
                self.position = transport;
            }
            if sliced {
                let edge = (self.position - first).min(last - self.position) / SLICE_FADE;
                *y *= edge.clamp(0.0, 1.0) as f32;
            }
            self.position += self.direction * advance;
            let previous = self.grain_phase;
            self.grain_phase += 1.0 / (0.05 * f64::from(self.sample_rate));
            let restart = if self.grain_phase >= 1.0 {
                self.grain_phase -= 1.0;
                Some(0)
            } else if previous < 0.5 && self.grain_phase >= 0.5 {
                Some(1)
            } else {
                None
            };
            if let Some(head) = restart {
                self.grain_offsets[head] = if stretch != 1.0 && mode >= 0.5 && !attack {
                    let next = if transient {
                        self.transient_position(self.position, stretch).0
                    } else {
                        self.position
                    };
                    self.aligned_offset(
                        next,
                        self.grain_offsets[1 - head],
                        step,
                        transient.then_some(bounds),
                    )
                } else {
                    0.0
                };
            }
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

    fn tone(frames: usize, hz: f32, rate: f32) -> Arc<Table> {
        Arc::new(Table::from_f32(
            (0..frames)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate).sin())
                .collect::<Vec<_>>(),
            Some(rate),
        ))
    }

    fn render_stretch(
        player: &mut Sampler,
        frames: usize,
        factor: f32,
        mode: f32,
        rate: f32,
        block: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        let freq = vec![440.0; block];
        let zero = vec![0.0; block];
        let rate = vec![rate; block];
        let stretch = vec![factor; block];
        let mode = vec![mode; block];
        for chunk in out.chunks_mut(block) {
            player.process(
                &ctx(),
                &[&freq, &zero, &rate, &zero, &zero, &zero, &stretch, &mode],
                chunk,
            );
        }
        out
    }

    #[test]
    fn fitted_loop_transport_keeps_phase_across_pitch_and_duration_changes() {
        for reverse in [false, true] {
            let mut player =
                sampler(tone(48_000, 440.0, 48_000.0), None).with_loop(LoopMode::Forward);
            let mut elapsed = 0.0;
            for (duration, pitch, frames) in [
                (2.0, 0.0, 120_000),
                (0.8, 7.0, 96_000),
                (1.3, -5.0, 160_000),
            ] {
                for _ in 0..frames / 64 {
                    let freq = [440.0; 64];
                    let pitch = [pitch; 64];
                    let rate = [if reverse { -1.0 } else { 1.0 }; 64];
                    let zero = [0.0; 64];
                    let one = [1.0; 64];
                    let duration = [duration; 64];
                    let mut out = [0.0; 64];
                    player.process(
                        &ctx(),
                        &[
                            &freq, &pitch, &rate, &zero, &zero, &zero, &one, &one, &zero, &one,
                            &zero, &duration,
                        ],
                        &mut out,
                    );
                    assert!(out.iter().all(|v| v.is_finite()));
                    elapsed += 64.0 / f64::from(duration[0]);
                }
            }
            let expected = if reverse { 47_999.0 - elapsed } else { elapsed };
            let error = (player.position - expected).rem_euclid(48_000.0);
            assert!(
                error.min(48_000.0 - error) < 1e-5,
                "reverse {reverse}: {error}"
            );
        }
    }

    #[test]
    fn sustain_loop_plays_intro_then_repeats_only_the_selected_region() {
        let freq = [440.0; 16];
        let zero = [0.0; 16];
        let one = [1.0; 16];
        let from = [0.2; 16];
        let to = [0.6; 16];
        let backwards = [-1.0; 16];
        for (mode, rate, expected) in [
            (
                LoopMode::Forward,
                &one,
                [
                    0., 1., 2., 3., 4., 5., 2., 3., 4., 5., 2., 3., 4., 5., 2., 3.,
                ],
            ),
            (
                LoopMode::Forward,
                &backwards,
                [
                    9., 8., 7., 6., 5., 4., 3., 2., 5., 4., 3., 2., 5., 4., 3., 2.,
                ],
            ),
            (
                LoopMode::Bidirectional,
                &one,
                [
                    0., 1., 2., 3., 4., 5., 4., 3., 2., 3., 4., 5., 4., 3., 2., 3.,
                ],
            ),
        ] {
            let mut player = sampler(ramp(10, 48000.0), None).with_loop(mode);
            let mut out = [0.0; 16];
            player.process(
                &ctx(),
                &[
                    &freq, &zero, rate, &zero, &zero, &zero, &one, &zero, &from, &to,
                ],
                &mut out,
            );
            assert_eq!(out, expected, "{mode:?}, rate {}", rate[0]);
            assert!(player.is_active());
        }
    }

    #[test]
    fn loop_points_update_at_wrap_and_invalid_points_fall_back() {
        let freq = [440.0; 8];
        let zero = [0.0; 8];
        let one = [1.0; 8];
        let mut from = [0.2; 8];
        let mut to = [0.6; 8];
        let mut player = sampler(ramp(10, 48000.0), None).with_loop(LoopMode::Forward);
        let mut out = [0.0; 8];
        player.process(
            &ctx(),
            &[
                &freq, &zero, &one, &zero, &zero, &zero, &one, &zero, &from, &to,
            ],
            &mut out,
        );
        from.fill(0.9);
        to.fill(0.1);
        player.process(
            &ctx(),
            &[
                &freq, &zero, &one, &zero, &zero, &zero, &one, &zero, &from, &to,
            ],
            &mut out,
        );
        assert_eq!(out, [4., 5., 0., 1., 2., 3., 4., 5.]);
        player.reset();
        player.process(
            &ctx(),
            &[
                &freq, &zero, &one, &zero, &zero, &zero, &one, &zero, &from, &to,
            ],
            &mut out,
        );
        assert_eq!(out, [0., 1., 2., 3., 4., 5., 6., 7.]);
        assert_eq!(player.loop_range, (0, 10));
    }

    #[test]
    fn loop_edits_wait_for_the_old_boundary_in_both_directions() {
        for (rate, expected) in [
            (1.0, [4., 5., 6., 7., 4., 5.]),
            (-1.0, [5., 4., 3., 2., 5., 4.]),
        ] {
            let mut player = sampler(ramp(10, 48000.0), None).with_loop(LoopMode::Forward);
            let render = |player: &mut Sampler, from, to, count| {
                let freq = vec![440.0; count];
                let zero = vec![0.0; count];
                let one = vec![1.0; count];
                let rate = vec![rate; count];
                let from = vec![from; count];
                let to = vec![to; count];
                let mut out = vec![0.0; count];
                player.process(
                    &ctx(),
                    &[
                        &freq, &zero, &rate, &zero, &zero, &zero, &one, &zero, &from, &to,
                    ],
                    &mut out,
                );
                out
            };
            render(&mut player, 0.2, 0.8, 4);
            assert_eq!(render(&mut player, 0.4, 0.6, 6), expected);
            assert!(player.is_active());
        }
    }

    #[test]
    fn crossfade_reduces_wrap_jump_and_is_independent_of_blocks() {
        let data = Arc::new(Table::from_f32(
            (0..1000).map(|i| i as f32 / 1000.0).collect::<Vec<_>>(),
            Some(48000.0),
        ));
        for mode in [LoopMode::Forward, LoopMode::Bidirectional] {
            for rate in [1.0, -1.0] {
                for interp in [SampleInterp::Cubic, SampleInterp::Sinc] {
                    let render = |fade: f32, block: usize, stretch: f32| {
                        let mut player = sampler(data.clone(), None)
                            .with_loop(mode)
                            .with_interp(interp);
                        let mut audio = vec![0.0; 5000];
                        for out in audio.chunks_mut(block) {
                            let freq = vec![440.0; out.len()];
                            let zero = vec![0.0; out.len()];
                            let one = vec![1.0; out.len()];
                            let rate = vec![rate; out.len()];
                            let fade = vec![fade; out.len()];
                            let stretch = vec![stretch; out.len()];
                            player.process(
                                &ctx(),
                                &[
                                    &freq, &zero, &rate, &zero, &zero, &zero, &stretch, &one,
                                    &zero, &one, &fade,
                                ],
                                out,
                            );
                        }
                        audio
                    };
                    for stretch in [1.0, 1.7] {
                        let faded = render(0.002, 64, stretch);
                        assert_eq!(faded, render(0.002, 257, stretch));
                        assert!(
                            faded.iter().all(|v| v.is_finite() && *v > -0.2 && *v < 1.2),
                            "{mode:?}, {interp:?}, rate {rate}, stretch {stretch}: {:?}",
                            faded.iter().fold(
                                (f32::INFINITY, f32::NEG_INFINITY),
                                |(lo, hi), &v| (lo.min(v), hi.max(v))
                            )
                        );
                        if mode == LoopMode::Forward && stretch == 1.0 {
                            let jump = |audio: &[f32]| {
                                audio[500..]
                                    .windows(2)
                                    .map(|w| (w[1] - w[0]).abs())
                                    .fold(0.0, f32::max)
                            };
                            assert!(jump(&faded) < jump(&render(0.0, 64, stretch)) * 0.1);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn transient_stretch_keeps_attack_shape_timing_and_total_duration() {
        let mut data = vec![0.0; 14400];
        let onsets = [0, 4800, 9600];
        for &at in &onsets {
            for i in 0..100 {
                data[at + i] = (i as f32 * 0.31).cos() * (1.0 - i as f32 / 100.0);
            }
        }
        let table = Arc::new(Table::from_f32(data.clone(), Some(48000.0)));
        for factor in [0.5, 2.0, 4.0] {
            let length = (data.len() as f32 * factor) as usize;
            let mut player = sampler(table.clone(), None).with_transients(Arc::from(onsets));
            let out = render_stretch(&mut player, length + 64, factor, 2.0, 1.0, 64);
            let mut expected = vec![0.0; out.len()];
            for &at in &onsets {
                let target = (at as f32 * factor) as usize;
                expected[target..target + 100].copy_from_slice(&data[at..at + 100]);
            }
            let error = out
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(error < 1e-4, "stretch {factor}: {error}");
            assert!(!player.is_active());
            assert!(out[length..].iter().all(|x| *x == 0.0));
        }
    }

    #[test]
    fn wsola_reduces_tonal_cancellation() {
        let data = tone(24000, 997.0, 48000.0);
        let mut granular = sampler(data.clone(), None);
        let mut wsola = sampler(data, None);
        let plain = render_stretch(&mut granular, 24000, 1.7, 0.0, 1.0, 64);
        let aligned = render_stretch(&mut wsola, 24000, 1.7, 1.0, 1.0, 64);
        // With unaligned windows the two tones periodically cancel. Matching
        // their phases should retain the original sine's mean-square level (0.5).
        let power_error = |out: &[f32]| {
            out[4800..]
                .chunks(1200)
                .map(|chunk| {
                    let power = chunk.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>()
                        / chunk.len() as f64;
                    (power - 0.5).abs()
                })
                .sum::<f64>()
        };
        let plain_error = power_error(&plain);
        let aligned_error = power_error(&aligned);
        assert!(
            aligned_error < plain_error * 0.2,
            "granular {plain_error}, WSOLA {aligned_error}"
        );
    }

    #[test]
    fn wsola_keeps_channels_aligned_across_block_sizes() {
        let reference = tone(16000, 173.0, 44100.0);
        let inverted = Arc::new(Table::from_f32(
            reference.data().iter().map(|x| -*x).collect::<Vec<_>>(),
            Some(44100.0),
        ));
        let mut left = sampler(reference.clone(), None).with_stretch_reference(reference.clone());
        let mut other =
            sampler(tone(16000, 997.0, 44100.0), None).with_stretch_reference(reference.clone());
        let mut right = sampler(inverted, None).with_stretch_reference(reference);
        let a = render_stretch(&mut left, 16000, 1.7, 1.0, 1.0, 64);
        let b = render_stretch(&mut right, 16000, 1.7, 1.0, 1.0, 257);
        assert!(a.iter().zip(&b).all(|(a, b)| (a + b).abs() < 1e-6));
        render_stretch(&mut other, 16000, 1.7, 1.0, 1.0, 113);
        assert_eq!(left.grain_offsets, other.grain_offsets);
        assert!(left.grain_offsets.iter().any(|offset| offset.abs() > 1.0));
    }

    #[test]
    fn wsola_respects_slice_boundaries_reverse_and_loops() {
        let mut values = vec![100.0; 5000];
        values[1000..3000].fill(0.0);
        let data = Arc::new(Table::from_f32(values, Some(48000.0)));
        for loop_mode in [LoopMode::None, LoopMode::Forward, LoopMode::Bidirectional] {
            for rate in [-1.0, 1.0] {
                let mut player = sampler(data.clone(), None)
                    .slice(1000, 3000)
                    .with_loop(loop_mode);
                let out = render_stretch(&mut player, 12000, 2.0, 1.0, rate, 64);
                assert!(out.iter().all(|x| *x == 0.0), "{loop_mode:?}, rate {rate}");
                assert_eq!(player.is_active(), loop_mode != LoopMode::None);
            }
        }
    }

    #[test]
    fn wsola_unity_bypasses_alignment_and_reset_restarts_cleanly() {
        let data = tone(10000, 321.0, 48000.0);
        let mut a = sampler(data.clone(), None);
        let mut b = sampler(data, None);
        assert_eq!(
            render_stretch(&mut a, 4000, 1.0, 0.0, 1.0, 64),
            render_stretch(&mut b, 4000, 1.0, 1.0, 1.0, 64)
        );
        a.reset();
        let first = render_stretch(&mut a, 4000, 2.0, 1.0, 1.0, 64);
        a.reset();
        assert_eq!(first, render_stretch(&mut a, 4000, 2.0, 1.0, 1.0, 257));
        let mut cloned = a.box_clone();
        let freq = [440.0; 64];
        let zero = [0.0; 64];
        let rate = [1.0; 64];
        let stretch = [2.0; 64];
        let mut out = [0.0; 64];
        cloned.process(
            &ctx(),
            &[&freq, &zero, &rate, &zero, &zero, &zero, &stretch, &rate],
            &mut out,
        );
        assert_eq!(out.as_slice(), &first[..64]);
    }

    #[test]
    fn stretch_changes_duration_without_halving_pitch() {
        let data = Arc::new(Table::from_f32(
            (0..4800)
                .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48000.0).sin())
                .collect::<Vec<_>>(),
            Some(48000.0),
        ));
        for (factor, mode) in [0.5, 1.0, 2.0]
            .into_iter()
            .flat_map(|factor| [(factor, 0.0), (factor, 1.0)])
        {
            let mut player = sampler(data.clone(), None);
            let freq = vec![440.0; 10000];
            let zero = vec![0.0; 10000];
            let rate = vec![1.0; 10000];
            let stretch = vec![factor; 10000];
            let mode = vec![mode; 10000];
            let mut out = vec![0.0; 10000];
            player.process(
                &ctx(),
                &[&freq, &zero, &rate, &zero, &zero, &zero, &stretch, &mode],
                &mut out,
            );
            let end = (4800.0 * factor) as usize;
            assert!(out[end..].iter().all(|x| *x == 0.0));
            assert!(!player.is_active());
            // A 1 kHz tone still has roughly 20 rising crossings in 20 ms.
            let crossings = out[600..1560]
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            assert!(
                (18..=22).contains(&crossings),
                "stretch {factor}: {crossings}"
            );
            assert!(out.iter().all(|x| x.is_finite()));
        }
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
    fn linear_reads_a_straight_line_between_frames_and_loops_with_a_crossfade() {
        let mut s = sampler(ramp(100, 48_000.0), None).with_interp(SampleInterp::Linear);
        for (position, expected) in [(40.0, 40.0), (40.25, 40.25), (40.5, 40.5), (98.75, 98.75)] {
            s.position = position;
            assert!((s.read_linear() - expected).abs() < 1e-4, "{position}");
        }
        // between a frame and the next, cubic bends and linear does not
        let data = Arc::new(Table::from_f32(
            vec![0.0, 0.0, 1.0, 0.0, 0.0],
            Some(48_000.0),
        ));
        let mut s = sampler(data, None).with_interp(SampleInterp::Linear);
        s.position = 1.5;
        assert!((s.read_linear() - 0.5).abs() < 1e-6 && (s.read() - 0.5625).abs() < 1e-6);
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
        s.process(
            &ctx(),
            &[&freq, &pitch, &[-1.0; 8], &start, &sample],
            &mut out,
        );
        assert!(
            (out[0] - 99.0).abs() < 0.01 && (out[7] - 92.0).abs() < 0.1,
            "{out:?}"
        );
        // an offset counts from the end too, and running out of sample ends the note
        let mut s = sampler(ramp(100, 48_000.0), None);
        s.process(
            &ctx(),
            &[&freq, &pitch, &[-1.0; 8], &[10.0 / 48_000.0; 8], &sample],
            &mut out,
        );
        assert!((out[0] - 89.0).abs() < 0.01, "{out:?}");
        let mut s = sampler(ramp(10, 48_000.0), None);
        s.process(
            &ctx(),
            &[&freq, &pitch, &[-1.0; 8], &start, &sample],
            &mut out,
        );
        s.process(
            &ctx(),
            &[&freq, &pitch, &[-1.0; 8], &start, &sample],
            &mut out,
        );
        assert!(
            out[3..].iter().all(|&x| x == 0.0) && !s.is_active(),
            "{out:?}"
        );
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
