use crate::opcode::{KernelStats, Opcode, Port, ProcessCtx, block_size, blocks};

/// A DSP body that runs once per block: `control { ... }`. Its inputs are read at the start of the
/// block, its state advances by one block's time, and each result is held through the block.
pub struct ControlRate {
    op: Box<dyn Opcode>,
    inputs: Vec<Vec<f32>>,
    indices: Vec<usize>,
    scratch: Vec<Vec<f32>>,
}

impl ControlRate {
    pub fn new(op: Box<dyn Opcode>) -> Self {
        let inputs = op.ports().len();
        let outputs = op.outputs();
        Self {
            op,
            inputs: blocks(inputs),
            indices: (0..inputs).collect(),
            scratch: blocks(outputs),
        }
    }
}

impl Opcode for ControlRate {
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
        "control"
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
        Box::new(Self::new(self.op.box_clone()))
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.op.prepare(sample_rate / block_size() as f32);
    }
    fn reset(&mut self) {
        self.op.reset();
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.op.carry_state(&mut *old.op);
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
    }
    fn has_side_effects(&self) -> bool {
        self.op.has_side_effects()
    }
    fn input_is_init(&self, index: usize) -> bool {
        self.op.input_is_init(index)
    }
    fn latency(&self) -> usize {
        self.op.latency()
    }
    fn kernel_stats(&self) -> Option<KernelStats> {
        self.op.kernel_stats().map(|stats| KernelStats {
            control_scope: true,
            ..stats
        })
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], _: &mut [f32]) {
        unreachable!("a control scope uses graph slots")
    }
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        if frames == 0 {
            return;
        }
        for (input, &index) in self.inputs.iter_mut().zip(indices) {
            input[0] = slots[index][0];
        }
        let sub = ProcessCtx {
            sample_rate: ctx.sample_rate / block_size() as f32,
            ..*ctx
        };
        self.op
            .process_slots(&sub, &self.inputs, &self.indices, &mut self.scratch, 1);
        for (out, held) in outs.iter_mut().zip(&self.scratch) {
            out[..frames].fill(held[0]);
        }
    }
}

/// `x.smooth`: a control value `x`, held through each block, ramped linearly from the previous
/// block's value to this one's so a stepped control doesn't zipper.
#[derive(Clone, Default)]
pub struct Ramp {
    to: f32,
    started: bool,
}

impl Opcode for Ramp {
    fn reset(&mut self) {
        self.started = false;
    }

    fn name(&self) -> &'static str {
        "smooth"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Ramp::default())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.to = old.to;
            self.started = old.started;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let Some(&target) = inputs[0].first() else {
            return;
        };
        let from = if self.started { self.to } else { target };
        self.started = true;
        self.to = target;
        let n = out.len() as f32;
        for (i, o) in out.iter_mut().enumerate() {
            *o = from + (target - from) * ((i + 1) as f32 / n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(op: &mut impl Opcode, x: f32, n: usize) -> Vec<f32> {
        let input = vec![x; n];
        let mut out = vec![0.0; n];
        op.process(&ProcessCtx::new(48_000.0, true), &[&input], &mut out);
        out
    }

    #[test]
    fn a_ramp_reaches_each_new_value_by_the_end_of_the_block() {
        let mut ramp = Ramp::default();
        assert!(run(&mut ramp, 1.0, 4).iter().all(|&v| v == 1.0));
        let out = run(&mut ramp, 5.0, 4);
        assert_eq!(out, [2.0, 3.0, 4.0, 5.0]);
        assert_eq!(run(&mut ramp, 5.0, 4), [5.0; 4]);
    }
}
