//! Functions with state. The language compiles a `fn` body to a small register
//! program (a [`Kernel`]) which [`Custom`] runs once per sample, so state
//! carries from one sample to the next exactly as in a hand-written filter.
//! Work that only depends on note-start or per-block values runs at those rates.

use std::sync::Arc;

use crate::opcode::{MAX_INPUTS, Opcode, Port, ProcessCtx, blocks};
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::lanes as lane_ops;
use crate::{Array, wrapped_index};

/// Scalar math functions, usable inside kernels and as the [`Func`] opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fn1 {
    Sin,
    Cos,
    Tanh,
    Exp,
    Abs,
    Tan,
    Floor,
    Ceil,
    Sqrt,
    Log,
    Sign,
    /// The part after the point: `x - floor(x)`.
    Fract,
}

impl Fn1 {
    /// Every function, for lookup by name.
    pub const ALL: [Fn1; 12] = [
        Fn1::Sin,
        Fn1::Cos,
        Fn1::Tanh,
        Fn1::Exp,
        Fn1::Abs,
        Fn1::Tan,
        Fn1::Floor,
        Fn1::Ceil,
        Fn1::Sqrt,
        Fn1::Log,
        Fn1::Sign,
        Fn1::Fract,
    ];
}

impl Fn1 {
    pub fn apply(self, x: f64) -> f64 {
        match self {
            Fn1::Sin => x.sin(),
            Fn1::Cos => x.cos(),
            Fn1::Tanh => x.tanh(),
            Fn1::Exp => x.exp(),
            Fn1::Abs => x.abs(),
            Fn1::Tan => x.tan(),
            Fn1::Floor => x.floor(),
            Fn1::Ceil => x.ceil(),
            Fn1::Sqrt => x.max(0.0).sqrt(),
            // the natural log, kept finite for a silent or negative input
            Fn1::Log => x.max(1e-30).ln(),
            Fn1::Sign => {
                if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
            Fn1::Fract => x - x.floor(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Fn1::Sin => "sin",
            Fn1::Cos => "cos",
            Fn1::Tanh => "tanh",
            Fn1::Exp => "exp",
            Fn1::Abs => "abs",
            Fn1::Tan => "tan",
            Fn1::Floor => "floor",
            Fn1::Ceil => "ceil",
            Fn1::Sqrt => "sqrt",
            Fn1::Log => "log",
            Fn1::Sign => "sign",
            Fn1::Fract => "fract",
        }
    }
}

/// `f(x)` applied to every sample.
#[derive(Clone)]
pub struct Func(pub Fn1);

impl Opcode for Func {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, x) in out.iter_mut().zip(inputs[0]) {
            *o = self.0.apply(f64::from(*x)) as f32;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Pow,
    /// The angle of the point (`b`, `a`), in radians.
    Atan2,
    /// The remainder, with the sign of the divisor: `-1.mod(3)` is 2. Zero if the divisor is 0.
    Rem,
}

impl BinOp {
    pub fn apply(self, x: f64, y: f64) -> f64 {
        match self {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => {
                if y == 0.0 {
                    0.0
                } else {
                    x / y
                }
            }
            BinOp::Min => x.min(y),
            BinOp::Max => x.max(y),
            BinOp::Pow => x.powf(y),
            BinOp::Atan2 => x.atan2(y),
            BinOp::Rem => rem(x, y),
        }
    }
}

/// `x.mod(y)`.
pub fn rem(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        0.0
    } else {
        x - y * (x / y).floor()
    }
}

impl Cmp {
    pub fn holds(self, x: f64, y: f64) -> bool {
        match self {
            Cmp::Lt => x < y,
            Cmp::Gt => x > y,
            Cmp::Le => x <= y,
            Cmp::Ge => x >= y,
            Cmp::Eq => x == y,
            Cmp::Ne => x != y,
        }
    }
}

/// A comparison, which gives 1 when it holds and 0 when it doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

/// One step of a kernel. Every instruction that has a `dst` writes a fresh
/// register, and registers are only read after they are written, so a kernel
/// is straight-line single-assignment code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Instr {
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
    Neg {
        dst: usize,
        a: usize,
    },
    Copy {
        dst: usize,
        src: usize,
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
    /// due into `dst`. Every half frame it runs the scope's code on the latest frame.
    Frame {
        scope: usize,
        input: usize,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// The lane the windowed frame is put in.
    pub window: usize,
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

/// The input and output of one frame scope while it runs.
struct FrameRun {
    input: Vec<f64>,
    output: Vec<f64>,
    pos: usize,
    window: Vec<f64>,
}

impl FrameRun {
    fn new(size: usize) -> Self {
        FrameRun {
            input: vec![0.0; size],
            output: vec![0.0; size],
            pos: 0,
            window: (0..size)
                .map(|i| 0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / size as f64).cos()))
                .collect(),
        }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.pos = 0;
    }
}

/// One size of FFT, with the space it works in.
struct FftPlan {
    fwd: Arc<dyn RealToComplex<f64>>,
    inv: Arc<dyn ComplexToReal<f64>>,
    spectrum: Vec<Complex<f64>>,
    time: Vec<f64>,
    scratch_fwd: Vec<Complex<f64>>,
    scratch_inv: Vec<Complex<f64>>,
}

impl FftPlan {
    fn new(size: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let (fwd, inv) = (
            planner.plan_fft_forward(size),
            planner.plan_fft_inverse(size),
        );
        FftPlan {
            spectrum: fwd.make_output_vec(),
            time: fwd.make_input_vec(),
            scratch_fwd: fwd.make_scratch_vec(),
            scratch_inv: inv.make_scratch_vec(),
            fwd,
            inv,
        }
    }
}

/// A running copy of a [`Kernel`], with its own state.
pub struct Custom {
    kernel: Arc<Kernel>,
    state: Vec<f64>,
    buffers: Vec<Array<f64>>,
    /// The arrays vector instructions compute into.
    lanes: Vec<Vec<f64>>,
    frame_runs: Vec<FrameRun>,
    fft_plans: Vec<FftPlan>,
    regs: Vec<f64>,
    sample_rate: f64,
    /// Whether the note-start section has run.
    started: bool,
    /// State carried over from an earlier definition, which the note-start section must not reset.
    carried: bool,
    /// Samples in the block being processed, for opcodes that run once per block.
    block_frames: usize,
    /// Which opcodes take a sample directly, and how many results each has.
    one_sample: Vec<bool>,
    native_outputs_n: Vec<usize>,
    /// This copy's own built-in opcodes, with their state.
    natives: Vec<Box<dyn Opcode>>,
    native_inputs: Vec<Vec<f32>>,
    native_outputs: Vec<Vec<f32>>,
    /// The one-result block `process` renders into.
    single: Vec<Vec<f32>>,
    native_indices: Vec<usize>,
}

impl Custom {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        let inputs = kernel
            .natives
            .iter()
            .map(|n| n.inputs.len())
            .max()
            .unwrap_or(0);
        let outputs = kernel
            .natives
            .iter()
            .map(|n| n.op.outputs())
            .max()
            .unwrap_or(1);
        Custom {
            native_inputs: blocks(inputs),
            native_outputs: blocks(outputs),
            single: blocks(1),
            native_indices: (0..inputs).collect(),
            state: kernel.state_init.clone(),
            buffers: kernel.buffers.clone(),
            lanes: kernel.lanes.iter().map(|&n| vec![0.0; n]).collect(),
            frame_runs: kernel
                .frames
                .iter()
                .map(|f| FrameRun::new(f.size))
                .collect(),
            fft_plans: kernel.ffts.iter().map(|&n| FftPlan::new(n)).collect(),
            regs: vec![0.0; kernel.registers],
            natives: kernel.natives.iter().map(|n| n.op.box_clone()).collect(),
            one_sample: kernel
                .natives
                .iter()
                .map(|n| {
                    n.op.has_process_one() && n.op.outputs() == 1 && n.inputs.len() <= MAX_INPUTS
                })
                .collect(),
            native_outputs_n: kernel.natives.iter().map(|n| n.op.outputs()).collect(),
            kernel,
            sample_rate: 48_000.0,
            started: false,
            carried: false,
            block_frames: 1,
        }
    }

