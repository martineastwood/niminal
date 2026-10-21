use std::cell::UnsafeCell;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

fn valid_fft_size(n: usize) -> bool {
    n.is_power_of_two() && (crate::block_size() * 2..=8192).contains(&n)
}

fn bins(n: usize) -> usize {
    n / 2 + 1
}

fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * t).cos())
        })
        .collect()
}

// ── Shared spectral frame ────────────────────────────────────────────────

/// Spectral data shared between FFT, spectral ops, and IFFT within one
/// voice. Written by FFT each hop, modified in-place by spectral ops,
/// read by IFFT.
///
/// # Safety
///
/// All access happens on the audio thread in topological graph order
/// (FFT → spectral ops → IFFT), so there are no data races. The wrapper
/// makes this `Send + Sync` so it can live behind an `Arc` as required
/// by the `Opcode` trait.
pub struct SpectralChain {
    inner: UnsafeCell<SpectralChainInner>,
}

// SAFETY: accessed only from the audio thread in graph-topological order.
unsafe impl Send for SpectralChain {}
unsafe impl Sync for SpectralChain {}

struct SpectralChainInner {
    size: usize,
    mags: Vec<f32>,
    phases: Vec<f32>,
    hop_counter: u64,
}

impl SpectralChain {
    pub fn new(fft_size: usize) -> Self {
        let nb = bins(fft_size);
        SpectralChain {
            inner: UnsafeCell::new(SpectralChainInner {
                size: fft_size,
                mags: vec![0.0; nb],
                phases: vec![0.0; nb],
                hop_counter: 0,
            }),
        }
    }

    pub fn fft_size(&self) -> usize {
        // SAFETY: reading a constant set at construction.
        unsafe { (*self.inner.get()).size }
    }

    /// # Safety
    /// Only call from the audio thread during graph processing.
    #[allow(clippy::mut_from_ref)]
    fn get(&self) -> &mut SpectralChainInner {
        unsafe { &mut *self.inner.get() }
    }
}

impl std::fmt::Debug for SpectralChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectralChain")
            .field("fft_size", &self.fft_size())
            .finish()
    }
}

// ── FFT ──────────────────────────────────────────────────────────────────

/// Forward STFT with 50% overlap and a Hann window. Collects input into a
/// ring, transforms each hop, and writes magnitude/phase into a shared
/// `SpectralChain`. The output passes the input through for graph ordering.
pub struct Fft {
    chain: Arc<SpectralChain>,
    fwd: Arc<dyn RealToComplex<f32>>,
    fft_size: usize,
    hop: usize,
    input_buf: Vec<f32>,
    write_pos: usize,
    since_hop: usize,
    window: Vec<f32>,
    windowed: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
}

impl Fft {
    pub fn new(fft_size: usize, chain: Arc<SpectralChain>) -> Self {
        assert!(valid_fft_size(fft_size));
        let hop = fft_size / 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(fft_size);
        let nb = bins(fft_size);
        Fft {
            chain,
            fwd,
            fft_size,
            hop,
            input_buf: vec![0.0; fft_size],
            write_pos: 0,
            since_hop: 0,
            window: hann_window(fft_size),
            windowed: vec![0.0; fft_size],
            spectrum: vec![Complex::new(0.0, 0.0); nb],
        }
    }

    fn transform(&mut self) {
        let n = self.fft_size;
        for i in 0..n {
            let idx = (self.write_pos + i) % n;
            self.windowed[i] = self.input_buf[idx] * self.window[i];
        }
        self.fwd
            .process(&mut self.windowed, &mut self.spectrum)
            .expect("fft forward");

        let chain = self.chain.get();
        let scale = 1.0 / n as f32;
        for (i, c) in self.spectrum.iter().enumerate() {
            let scaled = c * scale;
            chain.mags[i] = scaled.norm();
            chain.phases[i] = scaled.arg();
        }
        chain.hop_counter += 1;
    }
}

impl Opcode for Fft {
    fn name(&self) -> &str {
        "fft"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let chain = Arc::new(SpectralChain::new(self.fft_size));
        Box::new(Fft::new(self.fft_size, chain))
    }

