//! Bounded sinusoidal tracks with stable slots and interpolated oscillator-bank synthesis.
use super::{phase_vocoder::Analysis, spectral_frames::SpectralFrames};
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy, Default)]
struct Track {
    frequency: f64,
    amplitude: f64,
    target_frequency: f64,
    target_amplitude: f64,
    phase: f64,
    gap: usize,
    active: bool,
    matched: bool,
}
#[derive(Clone, Copy)]
struct Peak {
    frequency: f64,
    amplitude: f64,
    phase: f64,
}

pub struct SpectralPartials {
    frames: SpectralFrames,
    analysis: Analysis,
    tracks: Vec<Track>,
    peaks: Vec<Peak>,
    remaining: usize,
    quiet: usize,
}
impl SpectralPartials {
    pub fn new(size: usize, overlap: usize, max_tracks: usize) -> Self {
        Self {
            frames: SpectralFrames::new(size, overlap),
            analysis: Analysis::new(size, size / overlap),
            tracks: vec![Track::default(); max_tracks],
            peaks: Vec::with_capacity(size / 2 + 1),
            remaining: 0,
            quiet: 0,
        }
    }
    fn update(&mut self, count: usize, threshold: f64, lowest: f64, highest: f64) {
        let s = &self.analysis.spectrum;
        let size = self.frames.size as f64;
        let maximum = s.magnitude.iter().copied().fold(0.0, f64::max);
        self.peaks.clear();
        for k in 1..s.magnitude.len() - 1 {
            let m = s.magnitude[k];
            if m <= maximum * threshold
                || m < 1e-9
                || m <= s.magnitude[k - 1]
                || m < s.magnitude[k + 1]
            {
                continue;
            }
            let frequency = s.frequency[k];
            if !(lowest..=highest).contains(&frequency) {
                continue;
            }
            // Hann's main-lobe response corrects the peak amplitude between FFT-bin centres.
            let delta = (frequency * size - k as f64).clamp(-0.5, 0.5);
            let sinc = |x: f64| {
                if x.abs() < 1e-10 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                }
            };
            let response = sinc(delta) + 0.5 * (sinc(delta - 1.0) + sinc(delta + 1.0));
            self.peaks.push(Peak {
                frequency,
                amplitude: 4.0 * m / (size * response),
                // FFT phases refer to the start of the window; synthesize from its centre.
                phase: s.phase[k] + PI * k as f64 - TAU * frequency * self.frames.hop as f64,
            });
        }
        self.peaks
            .sort_unstable_by(|a, b| b.amplitude.total_cmp(&a.amplitude));
        for track in &mut self.tracks {
            track.matched = false;
        }
        for peak in self.peaks.iter().take(count) {
            let nearest = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.active
                        && !t.matched
                        && (t.target_frequency - peak.frequency).abs()
                            <= (3.0 / size).max(t.target_frequency * 0.04)
                })
                .min_by(|(_, a), (_, b)| {
                    (a.target_frequency - peak.frequency)
                        .abs()
                        .total_cmp(&(b.target_frequency - peak.frequency).abs())
                })
                .map(|(i, _)| i);
            let slot = nearest.or_else(|| self.tracks.iter().position(|t| !t.active));
            if let Some(slot) = slot {
                let t = &mut self.tracks[slot];
                if !t.active {
                    *t = Track {
                        frequency: peak.frequency,
                        phase: peak.phase,
                        active: true,
                        ..Track::default()
                    };
                }
                t.target_frequency = peak.frequency;
                t.target_amplitude = peak.amplitude;
                t.matched = true;
                t.gap = 0;
            }
        }
        for t in &mut self.tracks {
            if t.active && !t.matched {
                t.target_amplitude = 0.0;
                t.gap += 1;
                if t.gap > 3 && t.amplitude < 1e-8 {
                    *t = Track::default();
                }
            }
        }
    }
}
impl Opcode for SpectralPartials {
    fn name(&self) -> &str {
        "spectral_partials"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("count", 32.0),
            Port::optional("threshold", 0.01),
            Port::optional("shift", 0.0),
            Port::optional("freeze", 0.0),
            Port::optional("track", 0.0),
            Port::optional("lowest", 20.0),
            Port::optional("highest", 20000.0),
        ];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(
            self.frames.size,
            self.frames.size / self.frames.hop,
            self.tracks.len(),
        ))
    }
    fn reset(&mut self) {
        self.frames.reset();
        self.analysis.reset();
        self.tracks.fill(Track::default());
        self.peaks.clear();
        self.remaining = 0;
        self.quiet = 0;
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, sample) in out.iter_mut().enumerate() {
            if self.frames.analyse_sample(inputs[0][i]) {
                self.analysis.read(self.frames.spectrum());
                if inputs[4][i] <= 0.5 {
                    self.update(
                        (inputs[1][i].max(0.0) as usize).min(self.tracks.len()),
                        inputs[2][i].clamp(0.0, 1.0) as f64,
                        inputs[6][i].max(0.0) as f64 / ctx.sample_rate as f64,
                        inputs[7][i].max(0.0) as f64 / ctx.sample_rate as f64,
                    );
                }
                self.remaining = self.frames.hop;
            }
            let ratio = 2.0_f64.powf(inputs[3][i].clamp(-48.0, 48.0) as f64 / 12.0);
            let selected = inputs[5][i].max(0.0) as usize;
            let mut sum = 0.0;
            for (slot, t) in self.tracks.iter_mut().enumerate().filter(|(_, t)| t.active) {
                if self.remaining > 0 {
                    t.amplitude += (t.target_amplitude - t.amplitude) / self.remaining as f64;
                    t.frequency += (t.target_frequency - t.frequency) / self.remaining as f64;
                }
                let frequency = t.frequency * ratio;
                if (selected == 0 || selected == slot + 1) && (0.0..0.5).contains(&frequency) {
                    sum += t.amplitude * t.phase.cos();
                }
                t.phase = (t.phase + TAU * frequency).rem_euclid(TAU);
            }
            self.remaining = self.remaining.saturating_sub(1);
            *sample = sum as f32;
            self.quiet = if sample.abs() < TAIL_LEVEL {
                self.quiet.saturating_add(1)
            } else {
                0
            };
        }
    }
    fn latency(&self) -> usize {
        self.frames.size / 2 + self.frames.hop
    }
    fn is_active(&self) -> bool {
        self.quiet < self.frames.size * 2
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.frames.size == old.frames.size
            && self.frames.hop == old.frames.hop
            && self.tracks.len() == old.tracks.len()
        {
            std::mem::swap(self, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_keep_their_slots_when_loudness_order_changes_and_bridge_gaps() {
        let mut op = SpectralPartials::new(1024, 4, 8);
        for (k, m) in [(10, 1.0), (30, 0.5)] {
            op.analysis.spectrum.magnitude[k] = m;
            op.analysis.spectrum.frequency[k] = k as f64 / 1024.0;
        }
        op.update(8, 0.01, 0.0, 0.5);
        assert_eq!(op.tracks[0].target_frequency, 10.0 / 1024.0);
        op.analysis.spectrum.magnitude[30] = 2.0;
        op.update(8, 0.01, 0.0, 0.5);
        assert_eq!(op.tracks[0].target_frequency, 10.0 / 1024.0);
        assert_eq!(op.tracks[1].target_frequency, 30.0 / 1024.0);
        op.analysis.spectrum.magnitude[10] = 0.0;
        op.update(8, 0.01, 0.0, 0.5);
        op.analysis.spectrum.magnitude[11] = 1.0;
        op.analysis.spectrum.frequency[11] = 11.0 / 1024.0;
        op.update(8, 0.01, 0.0, 0.5);
        assert_eq!(op.tracks[0].target_frequency, 11.0 / 1024.0);
    }
}
