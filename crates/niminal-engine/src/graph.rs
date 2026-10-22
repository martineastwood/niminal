use std::fmt;
use std::sync::atomic::AtomicU32;

use crate::opcode::Opcode;

/// Where a node input comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Src {
    Const(f32),
    /// A voice parameter, by index (see [`GraphBuilder::param`]).
    Param(usize),
    /// A result of another node. Nodes' results are numbered in order, so a node with
    /// several results takes several numbers.
    Node(usize),
    /// An external input filled by the host each block (see [`GraphBuilder::input`]).
    Input(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    UnknownPort { opcode: String, port: String },
    DuplicatePort { opcode: String, port: String },
    MissingPort { opcode: String, port: String },
    DuplicateParam(String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::UnknownPort { opcode, port } => {
                write!(f, "`{opcode}` has no argument named `{port}`")
            }
            GraphError::DuplicatePort { opcode, port } => {
                write!(f, "`{opcode}` argument `{port}` given more than once")
            }
            GraphError::MissingPort { opcode, port } => {
                write!(f, "`{opcode}` needs an argument `{port}`")
            }
            GraphError::DuplicateParam(name) => write!(f, "parameter `{name}` declared twice"),
        }
    }
}

impl std::error::Error for GraphError {}

pub(crate) struct NodeDef {
    pub(crate) op: Box<dyn Opcode>,
    /// Slot index feeding each port.
    pub(crate) inputs: Vec<usize>,
    /// The number of this node's first result, and how many it has.
    pub(crate) first: usize,
    pub(crate) outputs: usize,
}

pub(crate) struct ParamDef {
    pub(crate) name: String,
    pub(crate) default: f32,
    pub(crate) range: Option<(f32, f32)>,
}

/// Collects nodes in dependency order. A node can only read nodes added before
/// it, so insertion order is already a valid execution order.
#[derive(Default)]
pub struct GraphBuilder {
    params: Vec<ParamDef>,
    consts: Vec<f32>,
    nodes: Vec<NodeDef>,
    /// Results of all nodes so far.
    results: usize,
    latencies: Vec<usize>,
    n_inputs: usize,
    input_latencies: Vec<usize>,
    /// (bus, channel, encoded slot)
    sends: Vec<(usize, usize, usize, bool)>,
    /// Shared buffers this graph writes (for cross-node ordering).
    buffer_writes: Vec<usize>,
    /// Shared buffers this graph reads (for cross-node ordering).
    buffer_reads: Vec<usize>,
    tail: f32,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Wrap just the nodes built inside a lexical reset scope.
    pub fn reset_since(&mut self, first: usize, trigger: Src) {
        let slot = self.slot_for(trigger);
        for node in &mut self.nodes[first..] {
            let op = std::mem::replace(&mut node.op, Box::new(crate::ops::Add));
            node.op = Box::new(crate::ops::ResetOn::new(op));
            node.inputs.push(slot);
        }
    }

    /// Declare a named voice parameter with a default value.
    pub fn param(&mut self, name: &str, default: f32) -> Result<Src, GraphError> {
        if self.params.iter().any(|p| p.name == name) {
            return Err(GraphError::DuplicateParam(name.to_string()));
        }
        self.params.push(ParamDef {
            name: name.to_string(),
            default,
            range: None,
        });
        Ok(Src::Param(self.params.len() - 1))
    }

    /// How long a voice of this graph may ring after its release when nothing in it (an envelope or
    /// a sample) decides when it ends.
    pub fn set_param_range(&mut self, name: &str, range: Option<(f32, f32)>) {
        if let Some(param) = self.params.iter_mut().find(|p| p.name == name) {
            param.range = range;
        }
    }
    pub fn set_tail(&mut self, seconds: f32) {
        self.tail = seconds.max(0.0);
    }

    /// Declare an external input. The host fills it before each block, for
    /// example with a node's incoming audio or the contents of a bus.
    pub fn input(&mut self) -> Src {
        self.input_with_latency(0)
    }