    fn reset(&mut self) {
        self.input_buf.fill(0.0);
        self.write_pos = 0;
        self.since_hop = 0;
        let chain = self.chain.get();
        chain.mags.fill(0.0);
        chain.phases.fill(0.0);
        chain.hop_counter = 0;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, &x) in inputs[0].iter().enumerate() {
            self.input_buf[self.write_pos] = x;
            self.write_pos = (self.write_pos + 1) % self.fft_size;
            self.since_hop += 1;

            if self.since_hop >= self.hop {
                self.transform();
                self.since_hop = 0;
            }
            out[i] = x;
        }
    }

    fn has_side_effects(&self) -> bool {
        true
    }

    fn latency(&self) -> usize {
        self.fft_size
    }
}

// ── IFFT ─────────────────────────────────────────────────────────────────

/// Inverse STFT. Reads magnitude/phase from a `SpectralChain` and
/// resynthesises via overlap-add with a Hann window.
pub struct Ifft {
    chain: Arc<SpectralChain>,
    inv: Arc<dyn ComplexToReal<f32>>,
    fft_size: usize,
    hop: usize,
    ola_bufs: [Vec<f32>; 2],
    ola_active: usize,
    read_pos: usize,
    since_hop: usize,
    last_hop: u64,
    spectrum: Vec<Complex<f32>>,
    time_out: Vec<f32>,
    quiet_run: usize,
}

impl Ifft {
    pub fn new(fft_size: usize, chain: Arc<SpectralChain>) -> Self {
        assert!(valid_fft_size(fft_size));
        let hop = fft_size / 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let inv = planner.plan_fft_inverse(fft_size);
        let nb = bins(fft_size);
        Ifft {
            chain,
            inv,
            fft_size,
            hop,
            ola_bufs: [vec![0.0; fft_size], vec![0.0; fft_size]],
            ola_active: 0,
            read_pos: 0,
            since_hop: 0,
            last_hop: 0,
            spectrum: vec![Complex::new(0.0, 0.0); nb],
            time_out: vec![0.0; fft_size],
            quiet_run: 0,
        }
    }

    fn resynthesize(&mut self) {
        let chain = self.chain.get();
        let nb = bins(self.fft_size);
        for i in 0..nb {
            self.spectrum[i] = Complex::from_polar(chain.mags[i], chain.phases[i]);
        }
        // DC and Nyquist bins must be purely real for the inverse transform.
        self.spectrum[0].im = 0.0;
        self.spectrum[nb - 1].im = 0.0;
        self.inv
            .process(&mut self.spectrum, &mut self.time_out)
            .expect("fft inverse");

        // Overlap-add without a synthesis window: the Hann analysis window
        // with 50% overlap already satisfies COLA (sum = 1).
        let next = 1 - self.ola_active;
        self.ola_bufs[next].fill(0.0);
        for i in 0..self.fft_size {
            self.ola_bufs[next][i] = self.time_out[i];
        }
        self.ola_active = next;
        self.last_hop = chain.hop_counter;
    }
}

impl Opcode for Ifft {
    fn name(&self) -> &str {
        "ifft"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("chain")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let chain = Arc::new(SpectralChain::new(self.fft_size));
        Box::new(Ifft::new(self.fft_size, chain))
    }

    fn reset(&mut self) {
        for buf in &mut self.ola_bufs {
            buf.fill(0.0);
        }
        self.ola_active = 0;
        self.read_pos = 0;
        self.since_hop = 0;
        self.last_hop = 0;
        self.quiet_run = 0;
    }

    fn process(&mut self, _ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        let hop_counter = self.chain.get().hop_counter;

        for o in out.iter_mut() {
            self.since_hop += 1;
            if self.since_hop >= self.hop && hop_counter != self.last_hop {
                self.resynthesize();
                self.since_hop = 0;
                self.read_pos = 0;
            }

            let a = self.ola_bufs[self.ola_active]
                .get(self.read_pos)
                .copied()
                .unwrap_or(0.0);
            let prev = 1 - self.ola_active;
            let b = self.ola_bufs[prev]
                .get(self.read_pos + self.hop)
                .copied()
                .unwrap_or(0.0);
            let sample = a + b;
            *o = sample;
            self.quiet_run = if sample.abs() < TAIL_LEVEL {
                self.quiet_run.saturating_add(1)
            } else {
                0
            };
            self.read_pos += 1;
        }
    }

