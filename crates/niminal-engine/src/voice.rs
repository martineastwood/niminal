use std::sync::Arc;

use crate::graph::Graph;
use crate::opcode::{BLOCK, MAX_INPUTS, Opcode, ProcessCtx};

/// One running note: a private copy of every opcode's state plus the signal
/// buffers that connect them.
pub struct Voice {
    graph: Arc<Graph>,
    ops: Vec<Box<dyn Opcode>>,
    slots: Vec<[f32; BLOCK]>,
    sample_rate: f32,
    released: bool,
    poisoned: bool,
}

impl Voice {
    pub fn new(graph: Arc<Graph>, sample_rate: f32) -> Self {
        let mut ops: Vec<Box<dyn Opcode>> = graph.nodes.iter().map(|n| n.op.box_clone()).collect();
        for op in &mut ops {
            op.prepare(sample_rate);
        }

        let mut slots = vec![[0.0; BLOCK]; graph.node_slot_base() + ops.len()];
        for (i, p) in graph.params.iter().enumerate() {
            slots[i] = [p.default; BLOCK];
        }
        for (i, &c) in graph.consts.iter().enumerate() {
            slots[graph.params.len() + i] = [c; BLOCK];
        }

        Voice { graph, ops, slots, sample_rate, released: false, poisoned: false }
    }

    /// Set a parameter by index. Takes effect from the next block.
    pub fn set_param(&mut self, index: usize, value: f32) {
        self.slots[index] = [value; BLOCK];
    }

    /// Set a parameter by name; returns false if there is no such parameter.
    pub fn set_param_by_name(&mut self, name: &str, value: f32) -> bool {
        match self.graph.param_index(name) {
            Some(i) => {
                self.set_param(i, value);
                true
            }
            None => false,
        }
    }

    /// Note off. Envelopes with a sustain point begin their release.
    pub fn release(&mut self) {
        self.released = true;
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// A voice lives until it has been released and every envelope has
    /// finished, which may be well after the scheduled note length.
    pub fn is_finished(&self) -> bool {
        self.poisoned || (self.released && !self.ops.iter().any(|op| op.is_active()))
    }

    /// True once the voice produced NaN or infinity. It is silenced from then
    /// on (its state can't be trusted) and counts as finished.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Render `out.len()` samples (at most [`BLOCK`]). Output is identical
    /// however a stretch of audio is divided into calls, so a host can split
    /// blocks at event times for sample-accurate timing.
    pub fn process(&mut self, out: &mut [f32]) {
        let frames = out.len();
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        if self.poisoned {
            out.fill(0.0);
            return;
        }
        let ctx = ProcessCtx { sample_rate: self.sample_rate, gate: !self.released };

        let base = self.graph.node_slot_base();
        for (i, op) in self.ops.iter_mut().enumerate() {
            let inputs = &self.graph.nodes[i].inputs;
            let (before, after) = self.slots.split_at_mut(base + i);

            let mut ins: [&[f32]; MAX_INPUTS] = [&[]; MAX_INPUTS];
            for (k, &slot) in inputs.iter().enumerate() {
                ins[k] = &before[slot][..frames];
            }
            op.process(&ctx, &ins[..inputs.len()], &mut after[0][..frames]);
        }

        out.copy_from_slice(&self.slots[self.graph.output_slot][..frames]);
        if out.iter().any(|s| !s.is_finite()) {
            self.poisoned = true;
            out.fill(0.0);
        }
    }
}