    pub fn input_with_latency(&mut self, latency: usize) -> Src {
        self.n_inputs += 1;
        self.input_latencies.push(latency);
        Src::Input(self.n_inputs - 1)
    }

    /// Add `src` to channel 0 of a bus every block.
    pub fn send(&mut self, bus: usize, src: Src) {
        self.send_channel(bus, 0, src);
    }

    /// Add `src` to one channel of a bus every block.
    pub fn send_channel(&mut self, bus: usize, channel: usize, src: Src) {
        let slot = self.slot_for(src);
        self.sends.push((bus, channel, slot, false));
    }

    /// Like [`GraphBuilder::send_channel`], but what is sent reaches the bus one block later than
    /// it is made. That is how a loop through buses is broken: a delayed send is not something
    /// the bus's readers have to wait for.
    pub fn send_channel_delayed(&mut self, bus: usize, channel: usize, src: Src) {
        let slot = self.slot_for(src);
        self.sends.push((bus, channel, slot, true));
    }

    /// Note that this graph writes a shared buffer. Used so nodes that write a
    /// buffer run before nodes that read it in the same block.
    pub fn touch_buffer_write(&mut self, id: usize) {
        if !self.buffer_writes.contains(&id) {
            self.buffer_writes.push(id);
        }
    }

    /// Note that this graph reads a shared buffer.
    pub fn touch_buffer_read(&mut self, id: usize) {
        if !self.buffer_reads.contains(&id) {
            self.buffer_reads.push(id);
        }
    }

    /// Add an opcode, wiring its ports by name. Ports left out take their
    /// default; a port with no default must be given. For an opcode with several
    /// results, this is the first; see [`GraphBuilder::add_outputs`].
    pub fn add(
        &mut self,
        op: impl Opcode + 'static,
        args: &[(&str, Src)],
    ) -> Result<Src, GraphError> {
        Ok(self.add_outputs(op, args)?[0])
    }

    /// Like [`GraphBuilder::add`], giving every result of the opcode.
    pub fn add_outputs(
        &mut self,
        op: impl Opcode + 'static,
        args: &[(&str, Src)],
    ) -> Result<Vec<Src>, GraphError> {
        let ports = op.ports().to_vec();
        assert!(
            ports.len() <= op.max_inputs(),
            "`{}` declares too many ports for its processing adapter",
            op.name()
        );

        let mut wired: Vec<Option<Src>> = vec![None; ports.len()];
        for &(name, src) in args {
            let Some(i) = ports.iter().position(|p| p.name == name) else {
                return Err(GraphError::UnknownPort {
                    opcode: op.name().to_string(),
                    port: name.to_string(),
                });
            };
            if wired[i].is_some() {
                return Err(GraphError::DuplicatePort {
                    opcode: op.name().to_string(),
                    port: name.to_string(),
                });
            }
            wired[i] = Some(src);
        }

        let input_latency = wired
            .iter()
            .flatten()
            .map(|s| self.source_latency(*s))
            .max()
            .unwrap_or(0);
        let mut inputs = Vec::with_capacity(ports.len());
        for (port_index, (port, src)) in ports.iter().zip(wired).enumerate() {
            let src = match (src, port.default) {
                (Some(s), _) => s,
                (None, Some(d)) => Src::Const(d),
                (None, None) => {
                    return Err(GraphError::MissingPort {
                        opcode: op.name().to_string(),
                        port: port.name.to_string(),
                    });
                }
            };
            let src = self.align_source(src, input_latency, op.input_is_init(port_index));
            inputs.push(self.slot_for(src));
        }

        let outputs = op.outputs().max(1);
        let first = self.results;
        self.results += outputs;
        self.latencies
            .extend(std::iter::repeat_n(input_latency + op.latency(), outputs));
        self.nodes.push(NodeDef {
            op: Box::new(op),
            inputs,
            first,
            outputs,
        });
        Ok((first..first + outputs).map(Src::Node).collect())
    }