    fn run(
        &mut self,
        section: Section,
        ctx: &ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        i: usize,
    ) {
        let kernel = self.kernel.clone();
        let code = match section {
            Section::Init => &kernel.init,
            Section::Block => &kernel.block,
            Section::Sample => &kernel.code,
        };
        self.run_code(code, section, ctx, input, i);
    }

    fn run_code(
        &mut self,
        code: &[Instr],
        section: Section,
        ctx: &ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        i: usize,
    ) {
        // most kernels have no reset scope, and then no instruction needs checking for one
        if self.kernel.resets.is_empty() {
            self.exec::<false>(code, section, ctx, input, i);
        } else {
            self.exec::<true>(code, section, ctx, input, i);
        }
    }

    fn exec<const RESETS: bool>(
        &mut self,
        code: &[Instr],
        section: Section,
        ctx: &ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        i: usize,
    ) {
        for instr in code {
            if let Instr::Frame {
                scope,
                input: x,
                dst,
            } = *instr
            {
                self.run_frame(scope, x, dst, ctx, input, i);
                continue;
            }
            if RESETS
                && let Instr::Reset {
                    scope,
                    trigger,
                    previous,
                    guard,
                } = *instr
            {
                if guard.is_none_or(|g| self.regs[g] != 0.0) {
                    let positive = self.regs[trigger] > 0.0;
                    let edge = positive && self.state[previous] <= 0.0;
                    self.state[previous] = if positive { 1.0 } else { 0.0 };
                    if edge {
                        let kernel = self.kernel.clone();
                        let k = &*kernel;
                        let scope = &k.resets[scope];
                        self.state[scope.states.clone()]
                            .copy_from_slice(&k.state_init[scope.states.clone()]);
                        for index in scope.buffers.clone() {
                            self.buffers[index].copy_from_slice(&k.buffers[index]);
                        }
                        for &index in &scope.natives {
                            self.natives[index].reset();
                        }
                        let carried = self.carried;
                        self.carried = false;
                        self.run_code(&scope.init, Section::Init, ctx, input, i);
                        self.run_code(&scope.block, Section::Block, ctx, input, i);
                        self.carried = carried;
                    }
                }
                continue;
            }
            let k = &*self.kernel;
            let (regs, state, buffers, lanes, plans) = (
                &mut self.regs,
                &mut self.state,
                &mut self.buffers,
                &mut self.lanes,
                &mut self.fft_plans,
            );
            match *instr {
                Instr::Reset { .. } | Instr::Frame { .. } => unreachable!("handled above"),
                Instr::Const { dst, value } => regs[dst] = value,
                Instr::Input { dst, port } => regs[dst] = f64::from(input(port, i)),
                Instr::SampleRate { dst } => regs[dst] = self.sample_rate,
                Instr::LoadState { dst, slot } => regs[dst] = state[slot],
                Instr::StoreState { slot, src } => {
                    if !(section == Section::Init && self.carried) {
                        state[slot] = regs[src];
                    }
                }
                Instr::Bin { dst, op, a, b } => regs[dst] = op.apply(regs[a], regs[b]),
                Instr::Cmp { dst, op, a, b } => {
                    regs[dst] = if op.holds(regs[a], regs[b]) { 1.0 } else { 0.0 };
                }
                Instr::Select { dst, cond, a, b } => {
                    regs[dst] = if regs[cond] != 0.0 { regs[a] } else { regs[b] };
                }
                Instr::Neg { dst, a } => regs[dst] = -regs[a],
                Instr::Copy { dst, src } => regs[dst] = regs[src],
                Instr::Call { dst, f, a } => regs[dst] = f.apply(regs[a]),
                Instr::Native { dst, index, guard } => {
                    if guard.is_some_and(|g| regs[g] == 0.0) {
                        continue;
                    }
                    let call = &k.natives[index];
                    // a native run once per block (a control scope) sees its inputs held through it
                    if section != Section::Block {
                        let op = &mut self.natives[index];
                        if self.one_sample[index] {
                            let mut x = [0.0f32; MAX_INPUTS];
                            for (value, input) in x.iter_mut().zip(&call.inputs) {
                                *value = match *input {
                                    NativeInput::Reg(r) => regs[r] as f32,
                                    NativeInput::Const(c) => c as f32,
                                };
                            }
                            regs[dst] = f64::from(op.process_one(ctx, &x[..call.inputs.len()]));
                            continue;
                        }
                        for (value, input) in self.native_inputs.iter_mut().zip(&call.inputs) {
                            value[0] = match *input {
                                NativeInput::Reg(r) => regs[r] as f32,
                                NativeInput::Const(c) => c as f32,
                            };
                        }
                        let outputs = self.native_outputs_n[index];
                        op.process_slots(
                            ctx,
                            &self.native_inputs,
                            &self.native_indices[..call.inputs.len()],
                            &mut self.native_outputs[..outputs],
                            1,
                        );
                        for (k, output) in self.native_outputs.iter().take(outputs).enumerate() {
                            regs[dst + k] = f64::from(output[0]);
                        }
                        continue;
                    }
                    let held = self.block_frames;
                    for (value, input) in self.native_inputs.iter_mut().zip(&call.inputs) {
                        let x = match *input {
                            NativeInput::Reg(r) => regs[r] as f32,
                            NativeInput::Const(c) => c as f32,
                        };
                        value[..held].fill(x);
                    }
                    let op = &mut self.natives[index];
                    op.process_slots(
                        ctx,
                        &self.native_inputs,
                        &self.native_indices[..call.inputs.len()],
                        &mut self.native_outputs[..op.outputs()],
                        held,
                    );
                    for (k, output) in self.native_outputs.iter().take(op.outputs()).enumerate() {
                        regs[dst + k] = f64::from(output[0]);
                    }
                }
                Instr::Read { dst, list, idx }
                | Instr::ReadAt {
                    dst,
                    list,
                    pos: idx,
                    ..
                } => {
                    let interp = match *instr {
                        Instr::ReadAt { interp, .. } => interp,
                        _ => Interp::None,
                    };
                    let value = match list {
                        ArraySource::Table(t) => k.tables[t].read_at(regs[idx], interp),
                        ArraySource::State(s) => read_at(&buffers[s], regs[idx], interp),
                        ArraySource::Lane(l) => read_at(&lanes[l], regs[idx], interp),
                        ArraySource::Values(a) => {
                            read_array(k.values[a].len(), regs[idx], interp, |i| {
                                regs[k.values[a][i]]
                            })
                        }
                    };
                    regs[dst] = value;
                }
                Instr::Write {
                    list,
                    idx,
                    src,
                    cond,
                } => {
                    if !(section == Section::Init && self.carried)
                        && cond.is_none_or(|c| regs[c] != 0.0)
                    {
                        let data = &mut buffers[list];
                        let n = data.len();
                        data[wrapped_index(regs[idx], n)] = regs[src];
                    }
                }
                Instr::LaneBin { dst, op, a, b } => {
                    let mut d = std::mem::take(&mut lanes[dst]);
                    let (a, b) = (
                        operand(a, regs, lanes, buffers, &k.tables_f64),
                        operand(b, regs, lanes, buffers, &k.tables_f64),
                    );
                    lane_ops::zip(op, &mut d, a, b);
                    lanes[dst] = d;
                }
                Instr::LaneCall { dst, f, a } => {
                    let mut d = std::mem::take(&mut lanes[dst]);
                    let a = operand(a, regs, lanes, buffers, &k.tables_f64);
                    lane_ops::map(f, &mut d, a);
                    lanes[dst] = d;
                }
                Instr::LaneFromRegs { dst, values } => {
                    for (d, &r) in lanes[dst].iter_mut().zip(k.values[values].iter()) {
                        *d = regs[r];
                    }
                }
                Instr::LaneStore { list, src, cond } => {
                    if !(section == Section::Init && self.carried)
                        && cond.is_none_or(|c| regs[c] != 0.0)
                    {
                        match src {
                            LaneSrc::Reg(r) => buffers[list].fill(regs[r]),
                            LaneSrc::Lane(l) => buffers[list].copy_from_slice(&lanes[l]),
                            LaneSrc::Table(t) => buffers[list].copy_from_slice(&k.tables_f64[t]),
                            LaneSrc::State(from) if from == list => {}
                            LaneSrc::State(from) => {
                                let (to, from) = two_mut(buffers, list, from);
                                to.copy_from_slice(from);
                            }
                        }
                    }
                }
                Instr::Reduce { dst, kind, a } => {
                    regs[dst] = match operand(a, regs, lanes, buffers, &k.tables_f64) {
                        lane_ops::Operand::Slice(s) => match kind {
                            Reduce::Sum => lane_ops::sum(s),
                            Reduce::Min => lane_ops::extreme(s, true),
                            Reduce::Max => lane_ops::extreme(s, false),
                        },
                        lane_ops::Operand::Scalar(v) => v,
                    };
                }
                Instr::Dot { dst, a, b } => {
                    regs[dst] = match (
                        operand(a, regs, lanes, buffers, &k.tables_f64),
                        operand(b, regs, lanes, buffers, &k.tables_f64),
                    ) {
                        (lane_ops::Operand::Slice(a), lane_ops::Operand::Slice(b)) => {
                            lane_ops::dot(a, b)
                        }
                        (lane_ops::Operand::Slice(a), lane_ops::Operand::Scalar(b))
                        | (lane_ops::Operand::Scalar(b), lane_ops::Operand::Slice(a)) => {
                            lane_ops::sum(a) * b
                        }
                        (lane_ops::Operand::Scalar(a), lane_ops::Operand::Scalar(b)) => a * b,
                    };
                }
                Instr::Fir {
                    dst,
                    taps,
                    ring,
                    from,
                } => {
                    regs[dst] = match operand(taps, regs, lanes, buffers, &k.tables_f64) {
                        lane_ops::Operand::Slice(taps) => {
                            lane_ops::fir(taps, &buffers[ring], regs[from])
                        }
                        lane_ops::Operand::Scalar(t) => {
                            t * read_at(&buffers[ring], regs[from], Interp::None)
                        }
                    };
                }
                Instr::Taps {
                    dst,
                    ring,
                    offsets,
                    from,
                    interp,
                } => {
                    let mut d = std::mem::take(&mut lanes[dst]);
                    let offsets = operand(offsets, regs, lanes, buffers, &k.tables_f64);
                    lane_ops::taps(&mut d, &buffers[ring], offsets, regs[from], interp);
                    lanes[dst] = d;
                }
                Instr::Fft { re, im, src, plan } => {
                    let (mut dre, mut dim) = (
                        std::mem::take(&mut lanes[re]),
                        std::mem::take(&mut lanes[im]),
                    );
                    if let lane_ops::Operand::Slice(s) =
                        operand(src, regs, lanes, buffers, &k.tables_f64)
                    {
                        let p = &mut plans[plan];
                        p.time.copy_from_slice(s);
                        p.fwd
                            .process_with_scratch(&mut p.time, &mut p.spectrum, &mut p.scratch_fwd)
                            .expect("fft forward");
                        for ((c, r), i) in p.spectrum.iter().zip(dre.iter_mut()).zip(dim.iter_mut())
                        {
                            (*r, *i) = (c.re, c.im);
                        }
                    }
                    lanes[re] = dre;
                    lanes[im] = dim;
                }
                Instr::Ifft { dst, re, im, plan } => {
                    let mut d = std::mem::take(&mut lanes[dst]);
                    if let (lane_ops::Operand::Slice(re), lane_ops::Operand::Slice(im)) = (
                        operand(re, regs, lanes, buffers, &k.tables_f64),
                        operand(im, regs, lanes, buffers, &k.tables_f64),
                    ) {
                        let p = &mut plans[plan];
                        for ((c, &r), &i) in p.spectrum.iter_mut().zip(re).zip(im) {
                            *c = Complex::new(r, i);
                        }
                        // a real signal has no imaginary part at the ends
                        let last = p.spectrum.len() - 1;
                        p.spectrum[0].im = 0.0;
                        p.spectrum[last].im = 0.0;
                        p.inv
                            .process_with_scratch(&mut p.spectrum, &mut p.time, &mut p.scratch_inv)
                            .expect("fft inverse");
                        let scale = 1.0 / p.time.len() as f64;
                        for (d, &t) in d.iter_mut().zip(&p.time) {
                            *d = t * scale;
                        }
                    }
                    lanes[dst] = d;
                }
                Instr::MatVec {
                    dst,
                    matrix,
                    cols,
                    v,
                } => {
                    let mut d = std::mem::take(&mut lanes[dst]);
                    if let (lane_ops::Operand::Slice(m), lane_ops::Operand::Slice(v)) = (
                        operand(matrix, regs, lanes, buffers, &k.tables_f64),
                        operand(v, regs, lanes, buffers, &k.tables_f64),
                    ) {
                        lane_ops::matvec(&mut d, m, cols, v);
                    }
                    lanes[dst] = d;
                }
            }
        }
    }
    /// One sample through a frame scope: the input goes in, the output that is due comes out, and each
    /// half frame the scope's code runs on the latest frame.
    fn run_frame(
        &mut self,
        scope: usize,
        x: usize,
        dst: usize,
        ctx: &ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        i: usize,
    ) {
        let kernel = self.kernel.clone();
        let frame = &kernel.frames[scope];
        let run = &mut self.frame_runs[scope];
        let n = run.input.len();
        self.regs[dst] = std::mem::take(&mut run.output[run.pos]);
        run.input[run.pos] = self.regs[x];
        run.pos = (run.pos + 1) % n;
        if !run.pos.is_multiple_of(n / 2) {
            return;
        }
        let pos = run.pos;
        let window = &mut self.lanes[frame.window];
        for (k, w) in window.iter_mut().enumerate() {
            *w = run.input[(pos + k) % n] * run.window[k];
        }
        self.run_code(&frame.code, Section::Sample, ctx, input, i);
        let run = &mut self.frame_runs[scope];
        let result = operand(
            frame.result,
            &self.regs,
            &self.lanes,
            &self.buffers,
            &kernel.tables_f64,
        );
        match result {
            lane_ops::Operand::Slice(r) => {
                for (k, &v) in r.iter().enumerate() {
                    run.output[(pos + k) % n] += v;
                }
            }
            lane_ops::Operand::Scalar(v) => run.output.iter_mut().for_each(|o| *o += v),
        }
    }

