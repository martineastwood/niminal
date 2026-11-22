use crate::opcode::{Opcode, Port, ProcessCtx, block_size, blocks};
use std::f64::consts::PI;
use std::sync::OnceLock;

/// `sum(x[i] * taps[i])`, with eight running sums so the loop vectorises.
#[inline(always)]
fn dot(x: &[f32], taps: &[f32]) -> f32 {
    let n = x.len().min(taps.len());
    let (x, taps) = (&x[..n], &taps[..n]);
    let mut acc = [0.0f32; 8];
    let ((xs, xr), (ts, tr)) = (x.as_chunks::<8>(), taps.as_chunks::<8>());
    for (a, b) in xs.iter().zip(ts) {
        for j in 0..8 {
            acc[j] += a[j] * b[j];
        }
    }
    let mut y = ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]));
    for (a, b) in xr.iter().zip(tr) {
        y += a * b;
    }
    y
}

/// The zeroth-order modified Bessel function, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..50 {
        term *= (x / (2.0 * f64::from(k))).powi(2);
        sum += term;
    }
    sum
}

struct FilterTables {
    /// The filter that brings the oversampled signal back down, last tap first.
    taps: Vec<f32>,
    /// One sub-filter per phase of the upsampling filter, over the base-rate samples, last tap first.
    /// Each phase sums to 1.0.
    up_phases: Vec<Vec<f32>>,
    /// The phase whose sub-filter is a pure unit impulse (all zeros except `center_offset`).
    center_phase: usize,
    /// Offset into the `up_len - 1` history + chunk window for the impulse phase.
    center_offset: usize,
    /// Maximum sub-filter length across upsampling phases (`26`).
    up_len: usize,
}

impl FilterTables {
    fn build(factor: usize) -> Self {
        let len = 25 * factor + 1;
        let half = len / 2;
        // Half the base rate, in cycles per sample at the oversampled rate: the middle of the transition.
        let cutoff = 0.5 / factor as f64;
        let beta = 0.1102 * (80.0 - 8.7);
        let denom = bessel_i0(beta);
        let mut taps: Vec<f32> = (0..len)
            .map(|i| {
                let d = i as isize - half as isize;
                let sinc = if d == 0 {
                    2.0 * cutoff
                } else if d % factor as isize == 0 {
                    // sin(pi * d / factor) is identically zero at non-zero multiples of `factor`.
                    0.0
                } else {
                    let x = d as f64;
                    (2.0 * PI * cutoff * x).sin() / (PI * x)
                };
                let r = 2.0 * i as f64 / (len - 1) as f64 - 1.0;
                let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom;
                (sinc * window) as f32
            })
            .collect();
        let sum: f32 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }
        let mut up_taps = taps.clone();
        for phase in 0..factor {
            let phase_sum: f32 = taps.iter().skip(phase).step_by(factor).sum();
            for tap in up_taps.iter_mut().skip(phase).step_by(factor) {
                *tap /= phase_sum;
            }
        }
        // Only every factor-th input of the upsampling filter is not zero, so each phase is a short filter
        // over the base-rate samples.
        let up_phases: Vec<Vec<f32>> = (0..factor)
            .map(|phase| {
                up_taps
                    .iter()
                    .skip(phase)
                    .step_by(factor)
                    .rev()
                    .copied()
                    .collect()
            })
            .collect();
        let up_len = up_phases.iter().map(Vec::len).max().unwrap_or(0);
        let center_phase = half % factor;
        let phase_len = up_phases[center_phase].len();
        let center_in_phase = phase_len - 1 - (half - center_phase) / factor;
        let center_offset = (up_len - phase_len) + center_in_phase;
        taps.reverse();

        Self {
            taps,
            up_phases,
            center_phase,
            center_offset,
            up_len,
        }
    }
}

