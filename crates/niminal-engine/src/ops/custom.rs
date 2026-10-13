//! Functions with state. The language compiles a `fn` body to a small register
//! program (a [`Kernel`]) which [`Custom`] runs once per sample, so state
//! carries from one sample to the next exactly as in a hand-written filter.
//! Work that only depends on note-start or per-block values runs at those rates.

use std::sync::Arc;

use crate::opcode::{BLOCK, MAX_INPUTS, Opcode, Port, ProcessCtx};

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
        Fn1::Sin, Fn1::Cos, Fn1::Tanh, Fn1::Exp, Fn1::Abs, Fn1::Tan,
        Fn1::Floor, Fn1::Ceil, Fn1::Sqrt, Fn1::Log, Fn1::Sign, Fn1::Fract,
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
            Fn1::Sign => if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 },
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
    /// The remainder, with the sign of the divisor: `-1.mod(3)` is 2. Zero if the divisor is 0.
    Rem,
}

impl BinOp {
    pub fn apply(self, x: f64, y: f64) -> f64 {
        match self {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => if y == 0.0 { 0.0 } else { x / y },
            BinOp::Min => x.min(y),
            BinOp::Max => x.max(y),
            BinOp::Pow => x.powf(y),
            BinOp::Rem => rem(x, y),
        }
    }
}

/// `x.mod(y)`.
pub fn rem(x: f64, y: f64) -> f64 {
    if y == 0.0 { 0.0 } else { x - y * (x / y).floor() }
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
    Const { dst: usize, value: f64 },
    /// Input port `port`: its current sample, or its first one in the note-start and block sections.
    Input { dst: usize, port: usize },
    SampleRate { dst: usize },
    LoadState { dst: usize, slot: usize },
    StoreState { slot: usize, src: usize },
    Bin { dst: usize, op: BinOp, a: usize, b: usize },
    Neg { dst: usize, a: usize },
    Copy { dst: usize, src: usize },
    Cmp { dst: usize, op: Cmp, a: usize, b: usize },
    /// `a` if `cond` is not 0, otherwise `b`.
    Select { dst: usize, cond: usize, a: usize, b: usize },
    Call { dst: usize, f: Fn1, a: usize },
    /// One sample of the kernel's `index`th built-in opcode.
    Native { dst: usize, index: usize },
    /// Entry `idx` of a list, wrapping at both ends.
    Read { dst: usize, list: List, idx: usize },
    /// A list read at a fractional position, wrapping.
    ReadAt { dst: usize, list: List, pos: usize, interp: Interp },
    /// Write entry `idx` (wrapping) of state list `list`, if `cond` is absent or not 0.
    Write { list: usize, idx: usize, src: usize, cond: Option<usize> },
}

/// Which list an instruction reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    /// A table shared by every voice.
    Table(usize),
    /// A state list, written by this copy of the kernel.
    State(usize),
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
        Native { op: self.op.box_clone(), inputs: self.inputs.clone() }
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

/// A compiled `fn` body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Kernel {
    pub name: String,
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
    pub lists: Vec<Vec<f64>>,
    /// Lists every voice reads and none writes.
    pub tables: Vec<Arc<[f64]>>,
    /// The registers holding the results, by name. Most kernels have one.
    pub outputs: Vec<(String, usize)>,
    /// Built-in opcodes the body calls.
    pub natives: Vec<Native>,
}

/// A running copy of a [`Kernel`], with its own state.
pub struct Custom {
    kernel: Arc<Kernel>,
    state: Vec<f64>,
    lists: Vec<Vec<f64>>,
    regs: Vec<f64>,
    sample_rate: f64,
    /// Whether the note-start section has run.
    started: bool,
    /// State carried over from an earlier definition, which the note-start section must not reset.
    carried: bool,
    /// This copy's own built-in opcodes, with their state.
    natives: Vec<Box<dyn Opcode>>,
}

