use crate::opcode::{Opcode, Port, ProcessCtx, blocks};
use std::f64::consts::PI;

struct Fir {
    history: Vec<f32>,
    pos: usize,
}
impl Fir {
    fn new(len: usize) -> Self {
        Self {
            history: vec![0.0; len],
            pos: 0,
        }
    }
    fn reset(&mut self) {
        self.history.fill(0.0);
        self.pos = 0;
    }
    fn push(&mut self, x: f32) {
        self.history[self.pos] = x;
        self.pos = (self.pos + 1) % self.history.len();
    }
    /// The sum of `taps[first + k * stride]` times the sample pushed `first + k * stride` pushes ago
    /// (0 is the latest).
    fn dot(&self, taps: &[f32], first: usize, stride: usize) -> f32 {
        let len = self.history.len();
        let mut y = 0.0;
        let mut index = (self.pos + len - 1 - first) % len;
        let mut k = first;
        while k < taps.len() {
            y += taps[k] * self.history[index];
            index = if index >= stride {
                index - stride
            } else {
                index + len - stride
            };
            k += stride;
        }
        y
    }
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

/// Band-limited rate conversion around DSP effects running at 2x, 4x or 8x.
/// Both filters are Kaiser-windowed sincs (80dB down) that are flat to 0.4 of the base rate, and the
/// stopband starts at 0.6 of it, so what folds back lands above the audible passband. Together they
/// have 25 base-rate samples of delay.
pub struct Oversample {
    op: Box<dyn Opcode>,
    factor: usize,
    taps: Vec<f32>,
    up_taps: Vec<f32>,
    up: Vec<Fir>,
    down: Vec<Fir>,
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
        let len = 25 * factor + 1;
        // Half the base rate, in cycles per sample at the oversampled rate: the middle of the transition.
        let cutoff = 0.5 / factor as f64;
        let beta = 0.1102 * (80.0 - 8.7);
        let mut taps: Vec<f32> = (0..len)
            .map(|i| {
                let x = i as f64 - (len / 2) as f64;
                let sinc = if x == 0.0 {
                    2.0 * cutoff
                } else {
                    (2.0 * PI * cutoff * x).sin() / (PI * x)
                };
                let r = 2.0 * i as f64 / (len - 1) as f64 - 1.0;
                let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
                (sinc * window) as f32
            })
            .collect();
        let sum: f32 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }
        let mut up_taps = taps.clone();
        for phase in 0..factor {
            let sum: f32 = taps.iter().skip(phase).step_by(factor).sum();
            for tap in up_taps.iter_mut().skip(phase).step_by(factor) {
                *tap /= sum * factor as f32;
            }
        }
        let inputs = op.ports().len();
        let outputs = op.outputs();
        Self {
            op,
            factor,
            taps,
            up_taps,
            up: (0..inputs).map(|_| Fir::new(len)).collect(),
            down: (0..outputs).map(|_| Fir::new(len)).collect(),
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
        for fir in self.up.iter_mut().chain(&mut self.down) {
            fir.reset();
        }
    }
    fn is_envelope(&self) -> bool {
        self.op.is_envelope()
    }
    fn has_active_envelope(&self) -> bool {
        self.op.has_active_envelope()
    }
    fn is_active(&self) -> bool {
        self.op.is_active()
            || match self.tail_left {
                Some(left) => left > 0,
                None => self.down.iter().any(|f| {
                    f.history
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
        (self.taps.len() - 1) / self.factor + self.op.latency().div_ceil(self.factor)
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
        for i in 0..frames {
            if !self.started {
                // Extend the first input backwards so init parameters see their actual value.
                for (fir, &index) in self.up.iter_mut().zip(indices) {
                    for k in 0..fir.history.len() {
                        fir.history[k] = if k == 0 || (fir.history.len() - k) % self.factor == 0 {
                            slots[index][i] * self.factor as f32
                        } else {
                            0.0
                        };
                    }
                }
                self.started = true;
            }
            for phase in 0..self.factor {
                for ((fir, input), &index) in self.up.iter_mut().zip(&mut self.inputs).zip(indices)
                {
                    // Only every factor-th input is not zero, so only those taps count.
                    fir.push(if phase == 0 {
                        slots[index][i] * self.factor as f32
                    } else {
                        0.0
                    });
                    input[0] = fir.dot(&self.up_taps, phase, self.factor);
                }
                self.op
                    .process_slots(&sub, &self.inputs, &self.indices, &mut self.scratch, 1);
                for ((fir, scratch), out) in
                    self.down.iter_mut().zip(&self.scratch).zip(outs.iter_mut())
                {
                    fir.push(scratch[0]);
                    if phase == 0 {
                        out[i] = fir.dot(&self.taps, 0, 1);
                    }
                }
            }
        }
        // The FIRs have finite support. A free-running generator inside this scope must not
        // keep an enclosing voice alive forever after its amplitude envelope has finished.
        let support = (2 * (self.taps.len() - 1)).div_ceil(self.factor);
        self.tail_left = if ctx.gate {
            None
        } else if keep_tail || self.op.is_active() {
            Some(support)
        } else {
            Some(self.tail_left.unwrap_or(support).saturating_sub(frames))
        };
    }
}
