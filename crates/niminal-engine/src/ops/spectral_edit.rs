//! Frame-domain selection, independent freezing, frequency shifting and two-source morphing.
use super::{
    phase_vocoder::{Analysis, PeakWarp, WarpControls},
    spectral_frames::SpectralFrames,
};
use crate::opcode::{Opcode, Port, ProcessCtx};
use realfft::num_complex::Complex;
use std::f64::consts::TAU;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpectralEditKind {
    Thin,
    Arp,
    Hold,
    Shift,
}

pub struct SpectralEdit {
    kind: SpectralEditKind,
    frames: SpectralFrames,
    analysis: Analysis,
    warp: PeakWarp,
    order: Vec<usize>,
    magnitudes: Vec<f64>,
    frequencies: Vec<f64>,
    phases: Vec<f64>,
    held_magnitude: bool,
    held_frequency: bool,
}
impl SpectralEdit {
    pub fn new(kind: SpectralEditKind, size: usize, overlap: usize) -> Self {
        let bins = size / 2 + 1;
        Self {
            kind,
            frames: SpectralFrames::new(size, overlap),
            analysis: Analysis::new(size, size / overlap),
            warp: PeakWarp::new(size, size / overlap, 32),
            order: (0..bins).collect(),
            magnitudes: vec![0.0; bins],
            frequencies: vec![0.0; bins],
            phases: vec![0.0; bins],
            held_magnitude: false,
            held_frequency: false,
        }
    }
}
impl Opcode for SpectralEdit {
    fn name(&self) -> &str {
        match self.kind {
            SpectralEditKind::Thin => "spectral_thin",
            SpectralEditKind::Arp => "spectral_arp",
            SpectralEditKind::Hold => "spectral_hold",
            SpectralEditKind::Shift => "spectral_shift",
        }
    }
    fn ports(&self) -> &[Port] {
        const THIN: &[Port] = &[Port::required("x"), Port::optional("count", 16.0)];
        const ARP: &[Port] = &[
            Port::required("x"),
            Port::optional("position", 0.1),
            Port::optional("depth", 1.0),
            Port::optional("gain", 1.0),
            Port::optional("width", 1.0),
        ];
        const HOLD: &[Port] = &[
            Port::required("x"),
            Port::optional("magnitude", 0.0),
            Port::optional("frequency", 0.0),
        ];
        const SHIFT: &[Port] = &[
            Port::required("x"),
            Port::optional("shift", 0.0),
            Port::optional("lowest", 0.0),
        ];
        match self.kind {
            SpectralEditKind::Thin => THIN,
            SpectralEditKind::Arp => ARP,
            SpectralEditKind::Hold => HOLD,
            SpectralEditKind::Shift => SHIFT,
        }
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(
            self.kind,
            self.frames.size,
            self.frames.size / self.frames.hop,
        ))
    }
    fn reset(&mut self) {
        self.frames.reset();
        self.analysis.reset();
        self.warp.reset();
        self.magnitudes.fill(0.0);
        self.frequencies.fill(0.0);
        self.phases.fill(0.0);
        self.held_magnitude = false;
        self.held_frequency = false;
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let Self {
            kind,
            frames,
            analysis,
            warp,
            order,
            magnitudes,
            frequencies,
            phases,
            held_magnitude,
            held_frequency,
        } = self;
        let hop = frames.hop as f64;
        frames.run(inputs[0], out, |spectrum, i| match kind {
            SpectralEditKind::Thin => {
                let count = (inputs[1][i].max(0.0) as usize).min(spectrum.len());
                if count == spectrum.len() {
                    return;
                }
                if count == 0 {
                    spectrum.fill(Complex::new(0.0, 0.0));
                    return;
                }
                // Partition in place without allocating or sorting bins we will discard.
                // Bin index breaks equal-magnitude ties deterministically.
                order.select_nth_unstable_by(count, |&a, &b| {
                    spectrum[b]
                        .norm_sqr()
                        .total_cmp(&spectrum[a].norm_sqr())
                        .then(a.cmp(&b))
                });
                for &k in &order[count..] {
                    spectrum[k] = Complex::new(0.0, 0.0);
                }
            }
            SpectralEditKind::Arp => {
                let position = inputs[1][i].clamp(0.0, 1.0) as f64 * (spectrum.len() - 1) as f64;
                let depth = inputs[2][i].clamp(0.0, 1.0) as f64;
                let gain = inputs[3][i].max(0.0) as f64;
                let width = inputs[4][i].max(1.0) as f64;
                for (k, value) in spectrum.iter_mut().enumerate() {
                    let focus = (1.0 - (k as f64 - position).abs() / width).max(0.0);
                    *value *= (1.0 - depth) + focus * (gain - (1.0 - depth));
                }
            }
            SpectralEditKind::Hold => {
                analysis.read(spectrum);
                let s = &analysis.spectrum;
                let magnitude = inputs[1][i] > 0.5;
                let frequency = inputs[2][i] > 0.5;
                if !*held_magnitude {
                    magnitudes.copy_from_slice(&s.magnitude);
                }
                if !*held_frequency {
                    frequencies.copy_from_slice(&s.frequency);
                }
                for (k, value) in spectrum.iter_mut().enumerate() {
                    if frequency {
                        if *held_frequency {
                            phases[k] = (phases[k] + TAU * frequencies[k] * hop).rem_euclid(TAU);
                        } else {
                            phases[k] = s.phase[k];
                        }
                    } else {
                        phases[k] = s.phase[k];
                    }
                    *value = Complex::from_polar(
                        if magnitude {
                            magnitudes[k]
                        } else {
                            s.magnitude[k]
                        },
                        phases[k],
                    );
                }
                *held_magnitude = magnitude;
                *held_frequency = frequency;
            }
            SpectralEditKind::Shift => {
                analysis.read(spectrum);
                warp.process_shift(
                    &analysis.spectrum,
                    WarpControls {
                        shift: 0.0,
                        formant: 0.0,
                        preserve: 0.0,
                        transients: 1.0,
                    },
                    (inputs[1][i] as f64 / ctx.sample_rate as f64).clamp(-1.0, 1.0),
                    inputs[2][i].max(0.0) as f64 / ctx.sample_rate as f64,
                    spectrum,
                );
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
            && self.kind == old.kind
            && self.frames.size == old.frames.size
            && self.frames.hop == old.frames.hop
        {
            std::mem::swap(self, old);
        }
    }
}

pub struct SpectralMorph {
    frames: SpectralFrames,
    other: SpectralFrames,
    a: Analysis,
    b: Analysis,
    phases: Vec<f64>,
    started: bool,
}
impl SpectralMorph {
    pub fn new(size: usize, overlap: usize) -> Self {
        Self {
            frames: SpectralFrames::new(size, overlap),
            other: SpectralFrames::new(size, overlap),
            a: Analysis::new(size, size / overlap),
            b: Analysis::new(size, size / overlap),
            phases: vec![0.0; size / 2 + 1],
            started: false,
        }
    }
}
impl Opcode for SpectralMorph {
    fn name(&self) -> &str {
        "spectral_morph"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("other"),
            Port::optional("magnitude", 0.5),
            Port::optional("frequency", 0.5),
        ];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(
            self.frames.size,
            self.frames.size / self.frames.hop,
        ))
    }
    fn reset(&mut self) {
        self.frames.reset();
        self.other.reset();
        self.a.reset();
        self.b.reset();
        self.phases.fill(0.0);
        self.started = false;
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let Self {
            frames,
            other,
            a,
            b,
            phases,
            started,
        } = self;
        let hop = frames.hop as f64;
        for i in 0..out.len() {
            if other.analyse_sample(inputs[1][i]) {
                b.read(other.spectrum());
            }
            frames.run(&inputs[0][i..i + 1], &mut out[i..i + 1], |spectrum, _| {
                a.read(spectrum);
                let am = inputs[2][i].clamp(0.0, 1.0) as f64;
                let fm = inputs[3][i].clamp(0.0, 1.0) as f64;
                for (k, value) in spectrum.iter_mut().enumerate() {
                    let a = &a.spectrum;
                    let b = &b.spectrum;
                    if fm == 0.0 {
                        phases[k] = a.phase[k];
                    } else if fm == 1.0 {
                        phases[k] = b.phase[k];
                    } else if !*started {
                        let difference = b.phase[k] - a.phase[k];
                        phases[k] = a.phase[k] + difference.sin().atan2(difference.cos()) * fm;
                    } else {
                        let frequency = a.frequency[k] * (1.0 - fm) + b.frequency[k] * fm;
                        phases[k] = (phases[k] + TAU * frequency * hop).rem_euclid(TAU);
                    }
                    *value = Complex::from_polar(
                        a.magnitude[k] * (1.0 - am) + b.magnitude[k] * am,
                        phases[k],
                    );
                }
                *started = true;
            });
        }
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
        {
            std::mem::swap(self, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::SpectralPartials;

    fn render(op: &mut dyn Opcode, offset: usize, held: bool) -> Vec<f32> {
        let len = 4096;
        let x: Vec<_> = (offset..offset + len)
            .map(|i| (TAU * 997.0 * i as f64 / 48000.0).sin() as f32 * 0.2)
            .collect();
        let other: Vec<_> = (offset..offset + len)
            .map(|i| (TAU * 1037.0 * i as f64 / 48000.0).sin() as f32 * 0.1)
            .collect();
        let controls: Vec<Vec<f32>> = op
            .ports()
            .iter()
            .skip(1)
            .map(|p| {
                if p.name == "other" {
                    return other.clone();
                }
                let value =
                    if held && ["magnitude", "frequency", "freeze"].contains(&p.name.as_ref()) {
                        1.0
                    } else {
                        p.default.unwrap()
                    };
                vec![value; len]
            })
            .collect();
        let mut inputs = vec![x.as_slice()];
        inputs.extend(controls.iter().map(Vec::as_slice));
        let mut out = vec![0.0; len];
        op.process(&ProcessCtx::new(48000.0, true), &inputs, &mut out);
        out
    }
    #[test]
    fn compatible_live_redefinitions_keep_state_and_reset_clears_it() {
        let mut ops: Vec<Box<dyn Opcode>> = [
            SpectralEditKind::Thin,
            SpectralEditKind::Arp,
            SpectralEditKind::Hold,
            SpectralEditKind::Shift,
        ]
        .into_iter()
        .map(|kind| Box::new(SpectralEdit::new(kind, 512, 4)) as Box<dyn Opcode>)
        .collect();
        ops.push(Box::new(SpectralMorph::new(512, 4)));
        ops.push(Box::new(SpectralPartials::new(512, 4, 16)));
        for mut original in ops {
            let mut parallel = original.box_clone();
            assert_eq!(
                render(original.as_mut(), 0, false),
                render(parallel.as_mut(), 0, false)
            );
            // Carry a populated capture/track bank into a freshly built compatible node.
            let mut replacement = original.box_clone();
            replacement.carry_state(original.as_mut());
            assert_eq!(
                render(replacement.as_mut(), 4096, true),
                render(parallel.as_mut(), 4096, true)
            );
            replacement.reset();
            let mut fresh = replacement.box_clone();
            assert_eq!(
                render(replacement.as_mut(), 0, false),
                render(fresh.as_mut(), 0, false)
            );
        }
    }
}