    fn source_latency(&self, source: Src) -> usize {
        match source {
            Src::Node(i) => self.latencies[i],
            Src::Input(i) => self.input_latencies[i],
            _ => 0,
        }
    }

    fn align_source(&mut self, source: Src, target: usize, init: bool) -> Src {
        let delay = target.saturating_sub(self.source_latency(source));
        if delay == 0 || matches!(source, Src::Const(_)) {
            return source;
        }
        let delay = if init || matches!(source, Src::Param(_)) {
            crate::ops::SampleDelay::warm(delay)
        } else {
            crate::ops::SampleDelay::new(delay)
        };
        self.add(delay, &[("x", source)]).expect("delay port")
    }

    // Slot layout, fixed once `build` knows the counts: params, then consts,
    // then node outputs. Until then node slots are encoded past `NODE_BASE`.
    fn slot_for(&mut self, src: Src) -> usize {
        match src {
            Src::Param(i) => i,
            Src::Const(v) => {
                let i = match self.consts.iter().position(|c| c.to_bits() == v.to_bits()) {
                    Some(i) => i,
                    None => {
                        self.consts.push(v);
                        self.consts.len() - 1
                    }
                };
                CONST_BASE + i
            }
            Src::Node(i) => {
                assert!(i < self.results, "DSP result {i} does not exist yet");
                NODE_BASE + i
            }
            Src::Input(i) => {
                assert!(i < self.n_inputs, "input {i} was not declared");
                INPUT_BASE + i
            }
        }
    }

    /// Finish a graph with a single output channel.
    pub fn build(self, output: Src) -> Graph {
        self.build_channels(&[output])
    }

