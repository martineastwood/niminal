use std::borrow::Cow;

/// The level (-90db) below which a fading tail counts as finished.
pub const TAIL_LEVEL: f32 = 3.162_277_7e-5;

/// Samples per processing block. Control changes and bus sends are
/// block-aligned; stateful opcodes still run per sample inside a block.
pub const BLOCK: usize = 32;

/// Stack input capacity for scalar opcodes. Array opcodes read graph slots directly.
pub const MAX_INPUTS: usize = 16;

/// A named input of an opcode. `default: None` means the port must be wired.
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub name: Cow<'static, str>,
    pub default: Option<f32>,
}

impl Port {
    pub const fn required(name: &'static str) -> Self {
        Port { name: Cow::Borrowed(name), default: None }
    }

    pub const fn optional(name: &'static str, default: f32) -> Self {
        Port { name: Cow::Borrowed(name), default: Some(default) }
    }

    /// For opcodes whose ports are only known at run time (user-defined ones).
    pub fn named(name: impl Into<String>, default: Option<f32>) -> Self {
        Port { name: Cow::Owned(name.into()), default }
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
pub trait Opcode: Send + Sync + std::any::Any {
    fn name(&self) -> &str;

    /// Input ports, in the order `process` receives them.
    fn ports(&self) -> &[Port];

    fn box_clone(&self) -> Box<dyn Opcode>;

    /// Called once per voice before the first block.
    fn prepare(&mut self, _sample_rate: f32) {}

    /// Restart runtime state in place, without allocating.
    fn reset(&mut self) {}

    /// Fill `out` from `inputs`. `inputs[i]` belongs to `ports()[i]`, and every
    /// slice has the same length as `out` (at most [`BLOCK`]).
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]);

    /// How many results the opcode delivers. Most deliver one.
    fn outputs(&self) -> usize {
        1
    }

    /// Fill the first `frames` samples of one block per result. Only opcodes with several results
    /// need this.
    fn process_outputs(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], outs: &mut [[f32; BLOCK]], frames: usize) {
        self.process(ctx, inputs, &mut outs[0][..frames]);
    }

    /// Input capacity of the processing adapter. Array-aware opcodes override
    /// this alongside `process_slots`, so invalid graphs fail before playback.
    fn max_inputs(&self) -> usize { MAX_INPUTS }

    /// Process directly from graph slots. Array inputs can have any fixed
    /// length; scalar built-ins use the small stack adapter below.
    fn process_slots(&mut self, ctx: &ProcessCtx, slots: &[[f32; BLOCK]], indices: &[usize], outs: &mut [[f32; BLOCK]], frames: usize) {
        assert!(indices.len() <= MAX_INPUTS, "scalar opcode has too many ports");
        let mut inputs: [&[f32]; MAX_INPUTS] = [&[]; MAX_INPUTS];
        for (input, &index) in inputs.iter_mut().zip(indices) {
            *input = &slots[index][..frames];
        }
        self.process_outputs(ctx, &inputs[..indices.len()], outs, frames);
    }

    /// True when processing has an observable effect beyond its output, such
    /// as an OSC tap. These nodes remain live even without an audio consumer.
    fn has_side_effects(&self) -> bool { false }

    /// True while this opcode still has sound to produce after the note ends
    /// (an envelope that has not finished, or a delay still ringing).
    fn is_active(&self) -> bool {
        false
    }

    /// True for an envelope with a release, which decides when its voice ends.
    fn is_envelope(&self) -> bool {
        false
    }

    /// Envelope activity separately from effect tails in compound processors.
    fn has_active_envelope(&self) -> bool { self.is_envelope() && self.is_active() }

    /// Carry compatible runtime state into a newly compiled instance. Settings
    /// and wiring belong to the new instance; implementations only copy memory.
    fn carry_state(&mut self, _old: &mut dyn Opcode) {}

    /// Apply a named control target, with a linear ramp measured in samples.
    fn set_control(&mut self, _key: &str, _start: f32, _target: f32, _frames: u64, _elapsed: u64) {}

    /// Samples of delay the opcode introduces, for latency compensation.
    fn latency(&self) -> usize {
        0
    }
}

/// Lets a boxed opcode be added to a graph like any other.
impl Opcode for Box<dyn Opcode> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn ports(&self) -> &[Port] {
        (**self).ports()
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        (**self).box_clone()
    }

    fn prepare(&mut self, sample_rate: f32) {
        (**self).prepare(sample_rate);
    }

    fn reset(&mut self) { (**self).reset(); }

    fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        (**self).set_control(key, start, target, frames, elapsed);
    }

    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        (**self).process(ctx, inputs, out);
    }

    fn outputs(&self) -> usize {
        (**self).outputs()
    }

    fn process_outputs(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], outs: &mut [[f32; BLOCK]], frames: usize) {
        (**self).process_outputs(ctx, inputs, outs, frames);
    }

    fn max_inputs(&self) -> usize { (**self).max_inputs() }

    fn process_slots(&mut self, ctx: &ProcessCtx, slots: &[[f32; BLOCK]], indices: &[usize], outs: &mut [[f32; BLOCK]], frames: usize) {
        (**self).process_slots(ctx, slots, indices, outs, frames);
    }

    fn has_side_effects(&self) -> bool { (**self).has_side_effects() }

    fn is_active(&self) -> bool {
        (**self).is_active()
    }

    fn is_envelope(&self) -> bool {
        (**self).is_envelope()
    }

    fn has_active_envelope(&self) -> bool { (**self).has_active_envelope() }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        (**self).carry_state(old);
    }

    fn latency(&self) -> usize {
        (**self).latency()
    }
}