    fn process_with(
        &mut self,
        ctx: &ProcessCtx,
        input: impl Fn(usize, usize) -> f32,
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        if frames == 0 {
            return;
        }
        if !self.started {
            self.run(Section::Init, ctx, &input, 0);
            self.started = true;
        }
        self.block_frames = frames;
        self.run(Section::Block, ctx, &input, 0);
        let kernel = self.kernel.clone();
        for i in 0..frames {
            self.run_code(&kernel.code, Section::Sample, ctx, &input, i);
            for (out, (_, reg)) in outs.iter_mut().zip(&self.kernel.outputs) {
                out[i] = self.regs[*reg] as f32;
            }
        }
    }
}

/// Two different entries of a list, the first mutably.
fn two_mut<T>(items: &mut [T], first: usize, second: usize) -> (&mut T, &T) {
    if first < second {
        let (low, high) = items.split_at_mut(second);
        (&mut low[first], &high[0])
    } else {
        let (low, high) = items.split_at_mut(first);
        (&mut high[0], &low[second])
    }
}

/// What a vector instruction reads, as something the loops can run over.
fn operand<'a>(
    src: LaneSrc,
    regs: &[f64],
    lanes: &'a [Vec<f64>],
    buffers: &'a [Array<f64>],
    tables: &'a [Box<[f64]>],
) -> lane_ops::Operand<'a> {
    match src {
        LaneSrc::Reg(r) => lane_ops::Operand::Scalar(regs[r]),
        LaneSrc::Lane(l) => lane_ops::Operand::Slice(&lanes[l]),
        LaneSrc::State(s) => lane_ops::Operand::Slice(&buffers[s]),
        LaneSrc::Table(t) => lane_ops::Operand::Slice(&tables[t]),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Init,
    Block,
    Sample,
}

