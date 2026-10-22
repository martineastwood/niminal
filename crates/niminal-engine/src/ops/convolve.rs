use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};
use crate::table::Buffer;

/// Partition size matches the engine block so each `process` call handles
/// exactly one partition with no extra buffering.
fn part() -> usize {
    crate::block_size()
}
/// FFT length for each partition: double the block for overlap-save.
fn fft_len() -> usize {
    part() * 2
}
/// Number of complex bins in a real FFT of `fft_len()` samples.
fn nbins() -> usize {
    fft_len() / 2 + 1
}

/// Pre-transformed IR partitions, shared across voices.
pub struct IrSpectrum {
    parts: Vec<Vec<Complex<f32>>>,
}

impl IrSpectrum {
    /// Build from an IR buffer (mono, channel 0).
    pub fn from_buffer(buffer: &Buffer) -> Self {
        Self::from_samples(buffer.len(), |i| buffer.read(i))
    }

    /// Build from the samples of one channel of an IR.
    pub fn from_slice(samples: &[f32]) -> Self {
        Self::from_samples(samples.len(), |i| samples[i])
    }

    /// The IR is split into block-sized partitions, each zero-padded to twice that and transformed.
    fn from_samples(n: usize, read: impl Fn(usize) -> f32) -> Self {
        let num_parts = n.div_ceil(part());
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(fft_len());

        let mut time = vec![0.0f32; fft_len()];
        let mut spectrum = vec![Complex::new(0.0, 0.0); nbins()];
        let mut parts = Vec::with_capacity(num_parts);

        for p in 0..num_parts {
            time.fill(0.0);
            let start = p * part();
            let end = (start + part()).min(n);
            for i in start..end {
                time[i - start] = read(i);
            }
            fwd.process(&mut time, &mut spectrum).expect("fft forward");
            let mut bins = vec![Complex::new(0.0, 0.0); nbins()];
            bins.copy_from_slice(&spectrum);
            parts.push(bins);
        }

        IrSpectrum { parts }
    }

    fn num_parts(&self) -> usize {
        self.parts.len()
    }
}

impl std::fmt::Debug for IrSpectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrSpectrum")
            .field("partitions", &self.parts.len())
            .finish()
    }
}

/// Uniformly-partitioned frequency-domain convolution.
///
/// Each `process` call takes one `PART`-sample block, FFTs it, multiplies
/// against all IR partitions (sliding the input history), sums, and IFFTs.
/// There is no latency beyond the block itself.
pub struct Convolve {
    ir: Arc<IrSpectrum>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// Ring buffer of input spectra, length = number of IR partitions.
    input_spectra: Vec<Vec<Complex<f32>>>,
    /// Which slot in `input_spectra` the next block writes to.
    head: usize,
    /// Scratch for the forward FFT input (overlap-save: last PART samples of
    /// the previous block followed by the current PART samples).
    prev_tail: Vec<f32>,
    /// Scratch for accumulating the frequency-domain product.
    accum: Vec<Complex<f32>>,
    /// Scratch for the inverse FFT output.
    time_out: Vec<f32>,
    /// Scratch for forward FFT input.
    time_in: Vec<f32>,
    /// Scratch for forward FFT output.
    spec_scratch: Vec<Complex<f32>>,
    /// The transforms' own working space, so the audio thread never allocates.
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    /// Consecutive output samples below tail level.
    quiet_run: usize,
    /// Total IR length in samples, for tail tracking.
    ir_samples: usize,
}

impl Convolve {
    pub fn new(ir: Arc<IrSpectrum>) -> Self {
        let np = ir.num_parts().max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(fft_len());
        let inv = planner.plan_fft_inverse(fft_len());
        let ir_samples = ir.num_parts() * part();
        Convolve {
            ir,
            input_spectra: vec![vec![Complex::new(0.0, 0.0); nbins()]; np],
            head: 0,
            prev_tail: vec![0.0; part()],
            accum: vec![Complex::new(0.0, 0.0); nbins()],
            time_out: vec![0.0; fft_len()],
            time_in: vec![0.0; fft_len()],
            spec_scratch: vec![Complex::new(0.0, 0.0); nbins()],
            scratch_fwd: fwd.make_scratch_vec(),
            scratch_inv: inv.make_scratch_vec(),
            fwd,
            inv,
            quiet_run: ir_samples,
            ir_samples,
        }
    }
}