    fn is_active(&self) -> bool {
        self.quiet_run < self.fft_size * 2
    }

    fn latency(&self) -> usize {
        self.fft_size
    }
}

// ── Spectral Freeze ──────────────────────────────────────────────────────

/// Holds the current spectral frame when `freeze` > 0. Magnitudes are
/// locked while phases keep advancing to avoid clicks.
pub struct SpectralFreeze {
    chain: Arc<SpectralChain>,
    frozen_mags: Vec<f32>,
    is_frozen: bool,
    last_hop: u64,
}

impl SpectralFreeze {
    pub fn new(fft_size: usize, chain: Arc<SpectralChain>) -> Self {
        let nb = bins(fft_size);
        SpectralFreeze {
            chain,
            frozen_mags: vec![0.0; nb],
            is_frozen: false,
            last_hop: 0,
        }
    }
}

impl Opcode for SpectralFreeze {
    fn name(&self) -> &str {
        "spectral_freeze"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("chain"), Port::optional("freeze", 0.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let fft_size = self.chain.fft_size();
        let chain = Arc::new(SpectralChain::new(fft_size));
        Box::new(SpectralFreeze::new(fft_size, chain))
    }

    fn reset(&mut self) {
        self.frozen_mags.fill(0.0);
        self.is_frozen = false;
        self.last_hop = 0;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let chain = self.chain.get();

        if chain.hop_counter != self.last_hop {
            self.last_hop = chain.hop_counter;

            let freeze = inputs[1][0] > 0.5;
            if freeze && !self.is_frozen {
                self.frozen_mags.copy_from_slice(&chain.mags);
                self.is_frozen = true;
            } else if !freeze {
                self.is_frozen = false;
            }

            if self.is_frozen {
                chain.mags.copy_from_slice(&self.frozen_mags);
            }
        }

        out.copy_from_slice(&inputs[0][..out.len()]);
    }

    fn has_side_effects(&self) -> bool {
        true
    }
}

// ── Spectral Delay ──────────────────────────────────────────────────────

/// Per-bin delay with frequency-dependent delay time. Higher bins can be
/// delayed more than lower ones via `spread`. Feedback re-injects delayed
/// spectral energy back into the delay line.
pub struct SpectralDelay {
    chain: Arc<SpectralChain>,
    num_bins: usize,
    hop: usize,
    max_hops: usize,
    mag_buf: Vec<f32>,
    phase_buf: Vec<f32>,
    write_pos: usize,
    last_hop: u64,
    sample_rate: f32,
}

impl SpectralDelay {
    pub fn new(
        fft_size: usize,
        chain: Arc<SpectralChain>,
        max_time: f32,
        sample_rate: f32,
    ) -> Self {
        assert!(valid_fft_size(fft_size));
        let nb = bins(fft_size);
        let hop = fft_size / 2;
        let max_hops = ((max_time * sample_rate / hop as f32).ceil() as usize).max(2);
        SpectralDelay {
            chain,
            num_bins: nb,
            hop,
            max_hops,
            mag_buf: vec![0.0; nb * max_hops],
            phase_buf: vec![0.0; nb * max_hops],
            write_pos: 0,
            last_hop: 0,
            sample_rate,
        }
    }
}

impl Opcode for SpectralDelay {
    fn name(&self) -> &str {
        "spectral_delay"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("chain"),
            Port::optional("time", 0.1),
            Port::optional("spread", 0.0),
            Port::optional("feedback", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let fft_size = self.chain.fft_size();
        let max_time = self.max_hops as f32 * self.hop as f32 / self.sample_rate;
        let chain = Arc::new(SpectralChain::new(fft_size));
        Box::new(SpectralDelay::new(
            fft_size,
            chain,
            max_time,
            self.sample_rate,
        ))
    }

    fn reset(&mut self) {
        self.mag_buf.fill(0.0);
        self.phase_buf.fill(0.0);
        self.write_pos = 0;
        self.last_hop = 0;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let chain = self.chain.get();

        if chain.hop_counter != self.last_hop {
            self.last_hop = chain.hop_counter;

            let max_time = self.max_hops as f32 * self.hop as f32 / self.sample_rate;
            let time = inputs[1][0].clamp(0.0, max_time);
            let spread = inputs[2][0].clamp(0.0, 1.0);
            let feedback = inputs[3][0].clamp(0.0, 0.99);

            let time_hops = time * self.sample_rate / self.hop as f32;

            for bin in 0..self.num_bins {
                let frac = if self.num_bins > 1 {
                    bin as f32 / (self.num_bins - 1) as f32
                } else {
                    1.0
                };
                let delay_frac = 1.0 - spread * (1.0 - frac);
                let delay_hops = (time_hops * delay_frac)
                    .round()
                    .clamp(1.0, (self.max_hops - 1) as f32)
                    as usize;

                let read_pos = (self.write_pos + self.max_hops - delay_hops) % self.max_hops;
                let ri = bin * self.max_hops + read_pos;
                let wi = bin * self.max_hops + self.write_pos;

                let delayed_mag = self.mag_buf[ri];
                let delayed_phase = self.phase_buf[ri];

                self.mag_buf[wi] = chain.mags[bin] + delayed_mag * feedback;
                self.phase_buf[wi] = chain.phases[bin];

                chain.mags[bin] = delayed_mag;
                chain.phases[bin] = delayed_phase;
            }

            self.write_pos = (self.write_pos + 1) % self.max_hops;
        }

        out.copy_from_slice(&inputs[0][..out.len()]);
    }

    fn has_side_effects(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::ProcessCtx;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 64;
    const FFT_SIZE: usize = BLOCK * 2;

    fn make_chain() -> Arc<SpectralChain> {
        Arc::new(SpectralChain::new(FFT_SIZE))
    }

    #[test]
    fn fft_ifft_roundtrip_recovers_sine() {
        let chain = make_chain();
        let mut fft = Fft::new(FFT_SIZE, chain.clone());
        let mut ifft = Ifft::new(FFT_SIZE, chain);
        let ctx = ProcessCtx::new(SR, true);

        // Feed several blocks of a 440hz sine so the STFT fills up.
        let blocks = FFT_SIZE / BLOCK + 4;
        let mut all_in = Vec::new();
        let mut all_out = Vec::new();

        for b in 0..blocks {
            let input: Vec<f32> = (0..BLOCK)
                .map(|i| {
                    let t = (b * BLOCK + i) as f32 / SR;
                    (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                })
                .collect();
            let mut fft_out = vec![0.0f32; BLOCK];
            fft.process(&ctx, &[&input], &mut fft_out);

            let mut ifft_out = vec![0.0f32; BLOCK];
            ifft.process(&ctx, &[&fft_out], &mut ifft_out);

            all_in.extend_from_slice(&input);
            all_out.extend_from_slice(&ifft_out);
        }

        // Skip the initial latency (FFT_SIZE samples) and check the rest.
        let skip = FFT_SIZE;
        let steady_in = &all_in[..all_in.len() - skip];
        let steady_out = &all_out[skip..];
        let len = steady_in.len().min(steady_out.len());

        let max_err: f32 = steady_in[..len]
            .iter()
            .zip(&steady_out[..len])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);

        assert!(
            max_err < 0.15,
            "FFT→IFFT roundtrip should approximate the input, max error = {max_err}"
        );
    }

    #[test]
    fn freeze_holds_magnitudes() {
        let chain = make_chain();
        let mut fft = Fft::new(FFT_SIZE, chain.clone());
        let mut freeze = SpectralFreeze::new(FFT_SIZE, chain.clone());
        let ctx = ProcessCtx::new(SR, true);

        // Fill FFT with a sine.
        for b in 0..4 {
            let input: Vec<f32> = (0..BLOCK)
                .map(|i| {
                    let t = (b * BLOCK + i) as f32 / SR;
                    (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                })
                .collect();
            let mut fft_out = vec![0.0f32; BLOCK];
            fft.process(&ctx, &[&input], &mut fft_out);

            let freeze_on = vec![1.0f32; BLOCK];
            let mut freeze_out = vec![0.0f32; BLOCK];
            freeze.process(&ctx, &[&fft_out, &freeze_on], &mut freeze_out);
        }

        // Snapshot the frozen magnitudes.
        let frozen_mags = freeze.frozen_mags.clone();
        assert!(freeze.is_frozen);

        // Feed silence — the chain's mags should still be the frozen ones.
        let silence = vec![0.0f32; BLOCK];
        let mut fft_out = vec![0.0f32; BLOCK];
        fft.process(&ctx, &[&silence], &mut fft_out);

        let freeze_on = vec![1.0f32; BLOCK];
        let mut freeze_out = vec![0.0f32; BLOCK];
        freeze.process(&ctx, &[&fft_out, &freeze_on], &mut freeze_out);

        let chain_inner = chain.get();
        assert_eq!(chain_inner.mags, frozen_mags);
    }

    #[test]
    fn spectral_delay_delays_by_one_hop() {
        let chain = make_chain();
        let mut delay = SpectralDelay::new(FFT_SIZE, chain.clone(), 1.0, SR);
        let ctx = ProcessCtx::new(SR, true);
        let nb = bins(FFT_SIZE);

        let inner = chain.get();
        for i in 0..nb {
            inner.mags[i] = (i + 1) as f32;
            inner.phases[i] = 0.1 * i as f32;
        }
        inner.hop_counter = 1;

        let dummy = vec![0.0f32; BLOCK];
        let time = vec![0.0014f32; BLOCK];
        let spread = vec![0.0f32; BLOCK];
        let feedback = vec![0.0f32; BLOCK];
        let mut out = vec![0.0f32; BLOCK];
        delay.process(&ctx, &[&dummy, &time, &spread, &feedback], &mut out);

        let inner = chain.get();
        for i in 0..nb {
            assert_eq!(
                inner.mags[i], 0.0,
                "first hop should output zeros (delay line empty)"
            );
        }

        for i in 0..nb {
            inner.mags[i] = 99.0;
            inner.phases[i] = 0.0;
        }
        inner.hop_counter = 2;
        delay.process(&ctx, &[&dummy, &time, &spread, &feedback], &mut out);

        let inner = chain.get();
        for i in 0..nb {
            assert!(
                (inner.mags[i] - (i + 1) as f32).abs() < 1e-6,
                "bin {i}: expected {}, got {}",
                (i + 1) as f32,
                inner.mags[i]
            );
        }
    }

    #[test]
    fn spectral_delay_passthrough_audio() {
        let chain = make_chain();
        let mut delay = SpectralDelay::new(FFT_SIZE, chain.clone(), 1.0, SR);
        let ctx = ProcessCtx::new(SR, true);

        let input: Vec<f32> = (0..BLOCK).map(|i| i as f32 * 0.01).collect();
        let time = vec![0.1f32; BLOCK];
        let spread = vec![0.0f32; BLOCK];
        let feedback = vec![0.0f32; BLOCK];
        let mut out = vec![0.0f32; BLOCK];
        delay.process(&ctx, &[&input, &time, &spread, &feedback], &mut out);
        assert_eq!(out, input, "audio should pass through unchanged");
    }

    #[test]
    fn fft_passthrough_equals_input() {
        let chain = make_chain();
        let mut fft = Fft::new(FFT_SIZE, chain);
        let ctx = ProcessCtx::new(SR, true);

        let input: Vec<f32> = (0..BLOCK).map(|i| i as f32 * 0.01).collect();
        let mut out = vec![0.0f32; BLOCK];
        fft.process(&ctx, &[&input], &mut out);
        assert_eq!(out, input);
    }
}
