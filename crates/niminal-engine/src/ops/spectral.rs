//! Spectral effects. Each is one node that analyses its input, changes the spectrum, and resynthesises, so
//! it owns all of its state and a voice's copy of it is independent.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

/// Whether `n` is an FFT size the spectral effects accept.
pub fn valid_fft_size(n: usize) -> bool {
    n.is_power_of_two() && (crate::block_size() * 2..=8192).contains(&n)
}

fn bins(n: usize) -> usize {
    n / 2 + 1
}

fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (TAU * i as f32 / n as f32).cos()))
        .collect()
}

/// A short-time Fourier transform with a Hann window and 50% overlap, in and out. A Hann window at that
/// overlap sums to one, so a spectrum left alone comes out as it went in, `size` samples later.
struct Stft {
    size: usize,
    hop: usize,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// The last `size` input samples; `pos` is the next one to be written, which makes it the oldest.
    input: Vec<f32>,
    /// Overlapped frames waiting to be played; the slot at `pos` is the next sample out.
    output: Vec<f32>,
    pos: usize,
    windowed: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    mags: Vec<f32>,
    phases: Vec<f32>,
    quiet_run: usize,
}

impl Stft {
    fn new(size: usize) -> Self {
        assert!(valid_fft_size(size));
        let mut planner = RealFftPlanner::<f32>::new();
        let (fwd, inv) = (
            planner.plan_fft_forward(size),
            planner.plan_fft_inverse(size),
        );
        Stft {
            size,
            hop: size / 2,
            window: hann_window(size),
            input: vec![0.0; size],
            output: vec![0.0; size],
            pos: 0,
            windowed: vec![0.0; size],
            spectrum: vec![Complex::new(0.0, 0.0); bins(size)],
            scratch_fwd: fwd.make_scratch_vec(),
            scratch_inv: inv.make_scratch_vec(),
            mags: vec![0.0; bins(size)],
            phases: vec![0.0; bins(size)],
            fwd,
            inv,
            quiet_run: 0,
        }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.pos = 0;
        self.quiet_run = 0;
    }

    /// True until the output has been silent for a whole window, plus `gap` samples that sound may still
    /// be on its way.
    fn is_active(&self, gap: usize) -> bool {
        self.quiet_run < self.size * 2 + gap
    }

    /// Plays `x` through the transform, calling `stage` with each frame's magnitudes and phases.
    fn run(&mut self, x: &[f32], out: &mut [f32], mut stage: impl FnMut(&mut [f32], &mut [f32])) {
        for (o, &s) in out.iter_mut().zip(x) {
            *o = std::mem::take(&mut self.output[self.pos]);
            self.quiet_run = if o.abs() < TAIL_LEVEL {
                self.quiet_run.saturating_add(1)
            } else {
                0
            };
            self.input[self.pos] = s;
            self.pos = (self.pos + 1) % self.size;
            if self.pos.is_multiple_of(self.hop) {
                self.frame(&mut stage);
            }
        }
    }

    /// Transforms the last `size` samples, lets `stage` change them, and overlaps the result into the output.
    fn frame(&mut self, stage: &mut impl FnMut(&mut [f32], &mut [f32])) {
        let n = self.size;
        for i in 0..n {
            self.windowed[i] = self.input[(self.pos + i) % n] * self.window[i];
        }
        self.fwd
            .process_with_scratch(
                &mut self.windowed,
                &mut self.spectrum,
                &mut self.scratch_fwd,
            )
            .expect("fft forward");
        let scale = 1.0 / n as f32;
        for (i, c) in self.spectrum.iter().enumerate() {
            let c = c * scale;
            self.mags[i] = c.norm();
            self.phases[i] = c.arg();
        }

        stage(&mut self.mags, &mut self.phases);

        for (i, c) in self.spectrum.iter_mut().enumerate() {
            *c = Complex::from_polar(self.mags[i], self.phases[i]);
        }
        // the inverse transform wants the DC and Nyquist bins real
        self.spectrum[0].im = 0.0;
        self.spectrum[bins(n) - 1].im = 0.0;
        self.inv
            .process_with_scratch(
                &mut self.spectrum,
                &mut self.windowed,
                &mut self.scratch_inv,
            )
            .expect("fft inverse");
        for i in 0..n {
            self.output[(self.pos + i) % n] += self.windowed[i];
        }
    }
}

// ── Spectral Freeze ──────────────────────────────────────────────────────