fn filter_tables(factor: usize) -> &'static FilterTables {
    static T2: OnceLock<FilterTables> = OnceLock::new();
    static T4: OnceLock<FilterTables> = OnceLock::new();
    static T8: OnceLock<FilterTables> = OnceLock::new();
    match factor {
        2 => T2.get_or_init(|| FilterTables::build(2)),
        4 => T4.get_or_init(|| FilterTables::build(4)),
        8 => T8.get_or_init(|| FilterTables::build(8)),
        _ => panic!("oversample factor must be 2, 4 or 8, not {factor}"),
    }
}

/// Band-limited rate conversion around DSP effects running at 2x, 4x or 8x.
/// Both filters are Kaiser-windowed sincs (80dB down) that are flat to 0.4 of the base rate, and the
/// stopband starts at 0.6 of it, so what folds back lands above the audible passband. Together they
/// have 25 base-rate samples of delay.
pub struct Oversample {
    op: Box<dyn Opcode>,
    factor: usize,
    tables: &'static FilterTables,
    /// Overlap-save buffers per input: `up_history` past samples followed by the current base chunk.
    up: Vec<Vec<f32>>,
    /// `(last_value, steady_samples)` per input to bypass FIR convolution on constant/steady inputs.
    up_steady: Vec<(f32, usize)>,
    /// Overlap-save buffers per output: `down_history` past oversampled samples followed by the chunk.
    down: Vec<Vec<f32>>,
    inputs: Vec<Vec<f32>>,
    indices: Vec<usize>,
    scratch: Vec<Vec<f32>>,
    started: bool,
    /// None while held; a finite rate-conversion tail budget after release.
    tail_left: Option<usize>,
}

impl Oversample {
    pub fn new(op: Box<dyn Opcode>, factor: usize) -> Self {
        assert!([2, 4, 8].contains(&factor));
        let tables = filter_tables(factor);
        let inputs = op.ports().len();
        let outputs = op.outputs();
        let max_chunk_out = block_size();
        let max_chunk_in = (max_chunk_out / factor).max(1);
        let up_history = tables.up_len - 1;
        let down_history = tables.taps.len() - 1;
        Self {
            op,
            factor,
            tables,
            up: vec![vec![0.0; up_history + max_chunk_in]; inputs],
            up_steady: vec![(0.0, up_history); inputs],
            down: vec![vec![0.0; down_history + max_chunk_out]; outputs],
            inputs: blocks(inputs),
            indices: (0..inputs).collect(),
            scratch: blocks(outputs),
            started: false,
            tail_left: None,
        }
    }
}

