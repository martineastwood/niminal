//! Pair persistent partial tracks in relative-frequency space and morph their sine resynthesis.
use super::{
    phase_vocoder::Analysis,
    spectral_frames::SpectralFrames,
    spectral_partials::{PartialTracker, Track},
};
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};
use std::f64::consts::{LN_2, TAU};

#[derive(Clone, Copy)]
struct Binding {
    slot: usize,
    id: u64,
}
impl Binding {
    fn get(self, tracker: &PartialTracker) -> Option<&Track> {
        tracker
            .tracks
            .get(self.slot)
            .filter(|t| t.active && t.id == self.id)
    }
}
#[derive(Clone, Copy, Default)]
struct Pair {
    a: Option<Binding>,
    b: Option<Binding>,
    frequency: f64,
    amplitude: f64,
    target_frequency: f64,
    target_amplitude: f64,
    phase: f64,
}
impl Pair {
    fn free(&self) -> bool {
        self.a.is_none() && self.b.is_none() && self.amplitude < 1e-12
    }
    fn targets(&mut self, a: &PartialTracker, b: &PartialTracker, magnitude: f64, frequency: f64) {
        let a = self.a.and_then(|key| key.get(a));
        let b = self.b.and_then(|key| key.get(b));
        self.target_amplitude = a.map_or(0.0, |t| t.target_amplitude) * (1.0 - magnitude)
            + b.map_or(0.0, |t| t.target_amplitude) * magnitude;
        self.target_frequency = match (a, b) {
            (Some(a), Some(b)) => {
                a.target_frequency * (b.target_frequency / a.target_frequency).powf(frequency)
            }
            (Some(t), None) | (None, Some(t)) => t.target_frequency,
            (None, None) => self.frequency,
        };
        if self.frequency == 0.0
            && let Some(t) = if magnitude < 0.5 { a.or(b) } else { b.or(a) }
        {
            self.frequency = self.target_frequency;
            self.phase = t.analysis_phase;
        }
    }
}

struct PairBank {
    pairs: Vec<Pair>,
    used_a: Vec<bool>,
    used_b: Vec<bool>,
    order: Vec<usize>,
}
impl PairBank {
    fn new(capacity: usize) -> Self {
        Self {
            // Each side may contribute entirely unmatched tracks.
            pairs: vec![Pair::default(); capacity * 2],
            used_a: vec![false; capacity],
            used_b: vec![false; capacity],
            order: Vec::with_capacity(capacity),
        }
    }
    fn reset(&mut self) {
        self.pairs.fill(Pair::default());
    }
    fn reference(tracker: &PartialTracker) -> f64 {
        tracker
            .tracks
            .iter()
            .filter(|t| t.active && t.target_amplitude > 0.0)
            .map(|t| t.target_frequency)
            .fold(f64::INFINITY, f64::min)
    }
    fn nearest(
        tracker: &PartialTracker,
        used: &[bool],
        reference: f64,
        relative: f64,
    ) -> Option<Binding> {
        tracker
            .tracks
            .iter()
            .enumerate()
            .filter(|(i, t)| !used[*i] && t.active && t.target_amplitude > 0.0)
            .map(|(slot, t)| {
                (
                    slot,
                    ((t.target_frequency / reference) / relative).ln().abs(),
                )
            })
            // Within three semitones of the relative position; retain missing components separately.
            .filter(|(_, distance)| *distance <= LN_2 * 3.0 / 12.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(slot, _)| Binding {
                slot,
                id: tracker.tracks[slot].id,
            })
    }
    fn update(&mut self, a: &PartialTracker, b: &PartialTracker, magnitude: f64, frequency: f64) {
        self.used_a.fill(false);
        self.used_b.fill(false);
        let base_a = Self::reference(a);
        let base_b = Self::reference(b);
        // A slot's generation prevents a replacement track from inheriting an unrelated pairing.
        for pair in &mut self.pairs {
            pair.a = pair.a.filter(|key| key.get(a).is_some());
            pair.b = pair.b.filter(|key| key.get(b).is_some());
            if let Some(key) = pair.a {
                self.used_a[key.slot] = true;
            }
            if let Some(key) = pair.b {
                self.used_b[key.slot] = true;
            }
            if pair.free() {
                *pair = Pair::default();
            }
        }
        // Let an existing one-sided voice find a partner as that source appears.
        for pair in &mut self.pairs {
            if let (Some(key), None) = (pair.a, pair.b)
                && let Some(t) = key.get(a)
                && t.target_amplitude > 0.0
            {
                pair.b = Self::nearest(b, &self.used_b, base_b, t.target_frequency / base_a);
                if let Some(key) = pair.b {
                    self.used_b[key.slot] = true;
                }
            } else if let (None, Some(key)) = (pair.a, pair.b)
                && let Some(t) = key.get(b)
                && t.target_amplitude > 0.0
            {
                pair.a = Self::nearest(a, &self.used_a, base_a, t.target_frequency / base_b);
                if let Some(key) = pair.a {
                    self.used_a[key.slot] = true;
                }
            }
        }
        self.order.clear();
        self.order.extend(
            a.tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| !self.used_a[*i] && t.active && t.target_amplitude > 0.0)
                .map(|(i, _)| i),
        );
        self.order.sort_unstable_by(|&i, &j| {
            a.tracks[i]
                .target_frequency
                .total_cmp(&a.tracks[j].target_frequency)
        });
        for &slot in &self.order {
            let Some(pair) = self.pairs.iter_mut().find(|pair| pair.free()) else {
                break;
            };
            let partner = Self::nearest(
                b,
                &self.used_b,
                base_b,
                a.tracks[slot].target_frequency / base_a,
            );
            *pair = Pair {
                a: Some(Binding {
                    slot,
                    id: a.tracks[slot].id,
                }),
                b: partner,
                ..Pair::default()
            };
            self.used_a[slot] = true;
            if let Some(key) = partner {
                self.used_b[key.slot] = true;
            }
        }
        for (slot, t) in b.tracks.iter().enumerate() {
            if self.used_b[slot] || !t.active || t.target_amplitude <= 0.0 {
                continue;
            }
            let Some(pair) = self.pairs.iter_mut().find(|pair| pair.free()) else {
                break;
            };
            *pair = Pair {
                b: Some(Binding { slot, id: t.id }),
                ..Pair::default()
            };
            self.used_b[slot] = true;
        }
        for pair in &mut self.pairs {
            pair.targets(a, b, magnitude, frequency);
        }
    }
    fn sample(&mut self, remaining: usize) -> f32 {
        let mut out = 0.0;
        for pair in &mut self.pairs {
            if pair.free() {
                continue;
            }
            if remaining > 0 {
                pair.amplitude += (pair.target_amplitude - pair.amplitude) / remaining as f64;
                pair.frequency += (pair.target_frequency - pair.frequency) / remaining as f64;
            }
            if (0.0..0.5).contains(&pair.frequency) {
                out += pair.amplitude * pair.phase.cos();
            }
            pair.phase = (pair.phase + TAU * pair.frequency).rem_euclid(TAU);
        }
        out as f32
    }
}