impl Custom {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Custom {
            state: kernel.state_init.clone(),
            lists: kernel.lists.clone(),
            regs: vec![0.0; kernel.registers],
            natives: kernel.natives.iter().map(|n| n.op.box_clone()).collect(),
            kernel,
            sample_rate: 48_000.0,
            started: false,
            carried: false,
        }
    }

    fn run(&mut self, section: Section, ctx: &ProcessCtx, inputs: &[&[f32]], i: usize) {
        let k = &*self.kernel;
        let code = match section {
            Section::Init => &k.init,
            Section::Block => &k.block,
            Section::Sample => &k.code,
        };
        let (regs, state, lists) = (&mut self.regs, &mut self.state, &mut self.lists);
        for instr in code {
            match *instr {
                Instr::Const { dst, value } => regs[dst] = value,
                Instr::Input { dst, port } => regs[dst] = f64::from(inputs[port][i]),
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
                Instr::Native { dst, index } => {
                    let call = &k.natives[index];
                    let mut values = [[0.0f32; 1]; MAX_INPUTS];
                    for (value, input) in values.iter_mut().zip(&call.inputs) {
                        value[0] = match *input {
                            NativeInput::Reg(r) => regs[r] as f32,
                            NativeInput::Const(c) => c as f32,
                        };
                    }
                    let mut slices: [&[f32]; MAX_INPUTS] = [&[]; MAX_INPUTS];
                    for (slice, value) in slices.iter_mut().zip(&values).take(call.inputs.len()) {
                        *slice = value;
                    }
                    let mut y = [0.0f32; 1];
                    self.natives[index].process(ctx, &slices[..call.inputs.len()], &mut y);
                    regs[dst] = f64::from(y[0]);
                }
                Instr::Read { dst, list, idx } => {
                    let data: &[f64] = match list {
                        List::Table(t) => &k.tables[t],
                        List::State(s) => &lists[s],
                    };
                    regs[dst] = data[wrap(regs[idx].floor(), data.len())];
                }
                Instr::ReadAt { dst, list, pos, interp } => {
                    let data: &[f64] = match list {
                        List::Table(t) => &k.tables[t],
                        List::State(s) => &lists[s],
                    };
                    regs[dst] = read_at(data, regs[pos], interp);
                }
                Instr::Write { list, idx, src, cond } => {
                    if cond.is_none_or(|c| regs[c] != 0.0) {
                        let data = &mut lists[list];
                        let n = data.len();
                        data[wrap(regs[idx].floor(), n)] = regs[src];
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Init,
    Block,
    Sample,
}

/// An index into a list of `len` entries, counting from either end.
fn wrap(i: f64, len: usize) -> usize {
    if !i.is_finite() {
        return 0;
    }
    i.rem_euclid(len as f64) as usize % len
}

/// The value of a list at a fractional position, wrapping.
pub fn read_at(data: &[f64], pos: f64, interp: Interp) -> f64 {
    let n = data.len();
    if !pos.is_finite() {
        return data[0];
    }
    let base = pos.floor();
    let t = pos - base;
    let at = |k: f64| data[wrap(base + k, n)];
    match interp {
        Interp::None => at(0.0),
        Interp::Linear => at(0.0) + (at(1.0) - at(0.0)) * t,
        Interp::Cubic => {
            // Catmull-Rom through the four entries around the position
            let (y0, y1, y2, y3) = (at(-1.0), at(0.0), at(1.0), at(2.0));
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            ((a * t + b) * t + c) * t + y1
        }
    }
}

impl Opcode for Custom {
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

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(sample_rate);
        self.natives.iter_mut().for_each(|n| n.prepare(sample_rate));
    }

    fn is_active(&self) -> bool {
        self.natives.iter().any(|n| n.is_active())
    }

    fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        self.natives.iter_mut().for_each(|n| n.set_control(key, start, target, frames, elapsed));
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.kernel == old.kernel
        {
            self.state.copy_from_slice(&old.state);
            for (new, old) in self.lists.iter_mut().zip(&old.lists) {
                new.copy_from_slice(old);
            }
            self.carried = true;
            for (new, old) in self.natives.iter_mut().zip(&mut old.natives) {
                new.carry_state(&mut **old);
            }
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let mut block = [[0.0f32; BLOCK]; 1];
        let frames = out.len();
        self.process_outputs(ctx, inputs, &mut block, frames);
        out.copy_from_slice(&block[0][..frames]);
    }

    fn process_outputs(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], outs: &mut [[f32; BLOCK]], frames: usize) {
        if frames == 0 {
            return;
        }
        if !self.started {
            self.run(Section::Init, ctx, inputs, 0);
            self.started = true;
        }
        self.run(Section::Block, ctx, inputs, 0);
        for i in 0..frames {
            self.run(Section::Sample, ctx, inputs, i);
            for (out, (_, reg)) in outs.iter_mut().zip(&self.kernel.outputs) {
                out[i] = self.regs[*reg] as f32;
            }
        }
    }
}