impl Opcode for Convolve {
    fn name(&self) -> &str {
        "convolve"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("mix", 1.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Convolve::new(self.ir.clone()))
    }

    fn reset(&mut self) {
        for s in &mut self.input_spectra {
            s.fill(Complex::new(0.0, 0.0));
        }
        self.head = 0;
        self.prev_tail.fill(0.0);
        self.quiet_run = self.ir_samples;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && Arc::ptr_eq(&self.ir, &old.ir)
        {
            std::mem::swap(&mut self.input_spectra, &mut old.input_spectra);
            self.head = old.head;
            self.prev_tail.copy_from_slice(&old.prev_tail);
            self.quiet_run = old.quiet_run;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let np = self.ir.num_parts();
        if np == 0 {
            out.iter_mut().zip(inputs[0]).for_each(|(o, &x)| *o = x);
            return;
        }

        // Build the overlap-save input: [prev_tail | current block].
        self.time_in[..part()].copy_from_slice(&self.prev_tail);
        self.time_in[part()..fft_len()].copy_from_slice(&inputs[0][..part()]);
        self.prev_tail.copy_from_slice(&inputs[0][..part()]);

        // Forward FFT of the input block.
        self.fwd
            .process_with_scratch(
                &mut self.time_in,
                &mut self.spec_scratch,
                &mut self.scratch_fwd,
            )
            .expect("fft forward");
        self.input_spectra[self.head].copy_from_slice(&self.spec_scratch);

        // Multiply-accumulate: sum over all partitions.
        self.accum.fill(Complex::new(0.0, 0.0));
        for p in 0..np {
            let input_idx = (self.head + np - p) % np;
            let ir_part = &self.ir.parts[p];
            let input_part = &self.input_spectra[input_idx];
            for (acc, (a, b)) in self.accum.iter_mut().zip(input_part.iter().zip(ir_part)) {
                *acc += a * b;
            }
        }

        // Inverse FFT.
        self.inv
            .process_with_scratch(&mut self.accum, &mut self.time_out, &mut self.scratch_inv)
            .expect("fft inverse");

        // The valid output is the second half (overlap-save).
        let scale = 1.0 / fft_len() as f32;
        let mut loudest: f32 = 0.0;
        for (i, o) in out.iter_mut().enumerate() {
            let wet = self.time_out[part() + i] * scale;
            let mix = inputs[1][i];
            *o = inputs[0][i] * (1.0 - mix) + wet * mix;
            loudest = loudest.max(wet.abs());
        }
        self.quiet_run = if loudest < TAIL_LEVEL {
            self.quiet_run.saturating_add(1)
        } else {
            0
        };

        self.head = (self.head + 1) % np;
    }

    fn is_active(&self) -> bool {
        self.quiet_run < self.ir_samples / part() + 1
    }

    // Overlap-save gives each block's output as soon as the block arrives, so there is no latency.

    fn per_sample(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::ProcessCtx;
    use crate::table::Buffer;

    fn dirac_ir(len: usize) -> Arc<Buffer> {
        let buf = Arc::new(Buffer::zeros(len, Some(48_000.0)));
        buf.write(0, 1.0);
        buf
    }

    #[test]
    fn identity_ir_passes_signal_through() {
        let ir_buf = dirac_ir(part());
        let ir = Arc::new(IrSpectrum::from_buffer(&ir_buf));
        let mut conv = Convolve::new(ir);

        let input: Vec<f32> = (0..part())
            .map(|i| (i as f32 + 1.0) / part() as f32)
            .collect();
        let mix = vec![1.0f32; part()];
        let mut out = vec![0.0f32; part()];

        conv.process(&ProcessCtx::new(48_000.0, true), &[&input, &mix], &mut out);

        let max_err: f32 = out
            .iter()
            .zip(&input)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            max_err < 0.01,
            "dirac IR should reproduce input, max error = {max_err}"
        );
    }

    #[test]
    fn mix_zero_is_dry_signal() {
        let ir_buf = dirac_ir(part() * 4);
        let ir = Arc::new(IrSpectrum::from_buffer(&ir_buf));
        let mut conv = Convolve::new(ir);

        let input: Vec<f32> = (0..part()).map(|i| (i as f32) * 0.01).collect();
        let mix = vec![0.0f32; part()];
        let mut out = vec![0.0f32; part()];

        conv.process(&ProcessCtx::new(48_000.0, true), &[&input, &mix], &mut out);

        assert_eq!(out, input, "mix=0 should pass the dry signal unchanged");
    }
}
