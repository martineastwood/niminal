use std::sync::Arc;

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Recorded audio: one buffer per channel, at the rate it was recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleData {
    pub channels: Vec<Vec<f32>>,
    pub sample_rate: f32,
}

impl SampleData {
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Frames over which a slice fades in and out.
const SLICE_FADE: f64 = 16.0;

/// Plays one channel of a sample, once, from the start of the note.
///
/// Pitch is `freq / root`, then `pitch` semitones, then `rate`, so a note at
/// the sample's root plays it as recorded. The sample is read with cubic
/// interpolation at whatever rate the engine runs. The gate is ignored: a
/// sample plays to its end unless the voice is stopped (choked, or panicked).
///
/// A player given a `range` (a slice of the sample) fades in and out at its
/// edges so the cut doesn't click.
///
/// In a kit, `member` is the sample's place in the kit and the player stays
/// silent unless the `sample` port selects it.
#[derive(Clone)]
pub struct Sampler {
    data: Arc<SampleData>,
    channel: usize,
    root_hz: f32,
    member: Option<usize>,
    /// The part of the sample this player covers, in frames.
    range: (usize, usize),
    /// A fixed playback speed on top of the pitch, to fit a loop to a tempo.
    speed: f64,
    position: f64,
    started: bool,
    done: bool,
    sample_rate: f32,
}

impl Sampler {
    pub fn new(data: Arc<SampleData>, channel: usize, root_hz: f32, member: Option<usize>) -> Self {
        let range = (0, data.len());
        Sampler { data, channel, root_hz, member, range, speed: 1.0, position: 0.0, started: false, done: false, sample_rate: 48_000.0 }
    }

    /// Play `speed` times as fast, whatever the pitch asked for.
    pub fn with_speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    /// Play only frames `start..end` of the sample.
    pub fn slice(mut self, start: usize, end: usize) -> Self {
        self.range = (start, end.min(self.data.len()));
        self
    }

    fn at(&self, i: i64) -> f32 {
        let buffer = &self.data.channels[self.channel];
        if i < 0 { 0.0 } else { buffer.get(i as usize).copied().unwrap_or(0.0) }
    }

    fn read(&self) -> f32 {
        let i = self.position.floor();
        let t = (self.position - i) as f32;
        let i = i as i64;
        let (a, b, c, d) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        // Catmull-Rom spline through b and c.
        b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
    }
}

impl Opcode for Sampler {
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
        Box::new(Sampler { position: 0.0, started: false, done: false, ..self.clone() })
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if !self.started {
            self.started = true;
            let selected = self.member.is_none_or(|m| inputs[4][0].round() as i64 == m as i64);
            if selected && !self.data.is_empty() {
                self.position = self.range.0 as f64 + f64::from(inputs[3][0].max(0.0)) * f64::from(self.data.sample_rate);
            } else {
                self.done = true;
            }
        }
        if self.done {
            out.fill(0.0);
            return;
        }

        let (first, last) = (self.range.0 as f64, self.range.1 as f64);
        let sliced = self.range != (0, self.data.len());
        for (n, y) in out.iter_mut().enumerate() {
            if self.position >= last {
                self.done = true;
                *y = 0.0;
                continue;
            }
            *y = self.read();
            if sliced {
                let edge = (self.position - first).min(last - self.position) / SLICE_FADE;
                *y *= edge.clamp(0.0, 1.0) as f32;
            }
            let pitch = f64::from(inputs[1][n]) / 12.0;
            let ratio = f64::from(inputs[0][n]) / f64::from(self.root_hz) * pitch.exp2() * f64::from(inputs[2][n]) * self.speed;
            self.position += ratio.max(0.0) * f64::from(self.data.sample_rate) / f64::from(self.sample_rate);
        }
    }

