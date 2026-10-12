//! User-defined opcodes. The language compiles an `opcode` body to a small
//! register program (a [`Kernel`]) which [`Custom`] runs once per sample, so
//! state carries from one sample to the next exactly as in a hand-written
//! filter.

use std::sync::Arc;

use crate::opcode::{MAX_INPUTS, Opcode, Port, ProcessCtx};

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
    /// The remainder, with the sign of the divisor: `-1 % 3` is 2. Zero if the divisor is 0.
    Rem,
}

/// `x % y`.
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

/// One step of a kernel. Every instruction writes a fresh register (`dst`), and
/// registers are only read after they are written, so a kernel is a straight
/// line of single-assignment code run once per sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Instr {
    Const { dst: usize, value: f64 },
    /// The current sample of input port `port`.
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

/// A compiled `opcode` body.
#[derive(Debug, Clone, PartialEq)]
pub struct Kernel {
    pub name: String,
    pub ports: Vec<Port>,
    pub code: Vec<Instr>,
    pub registers: usize,
    /// Initial value of each state slot.
    pub state_init: Vec<f64>,
    /// The registers holding the opcode's results, by name. An ordinary opcode has one.
    pub outputs: Vec<(String, usize)>,
    /// Built-in opcodes the body calls.
    pub natives: Vec<Native>,
}

/// A running copy of a [`Kernel`], with its own state.
pub struct Custom {
    kernel: Arc<Kernel>,
    state: Vec<f64>,
    regs: Vec<f64>,
    sample_rate: f64,
    /// Which of the kernel's outputs this copy delivers.
    which: usize,
    /// This copy's own built-in opcodes, with their state.
    natives: Vec<Box<dyn Opcode>>,
}

impl Clone for Custom {
    fn clone(&self) -> Self {
        Custom {
            kernel: self.kernel.clone(),
            state: self.state.clone(),
            regs: self.regs.clone(),
            sample_rate: self.sample_rate,
            which: self.which,
            natives: self.natives.iter().map(|n| n.box_clone()).collect(),
        }
    }
}

impl Custom {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Custom {
            state: kernel.state_init.clone(),
            regs: vec![0.0; kernel.registers],
            natives: kernel.natives.iter().map(|n| n.op.box_clone()).collect(),
            kernel,
            sample_rate: 48_000.0,
            which: 0,
        }
    }

    /// Deliver the kernel's `which`th output (an opcode with several runs once for each).
    pub fn output(mut self, which: usize) -> Self {
        assert!(which < self.kernel.outputs.len(), "the kernel has no such output");
        self.which = which;
        self
    }
}

impl Opcode for Custom {
    fn name(&self) -> &str {
        &self.kernel.name
    }

    fn ports(&self) -> &[Port] {
        &self.kernel.ports
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        // A new copy starts from the kernel's initial state, not ours.
        Box::new(Custom::new(self.kernel.clone()).output(self.which))
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
            && self.which == old.which
        {
            self.state.copy_from_slice(&old.state);
            for (new, old) in self.natives.iter_mut().zip(&mut old.natives) {
                new.carry_state(&mut **old);
            }
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let k = &*self.kernel;
        let (regs, state) = (&mut self.regs, &mut self.state);
        for (i, o) in out.iter_mut().enumerate() {
            for instr in &k.code {
                match *instr {
                    Instr::Const { dst, value } => regs[dst] = value,
                    Instr::Input { dst, port } => regs[dst] = f64::from(inputs[port][i]),
                    Instr::SampleRate { dst } => regs[dst] = self.sample_rate,
                    Instr::LoadState { dst, slot } => regs[dst] = state[slot],
                    Instr::StoreState { slot, src } => state[slot] = regs[src],
                    Instr::Bin { dst, op, a, b } => {
                        let (x, y) = (regs[a], regs[b]);
                        regs[dst] = match op {
                            BinOp::Add => x + y,
                            BinOp::Sub => x - y,
                            BinOp::Mul => x * y,
                            BinOp::Div => if y == 0.0 { 0.0 } else { x / y },
                            BinOp::Min => x.min(y),
                            BinOp::Max => x.max(y),
                            BinOp::Pow => x.powf(y),
                            BinOp::Rem => rem(x, y),
                        };
                    }
                    Instr::Cmp { dst, op, a, b } => {
                        let (x, y) = (regs[a], regs[b]);
                        regs[dst] = if op.holds(x, y) { 1.0 } else { 0.0 };
                    }
                    Instr::Select { dst, cond, a, b } => regs[dst] = if regs[cond] != 0.0 { regs[a] } else { regs[b] },
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
                }
            }
            *o = regs[k.outputs[self.which].1] as f32;
        }
    }
}
