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
    fn step(&mut self, x: f32, taps: &[f32]) -> f32 {
        self.history[self.pos] = x;
        let mut y = 0.0;
        let mut index = self.pos;
        for &tap in taps {
            y += tap * self.history[index];
            index = if index == 0 {
                self.history.len() - 1
            } else {
                index - 1
            };
        }
        self.pos = (self.pos + 1) % self.history.len();
        y
    }
}

/// Band-limited rate conversion around DSP effects running at 2x, 4x or 8x.
/// Both FIRs have eight base-rate samples of group delay.
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
        let len = 16 * factor + 1;
        let cutoff = 0.45 / factor as f64;
        let mut taps: Vec<f32> = (0..len)
            .map(|i| {
                let x = i as f64 - (len / 2) as f64;
                let sinc = if x == 0.0 {
                    2.0 * cutoff
                } else {
                    (2.0 * PI * cutoff * x).sin() / (PI * x)
                };
                let window = 0.42 - 0.5 * (2.0 * PI * i as f64 / (len - 1) as f64).cos()
                    + 0.08 * (4.0 * PI * i as f64 / (len - 1) as f64).cos();
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
        16 + self.op.latency().div_ceil(self.factor)
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
                    let x = if phase == 0 {
                        slots[index][i] * self.factor as f32
                    } else {
                        0.0
                    };
                    input[0] = fir.step(x, &self.up_taps);
                }
                self.op
                    .process_slots(&sub, &self.inputs, &self.indices, &mut self.scratch, 1);
                for ((fir, scratch), out) in
                    self.down.iter_mut().zip(&self.scratch).zip(outs.iter_mut())
                {
                    let y = fir.step(scratch[0], &self.taps);
                    if phase == 0 {
                        out[i] = y;
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
