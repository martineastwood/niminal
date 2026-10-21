use crate::opcode::{Opcode, Port, ProcessCtx, blocks};

/// Restarts a node at positive trigger edges. Storage is allocated with the voice.
pub struct ResetOn {
    op: Box<dyn Opcode>,
    ports: Vec<Port>,
    slots: Vec<Vec<f32>>,
    indices: Vec<usize>,
    scratch: Vec<Vec<f32>>,
    positive: bool,
}

impl ResetOn {
    pub fn new(op: Box<dyn Opcode>) -> Self {
        let mut ports = op.ports().to_vec();
        let inputs = ports.len();
        ports.push(Port::required("__reset"));
        Self {
            scratch: blocks(op.outputs()),
            op,
            ports,
            slots: blocks(inputs),
            indices: (0..inputs).collect(),
            positive: false,
        }
    }
}

impl Opcode for ResetOn {
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
        self.op.name()
    }
    fn ports(&self) -> &[Port] {
        &self.ports
    }
    fn outputs(&self) -> usize {
        self.op.outputs()
    }
    fn max_inputs(&self) -> usize {
        self.ports.len()
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.op.box_clone()))
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.op.prepare(sample_rate);
    }
    fn reset(&mut self) {
        self.positive = false;
        self.op.reset();
    }
    fn is_active(&self) -> bool {
        self.op.is_active()
    }
    fn has_active_envelope(&self) -> bool {
        self.op.has_active_envelope()
    }
    fn is_envelope(&self) -> bool {
        self.op.is_envelope()
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
    fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        self.op.set_control(key, start, target, frames, elapsed);
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.positive = old.positive;
            self.op.carry_state(&mut *old.op);
        }
    }
    fn process(&mut self, _ctx: &ProcessCtx, _inputs: &[&[f32]], _out: &mut [f32]) {
        unreachable!("reset scopes use the graph slot adapter")
    }
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        let trigger = &slots[*indices.last().expect("reset input")];
        let mut start = 0;
        while start < frames {
            let positive = trigger[start] > 0.0;
            if positive && !self.positive {
                self.op.reset();
            }
            self.positive = positive;
            let mut end = start + 1;
            while end < frames {
                let next = trigger[end] > 0.0;
                if next && !self.positive {
                    break;
                }
                self.positive = next;
                end += 1;
            }
            let count = end - start;
            for (scratch, &index) in self.slots.iter_mut().zip(indices) {
                scratch[..count].copy_from_slice(&slots[index][start..end]);
            }
            self.op
                .process_slots(ctx, &self.slots, &self.indices, &mut self.scratch, count);
            for (out, scratch) in outs.iter_mut().zip(&self.scratch) {
                out[start..end].copy_from_slice(&scratch[..count]);
            }
            start = end;
        }
    }
}
