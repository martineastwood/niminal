//! User-defined opcodes. The language compiles an `opcode` body to a small
//! register program (a [`Kernel`]) which [`Custom`] runs once per sample, so
//! state carries from one sample to the next exactly as in a hand-written
//! filter.

use std::sync::Arc;

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Scalar math functions, usable inside kernels and as the [`Func`] opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fn1 {
    Sin,
    Cos,
    Tanh,
    Exp,
    Abs,
}

impl Fn1 {
    pub fn apply(self, x: f64) -> f64 {
        match self {
            Fn1::Sin => x.sin(),
            Fn1::Cos => x.cos(),
            Fn1::Tanh => x.tanh(),
            Fn1::Exp => x.exp(),
            Fn1::Abs => x.abs(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Fn1::Sin => "sin",
            Fn1::Cos => "cos",
            Fn1::Tanh => "tanh",
            Fn1::Exp => "exp",
            Fn1::Abs => "abs",
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
    Call { dst: usize, f: Fn1, a: usize },
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
    /// The register holding the opcode's result.
    pub output: usize,
}

/// A running copy of a [`Kernel`], with its own state.
#[derive(Clone)]
pub struct Custom {
    kernel: Arc<Kernel>,
    state: Vec<f64>,
    regs: Vec<f64>,
    sample_rate: f64,
}

impl Custom {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Custom {
            state: kernel.state_init.clone(),
            regs: vec![0.0; kernel.registers],
            kernel,
            sample_rate: 48_000.0,
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

    fn box_clone(&self) -> Box<dyn Opcode> {
        // A new copy starts from the kernel's initial state, not ours.
        Box::new(Custom::new(self.kernel.clone()))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(sample_rate);
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
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
                            BinOp::Div => x / y,
                        };
                    }
                    Instr::Neg { dst, a } => regs[dst] = -regs[a],
                    Instr::Call { dst, f, a } => regs[dst] = f.apply(regs[a]),
                }
            }
            *o = regs[k.output] as f32;
        }
    }
}
