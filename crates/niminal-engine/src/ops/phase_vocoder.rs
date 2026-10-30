//! Peak-region pitch remapping, identity phase locking and cepstral envelope control.
use super::spectral_frames::SpectralFrames;
use crate::opcode::{Opcode, Port, ProcessCtx};
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex, num_complex::Complex};
use std::{f64::consts::TAU, sync::Arc};

fn wrap(x: f64) -> f64 {
    x.sin().atan2(x.cos())
}

pub(super) struct Spectrum {
    pub magnitude: Vec<f64>,
    pub phase: Vec<f64>,
    pub frequency: Vec<f64>,
}
impl Spectrum {
    pub fn new(size: usize) -> Self {
        Self {
            magnitude: vec![0.0; size / 2 + 1],
            phase: vec![0.0; size / 2 + 1],
            frequency: vec![0.0; size / 2 + 1],
        }
    }
    pub fn clear(&mut self) {
        self.magnitude.fill(0.0);
        self.phase.fill(0.0);
        self.frequency.fill(0.0);
    }
}

pub(super) struct Analysis {
    pub spectrum: Spectrum,
    previous: Vec<f64>,
    size: usize,
    hop: usize,
}
impl Analysis {
    pub fn new(size: usize, hop: usize) -> Self {
        Self {
            spectrum: Spectrum::new(size),
            previous: vec![0.0; size / 2 + 1],
            size,
            hop,
        }
    }
    pub fn reset(&mut self) {
        self.previous.fill(0.0);
        self.spectrum.clear();
    }
    pub fn read(&mut self, input: &[Complex<f64>]) {
        for (k, c) in input.iter().enumerate() {
            let phase = c.arg();
            let centre = k as f64 / self.size as f64;
            self.spectrum.frequency[k] = centre
                + wrap(phase - self.previous[k] - TAU * centre * self.hop as f64)
                    / (TAU * self.hop as f64);
            self.spectrum.magnitude[k] = c.norm();
            self.spectrum.phase[k] = phase;
            self.previous[k] = phase;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct WarpControls {
    pub shift: f64,
    pub formant: f64,
    pub preserve: f64,
    pub transients: f64,
}
impl WarpControls {
    pub fn at(inputs: &[&[f32]], i: usize, first: usize) -> Self {
        Self {
            shift: (inputs[first][i] as f64).clamp(-48.0, 48.0),
            formant: (inputs[first + 1][i] as f64).clamp(-48.0, 48.0),
            preserve: (inputs[first + 2][i] as f64).clamp(0.0, 1.0),
            transients: (inputs[first + 3][i] as f64).clamp(0.0, 1.0),
        }
    }
}

pub(super) struct PeakWarp {
    size: usize,
    hop: usize,
    pub detail: usize,
    previous_magnitude: Vec<f64>,
    previous_phase: Vec<f64>,
    previous_peaks: Vec<usize>,
    peaks: Vec<usize>,
    previous_synthesis: Vec<f64>,
    synthesis: Vec<f64>,
    envelope: Vec<f64>,
    cepstrum: Vec<f64>,
    complex: Vec<Complex<f64>>,
    forward: Arc<dyn RealToComplex<f64>>,
    inverse: Arc<dyn ComplexToReal<f64>>,
    scratch_forward: Vec<Complex<f64>>,
    scratch_inverse: Vec<Complex<f64>>,
    started: bool,
}
impl PeakWarp {
    pub fn new(size: usize, hop: usize, detail: usize) -> Self {
        let bins = size / 2 + 1;
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        Self {
            size,
            hop,
            detail,
            previous_magnitude: vec![0.0; bins],
            previous_phase: vec![0.0; bins],
            previous_peaks: Vec::with_capacity(bins),
            peaks: Vec::with_capacity(bins),
            previous_synthesis: vec![0.0; bins],
            synthesis: vec![0.0; bins],
            envelope: vec![0.0; bins],
            cepstrum: vec![0.0; size],
            complex: forward.make_output_vec(),
            scratch_forward: forward.make_scratch_vec(),
            scratch_inverse: inverse.make_scratch_vec(),
            forward,
            inverse,
            started: false,
        }
    }
    pub fn reset(&mut self) {
        self.previous_magnitude.fill(0.0);
        self.previous_phase.fill(0.0);
        self.previous_peaks.clear();
        self.peaks.clear();
        self.previous_synthesis.fill(0.0);
        self.synthesis.fill(0.0);
        self.started = false;
    }
    fn envelope(&mut self, magnitude: &[f64]) {
        for (c, &m) in self.complex.iter_mut().zip(magnitude) {
            *c = Complex::new(m.max(self.size as f64 * 1e-6).ln(), 0.0);
        }
        self.inverse
            .process_with_scratch(
                &mut self.complex,
                &mut self.cepstrum,
                &mut self.scratch_inverse,
            )
            .expect("envelope cepstrum");
        for (i, v) in self.cepstrum.iter_mut().enumerate() {
            let lifter = (1.0 - i.min(self.size - i) as f64 / self.detail as f64).max(0.0);
            *v *= lifter / self.size as f64;
        }
        self.forward
            .process_with_scratch(
                &mut self.cepstrum,
                &mut self.complex,
                &mut self.scratch_forward,
            )
            .expect("spectral envelope");
        for (e, c) in self.envelope.iter_mut().zip(&self.complex) {
            *e = c.re.exp();
        }
    }
    pub fn process(&mut self, s: &Spectrum, controls: WarpControls, out: &mut [Complex<f64>]) {
        let bins = s.magnitude.len();
        let ratio = 2.0_f64.powf(controls.shift / 12.0);
        let envelope_ratio =
            ratio.powf(1.0 - controls.preserve) * 2.0_f64.powf(controls.formant / 12.0);
        let shape = controls.formant != 0.0 || controls.preserve != 0.0;
        if shape {
            self.envelope(&s.magnitude);
        }
        self.peaks.clear();
        for k in 0..bins {
            let m = s.magnitude[k];
            if m > 0.0
                && (k == 0 || m > s.magnitude[k - 1])
                && (k + 1 == bins || m >= s.magnitude[k + 1])
            {
                self.peaks.push(k);
            }
        }
        out.fill(Complex::new(0.0, 0.0));
        let energy: f64 = s.magnitude.iter().sum();
        let flux: f64 = s
            .magnitude
            .iter()
            .zip(&self.previous_magnitude)
            .map(|(a, b)| (a - b).max(0.0))
            .sum();
        let onset = energy > 1e-8 && flux > energy * 0.25;
        let mut old_cursor = 0;
        for (p_index, &peak) in self.peaks.iter().enumerate() {
            let from = if p_index == 0 {
                0
            } else {
                (self.peaks[p_index - 1] + peak) / 2 + 1
            };
            let to = if p_index + 1 == self.peaks.len() {
                bins
            } else {
                (peak + self.peaks[p_index + 1]) / 2 + 1
            };
            while old_cursor + 1 < self.previous_peaks.len()
                && self.previous_peaks[old_cursor + 1].abs_diff(peak)
                    < self.previous_peaks[old_cursor].abs_diff(peak)
            {
                old_cursor += 1;
            }
            let matched = self
                .previous_peaks
                .get(old_cursor)
                .copied()
                .filter(|&old| old.abs_diff(peak) <= 4);
            let mut phase = if self.started {
                matched.map_or(s.phase[peak], |old| {
                    self.previous_synthesis[old]
                        + wrap(self.previous_phase[peak] - self.previous_phase[old])
                        + TAU * s.frequency[peak] * ratio * self.hop as f64
                })
            } else {
                s.phase[peak]
            };
            if onset && controls.transients > 0.0 {
                let region_energy: f64 = s.magnitude[from..to].iter().sum();
                let region_flux: f64 = s.magnitude[from..to]
                    .iter()
                    .zip(&self.previous_magnitude[from..to])
                    .map(|(a, b)| (a - b).max(0.0))
                    .sum();
                if region_flux > region_energy * 0.35 {
                    phase += wrap(s.phase[peak] - phase) * controls.transients;
                }
            }
            phase = wrap(phase);
            self.synthesis[peak] = phase;
            let displacement =
                (s.frequency[peak] * self.size as f64 * (ratio - 1.0)).round() as isize;
            // Translate complete peak regions; add collisions rather than overwriting partials.
            for k in from..to {
                let destination = k as isize + displacement;
                if !(0..bins as isize).contains(&destination) {
                    continue;
                }
                let destination = destination as usize;
                let gain = if shape {
                    let natural = read_envelope(&self.envelope, destination as f64 / ratio);
                    let desired =
                        read_envelope(&self.envelope, destination as f64 / envelope_ratio);
                    (desired / natural).clamp(0.0, 32.0)
                } else {
                    1.0
                };
                out[destination] += Complex::from_polar(
                    s.magnitude[k] * gain,
                    phase + wrap(s.phase[k] - s.phase[peak]),
                );
            }
        }
        self.previous_magnitude.copy_from_slice(&s.magnitude);
        self.previous_phase.copy_from_slice(&s.phase);
        self.previous_synthesis.copy_from_slice(&self.synthesis);
        std::mem::swap(&mut self.previous_peaks, &mut self.peaks);
        self.started = true;
    }
}
fn read_envelope(values: &[f64], position: f64) -> f64 {
    let p = position.clamp(0.0, (values.len() - 1) as f64);
    let a = p as usize;
    let b = (a + 1).min(values.len() - 1);
    values[a] + (values[b] - values[a]) * (p - a as f64)
}

pub struct SpectralWarp {
    frames: SpectralFrames,
    analysis: Analysis,
    warp: PeakWarp,
}
impl SpectralWarp {
    pub fn new(size: usize, overlap: usize, detail: usize) -> Self {
        Self {
            frames: SpectralFrames::new(size, overlap),
            analysis: Analysis::new(size, size / overlap),
            warp: PeakWarp::new(size, size / overlap, detail),
        }
    }
}
impl Opcode for SpectralWarp {
    fn name(&self) -> &str {
        "spectral_warp"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("shift", 0.0),
            Port::optional("formant", 0.0),
            Port::optional("preserve", 0.0),
            Port::optional("transients", 1.0),
        ];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(
            self.frames.size,
            self.frames.size / self.frames.hop,
            self.warp.detail,
        ))
    }
    fn reset(&mut self) {
        self.frames.reset();
        self.analysis.reset();
        self.warp.reset();
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let Self {
            frames,
            analysis,
            warp,
        } = self;
        frames.run(inputs[0], out, |spectrum, i| {
            analysis.read(spectrum);
            warp.process(&analysis.spectrum, WarpControls::at(inputs, i, 1), spectrum);
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
            && self.warp.detail == old.warp.detail
        {
            std::mem::swap(self, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controls(shift: f64, transients: f64) -> WarpControls {
        WarpControls {
            shift,
            formant: 0.0,
            preserve: 0.0,
            transients,
        }
    }
    #[test]
    fn downward_peak_collisions_add_instead_of_losing_a_partial() {
        let mut s = Spectrum::new(128);
        s.magnitude[10] = 1.0;
        s.magnitude[12] = 0.5;
        s.frequency[10] = 10.0 / 128.0;
        s.frequency[12] = 12.0 / 128.0;
        let mut warp = PeakWarp::new(128, 32, 16);
        let mut out = vec![Complex::new(0.0, 0.0); 65];
        warp.process(&s, controls(-48.0, 0.0), &mut out);
        assert!((out[1].re - 1.5).abs() < 1e-12);
    }
    #[test]
    fn transient_reset_only_changes_the_growing_peak_region() {
        let mut s = Spectrum::new(128);
        s.magnitude[10] = 1.0;
        s.magnitude[30] = 0.1;
        for k in 0..65 {
            s.frequency[k] = k as f64 / 128.0;
        }
        let mut on = PeakWarp::new(128, 32, 16);
        let mut off = PeakWarp::new(128, 32, 16);
        let mut a = vec![Complex::new(0.0, 0.0); 65];
        let mut b = a.clone();
        on.process(&s, controls(7.0, 1.0), &mut a);
        off.process(&s, controls(7.0, 0.0), &mut b);
        s.magnitude[30] = 10.0;
        on.process(&s, controls(7.0, 1.0), &mut a);
        off.process(&s, controls(7.0, 0.0), &mut b);
        let steady = (10.0 * 2.0_f64.powf(7.0 / 12.0)).round() as usize;
        let attack = (30.0 * 2.0_f64.powf(7.0 / 12.0)).round() as usize;
        assert!((a[steady] - b[steady]).norm() < 1e-12);
        assert!(a[attack].arg().abs() < 1e-12);
        assert!((a[attack] - b[attack]).norm() > 1.0);
    }
}