impl Opcode for Oversample {
    fn buffer_channels(&self, key: u64) -> usize {
        self.op.buffer_channels(key)
    }
    fn buffer_bindings(&self, key: u64) -> usize {
        self.op.buffer_bindings(key)
    }
    fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank], cursor: &mut usize) {
        self.op.swap_buffers(key, banks, cursor);
    }

    fn reports(&self, visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {
        self.op.reports(visit);
    }
    fn name(&self) -> &str {
        "oversample"
    }
    fn ports(&self) -> &[Port] {
        self.op.ports()
    }
    fn outputs(&self) -> usize {
        self.op.outputs()
    }
    fn max_inputs(&self) -> usize {
        self.op.max_inputs()
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.op.box_clone(), self.factor))
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.op.prepare(sample_rate * self.factor as f32);
    }
    fn reset(&mut self) {
        self.op.reset();
        self.started = false;
        self.tail_left = None;
        let up_history = self.tables.up_len - 1;
        for buf in &mut self.up {
            buf.fill(0.0);
        }
        self.up_steady.fill((0.0, up_history));
        for buf in &mut self.down {
            buf.fill(0.0);
        }
    }
    fn is_envelope(&self) -> bool {
        self.op.is_envelope()
    }
    fn has_active_envelope(&self) -> bool {
        self.op.has_active_envelope()
    }
    fn is_active(&self) -> bool {
        let down_history = self.tables.taps.len() - 1;
        self.op.is_active()
            || match self.tail_left {
                Some(left) => left > 0,
                None => self.down.iter().any(|buf| {
                    buf[..down_history]
                        .iter()
                        .any(|x| x.abs() > crate::opcode::TAIL_LEVEL)
                }),
            }
    }
    fn has_side_effects(&self) -> bool {
        self.op.has_side_effects()
    }
    fn input_is_init(&self, index: usize) -> bool {
        self.op.input_is_init(index)
    }
    fn latency(&self) -> usize {
        (self.tables.taps.len() - 1) / self.factor + self.op.latency().div_ceil(self.factor)
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], _: &mut [f32]) {
        unreachable!("oversampling uses graph slots")
    }
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        let keep_tail = ctx.gate || ctx.envelopes_active || self.op.is_active();
        let sub = ProcessCtx {
            sample_rate: ctx.sample_rate * self.factor as f32,
            ..*ctx
        };
        let factor = self.factor;
        let tables = self.tables;
        let up_history = tables.up_len - 1;
        let down_history = tables.taps.len() - 1;
        let max_chunk_in = (block_size() / factor).max(1);

        if frames > 0 && !self.started {
            // Extend the first input backwards so init parameters see their actual value.
            for ((buf, steady), &index) in self.up.iter_mut().zip(&mut self.up_steady).zip(indices)
            {
                let x0 = slots[index][0];
                buf[..up_history].fill(x0);
                *steady = (x0, up_history);
            }
            self.started = true;
        }

        let mut offset = 0;
        while offset < frames {
            let chunk_in = (frames - offset).min(max_chunk_in);
            let chunk_out = chunk_in * factor;

            // Upsample each input port into `self.inputs[p][..chunk_out]`.
            for (((buf, steady), &index), input) in self
                .up
                .iter_mut()
                .zip(&mut self.up_steady)
                .zip(indices)
                .zip(&mut self.inputs)
            {
                let src = &slots[index][offset..offset + chunk_in];
                let first = src[0];
                let chunk_constant = src.iter().all(|&x| x.to_bits() == first.to_bits());
                if chunk_constant && steady.1 >= up_history && first.to_bits() == steady.0.to_bits()
                {
                    input[..chunk_out].fill(first);
                    continue;
                }

                buf[up_history..up_history + chunk_in].copy_from_slice(src);
                for i in 0..chunk_in {
                    let base = i * factor;
                    for (phase, taps) in tables.up_phases.iter().enumerate() {
                        input[base + phase] = if phase == tables.center_phase {
                            buf[i + tables.center_offset]
                        } else {
                            let start = i + tables.up_len - taps.len();
                            dot(&buf[start..start + taps.len()], taps)
                        };
                    }
                }
                buf.copy_within(chunk_in..chunk_in + up_history, 0);

                if chunk_constant {
                    if first.to_bits() == steady.0.to_bits() {
                        steady.1 = (steady.1 + chunk_in).min(up_history);
                    } else {
                        *steady = (first, chunk_in.min(up_history));
                    }
                } else {
                    *steady = (src[chunk_in - 1], 0);
                }
            }

            // Run the oversampled inner opcode over the whole chunk at once.
            self.op.process_slots(
                &sub,
                &self.inputs,
                &self.indices,
                &mut self.scratch,
                chunk_out,
            );

            // Downsample each output channel into `outs[o][offset..offset + chunk_in]`.
            let taps = &tables.taps;
            for ((buf, scratch), out) in self
                .down
                .iter_mut()
                .zip(&self.scratch)
                .zip(outs.iter_mut())
            {
                buf[down_history..down_history + chunk_out].copy_from_slice(&scratch[..chunk_out]);
                for i in 0..chunk_in {
                    let start = i * factor;
                    out[offset + i] = dot(&buf[start..start + taps.len()], taps);
                }
                buf.copy_within(chunk_out..chunk_out + down_history, 0);
            }

            offset += chunk_in;
        }

        // The FIRs have finite support. A free-running generator inside this scope must not
        // keep an enclosing voice alive forever after its amplitude envelope has finished.
        let support = (2 * (tables.taps.len() - 1)).div_ceil(factor);
        self.tail_left = if ctx.gate {
            None
        } else if keep_tail || self.op.is_active() {
            Some(support)
        } else {
            Some(self.tail_left.unwrap_or(support).saturating_sub(frames))
        };
    }
}