/// Read a numeric array through its storage-independent element accessor.
/// This lets tables, buffers and computed arrays share interpolation without
/// gathering their elements or allocating on the audio thread.
pub fn read_array(len: usize, pos: f64, interp: Interp, element: impl Fn(usize) -> f64) -> f64 {
    assert!(len > 0, "cannot read an empty array");
    if !pos.is_finite() {
        return element(0);
    }
    let base = pos.floor();
    let t = pos - base;
    let at = |k: f64| element(wrapped_index(base + k, len));
    match interp {
        Interp::None => at(0.0),
        Interp::Linear => at(0.0) + (at(1.0) - at(0.0)) * t,
        Interp::Cubic => {
            let (y0, y1, y2, y3) = (at(-1.0), at(0.0), at(1.0), at(2.0));
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            ((a * t + b) * t + c) * t + y1
        }
    }
}

pub fn read_at(data: &[f64], pos: f64, interp: Interp) -> f64 {
    read_array(data.len(), pos, interp, |i| data[i])
}

impl Opcode for Custom {
    fn buffer_channels(&self, key: u64) -> usize {
        self.natives
            .iter()
            .map(|n| n.buffer_channels(key))
            .max()
            .unwrap_or(0)
    }
    fn buffer_bindings(&self, key: u64) -> usize {
        self.natives.iter().map(|n| n.buffer_bindings(key)).sum()
    }
    fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank], cursor: &mut usize) {
        for n in &mut self.natives {
            n.swap_buffers(key, banks, cursor);
        }
    }

    fn reports(&self, visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {
        for n in &self.natives {
            n.reports(visit);
        }
    }
    fn input_is_init(&self, index: usize) -> bool {
        self.kernel
            .init
            .iter()
            .any(|i| matches!(i, Instr::Input { port, .. } if *port == index))
    }
    fn latency(&self) -> usize {
        self.kernel.latency
    }

    fn control_warnings(&self, sample_rate: f32) -> Vec<String> {
        self.natives
            .iter()
            .flat_map(|n| n.control_warnings(sample_rate))
            .collect()
    }

    fn step_warnings(&self, step: f32) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (native, template) in self.natives.iter().zip(&self.kernel.natives) {
            out.extend(native.step_warnings(step));
            if native.name() == "osc"
                && let Some(i) = native.ports().iter().position(|p| p.name == "freq")
                && let Some(hz) = match template.inputs.get(i) {
                    Some(NativeInput::Const(hz)) => Some(*hz),
                    Some(NativeInput::Reg(reg)) => self.kernel.init.iter().find_map(|instr| {
                        matches!(instr, Instr::Const { dst, .. } if dst == reg).then(|| match instr
                        {
                            Instr::Const { value, .. } => *value,
                            _ => unreachable!(),
                        })
                    }),
                    None => None,
                }
                && hz > 0.5 / f64::from(step)
            {
                out.push(format!(
                    "an oscillator at {hz}hz is above half the control rate ({}hz), so it aliases",
                    (0.5 / step).round()
                ));
            }
        }
        out
    }

    fn kernel_stats(&self) -> Option<crate::KernelStats> {
        let k = &self.kernel;
        let mut natives = Vec::new();
        let mut visit = |code: &[Instr], per_block: bool| {
            for instr in code {
                if let Instr::Native { index, .. } = instr {
                    natives.push((k.natives[*index].op.name().to_string(), per_block));
                }
            }
        };
        visit(&k.init, true);
        visit(&k.block, true);
        visit(&k.code, false);
        Some(crate::KernelStats {
            init: k.init.len(),
            block: k.block.len(),
            sample: k.code.len(),
            natives,
            control_scope: false,
        })
    }

    fn name(&self) -> &str {
        &self.kernel.name
    }

    fn ports(&self) -> &[Port] {
        &self.kernel.ports
    }

    fn outputs(&self) -> usize {
        self.kernel.outputs.len()
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        // A new copy starts from the kernel's initial state, not ours.
        Box::new(Custom::new(self.kernel.clone()))
    }

    fn reset(&mut self) {
        self.state.copy_from_slice(&self.kernel.state_init);
        for (buffer, initial) in self.buffers.iter_mut().zip(&self.kernel.buffers) {
            buffer.copy_from_slice(initial);
        }
        self.regs.fill(0.0);
        self.frame_runs.iter_mut().for_each(FrameRun::reset);
        self.started = false;
        self.carried = false;
        for native in &mut self.natives {
            native.reset();
        }
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(sample_rate);
        self.natives.iter_mut().for_each(|n| n.prepare(sample_rate));
    }

    fn has_side_effects(&self) -> bool {
        self.kernel.natives.iter().any(|n| n.op.has_side_effects())
    }

    fn is_envelope(&self) -> bool {
        self.natives.iter().any(|n| n.is_envelope())
    }

    fn has_active_envelope(&self) -> bool {
        self.natives.iter().any(|n| n.has_active_envelope())
    }

    fn is_active(&self) -> bool {
        self.natives.iter().any(|n| n.is_active())
    }

    fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        self.natives
            .iter_mut()
            .for_each(|n| n.set_control(key, start, target, frames, elapsed));
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.kernel == old.kernel
        {
            self.state.copy_from_slice(&old.state);
            for (new, old) in self.buffers.iter_mut().zip(&old.buffers) {
                new.copy_from_slice(old);
            }
            self.carried = true;
            for (new, old) in self.natives.iter_mut().zip(&mut old.natives) {
                new.carry_state(&mut **old);
            }
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let mut single = std::mem::take(&mut self.single);
        let frames = out.len();
        self.process_outputs(ctx, inputs, &mut single, frames);
        out.copy_from_slice(&single[0][..frames]);
        self.single = single;
    }

    fn process_outputs(
        &mut self,
        ctx: &ProcessCtx,
        inputs: &[&[f32]],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        self.process_with(ctx, |port, i| inputs[port][i], outs, frames);
    }

    fn max_inputs(&self) -> usize {
        self.ports().len()
    }

    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        self.process_with(ctx, |port, i| slots[indices[port]][i], outs, frames);
    }
}
