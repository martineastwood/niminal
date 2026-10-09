/// Samples per processing block. Control changes and bus sends are
/// block-aligned; stateful opcodes still run per sample inside a block.
pub const BLOCK: usize = 32;

/// The most input ports an opcode may declare.
pub const MAX_INPUTS: usize = 8;

/// A named input of an opcode. `default: None` means the port must be wired.
#[derive(Debug, Clone, Copy)]
pub struct Port {
    pub name: &'static str,
    pub default: Option<f32>,
}

impl Port {
    pub const fn required(name: &'static str) -> Self {
        Port { name, default: None }
    }

    pub const fn optional(name: &'static str, default: f32) -> Self {
        Port { name, default: Some(default) }
    }
}

/// What an opcode needs to know about the block being processed.
#[derive(Debug, Clone, Copy)]
pub struct ProcessCtx {
    pub sample_rate: f32,
    /// True while the note is held; false once it has been released.
    pub gate: bool,
}

/// A unit generator. Built-in opcodes, niminal-defined opcodes and plugins all
/// implement this trait, so they look identical to the graph.
///
/// A graph holds one template of each opcode; every voice gets its own copy via
/// [`Opcode::box_clone`], which must return a freshly initialised instance.
pub trait Opcode: Send + Sync {
    fn name(&self) -> &'static str;

    /// Input ports, in the order `process` receives them.
    fn ports(&self) -> &'static [Port];

    fn box_clone(&self) -> Box<dyn Opcode>;

    /// Called once per voice before the first block.
    fn prepare(&mut self, _sample_rate: f32) {}

    /// Fill `out` from `inputs`. `inputs[i]` belongs to `ports()[i]`, and every
    /// slice has the same length as `out` (at most [`BLOCK`]).
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]);

    /// True while this opcode still has sound to produce after the note ends
    /// (an envelope that has not finished). A voice lives until none are active.
    fn is_active(&self) -> bool {
        false
    }

    /// Samples of delay the opcode introduces, for latency compensation.
    fn latency(&self) -> usize {
        0
    }
}
