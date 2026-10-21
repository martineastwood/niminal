use std::borrow::Cow;

/// The level (-90db) below which a fading tail counts as finished.
pub const TAIL_LEVEL: f32 = 3.162_277_7e-5;

/// Samples per processing block unless the server is started with another size.
pub const DEFAULT_BLOCK: usize = 64;
pub const MIN_BLOCK: usize = 16;
pub const MAX_BLOCK: usize = 1024;

static BLOCK_SIZE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Samples per processing block. Control changes and bus sends are
/// block-aligned; stateful opcodes still run per sample inside a block.
/// Fixed for the life of the process: it is whatever [`set_block_size`] chose
/// before the first call here, else [`DEFAULT_BLOCK`].
pub fn block_size() -> usize {
    use std::sync::atomic::Ordering::Relaxed;
    match BLOCK_SIZE.load(Relaxed) {
        0 => match BLOCK_SIZE.compare_exchange(0, DEFAULT_BLOCK, Relaxed, Relaxed) {
            Ok(_) => DEFAULT_BLOCK,
            Err(set) => set,
        },
        n => n,
    }
}

/// Choose the block size. It must be a power of two between [`MIN_BLOCK`] and
/// [`MAX_BLOCK`], and must happen before anything is built, since buffers are
/// sized from it.
pub fn set_block_size(n: usize) -> Result<(), String> {
    use std::sync::atomic::Ordering::Relaxed;
    if !n.is_power_of_two() || !(MIN_BLOCK..=MAX_BLOCK).contains(&n) {
        return Err(format!(
            "block size must be a power of two from {MIN_BLOCK} to {MAX_BLOCK}, not {n}"
        ));
    }
    match BLOCK_SIZE.compare_exchange(0, n, Relaxed, Relaxed) {
        Ok(_) => Ok(()),
        Err(set) if set == n => Ok(()),
        Err(set) => Err(format!("the block size is already {set}")),
    }
}

/// `count` silent blocks.
pub fn blocks(count: usize) -> Vec<Vec<f32>> {
    vec![vec![0.0; block_size()]; count]
}

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
        Port {
            name: Cow::Borrowed(name),
            default: None,
        }
    }

    pub const fn optional(name: &'static str, default: f32) -> Self {
        Port {
            name: Cow::Borrowed(name),
            default: Some(default),
        }
    }

    /// For opcodes whose ports are only known at run time (user-defined ones).
    pub fn named(name: impl Into<String>, default: Option<f32>) -> Self {
        Port {
            name: Cow::Owned(name.into()),
            default,
        }
    }
}

/// Where the session's clock is at the start of a block.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Clock {
    /// Absolute sample at the start of this block.
    pub sample: u64,
}

/// What an opcode needs to know about the block being processed.
#[derive(Debug, Clone, Copy)]
pub struct ProcessCtx {
    pub realtime: bool,
    pub note_token: u64,
    pub instance: u64,
    pub sample_rate: f32,
    /// True while the note is held; false once it has been released.
    pub gate: bool,
    /// The scheduled length of the note in seconds; 0 when a key holds it open.
    pub dur: f32,
    /// How many notes of this instrument started before this one.
    pub index: u32,
    pub clock: Clock,
}

impl ProcessCtx {
    pub fn new(sample_rate: f32, gate: bool) -> Self {
        ProcessCtx {
            realtime: false,
            note_token: 0,
            instance: 0,
            sample_rate,
            gate,
            dur: 0.0,
            index: 0,
            clock: Clock::default(),
        }
    }
}

