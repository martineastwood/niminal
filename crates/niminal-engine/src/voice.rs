use std::sync::Arc;

use crate::graph::Graph;
use crate::mixer::Buses;
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
    /// The choke group this note belongs to, if any.
    group: Option<u32>,
    bus_map: Vec<Option<usize>>,
    /// Samples of fade-out left once the voice has been choked.
    choke_left: Option<usize>,
}

/// How long a choked voice takes to fade out, in seconds.
const CHOKE_FADE: f32 = 0.005;

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

        let bus_map = (0..graph.sends.iter().map(|s| s.0 + 1).max().unwrap_or(0)).map(Some).collect();
        Voice { graph, ops, slots, sample_rate, released: false, poisoned: false, group: None, bus_map, choke_left: None }
    }

    /// Keep memory at matching call sites while retaining the new graph's
    /// settings and wiring. Repeated identical nodes match in occurrence order.
    pub fn carry_state(&mut self, old: &mut Voice) {
        if self.sample_rate != old.sample_rate || old.poisoned {
            return;
        }
        let old_keys = old.graph.state_keys();
        let keys = self.graph.state_keys();
        for (new_index, (op, key)) in self.ops.iter_mut().zip(keys).enumerate() {
            let occurrence = keys[..new_index].iter().filter(|k| *k == key).count();
            if let Some(i) = old_keys.iter().enumerate().filter(|(_, k)| *k == key)
                .nth(occurrence).map(|(i, _)| i)
            {
                op.carry_state(&mut *old.ops[i]);
            }
        }
    }

    /// Update existing send mappings in place; the number of send identities
    /// belongs to the voice's original graph and never grows during playback.
    pub fn remap_buses(&mut self, mapping: &[Option<usize>]) {
        self.remap_buses_with(&mut |i| mapping.get(i).copied().flatten());
    }

    pub fn remap_buses_with(&mut self, mapping: &mut impl FnMut(usize) -> Option<usize>) {
        for bus in &mut self.bus_map {
            *bus = bus.and_then(&mut *mapping);
        }
    }

    /// Fill external input `index` for the coming block. Only the first
    /// `data.len()` samples are meaningful, so process no more than that.
    pub fn set_input(&mut self, index: usize, data: &[f32]) {
        let slot = self.graph.input_slot_base() + index;
        self.slots[slot][..data.len()].copy_from_slice(data);
    }

    pub fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        for op in &mut self.ops { op.set_control(key, start, target, frames, elapsed); }
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

    /// Put the note in a choke group: a later note in the same group ends it.
    pub fn set_group(&mut self, group: Option<u32>) {
        self.group = group;
    }

    pub fn group(&self) -> Option<u32> {
        self.group
    }

    /// End the note now, with a short fade so it doesn't click.
    pub fn choke(&mut self) {
        self.released = true;
        if self.choke_left.is_none() {
            self.choke_left = Some(((self.sample_rate * CHOKE_FADE) as usize).max(1));
        }
    }

    pub fn is_choked(&self) -> bool {
        self.choke_left.is_some()
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// A voice lives until it has been released and every envelope has
    /// finished, which may be well after the scheduled note length.
    pub fn is_finished(&self) -> bool {
        self.poisoned || self.choke_left == Some(0) || (self.released && self.choke_left.is_none() && !self.ops.iter().any(|op| op.is_active()))
    }

    /// True once the voice produced NaN or infinity. It is silenced from then
    /// on (its state can't be trusted) and counts as finished.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Number of output channels.
    pub fn channels(&self) -> usize {
        self.graph.channels()
    }

    /// Render `out.len()` samples (at most [`BLOCK`]) of a one-channel graph.
    /// Output is identical however a stretch of audio is divided into calls, so
    /// a host can split blocks at event times for sample-accurate timing.
    ///
    /// Anything the graph sends to a bus is dropped; use [`Voice::process_routed`]
    /// for graphs that send.
    pub fn process(&mut self, out: &mut [f32]) {
        self.process_routed(out, &mut Buses::new(&[]));
    }

    /// Like [`Voice::process`], adding the graph's sends into `buses`.
    pub fn process_routed(&mut self, out: &mut [f32], buses: &mut Buses) {
        assert_eq!(self.channels(), 1, "this graph has {} channels; use process_blocks", self.channels());
        let frames = out.len();
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        let mut block = [[0.0f32; BLOCK]; 1];
        self.process_blocks(&mut block, frames, buses);
        out.copy_from_slice(&block[0][..frames]);
    }

    /// Render `frames` samples (at most [`BLOCK`]) of every output channel into
    /// `out`, which must have one block per channel, and add the graph's sends
    /// to `buses`.
    pub fn process_blocks(&mut self, out: &mut [[f32; BLOCK]], frames: usize, buses: &mut Buses) {
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        assert_eq!(out.len(), self.channels(), "one output block per channel");
        if self.poisoned {
            out.iter_mut().for_each(|c| c[..frames].fill(0.0));
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

        let finite = |slot: usize| self.slots[slot][..frames].iter().all(|s| s.is_finite());
        let healthy = self.graph.output_slots.iter().all(|&s| finite(s))
            && self.graph.sends.iter().all(|&(_, _, s)| finite(s));
        if !healthy {
            self.poisoned = true;
            out.iter_mut().for_each(|c| c[..frames].fill(0.0));
            return;
        }
        // A choked voice fades linearly to nothing over `CHOKE_FADE`.
        let fade = self.choke_left.map(|left| {
            let total = ((self.sample_rate * CHOKE_FADE) as usize).max(1) as f32;
            let mut gains = [0.0f32; BLOCK];
            for (n, g) in gains[..frames].iter_mut().enumerate() {
                *g = (left.saturating_sub(n) as f32 / total).max(0.0);
            }
            self.choke_left = Some(left.saturating_sub(frames));
            gains
        });
        for (channel, &slot) in out.iter_mut().zip(&self.graph.output_slots) {
            channel[..frames].copy_from_slice(&self.slots[slot][..frames]);
            if let Some(g) = &fade {
                channel[..frames].iter_mut().zip(g).for_each(|(y, g)| *y *= g);
            }
        }
        for &(bus, channel, slot) in &self.graph.sends {
            let Some(bus) = self.bus_map.get(bus).copied().flatten() else { continue };
            match &fade {
                None => buses.add(bus, channel, &self.slots[slot][..frames]),
                Some(g) => {
                    let mut faded = [0.0f32; BLOCK];
                    faded[..frames].iter_mut().zip(&self.slots[slot]).zip(g).for_each(|((y, x), g)| *y = x * g);
                    buses.add(bus, channel, &faded[..frames]);
                }
            }
        }
    }
}
