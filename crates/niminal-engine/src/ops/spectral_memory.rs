//! A fixed-capacity chronological ring of magnitude, phase and instantaneous-frequency frames.
use super::{
    phase_vocoder::{Analysis, PeakWarp, Spectrum, WarpControls},
    spectral_frames::SpectralFrames,
};
use crate::opcode::{Opcode, Port, ProcessCtx};

pub struct SpectralMemory {
    frames: SpectralFrames,
    analysis: Analysis,
    playback: Spectrum,
    warp: PeakWarp,
    history: Vec<f32>,
    capacity: usize,
    write: usize,
    count: usize,
    recording: bool,
}
impl SpectralMemory {
    pub fn new(size: usize, overlap: usize, capacity: usize, detail: usize) -> Self {
        Self {
            frames: SpectralFrames::new(size, overlap),
            analysis: Analysis::new(size, size / overlap),
            playback: Spectrum::new(size),
            warp: PeakWarp::new(size, size / overlap, detail),
            history: vec![0.0; capacity * (size / 2 + 1) * 3],
            capacity,
            write: 0,
            count: 0,
            recording: true,
        }
    }
}
impl Opcode for SpectralMemory {
    fn name(&self) -> &str {
        "spectral_memory"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("record", 1.0),
            Port::optional("position", 0.0),
            Port::optional("shift", 0.0),
            Port::optional("formant", 0.0),
            Port::optional("preserve", 0.0),
            Port::optional("transients", 0.0),
        ];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(
            self.frames.size,
            self.frames.size / self.frames.hop,
            self.capacity,
            self.warp.detail,
        ))
    }
    fn reset(&mut self) {
        self.frames.reset();
        self.analysis.reset();
        self.playback.clear();
        self.warp.reset();
        self.write = 0;
        self.count = 0;
        self.recording = true;
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let Self {
            frames,
            analysis,
            playback,
            warp,
            history,
            capacity,
            write,
            count,
            recording,
        } = self;
        frames.run(inputs[0], out, |spectrum, i| {
            analysis.read(spectrum);
            let record = inputs[1][i] > 0.5;
            if record != *recording {
                warp.reset();
            }
            *recording = record;
            let bins = spectrum.len();
            if record {
                let base = *write * bins * 3;
                for k in 0..bins {
                    history[base + k * 3] = analysis.spectrum.magnitude[k] as f32;
                    history[base + k * 3 + 1] = analysis.spectrum.phase[k] as f32;
                    history[base + k * 3 + 2] = analysis.spectrum.frequency[k] as f32;
                }
                *write = (*write + 1) % *capacity;
                *count = (*count + 1).min(*capacity);
                warp.process(&analysis.spectrum, WarpControls::at(inputs, i, 3), spectrum);
            } else if *count == 0 {
                spectrum.fill(realfft::num_complex::Complex::new(0.0, 0.0));
            } else {
                let position = (inputs[2][i] as f64).clamp(0.0, 1.0) * (*count - 1) as f64;
                let a = position as usize;
                let b = (a + 1).min(*count - 1);
                let mix = position - a as f64;
                let oldest = (*write + *capacity - *count) % *capacity;
                let a = ((oldest + a) % *capacity) * bins * 3;
                let b = ((oldest + b) % *capacity) * bins * 3;
                for k in 0..bins {
                    let at = a + k * 3;
                    let bt = b + k * 3;
                    let left = history[at] as f64 * (1.0 - mix);
                    let right = history[bt] as f64 * mix;
                    let magnitude = left + right;
                    playback.magnitude[k] = magnitude;
                    // Empty bins must not pull a partial's tuning toward an undefined frequency.
                    playback.frequency[k] = if magnitude > 0.0 {
                        (left * history[at + 2] as f64 + right * history[bt + 2] as f64) / magnitude
                    } else {
                        0.0
                    };
                    let start = history[at + 1] as f64;
                    let difference = history[bt + 1] as f64 - start;
                    let weight = if magnitude > 0.0 {
                        right / magnitude
                    } else {
                        mix
                    };
                    playback.phase[k] = start + difference.sin().atan2(difference.cos()) * weight;
                }
                warp.process(playback, WarpControls::at(inputs, i, 3), spectrum);
            }
        });
    }
    fn latency(&self) -> usize {
        self.frames.size
    }
    fn is_active(&self) -> bool {
        self.frames.active()
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.frames.size == old.frames.size
            && self.frames.hop == old.frames.hop
            && self.capacity == old.capacity
            && self.warp.detail == old.warp.detail
        {
            std::mem::swap(self, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_forgets_capture_and_matching_redefinitions_keep_it() {
        let mut original = SpectralMemory::new(512, 4, 8, 32);
        original.count = 3;
        original.write = 3;
        original.history[0] = 7.0;
        let mut replacement = SpectralMemory::new(512, 4, 8, 32);
        replacement.carry_state(&mut original);
        assert_eq!(replacement.count, 3);
        assert_eq!(replacement.history[0], 7.0);
        replacement.reset();
        assert_eq!(replacement.count, 0);
        assert_eq!(replacement.write, 0);
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    #[test]
    fn a_wrapped_capture_scans_forward_backward_and_around_a_loop() {
        let mut memory = SpectralMemory::new(16, 4, 4, 8);
        memory.count = 4;
        memory.write = 2;
        memory.recording = false;
        for (order, slot) in [2, 3, 0, 1].into_iter().enumerate() {
            memory.history[slot * 9 * 3 + 3] = (order + 1) as f32;
            memory.history[slot * 9 * 3 + 5] = 1.0 / 16.0;
        }
        let zero = [0.0; 4];
        let mut out = [0.0; 4];
        for (position, expected) in [
            (0.0, 1.0),
            (0.5, 2.5),
            (1.0, 4.0),
            (0.75, 3.25),
            (0.25, 1.75),
            (0.0, 1.0),
        ] {
            let p = [position; 4];
            memory.process(
                &ProcessCtx::new(48000.0, true),
                &[&zero, &zero, &p, &zero, &zero, &zero, &zero],
                &mut out,
            );
            assert!((memory.playback.magnitude[1] - expected).abs() < 1e-12);
            assert!((memory.playback.frequency[1] - 1.0 / 16.0).abs() < 1e-12);
        }
    }
}
