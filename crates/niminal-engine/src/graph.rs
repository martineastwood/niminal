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
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    UnknownPort { opcode: &'static str, port: String },
    DuplicatePort { opcode: &'static str, port: String },
    MissingPort { opcode: &'static str, port: &'static str },
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

    /// Add an opcode, wiring its ports by name. Ports left out take their
    /// default; a port with no default must be given.
    pub fn add(&mut self, op: impl Opcode + 'static, args: &[(&str, Src)]) -> Result<Src, GraphError> {
        let ports = op.ports();
        assert!(ports.len() <= MAX_INPUTS, "`{}` declares too many ports", op.name());

        let mut wired: Vec<Option<Src>> = vec![None; ports.len()];
        for &(name, src) in args {
            let Some(i) = ports.iter().position(|p| p.name == name) else {
                return Err(GraphError::UnknownPort { opcode: op.name(), port: name.to_string() });
            };
            if wired[i].is_some() {
                return Err(GraphError::DuplicatePort { opcode: op.name(), port: name.to_string() });
            }
            wired[i] = Some(src);
        }

        let mut inputs = Vec::with_capacity(ports.len());
        for (port, src) in ports.iter().zip(wired) {
            let src = match (src, port.default) {
                (Some(s), _) => s,
                (None, Some(d)) => Src::Const(d),
                (None, None) => {
                    return Err(GraphError::MissingPort { opcode: op.name(), port: port.name });
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
        }
    }

    pub fn build(mut self, output: Src) -> Graph {
        if matches!(output, Src::Const(_)) {
            self.slot_for(output); // make sure a constant output has a slot
        }
        let n_params = self.params.len();
        let n_consts = self.consts.len();
        let n_fixed = n_params + n_consts;
        let resolve = |slot: usize| {
            if slot >= NODE_BASE {
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

        let output_slot = match output {
            Src::Param(i) => i,
            Src::Const(v) => match self.consts.iter().position(|c| c.to_bits() == v.to_bits()) {
                Some(i) => n_params + i,
                None => unreachable!("constant interned above"),
            },
            Src::Node(i) => n_fixed + i,
        };

        Graph { params: self.params, consts: self.consts, nodes, output_slot }
    }
}

const CONST_BASE: usize = 1 << 20;
const NODE_BASE: usize = 1 << 21;

/// An immutable compiled graph, shared between voices.
pub struct Graph {
    pub(crate) params: Vec<ParamDef>,
    pub(crate) consts: Vec<f32>,
    pub(crate) nodes: Vec<NodeDef>,
    pub(crate) output_slot: usize,
}

impl Graph {
    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    pub fn param_names(&self) -> impl Iterator<Item = &str> {
        self.params.iter().map(|p| p.name.as_str())
    }

    pub(crate) fn node_slot_base(&self) -> usize {
        self.params.len() + self.consts.len()
    }
}
