//! Register-machine IR for compiled `fn` bodies.

use std::sync::Arc;

use crate::Array;
use crate::opcode::{Opcode, Port};

use super::{BinOp, Cmp, Fn1};

/// Pure scalar math that can reuse its last result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoMath {
    Unary(Fn1),
    Binary(BinOp, usize),
}

/// One step of a kernel. Scalar destinations use distinct registers. Forward skips leave
/// untaken results unread by branch merges; reset scopes may rerun initialization code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Instr {
    /// Compiler markers, resolved after latency compensation by `Kernel::optimize_branches`.
    BeginBranch {
        cond: usize,
    },
    EndBranch,
    /// Skip the next `count` instructions when `cond` is zero.
    SkipIfZero {
        cond: usize,
        count: usize,
    },
    /// Compare, save the boolean result, and skip when the comparison is false.
    CmpSkip {
        dst: usize,
        op: Cmp,
        a: usize,
        b: usize,
        count: usize,
    },
    Reset {
        scope: usize,
        trigger: usize,
        previous: usize,
        guard: Option<usize>,
    },
    Const {
        dst: usize,
        value: f64,
    },
    /// Input port `port`: its current sample, or its first one in the note-start and block sections.
    Input {
        dst: usize,
        port: usize,
    },
    SampleRate {
        dst: usize,
    },
    LoadState {
        dst: usize,
        slot: usize,
    },
    StoreState {
        slot: usize,
        src: usize,
    },
    Bin {
        dst: usize,
        op: BinOp,
        a: usize,
        b: usize,
    },
    Memo {
        dst: usize,
        slot: usize,
        math: MemoMath,
        a: usize,
    },
    Neg {
        dst: usize,
        a: usize,
    },
    Copy {
        dst: usize,
        src: usize,
    },
    /// A multiply followed by an add, with the intermediate rounded as f64 (not FMA).
    MulAdd {
        dst: usize,
        a: usize,
        b: usize,
        c: usize,
    },
    /// `(a * b) - c`, with the product rounded as f64 before subtraction.
    MulSub {
        dst: usize,
        a: usize,
        b: usize,
        c: usize,
    },
    /// `c - (a * b)`, with the product rounded as f64 before subtraction.
    SubMul {
        dst: usize,
        a: usize,
        b: usize,
        c: usize,
    },
    /// `x.max(lo).min(hi)`.
    Clamp {
        dst: usize,
        x: usize,
        lo: usize,
        hi: usize,
    },
    Cmp {
        dst: usize,
        op: Cmp,
        a: usize,
        b: usize,
    },
    /// `a` if `cond` is not 0, otherwise `b`.
    Select {
        dst: usize,
        cond: usize,
        a: usize,
        b: usize,
    },
    Call {
        dst: usize,
        f: Fn1,
        a: usize,
    },
    /// One sample of the kernel's `index`th built-in opcode.
    Native {
        dst: usize,
        index: usize,
        guard: Option<usize>,
    },
    /// Entry `idx` of a list, wrapping at both ends.
    Read {
        dst: usize,
        list: ArraySource,
        idx: usize,
    },
    /// A list read at a fractional position, wrapping.
    ReadAt {
        dst: usize,
        list: ArraySource,
        pos: usize,
        interp: Interp,
    },
    /// Write entry `idx` (wrapping) of state list `list`, if `cond` is absent or not 0.
    Write {
        list: usize,
        idx: usize,
        src: usize,
        cond: Option<usize>,
    },
    /// `lanes[dst] = a op b`, element by element.
    LaneBin {
        dst: usize,
        op: BinOp,
        a: LaneSrc,
        b: LaneSrc,
    },
    /// `lanes[dst] = f(a)`, element by element.
    LaneCall {
        dst: usize,
        f: Fn1,
        a: LaneSrc,
    },
    /// The registers of computed array `values` gathered into `lanes[dst]`.
    LaneFromRegs {
        dst: usize,
        values: usize,
    },
    /// Copy an array into state list `list`, if `cond` is absent or not 0.
    LaneStore {
        list: usize,
        src: LaneSrc,
        cond: Option<usize>,
    },
    /// One value from the entries of an array.
    Reduce {
        dst: usize,
        kind: Reduce,
        a: LaneSrc,
    },
    /// `Σ a[i] * b[i]`.
    Dot {
        dst: usize,
        a: LaneSrc,
        b: LaneSrc,
    },
    /// `Σ taps[i] * ring[from - i]` over state list `ring`, newest first.
    Fir {
        dst: usize,
        taps: LaneSrc,
        ring: usize,
        from: usize,
    },
    /// `lanes[dst][k] = ring[from - offsets[k]]`: many taps of the state list `ring`.
    Taps {
        dst: usize,
        ring: usize,
        offsets: LaneSrc,
        from: usize,
        interp: Interp,
    },
    /// Feed the sample in `input` to frame scope `scope` and take the sample of its output that is
    /// due into `dst`. Every hop it runs the scope's code on the latest frame.
    Frame {
        scope: usize,
        input: usize,
        input2: Option<usize>,
        dst: usize,
    },
    /// The real FFT of an array into lanes `re` and `im`, which have `n / 2 + 1` entries each.
    Fft {
        re: usize,
        im: usize,
        src: LaneSrc,
        plan: usize,
    },
    /// The inverse of [`Instr::Fft`]: the `n` samples with the spectrum `re`, `im`, into `dst`.
    Ifft {
        dst: usize,
        re: LaneSrc,
        im: LaneSrc,
        plan: usize,
    },
    /// `lanes[dst] = matrix * v`, for a matrix of `cols` columns stored row by row.
    MatVec {
        dst: usize,
        matrix: LaneSrc,
        cols: usize,
        v: LaneSrc,
    },
}