    fn is_active(&self) -> bool {
        !self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ProcessCtx {
        ProcessCtx { sample_rate: 48_000.0, gate: true }
    }

    fn ramp(len: usize, rate: f32) -> Arc<SampleData> {
        Arc::new(SampleData { channels: vec![(0..len).map(|i| i as f32).collect()], sample_rate: rate })
    }

    /// Run a sampler for `blocks` blocks with constant inputs.
    fn run(s: &mut Sampler, freq: f32, pitch: f32, rate: f32, start: f32, sample: f32, blocks: usize) -> Vec<f32> {
        let n = crate::BLOCK;
        let (f, p, r, st, sm) = (vec![freq; n], vec![pitch; n], vec![rate; n], vec![start; n], vec![sample; n]);
        let mut out = Vec::new();
        for _ in 0..blocks {
            let mut block = vec![0.0; n];
            s.process(&ctx(), &[&f, &p, &r, &st, &sm], &mut block);
            out.extend(block);
        }
        out
    }

    fn sampler(data: Arc<SampleData>, member: Option<usize>) -> Sampler {
        let mut s = Sampler::new(data, 0, 100.0, member);
        s.prepare(48_000.0);
        s
    }

    #[test]
    fn plays_as_recorded_at_the_root_and_then_ends() {
        let mut s = sampler(ramp(40, 48_000.0), None);
        let out = run(&mut s, 100.0, 0.0, 1.0, 0.0, 0.0, 2);
        assert!((0..40).all(|i| (out[i] - i as f32).abs() < 1e-4), "{out:?}");
        assert!(out[40..].iter().all(|&y| y == 0.0));
        assert!(!s.is_active());
    }

    #[test]
    fn an_octave_up_plays_twice_as_fast() {
        let mut s = sampler(ramp(200, 48_000.0), None);
        let out = run(&mut s, 200.0, 0.0, 1.0, 0.0, 0.0, 1);
        assert!((out[10] - 20.0).abs() < 1e-3, "{}", out[10]);
        let mut t = sampler(ramp(200, 48_000.0), None);
        let out = run(&mut t, 100.0, 12.0, 1.0, 0.0, 0.0, 1);
        assert!((out[10] - 20.0).abs() < 1e-3, "{}", out[10]);
        let mut u = sampler(ramp(200, 48_000.0), None);
        let out = run(&mut u, 100.0, 0.0, 2.0, 0.0, 0.0, 1);
        assert!((out[10] - 20.0).abs() < 1e-3, "{}", out[10]);
    }

    #[test]
    fn a_file_at_another_rate_still_plays_at_the_right_speed() {
        let mut s = sampler(ramp(400, 24_000.0), None);
        let out = run(&mut s, 100.0, 0.0, 1.0, 0.0, 0.0, 1);
        assert!((out[10] - 5.0).abs() < 1e-3, "{}", out[10]);
    }

    #[test]
    fn start_skips_into_the_sample() {
        let mut s = sampler(ramp(48_000, 48_000.0), None);
        let out = run(&mut s, 100.0, 0.0, 1.0, 0.25, 0.0, 1);
        assert!((out[0] - 12_000.0).abs() < 1e-2, "{}", out[0]);
    }

    #[test]
    fn a_kit_member_is_silent_unless_selected() {
        let mut yes = sampler(ramp(40, 48_000.0), Some(2));
        assert!(run(&mut yes, 100.0, 0.0, 1.0, 0.0, 2.0, 1)[5] > 0.0);
        let mut no = sampler(ramp(40, 48_000.0), Some(2));
        assert!(run(&mut no, 100.0, 0.0, 1.0, 0.0, 1.0, 1).iter().all(|&y| y == 0.0));
        assert!(!no.is_active());
    }

    #[test]
    fn a_fixed_speed_scales_the_playback() {
        let mut s = sampler(ramp(200, 48_000.0), None).with_speed(0.5);
        let out = run(&mut s, 100.0, 0.0, 1.0, 0.0, 0.0, 1);
        assert!((out[10] - 5.0).abs() < 1e-3, "{}", out[10]);
    }

    #[test]
    fn a_slice_plays_only_its_part_and_fades_at_the_edges() {
        let data = Arc::new(SampleData { channels: vec![vec![1.0; 1000]], sample_rate: 48_000.0 });
        let mut s = Sampler::new(data, 0, 100.0, None).slice(200, 600);
        s.prepare(48_000.0);
        let out = run(&mut s, 100.0, 0.0, 1.0, 0.0, 0.0, 14);
        assert!(out[0].abs() < 0.07 && out[8] > 0.4 && out[40] > 0.99, "fades in: {:?}", &out[..4]);
        assert!(out[392] < 0.55 && out[399] < 0.07, "fades out");
        assert!(out[400..].iter().all(|&y| y == 0.0), "and stops at the end of the slice");
        assert!(!s.is_active());
    }

    #[test]
    fn a_fresh_voice_starts_from_the_beginning() {
        let mut s = sampler(ramp(40, 48_000.0), None);
        run(&mut s, 100.0, 0.0, 1.0, 0.0, 0.0, 2);
        let mut again = s.box_clone();
        assert!(again.is_active());
        let (f, z, one) = (vec![100.0; 32], vec![0.0; 32], vec![1.0; 32]);
        let mut out = vec![0.0; 32];
        again.process(&ctx(), &[&f, &z, &one, &z, &z], &mut out);
        assert_eq!(out[3], 3.0);
    }
}