pub struct PartialMorph {
    frames_a: SpectralFrames,
    frames_b: SpectralFrames,
    analysis_a: Analysis,
    analysis_b: Analysis,
    a: PartialTracker,
    b: PartialTracker,
    bank: PairBank,
    remaining: usize,
    quiet: usize,
}
impl PartialMorph {
    pub fn new(size: usize, overlap: usize, max_tracks: usize) -> Self {
        let hop = size / overlap;
        Self {
            frames_a: SpectralFrames::new(size, overlap),
            frames_b: SpectralFrames::new(size, overlap),
            analysis_a: Analysis::new(size, hop),
            analysis_b: Analysis::new(size, hop),
            a: PartialTracker::new(size, hop, max_tracks),
            b: PartialTracker::new(size, hop, max_tracks),
            bank: PairBank::new(max_tracks),
            remaining: 0,
            quiet: 0,
        }
    }
}
impl Opcode for PartialMorph {
    fn name(&self) -> &str {
        "partial_morph"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("other"),
            Port::optional("magnitude", 0.5),
            Port::optional("frequency", 0.5),
            Port::optional("count", 32.0),
            Port::optional("threshold", 0.01),
            Port::optional("freeze", 0.0),
        ];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(
            self.frames_a.size,
            self.frames_a.size / self.frames_a.hop,
            self.a.tracks.len(),
        ))
    }
    fn reset(&mut self) {
        self.frames_a.reset();
        self.frames_b.reset();
        self.analysis_a.reset();
        self.analysis_b.reset();
        self.a.reset();
        self.b.reset();
        self.bank.reset();
        self.remaining = 0;
        self.quiet = 0;
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, sample) in out.iter_mut().enumerate() {
            let ready = self.frames_a.analyse_sample(inputs[0][i]);
            self.frames_b.analyse_sample(inputs[1][i]);
            if ready {
                self.analysis_a.read(self.frames_a.spectrum());
                self.analysis_b.read(self.frames_b.spectrum());
                if inputs[6][i] <= 0.5 {
                    let count = (inputs[4][i].max(0.0) as usize).min(self.a.tracks.len());
                    let threshold = inputs[5][i].clamp(0.0, 1.0) as f64;
                    let lowest = 20.0 / ctx.sample_rate as f64;
                    self.a
                        .update(&self.analysis_a.spectrum, count, threshold, lowest, 0.5);
                    self.b
                        .update(&self.analysis_b.spectrum, count, threshold, lowest, 0.5);
                }
                self.bank.update(
                    &self.a,
                    &self.b,
                    inputs[2][i].clamp(0.0, 1.0) as f64,
                    inputs[3][i].clamp(0.0, 1.0) as f64,
                );
                self.remaining = self.frames_a.hop;
            }
            *sample = self.bank.sample(self.remaining);
            self.remaining = self.remaining.saturating_sub(1);
            self.quiet = if sample.abs() < TAIL_LEVEL {
                self.quiet.saturating_add(1)
            } else {
                0
            };
        }
    }
    fn latency(&self) -> usize {
        self.frames_a.size / 2 + self.frames_a.hop
    }
    fn is_active(&self) -> bool {
        self.quiet < self.frames_a.size * 2
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.frames_a.size == old.frames_a.size
            && self.frames_a.hop == old.frames_a.hop
            && self.a.tracks.len() == old.a.tracks.len()
        {
            std::mem::swap(self, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::phase_vocoder::Spectrum;

    fn tracker(peaks: &[(usize, f64)]) -> PartialTracker {
        let mut tracker = PartialTracker::new(4096, 1024, 8);
        update(&mut tracker, peaks);
        tracker
    }
    fn update(tracker: &mut PartialTracker, peaks: &[(usize, f64)]) {
        let mut spectrum = Spectrum::new(4096);
        for &(k, amp) in peaks {
            spectrum.magnitude[k] = amp;
            spectrum.frequency[k] = k as f64 / 4096.0;
        }
        tracker.update(&spectrum, 8, 0.01, 0.0, 0.5);
    }
    #[test]
    fn corresponding_harmonics_pair_across_octaves_and_interpolate_in_intervals() {
        let a = tracker(&[(100, 1.0), (200, 0.5), (300, 0.25)]);
        let b = tracker(&[(200, 0.3), (400, 0.7), (600, 0.5)]);
        let mut bank = PairBank::new(8);
        bank.update(&a, &b, 0.25, 0.5);
        let pairs: Vec<_> = bank
            .pairs
            .iter()
            .filter(|p| p.a.is_some() && p.b.is_some())
            .collect();
        assert_eq!(pairs.len(), 3);
        for pair in pairs {
            let ta = pair.a.unwrap().get(&a).unwrap();
            let tb = pair.b.unwrap().get(&b).unwrap();
            assert_eq!(tb.target_frequency, ta.target_frequency * 2.0);
            assert!((pair.target_frequency - ta.target_frequency * 2.0_f64.sqrt()).abs() < 1e-12);
            assert!(
                (pair.target_amplitude - (ta.target_amplitude * 0.75 + tb.target_amplitude * 0.25))
                    .abs()
                    < 1e-12
            );
        }
    }
    #[test]
    fn missing_harmonics_fade_instead_of_stealing_the_next_harmonic() {
        let a = tracker(&[(100, 1.0), (200, 0.5), (300, 0.25)]);
        let b = tracker(&[(200, 0.3), (600, 0.5)]);
        let mut bank = PairBank::new(8);
        bank.update(&a, &b, 0.0, 0.0);
        let unmatched = bank
            .pairs
            .iter()
            .find(|p| p.a.is_some() && p.b.is_none())
            .unwrap();
        assert_eq!(
            unmatched.a.unwrap().get(&a).unwrap().target_frequency,
            200.0 / 4096.0
        );
        assert!(unmatched.target_amplitude > 0.0);
        bank.update(&a, &b, 1.0, 1.0);
        let unmatched = bank
            .pairs
            .iter()
            .find(|p| p.a.is_some() && p.b.is_none())
            .unwrap();
        assert_eq!(unmatched.target_amplitude, 0.0);
        assert_eq!(
            bank.pairs
                .iter()
                .filter(|p| p.a.is_some() && p.b.is_some())
                .count(),
            2
        );
    }
    #[test]
    fn pair_identity_survives_loudness_reordering_and_a_late_source_can_join() {
        let mut a = tracker(&[(100, 1.0), (200, 0.5)]);
        let mut b = tracker(&[]);
        let mut bank = PairBank::new(8);
        bank.update(&a, &b, 0.0, 0.0);
        let ids: Vec<_> = bank
            .pairs
            .iter()
            .filter_map(|p| p.a.map(|key| key.id))
            .collect();
        update(&mut b, &[(200, 0.2), (400, 1.0)]);
        bank.update(&a, &b, 0.5, 0.5);
        update(&mut a, &[(100, 0.1), (200, 1.0)]);
        bank.update(&a, &b, 0.5, 0.5);
        assert_eq!(
            ids,
            bank.pairs
                .iter()
                .filter_map(|p| p.a.map(|key| key.id))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bank.pairs
                .iter()
                .filter(|p| p.a.is_some() && p.b.is_some())
                .count(),
            2
        );
    }
    #[test]
    fn retired_slot_generations_do_not_inherit_old_partners() {
        let a = tracker(&[(100, 1.0)]);
        let mut b = tracker(&[(200, 1.0)]);
        let mut bank = PairBank::new(8);
        bank.update(&a, &b, 0.5, 0.5);
        let old = bank.pairs[0].b.unwrap();
        for _ in 0..4 {
            update(&mut b, &[]);
        }
        update(&mut b, &[(500, 1.0)]);
        assert!(old.get(&b).is_none());
        assert_eq!(old.slot, b.tracks.iter().position(|t| t.active).unwrap());
        bank.update(&a, &b, 0.5, 0.5);
        assert!(
            bank.pairs
                .iter()
                .filter_map(|p| p.b)
                .all(|key| key.id != old.id)
        );
    }
}