/// Which list an instruction reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArraySource {
    /// A table shared by every voice.
    Table(usize),
    /// An immutable array of computed register values.
    Values(usize),
    /// A state list, written by this copy of the kernel.
    State(usize),
    /// A lane: an array computed by a vector instruction, held in this copy of the kernel.
    Lane(usize),
}

/// What a vector instruction reads: a single value that stands for every element, or an array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneSrc {
    Reg(usize),
    Lane(usize),
    State(usize),
    Table(usize),
}

/// How a vector instruction reduces an array to one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reduce {
    Sum,
    Min,
    Max,
}

/// How a read between entries is worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interp {
    None,
    Linear,
    Cubic,
}

/// Where a built-in opcode called inside a kernel gets one of its inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NativeInput {
    Reg(usize),
    Const(f64),
}

/// A built-in opcode (a filter, an oscillator...) called from inside a kernel, which runs it one
/// sample at a time. Each running copy of the kernel makes its own copy, with its own state.
pub struct Native {
    pub op: Box<dyn Opcode>,
    /// One for each of the opcode's ports.
    pub inputs: Vec<NativeInput>,
}

impl Clone for Native {
    fn clone(&self) -> Self {
        Native {
            op: self.op.box_clone(),
            inputs: self.inputs.clone(),
        }
    }
}

impl std::fmt::Debug for Native {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Native({}, {:?})", self.op.name(), self.inputs)
    }
}

impl PartialEq for Native {
    fn eq(&self, other: &Self) -> bool {
        self.op.name() == other.op.name() && self.inputs == other.inputs
    }
}

/// Memory and initialization code owned by one lexical reset scope.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetScope {
    pub states: std::ops::Range<usize>,
    pub buffers: std::ops::Range<usize>,
    pub natives: Vec<usize>,
    pub registers: std::ops::Range<usize>,
    pub init: Vec<Instr>,
    pub block: Vec<Instr>,
}

/// Code that runs once every half frame on the last `size` samples of a signal, and whose result is
/// overlapped back into a signal. The frame is windowed (Hann) on the way in, so overlapping half
/// frames that come out unchanged add up to the input, `size` samples late.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameScope {
    pub size: usize,
    pub hop: usize,
    /// The lane the windowed frame is put in.
    pub window: usize,
    /// The second input's window lane, when present.
    pub window2: Option<usize>,
    /// What the code leaves: `size` samples to overlap into the output.
    pub result: LaneSrc,
    pub code: Vec<Instr>,
}

/// A compiled `fn` body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Kernel {
    pub latency: usize,
    pub name: String,
    pub resets: Vec<ResetScope>,
    pub ports: Vec<Port>,
    /// Run once, before the first sample: what depends only on constants and note-start values,
    /// including the starting values of state.
    pub init: Vec<Instr>,
    /// Run at the start of every block: what depends on once-per-block values.
    pub block: Vec<Instr>,
    /// Run every sample.
    pub code: Vec<Instr>,
    pub registers: usize,
    pub memo_slots: usize,
    /// Initial value of each state slot, before `init` runs.
    pub state_init: Vec<f64>,
    /// Starting contents of each state list.
    pub buffers: Vec<Array<f64>>,
    /// Lists every voice reads and none writes.
    pub tables: Vec<Arc<crate::Table>>,
    /// The same tables as `f64`, for vector instructions.
    pub tables_f64: Vec<Box<[f64]>>,
    /// The length of each lane.
    pub lanes: Vec<usize>,
    pub frames: Vec<FrameScope>,
    /// The sizes of the FFTs the code does.
    pub ffts: Vec<usize>,
    /// Register indices of computed arrays, built once by the compiler.
    pub values: Vec<Array<usize>>,
    /// The registers holding the results, by name. Most kernels have one.
    pub outputs: Vec<(String, usize)>,
    /// Built-in opcodes the body calls.
    pub natives: Vec<Native>,
}