    /// Finish a graph with one output per channel.
    pub fn build_channels(mut self, outputs: &[Src]) -> Graph {
        assert!(
            !outputs.is_empty(),
            "a graph needs at least one output channel"
        );
        let source = |slot: usize| {
            if slot >= INPUT_BASE {
                Src::Input(slot - INPUT_BASE)
            } else if slot >= NODE_BASE {
                Src::Node(slot - NODE_BASE)
            } else if slot >= CONST_BASE {
                Src::Const(self.consts[slot - CONST_BASE])
            } else {
                Src::Param(slot)
            }
        };
        let sends: Vec<_> = self
            .sends
            .iter()
            .map(|&(b, c, s, d)| (b, c, source(s), d))
            .collect();
        let latency = outputs
            .iter()
            .copied()
            .chain(sends.iter().filter(|s| !s.3).map(|s| s.2))
            .map(|o| self.source_latency(o))
            .max()
            .unwrap_or(0);
        self.sends.clear();
        for (b, c, s, d) in sends {
            let s = if d {
                s
            } else {
                self.align_source(s, latency, false)
            };
            if d {
                self.send_channel_delayed(b, c, s);
            } else {
                self.send_channel(b, c, s);
            }
        }
        let outputs: Vec<_> = outputs
            .iter()
            .map(|&o| self.align_source(o, latency, false))
            .collect();
        let output_enc: Vec<usize> = outputs.iter().map(|&o| self.slot_for(o)).collect();
        // Nodes are topologically ordered, so one backwards pass retains
        // exactly the dependencies of outputs, sends and observable effects.
        let mut used = vec![false; self.results];
        let mut constants = vec![false; self.consts.len()];
        let mark = |slot: usize, used: &mut [bool], constants: &mut [bool]| {
            if (NODE_BASE..INPUT_BASE).contains(&slot) {
                used[slot - NODE_BASE] = true;
            } else if (CONST_BASE..NODE_BASE).contains(&slot) {
                constants[slot - CONST_BASE] = true;
            }
        };
        for &slot in &output_enc {
            mark(slot, &mut used, &mut constants);
        }
        for &(_, _, slot, _) in &self.sends {
            mark(slot, &mut used, &mut constants);
        }
        let mut keep = vec![false; self.nodes.len()];
        for (i, node) in self.nodes.iter().enumerate().rev() {
            if node.op.has_side_effects()
                || used[node.first..node.first + node.outputs]
                    .iter()
                    .any(|v| *v)
            {
                keep[i] = true;
                for &slot in &node.inputs {
                    mark(slot, &mut used, &mut constants);
                }
            }
        }
        let mut result_map = vec![usize::MAX; self.results];
        let mut results = 0;
        self.nodes = self
            .nodes
            .into_iter()
            .zip(keep)
            .filter_map(|(mut node, keep)| {
                if !keep {
                    return None;
                }
                for k in 0..node.outputs {
                    result_map[node.first + k] = results + k;
                }
                node.first = results;
                results += node.outputs;
                Some(node)
            })
            .collect();
        self.results = results;
        let mut constant_map = vec![usize::MAX; self.consts.len()];
        let mut count = 0;
        self.consts = self
            .consts
            .into_iter()
            .enumerate()
            .filter_map(|(i, value)| {
                if !constants[i] {
                    return None;
                }
                constant_map[i] = count;
                count += 1;
                Some(value)
            })
            .collect();
        let n_params = self.params.len();
        let n_consts = self.consts.len();
        let n_inputs = self.n_inputs;
        let n_fixed = n_params + n_consts + n_inputs;
        let resolve = |slot: usize| {
            if slot >= INPUT_BASE {
                n_params + n_consts + (slot - INPUT_BASE)
            } else if slot >= NODE_BASE {
                n_fixed + result_map[slot - NODE_BASE]
            } else if slot >= CONST_BASE {
                n_params + constant_map[slot - CONST_BASE]
            } else {
                slot
            }
        };

        let nodes = self
            .nodes
            .into_iter()
            .map(|mut n| {
                for s in &mut n.inputs {
                    *s = resolve(*s);
                }
                n
            })
            .collect();
        let sends = self
            .sends
            .iter()
            .map(|&(bus, channel, slot, delayed)| (bus, channel, resolve(slot), delayed))
            .collect();

        let mut buffer_writes = self.buffer_writes;
        buffer_writes.sort_unstable();
        buffer_writes.dedup();
        let mut buffer_reads = self.buffer_reads;
        buffer_reads.sort_unstable();
        buffer_reads.dedup();
        let mut graph = Graph {
            latency,
            state_keys: Vec::new(),
            params: self.params,
            consts: self.consts,
            results: self.results,
            tail: self.tail,
            nodes,
            n_inputs,
            sends,
            buffer_writes,
            buffer_reads,
            output_slots: output_enc.into_iter().map(resolve).collect(),
            notes_started: AtomicU32::new(0),
        };
        graph.state_keys = graph.compute_state_keys();
        graph
    }
}

const CONST_BASE: usize = 1 << 20;
const NODE_BASE: usize = 1 << 21;
const INPUT_BASE: usize = 1 << 22;

/// One node of a compiled graph, as `explain` shows it.
pub struct NodeReport {
    pub name: String,
    /// What its kernel runs in each section, if it has one.
    pub stats: Option<crate::KernelStats>,
    /// Nothing reads it as signal: it only sets parameters of other nodes.
    pub modulation_only: bool,
    /// Each input's value if it is a constant.
    pub inputs: Vec<Option<f32>>,
    /// For a `control` scope: what its timing gets wrong at this sample rate.
    pub warnings: Vec<String>,
}