/// Holds the spectrum it hears when `freeze` goes above one half, until it goes back.
pub struct SpectralFreeze {
    stft: Stft,
    frozen_mags: Vec<f32>,
    frozen_phases: Vec<f32>,
    /// How far the held phases have been turned, as the sound they hold would have turned them.
    turned: Vec<f32>,
    is_frozen: bool,
}

impl SpectralFreeze {
    pub fn new(fft_size: usize) -> Self {
        let nb = bins(fft_size);
        SpectralFreeze {
            stft: Stft::new(fft_size),
            frozen_mags: vec![0.0; nb],
            frozen_phases: vec![0.0; nb],
            turned: vec![0.0; nb],
            is_frozen: false,
        }
    }
}

impl Opcode for SpectralFreeze {
    fn name(&self) -> &str {
        "spectral_freeze"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("freeze", 0.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(SpectralFreeze::new(self.stft.size))
    }

    fn reset(&mut self) {
        self.stft.reset();
        self.is_frozen = false;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let freeze = inputs[1][0] > 0.5;
        let Self {
            stft,
            frozen_mags,
            frozen_phases,
            turned,
            is_frozen,
        } = self;
        stft.run(inputs[0], out, |mags, phases| {
            if !freeze {
                *is_frozen = false;
                return;
            }
            if !*is_frozen {
                frozen_mags.copy_from_slice(mags);
                frozen_phases.copy_from_slice(phases);
                turned.fill(0.0);
                *is_frozen = true;
            }
            // A steady partial in bin k turns by pi * k each hop at half overlap.
            for (k, ((m, p), t)) in mags
                .iter_mut()
                .zip(phases)
                .zip(turned.iter_mut())
                .enumerate()
            {
                *m = frozen_mags[k];
                *p = frozen_phases[k] + *t;
                *t = (*t + PI * k as f32) % TAU;
            }
        });
    }

    fn is_active(&self) -> bool {
        self.stft.is_active(0)
    }

    fn latency(&self) -> usize {
        self.stft.size
    }
}

// ── Spectral Delay ──────────────────────────────────────────────────────

/// Per-bin delay: `time` seconds, which `spread` makes shorter for the lower bins, and `feedback` of what
/// comes out goes back in.
pub struct SpectralDelay {
    stft: Stft,
    max_time: f32,
    max_hops: usize,
    mag_buf: Vec<f32>,
    phase_buf: Vec<f32>,
    write_pos: usize,
    sample_rate: f32,
    /// The delay time in samples: how long a silence can be before the sound is back.
    gap: usize,
}

impl SpectralDelay {
    pub fn new(fft_size: usize, max_time: f32) -> Self {
        let mut delay = SpectralDelay {
            stft: Stft::new(fft_size),
            max_time,
            max_hops: 0,
            mag_buf: Vec::new(),
            phase_buf: Vec::new(),
            write_pos: 0,
            sample_rate: 0.0,
            gap: 0,
        };
        delay.prepare(48_000.0);
        delay
    }
}

impl Opcode for SpectralDelay {
    fn name(&self) -> &str {
        "spectral_delay"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("time", 0.1),
            Port::optional("spread", 0.0),
            Port::optional("feedback", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut copy = SpectralDelay::new(self.stft.size, self.max_time);
        copy.prepare(self.sample_rate);
        Box::new(copy)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let hop = self.stft.hop as f32;
        self.max_hops = ((self.max_time * sample_rate / hop).ceil() as usize).max(2);
        let len = bins(self.stft.size) * self.max_hops;
        self.mag_buf = vec![0.0; len];
        self.phase_buf = vec![0.0; len];
        self.write_pos = 0;
    }

    fn reset(&mut self) {
        self.stft.reset();
        self.mag_buf.fill(0.0);
        self.phase_buf.fill(0.0);
        self.write_pos = 0;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let time = inputs[1][0].clamp(0.0, self.max_time);
        let spread = inputs[2][0].clamp(0.0, 1.0);
        let feedback = inputs[3][0].clamp(0.0, 0.99);
        self.gap = (time * self.sample_rate) as usize;
        let time_hops = time * self.sample_rate / self.stft.hop as f32;
        let (max_hops, num_bins) = (self.max_hops, bins(self.stft.size));
        let Self {
            stft,
            mag_buf,
            phase_buf,
            write_pos,
            ..
        } = self;
        stft.run(inputs[0], out, |mags, phases| {
            for bin in 0..num_bins {
                let frac = bin as f32 / (num_bins - 1) as f32;
                let delay_frac = 1.0 - spread * (1.0 - frac);
                let delay_hops = (time_hops * delay_frac)
                    .round()
                    .clamp(1.0, (max_hops - 1) as f32) as usize;
                let read = bin * max_hops + (*write_pos + max_hops - delay_hops) % max_hops;
                let write = bin * max_hops + *write_pos;
                let (delayed_mag, delayed_phase) = (mag_buf[read], phase_buf[read]);
                mag_buf[write] = mags[bin] + delayed_mag * feedback;
                phase_buf[write] = phases[bin];
                mags[bin] = delayed_mag;
                phases[bin] = delayed_phase;
            }
            *write_pos = (*write_pos + 1) % max_hops;
        });
    }

    fn is_active(&self) -> bool {
        self.stft.is_active(self.gap)
    }

    fn latency(&self) -> usize {
        self.stft.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const SIZE: usize = 256;

    fn sine(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (TAU * 440.0 * i as f32 / SR).sin())
            .collect()
    }

    /// Runs `op` over `input` in blocks, with every control port held at `controls`.
    fn run(op: &mut dyn Opcode, input: &[f32], controls: &[f32]) -> Vec<f32> {
        let block = crate::block_size();
        let ctx = ProcessCtx::new(SR, true);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            let control: Vec<Vec<f32>> = controls.iter().map(|c| vec![*c; chunk.len()]).collect();
            let mut inputs: Vec<&[f32]> = vec![chunk];
            inputs.extend(control.iter().map(Vec::as_slice));
            let mut block_out = vec![0.0; chunk.len()];
            op.process(&ctx, &inputs, &mut block_out);
            out.extend(block_out);
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn an_untouched_spectrum_comes_back_unchanged_after_the_latency() {
        // a delay of nothing but its input at full dry would change it, so run the transform with
        // a stage that leaves the spectrum alone
        let mut stft = Stft::new(SIZE);
        let input = sine(SIZE * 16);
        let mut out = vec![0.0; input.len()];
        stft.run(&input, &mut out, |_, _| {});
        let steady = SIZE * 2..input.len();
        let worst = steady
            .map(|i| (out[i] - input[i - SIZE]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn freeze_holds_the_sound_after_the_input_stops() {
        let mut freeze = SpectralFreeze::new(SIZE);
        let mut input = sine(SIZE * 8);
        input.extend(vec![0.0; SIZE * 24]);
        let held = run(&mut freeze, &input, &[1.0]);
        let tail = &held[held.len() - SIZE * 4..];
        assert!(rms(tail) > 0.2, "{}", rms(tail));

        let mut unfrozen = SpectralFreeze::new(SIZE);
        let gone = run(&mut unfrozen, &input, &[0.0]);
        assert!(rms(&gone[gone.len() - SIZE * 4..]) < 1e-3);
    }

    #[test]
    fn delay_moves_the_sound_later_by_the_time() {
        let mut delay = SpectralDelay::new(SIZE, 1.0);
        let mut input = sine(SIZE * 4);
        input.extend(vec![0.0; SIZE * 60]);
        // 40ms is 1920 samples, 15 hops at 128
        let out = run(&mut delay, &input, &[0.04, 0.0, 0.0]);
        let (early, late) = (&out[..SIZE * 8], &out[SIZE * 8..SIZE * 40]);
        assert!(rms(early) < 0.05, "heard {} before the delay", rms(early));
        assert!(rms(late) > 0.05, "heard {} after it", rms(late));
    }

    #[test]
    fn copies_of_a_voice_do_not_share_state() {
        let mut a = SpectralFreeze::new(SIZE);
        let mut b = a.box_clone();
        let input = sine(SIZE * 8);
        let first = run(&mut a, &input, &[1.0]);
        let second = run(b.as_mut(), &input, &[1.0]);
        assert_eq!(first, second);
    }

    #[test]
    fn a_ringing_tail_keeps_the_voice_alive() {
        let mut delay = SpectralDelay::new(SIZE, 1.0);
        run(&mut delay, &sine(SIZE * 4), &[0.05, 0.0, 0.0]);
        assert!(delay.is_active());
        run(&mut delay, &vec![0.0; SIZE * 400], &[0.05, 0.0, 0.0]);
        assert!(!delay.is_active());
    }
}