/// A unit generator. Built-in opcodes, niminal-defined opcodes and plugins all
/// implement this trait, so they look identical to the graph.
///
/// A graph holds one template of each opcode; every voice gets its own copy via
/// [`Opcode::box_clone`], which must return a freshly initialised instance.
pub trait Opcode: Send + Sync + std::any::Any {
    fn name(&self) -> &str;
    fn control_info(&self) -> Option<(&str, f32)> {
        None
    }
    fn buffer_bindings(&self, _key: u64) -> usize {
        0
    }
    fn buffer_channels(&self, _key: u64) -> usize {
        0
    }
    fn stream_cache(&self) -> Option<&std::sync::Arc<crate::ops::StreamCache>> {
        None
    }
    fn swap_buffers(&mut self, _key: u64, _banks: &mut [crate::BufferBank], _cursor: &mut usize) {}
    fn reports(&self, _visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {}

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
    fn process_outputs(
        &mut self,
        ctx: &ProcessCtx,
        inputs: &[&[f32]],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        self.process(ctx, inputs, &mut outs[0][..frames]);
    }

    /// Input capacity of the processing adapter. Array-aware opcodes override
    /// this alongside `process_slots`, so invalid graphs fail before playback.
    fn max_inputs(&self) -> usize {
        MAX_INPUTS
    }

    /// Process directly from graph slots. Array inputs can have any fixed
    /// length; scalar built-ins use the small stack adapter below.
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        assert!(
            indices.len() <= MAX_INPUTS,
            "scalar opcode has too many ports"
        );
        let mut inputs: [&[f32]; MAX_INPUTS] = [&[]; MAX_INPUTS];
        for (input, &index) in inputs.iter_mut().zip(indices) {
            *input = &slots[index][..frames];
        }
        self.process_outputs(ctx, &inputs[..indices.len()], outs, frames);
    }

    /// True when processing has an observable effect beyond its output, such
    /// as an OSC tap. These nodes remain live even without an audio consumer.
    fn has_side_effects(&self) -> bool {
        false
    }

    /// True while this opcode still has sound to produce after the note ends
    /// (an envelope that has not finished, or a delay still ringing).
    fn is_active(&self) -> bool {
        false
    }

    /// True for an envelope with a release, which decides when its voice ends.
    fn is_envelope(&self) -> bool {
        false
    }

    /// Envelope activity separately from effect tails in compound DSP graphs.
    fn has_active_envelope(&self) -> bool {
        self.is_envelope() && self.is_active()
    }

    /// Carry compatible runtime state into a newly compiled instance. Settings
    /// and wiring belong to the new instance; implementations only copy memory.
    fn carry_state(&mut self, _old: &mut dyn Opcode) {}

    /// Apply a named control target, with a linear ramp measured in samples.
    fn set_control(&mut self, _key: &str, _start: f32, _target: f32, _frames: u64, _elapsed: u64) {}

    /// Whether this input is sampled during effect initialization.
    fn input_is_init(&self, _index: usize) -> bool {
        false
    }

    /// Samples of delay the opcode introduces, for latency compensation.
    fn latency(&self) -> usize {
        0
    }
}

/// Lets a boxed opcode be added to a graph like any other.
impl Opcode for Box<dyn Opcode> {
    fn buffer_channels(&self, key: u64) -> usize {
        (**self).buffer_channels(key)
    }
    fn stream_cache(&self) -> Option<&std::sync::Arc<crate::ops::StreamCache>> {
        (**self).stream_cache()
    }
    fn control_info(&self) -> Option<(&str, f32)> {
        (**self).control_info()
    }
    fn buffer_bindings(&self, key: u64) -> usize {
        (**self).buffer_bindings(key)
    }
    fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank], cursor: &mut usize) {
        (**self).swap_buffers(key, banks, cursor);
    }
    fn reports(&self, visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {
        (**self).reports(visit);
    }

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

    fn reset(&mut self) {
        (**self).reset();
    }

    fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        (**self).set_control(key, start, target, frames, elapsed);
    }

    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        (**self).process(ctx, inputs, out);
    }

    fn outputs(&self) -> usize {
        (**self).outputs()
    }

    fn process_outputs(
        &mut self,
        ctx: &ProcessCtx,
        inputs: &[&[f32]],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        (**self).process_outputs(ctx, inputs, outs, frames);
    }

    fn max_inputs(&self) -> usize {
        (**self).max_inputs()
    }

    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        (**self).process_slots(ctx, slots, indices, outs, frames);
    }

    fn has_side_effects(&self) -> bool {
        (**self).has_side_effects()
    }

    fn is_active(&self) -> bool {
        (**self).is_active()
    }

    fn is_envelope(&self) -> bool {
        (**self).is_envelope()
    }

    fn has_active_envelope(&self) -> bool {
        (**self).has_active_envelope()
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        (**self).carry_state(old);
    }

    fn input_is_init(&self, index: usize) -> bool {
        (**self).input_is_init(index)
    }

    fn latency(&self) -> usize {
        (**self).latency()
    }
}