/// An immutable compiled graph, shared between voices.
pub struct Graph {
    latency: usize,
    state_keys: Vec<u64>,
    pub(crate) params: Vec<ParamDef>,
    pub(crate) consts: Vec<f32>,
    pub(crate) nodes: Vec<NodeDef>,
    /// How many results the nodes have between them.
    pub(crate) results: usize,
    /// Seconds a voice without an envelope rings after its release.
    pub(crate) tail: f32,
    pub(crate) n_inputs: usize,
    /// (bus, channel, slot) triples added to buses after every block.
    /// Bus, channel, the slot sent, and whether it arrives a block late.
    pub(crate) sends: Vec<(usize, usize, usize, bool)>,
    pub(crate) buffer_writes: Vec<usize>,
    pub(crate) buffer_reads: Vec<usize>,
    pub(crate) output_slots: Vec<usize>,
    /// Notes of this graph that have started, which numbers the next one (`note.index`).
    pub(crate) notes_started: AtomicU32,
}

impl Graph {
    /// Reuse a compiled fx after the surrounding bus registry changes.
    /// Fx controls become ordinary live parameters, sharing note modulation machinery.
    pub fn expose_controls(&self, keys: &[(&str, &str)]) -> Graph {
        let mut g = GraphBuilder::new();
        let mut slots = Vec::new();
        for p in &self.params {
            let name = keys
                .iter()
                .find(|(old, _)| *old == p.name)
                .map_or(p.name.as_str(), |(_, new)| *new);
            slots.push(g.param(name, p.default).unwrap());
        }
        slots.extend(self.consts.iter().map(|v| Src::Const(*v)));
        for _ in 0..self.n_inputs {
            slots.push(g.input());
        }
        for node in &self.nodes {
            if let Some((key, value)) = node
                .op
                .control_info()
                .filter(|(key, _)| keys.iter().any(|(old, _)| old == key))
            {
                let public = keys.iter().find(|(old, _)| *old == key).unwrap().1;
                let src = g
                    .params
                    .iter()
                    .position(|p| p.name == public)
                    .map(Src::Param)
                    .unwrap_or_else(|| g.param(public, value).unwrap());
                slots.push(src);
            } else {
                let ports: Vec<_> = node
                    .op
                    .ports()
                    .iter()
                    .zip(&node.inputs)
                    .map(|(p, s)| (p.name.as_ref(), slots[*s]))
                    .collect();
                slots.extend(
                    g.add_outputs(node.op.box_clone(), &ports)
                        .expect("valid graph"),
                );
            }
        }
        for &(bus, channel, slot, late) in &self.sends {
            if late {
                g.send_channel_delayed(bus, channel, slots[slot]);
            } else {
                g.send_channel(bus, channel, slots[slot]);
            }
        }
        for &id in &self.buffer_reads {
            g.touch_buffer_read(id);
        }
        for &id in &self.buffer_writes {
            g.touch_buffer_write(id);
        }
        g.set_tail(self.tail);
        g.build_channels(
            &self
                .output_slots
                .iter()
                .map(|s| slots[*s])
                .collect::<Vec<_>>(),
        )
    }
    pub fn set_param_range(&mut self, name: &str, range: Option<(f32, f32)>) {
        if let Some(param) = self.params.iter_mut().find(|p| p.name == name) {
            param.range = range;
        }
    }
    pub fn buffer_channels(&self, key: u64) -> usize {
        self.nodes
            .iter()
            .map(|n| n.op.buffer_channels(key))
            .max()
            .unwrap_or(0)
    }
    pub fn stream_cache(&self) -> Option<&std::sync::Arc<crate::ops::StreamCache>> {
        self.nodes.iter().find_map(|n| n.op.stream_cache())
    }
    pub fn buffer_bindings(&self, key: u64) -> usize {
        self.nodes.iter().map(|n| n.op.buffer_bindings(key)).sum()
    }
    pub fn reports(&self, visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {
        for node in &self.nodes {
            node.op.reports(visit);
        }
    }
    pub fn with_bus_mapping(&self, mapping: &[Option<usize>]) -> Graph {
        let mut graph = self.with_input_latencies(&vec![0; self.input_count()]);
        graph.sends.retain_mut(|(bus, _, _, _)| {
            if let Some(next) = mapping.get(*bus).copied().flatten() {
                *bus = next;
                true
            } else {
                false
            }
        });
        graph
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Rebuild with the known timing of external inputs, aligning them before processing.
    pub fn with_input_latencies(&self, latencies: &[usize]) -> Graph {
        assert_eq!(latencies.len(), self.n_inputs);
        let mut g = GraphBuilder::new();
        let mut slots = Vec::new();
        for p in &self.params {
            slots.push(g.param(&p.name, p.default).unwrap());
            g.params.last_mut().unwrap().range = p.range;
        }
        slots.extend(self.consts.iter().map(|&v| Src::Const(v)));
        let target = latencies.iter().copied().max().unwrap_or(0);
        for &latency in latencies {
            let input = g.input_with_latency(latency);
            slots.push(g.align_source(input, target, false));
        }
        for node in &self.nodes {
            let ports: Vec<_> = node
                .op
                .ports()
                .iter()
                .zip(&node.inputs)
                .map(|(p, &s)| (p.name.as_ref(), slots[s]))
                .collect();
            slots.extend(
                g.add_outputs(node.op.box_clone(), &ports)
                    .expect("valid graph ports"),
            );
        }
        for &(bus, channel, slot, late) in &self.sends {
            if late {
                g.send_channel_delayed(bus, channel, slots[slot]);
            } else {
                g.send_channel(bus, channel, slots[slot]);
            }
        }
        for &id in &self.buffer_reads {
            g.touch_buffer_read(id);
        }
        for &id in &self.buffer_writes {
            g.touch_buffer_write(id);
        }
        g.set_tail(self.tail);
        g.build_channels(
            &self
                .output_slots
                .iter()
                .map(|&s| slots[s])
                .collect::<Vec<_>>(),
        )
    }

    fn delay_slot(&mut self, slot: usize, frames: usize) -> usize {
        if frames == 0 {
            return slot;
        }
        let first = self.results;
        self.results += 1;
        self.nodes.push(NodeDef {
            op: Box::new(crate::ops::SampleDelay::new(frames)),
            inputs: vec![slot],
            first,
            outputs: 1,
        });
        self.node_slot_base() + first
    }

    pub fn pad_outputs(&mut self, latency: usize) {
        let delay = latency.saturating_sub(self.latency);
        for i in 0..self.output_slots.len() {
            self.output_slots[i] = self.delay_slot(self.output_slots[i], delay);
        }
        self.latency = self.latency.max(latency);
        self.state_keys = self.compute_state_keys();
    }

    pub fn pad_sends(&mut self, bus_latencies: &[usize]) {
        for i in 0..self.sends.len() {
            let (bus, _, slot, late) = self.sends[i];
            if !late {
                self.sends[i].2 =
                    self.delay_slot(slot, bus_latencies[bus].saturating_sub(self.latency));
            }
        }
        self.state_keys = self.compute_state_keys();
    }

    /// Structural node identities for live state transfer. Constants are deliberately
    /// omitted: changing a cutoff or gain must not erase filter or delay memory.
    fn compute_state_keys(&self) -> Vec<u64> {
        use std::hash::{Hash, Hasher};
        let mut keys: Vec<u64> = Vec::with_capacity(self.nodes.len());
        let base = self.node_slot_base();
        // which node, and which of its results, each result slot is
        let mut owner = Vec::with_capacity(self.results);
        for (n, node) in self.nodes.iter().enumerate() {
            owner.extend((0..node.outputs).map(|k| (n, k)));
        }
        for node in &self.nodes {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            node.op.name().hash(&mut h);
            for &slot in &node.inputs {
                if slot < self.params.len() {
                    0u8.hash(&mut h);
                    self.params[slot].name.hash(&mut h);
                } else if slot < self.input_slot_base() {
                    1u8.hash(&mut h);
                } else if slot < base {
                    2u8.hash(&mut h);
                    (slot - self.input_slot_base()).hash(&mut h);
                } else {
                    3u8.hash(&mut h);
                    let (n, k) = owner[slot - base];
                    keys[n].hash(&mut h);
                    k.hash(&mut h);
                }
            }
            keys.push(h.finish());
        }
        keys
    }

    pub(crate) fn state_keys(&self) -> &[u64] {
        &self.state_keys
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    pub fn param_names(&self) -> impl Iterator<Item = &str> {
        self.params.iter().map(|p| p.name.as_str())
    }

    /// Number of DSP nodes that contribute audio or observable effects.
    /// What each node is, in execution order, for `explain`.
    pub fn describe(&self, sample_rate: f32) -> Vec<NodeReport> {
        let (n_params, n_consts) = (self.params.len(), self.consts.len());
        let n_fixed = n_params + n_consts + self.n_inputs;
        let const_value = |slot: usize| {
            (n_params..n_params + n_consts)
                .contains(&slot)
                .then(|| self.consts[slot - n_params])
        };
        let mut owner = vec![usize::MAX; self.results];
        for (i, node) in self.nodes.iter().enumerate() {
            owner[node.first..node.first + node.outputs].fill(i);
        }
        let producer = |slot: usize| {
            slot.checked_sub(n_fixed)
                .and_then(|r| owner.get(r).copied())
                .filter(|&i| i != usize::MAX)
        };
        // Which nodes reach the output (or a send) through a signal path, rather than only
        // setting a parameter of another node.
        let mut signal = vec![false; self.nodes.len()];
        let sends = self.sends.iter().map(|s| s.2);
        for slot in self.output_slots.iter().copied().chain(sends) {
            if let Some(i) = producer(slot) {
                signal[i] = true;
            }
        }
        for (i, node) in self.nodes.iter().enumerate().rev() {
            let ports = node.op.ports();
            // arithmetic passes every input on; anything else passes its audio inputs
            let arithmetic = ports
                .iter()
                .all(|p| matches!(&*p.name, "x" | "a" | "b" | "cond" | "idx"));
            let effects = node.op.has_side_effects();
            if !signal[i] && !effects {
                continue;
            }
            for (k, &slot) in node.inputs.iter().enumerate() {
                let passes = arithmetic
                    || effects
                    || ports.get(k).is_none_or(|p| {
                        matches!(&*p.name, "x" | "chain" | "sidechain")
                    });
                if passes && let Some(j) = producer(slot) {
                    signal[j] = true;
                }
            }
        }
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| NodeReport {
                name: node.op.name().to_string(),
                stats: node.op.kernel_stats(),
                modulation_only: !signal[i],
                warnings: node.op.control_warnings(sample_rate),
                inputs: node
                    .inputs
                    .iter()
                    .map(|&slot| const_value(slot))
                    .collect(),
            })
            .collect()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of output channels.
    pub fn channels(&self) -> usize {
        self.output_slots.len()
    }

    pub fn input_count(&self) -> usize {
        self.n_inputs
    }

    /// The buses this graph sends to, once each, in ascending order, that must be written before
    /// they are read. Delayed sends are left out: they do not constrain the order.
    pub fn send_buses(&self) -> Vec<usize> {
        let mut buses: Vec<usize> = self
            .sends
            .iter()
            .filter(|s| !s.3)
            .map(|&(b, _, _, _)| b)
            .collect();
        buses.sort_unstable();
        buses.dedup();
        buses
    }

    /// Shared buffers this graph writes, for cross-node ordering.
    pub fn buffer_writes(&self) -> &[usize] {
        &self.buffer_writes
    }

    /// Shared buffers this graph reads, for cross-node ordering.
    pub fn buffer_reads(&self) -> &[usize] {
        &self.buffer_reads
    }

    pub(crate) fn input_slot_base(&self) -> usize {
        self.params.len() + self.consts.len()
    }

    pub(crate) fn node_slot_base(&self) -> usize {
        self.input_slot_base() + self.n_inputs
    }
}
