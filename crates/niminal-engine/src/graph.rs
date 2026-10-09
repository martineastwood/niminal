use std::fmt;

use crate::opcode::{MAX_INPUTS, Opcode};

/// Where a node input comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Src {
    Const(f32),
    /// A voice parameter, by index (see [`GraphBuilder::param`]).
    Param(usize),
    /// The output of another node.
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
}

pub(crate) struct ParamDef {
    pub(crate) name: String,
    pub(crate) default: f32,
}

/// Collects nodes in dependency order. A node can only read nodes added before
/// it, so insertion order is already a valid execution order.
#[derive(Default)]
pub struct GraphBuilder {
    params: Vec<ParamDef>,
    consts: Vec<f32>,
    nodes: Vec<NodeDef>,
    n_inputs: usize,
    /// (bus, channel, encoded slot)
    sends: Vec<(usize, usize, usize)>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a named voice parameter with a default value.
    pub fn param(&mut self, name: &str, default: f32) -> Result<Src, GraphError> {
        if self.params.iter().any(|p| p.name == name) {
            return Err(GraphError::DuplicateParam(name.to_string()));
        }
        self.params.push(ParamDef { name: name.to_string(), default });
        Ok(Src::Param(self.params.len() - 1))
    }

    /// Declare an external input. The host fills it before each block, for
    /// example with a track's incoming audio or the contents of a bus.
    pub fn input(&mut self) -> Src {
        self.n_inputs += 1;
        Src::Input(self.n_inputs - 1)
    }

    /// Add `src` to channel 0 of a bus every block.
    pub fn send(&mut self, bus: usize, src: Src) {
        self.send_channel(bus, 0, src);
    }

    /// Add `src` to one channel of a bus every block.
    pub fn send_channel(&mut self, bus: usize, channel: usize, src: Src) {
        let slot = self.slot_for(src);
        self.sends.push((bus, channel, slot));
    }

    /// Add an opcode, wiring its ports by name. Ports left out take their
    /// default; a port with no default must be given.
    pub fn add(&mut self, op: impl Opcode + 'static, args: &[(&str, Src)]) -> Result<Src, GraphError> {
        let ports = op.ports().to_vec();
        assert!(ports.len() <= MAX_INPUTS, "`{}` declares too many ports", op.name());

        let mut wired: Vec<Option<Src>> = vec![None; ports.len()];
        for &(name, src) in args {
            let Some(i) = ports.iter().position(|p| p.name == name) else {
                return Err(GraphError::UnknownPort { opcode: op.name().to_string(), port: name.to_string() });
            };
            if wired[i].is_some() {
                return Err(GraphError::DuplicatePort { opcode: op.name().to_string(), port: name.to_string() });
            }
            wired[i] = Some(src);
        }

        let mut inputs = Vec::with_capacity(ports.len());
        for (port, src) in ports.iter().zip(wired) {
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
            inputs.push(self.slot_for(src));
        }

        self.nodes.push(NodeDef { op: Box::new(op), inputs });
        Ok(Src::Node(self.nodes.len() - 1))
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
                assert!(i < self.nodes.len(), "node {i} does not exist yet");
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
        assert!(!outputs.is_empty(), "a graph needs at least one output channel");
        // interns constant outputs
        let output_enc: Vec<usize> = outputs.iter().map(|&o| self.slot_for(o)).collect();
        let n_params = self.params.len();
        let n_consts = self.consts.len();
        let n_inputs = self.n_inputs;
        let n_fixed = n_params + n_consts + n_inputs;
        let resolve = |slot: usize| {
            if slot >= INPUT_BASE {
                n_params + n_consts + (slot - INPUT_BASE)
            } else if slot >= NODE_BASE {
                n_fixed + (slot - NODE_BASE)
            } else if slot >= CONST_BASE {
                n_params + (slot - CONST_BASE)
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
        let sends = self.sends.iter().map(|&(bus, channel, slot)| (bus, channel, resolve(slot))).collect();

        Graph {
            params: self.params,
            consts: self.consts,
            nodes,
            n_inputs,
            sends,
            output_slots: output_enc.into_iter().map(resolve).collect(),
        }
    }
}

const CONST_BASE: usize = 1 << 20;
const NODE_BASE: usize = 1 << 21;
const INPUT_BASE: usize = 1 << 22;

/// An immutable compiled graph, shared between voices.
pub struct Graph {
    pub(crate) params: Vec<ParamDef>,
    pub(crate) consts: Vec<f32>,
    pub(crate) nodes: Vec<NodeDef>,
    pub(crate) n_inputs: usize,
    /// (bus, channel, slot) triples added to buses after every block.
    pub(crate) sends: Vec<(usize, usize, usize)>,
    pub(crate) output_slots: Vec<usize>,
}

impl Graph {
    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    pub fn param_names(&self) -> impl Iterator<Item = &str> {
        self.params.iter().map(|p| p.name.as_str())
    }

    /// Number of output channels.
    pub fn channels(&self) -> usize {
        self.output_slots.len()
    }

    pub fn input_count(&self) -> usize {
        self.n_inputs
    }

    /// The buses this graph sends to, once each, in ascending order.
    pub fn send_buses(&self) -> Vec<usize> {
        let mut buses: Vec<usize> = self.sends.iter().map(|&(b, _, _)| b).collect();
        buses.sort_unstable();
        buses.dedup();
        buses
    }

    pub(crate) fn input_slot_base(&self) -> usize {
        self.params.len() + self.consts.len()
    }

    pub(crate) fn node_slot_base(&self) -> usize {
        self.input_slot_base() + self.n_inputs
    }
}
