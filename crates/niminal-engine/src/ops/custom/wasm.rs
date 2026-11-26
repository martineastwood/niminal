//! Prototype: emits a kernel's per-sample code as a WebAssembly module.
//!
//! This file needs only `wasm-encoder`, so it builds for any target. Running the module is
//! `wasm_run.rs`, which is native-only (a browser build would instantiate the bytes itself).
//!
//! The module imports three pure host functions (`bin`, `f1`, `wrapx`: the maths the engine
//! shares with the interpreter) and exports its memory and a `render(frames)` function that runs
//! the sample loop. Registers and state live in wasm locals; linear memory holds the block's
//! inputs and outputs, the shared tables, the state lists and the coefficient cache.
//!
//! Only kernels made entirely of inline instructions compile. A kernel that calls a built-in
//! opcode, runs a frame scope or a vector instruction stays on the interpreter.
//!
//! Rounding is exactly the interpreter's: no FMA, no reassociation, the same guarded division,
//! and the same helpers for everything that is not a wasm primitive.

use wasm_encoder::{
    BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Instruction as I, MemArg, MemArgTarget, MemorySection, MemoryType, Module, TypeSection, ValType,
};

use super::{ArraySource, BinOp, Cmp, Fn1, Instr, Interp, Kernel};

/// Host function ids for the `bin` and `f1` imports.
pub fn bin_id(op: BinOp) -> i32 {
    match op {
        BinOp::Add => 0,
        BinOp::Sub => 1,
        BinOp::Mul => 2,
        BinOp::Div => 3,
        BinOp::Min => 4,
        BinOp::Max => 5,
        BinOp::Pow => 6,
        BinOp::Atan2 => 7,
        BinOp::Rem => 8,
    }
}

pub fn bin_from_id(id: i32) -> BinOp {
    [
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::Min,
        BinOp::Max,
        BinOp::Pow,
        BinOp::Atan2,
        BinOp::Rem,
    ][id as usize]
}

pub fn fn_id(f: Fn1) -> i32 {
    Fn1::ALL.iter().position(|&g| g == f).unwrap_or(0) as i32
}

/// Where the emitted code keeps things in linear memory.
#[derive(Debug, Clone)]
pub struct Layout {
    pub regs: usize,
    pub state: usize,
    pub sample_rate: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub memo: usize,
    /// Byte offset and length of each shared table and each state list.
    pub tables: Vec<(usize, usize)>,
    pub buffers: Vec<(usize, usize)>,
    pub block: usize,
    pub n_regs: usize,
    pub n_state: usize,
    pub n_ports: usize,
    pub n_outputs: usize,
    pub pages: u64,
}

impl Layout {
    fn new(kernel: &Kernel, block: usize) -> Self {
        let n_regs = kernel.registers;
        let n_state = kernel.state_init.len();
        let n_ports = kernel.ports.len();
        let n_outputs = kernel.outputs.len();
        let regs = 0;
        let state = regs + n_regs * 8;
        let sample_rate = state + n_state * 8;
        let inputs = sample_rate + 8;
        let outputs = inputs + n_ports * block * 4;
        let memo = (outputs + n_outputs * block * 4).next_multiple_of(8);
        let mut at = memo + kernel.memo_slots * 32;
        let mut tables = Vec::new();
        for t in &kernel.tables_f64 {
            tables.push((at, t.len()));
            at += t.len() * 8;
        }
        let mut buffers = Vec::new();
        for b in &kernel.buffers {
            buffers.push((at, b.len()));
            at += b.len() * 8;
        }
        let end = at;
        Layout {
            regs,
            state,
            sample_rate,
            inputs,
            outputs,
            memo,
            tables,
            buffers,
            block,
            n_regs,
            n_state,
            n_ports,
            n_outputs,
            pages: end.div_ceil(65536).max(1) as u64,
        }
    }
}

pub struct Emitted {
    pub bytes: Vec<u8>,
    pub layout: Layout,
    /// Sample-section instructions emitted.
    pub inline: usize,
}

pub fn is_inline(instr: &Instr) -> bool {
    if let Instr::Read { list, .. } | Instr::ReadAt { list, .. } = instr {
        return matches!(list, ArraySource::State(_) | ArraySource::Table(_));
    }
    matches!(
        instr,
        Instr::Const { .. }
            | Instr::Input { .. }
            | Instr::SampleRate { .. }
            | Instr::LoadState { .. }
            | Instr::StoreState { .. }
            | Instr::Bin { .. }
            | Instr::Neg { .. }
            | Instr::Copy { .. }
            | Instr::MulAdd { .. }
            | Instr::MulSub { .. }
            | Instr::SubMul { .. }
            | Instr::Clamp { .. }
            | Instr::Cmp { .. }
            | Instr::Select { .. }
            | Instr::Call { .. }
            | Instr::Memo { .. }
            | Instr::Write { .. }
            | Instr::SkipIfZero { .. }
            | Instr::CmpSkip { .. }
    )
}

/// Why a kernel cannot be emitted.
pub fn supported(kernel: &Kernel) -> Result<(), String> {
    if !kernel.resets.is_empty() {
        return Err("reset scopes".into());
    }
    if kernel.code.iter().any(|i| {
        matches!(
            i,
            Instr::Reset { .. } | Instr::BeginBranch { .. } | Instr::EndBranch
        )
    }) {
        return Err("unresolved branch".into());
    }
    if let Some(instr) = kernel.code.iter().find(|i| !is_inline(i)) {
        let name = format!("{instr:?}");
        return Err(format!(
            "{} needs the interpreter",
            name.split([' ', '{']).next().unwrap_or("an instruction")
        ));
    }
    // State lists live in wasm memory while it renders, so only instructions emitted inline may
    // touch them in the sample section, and nothing else may run against the interpreter's copy.
    let touches_list = |i: &Instr| {
        matches!(
            i,
            Instr::Write { .. } | Instr::LaneStore { .. } | Instr::Fir { .. } | Instr::Taps { .. }
        ) || format!("{i:?}").contains("State(")
    };
    if !kernel.buffers.is_empty()
        && (kernel.block.iter().any(touches_list)
            || kernel.frames.iter().any(|f| f.code.iter().any(touches_list)))
    {
        return Err("a state list is used outside the sample section".into());
    }
    Ok(())
}

// Local indices: 0 frames (i32), 1 sample counter (i32), 2..=4 f64 temporaries, then registers
// and state slots when they live in locals.
const FRAMES: u32 = 0;
const SAMPLE: u32 = 1;
// 2 and 3 are i64 temporaries (memo keys)
const K0: u32 = 2;
const K1: u32 = 3;
const T0: u32 = 4;
const T1: u32 = 5;
const T2: u32 = 6;
const FIRST: u32 = 7;

const WRAP: u32 = 4;
const READ_LINEAR: u32 = 5;
const READ_CUBIC: u32 = 6;
const BIN: u32 = 0;
const F1: u32 = 1;
const WRAPX: u32 = 2;

struct Emit<'a> {
    f: Function,
    layout: &'a Layout,
    /// Registers whose value stays on the wasm operand stack from its definition to its one use.
    stack: std::collections::HashSet<usize>,
}

fn mem(offset: usize) -> MemArg {
    MemArg {
        offset: offset as u64,
        align: 3,
        target: MemArgTarget::Memory(0),
        ordering: None,
    }
}

impl Emit<'_> {
    fn reg_local(&self, r: usize) -> u32 {
        FIRST + r as u32
    }
    fn state_local(&self, s: usize) -> u32 {
        FIRST + (self.layout.n_regs + s) as u32
    }
    /// Pushes register `r`.
    fn get(&mut self, r: usize) {
        if !self.stack.contains(&r) {
            self.f.instruction(&I::LocalGet(self.reg_local(r)));
        }
    }
    /// Pops the value into register `r`.
    fn end_set(&mut self, r: usize) {
        if !self.stack.contains(&r) {
            self.f.instruction(&I::LocalSet(self.reg_local(r)));
        }
    }
    fn get_state(&mut self, s: usize) {
        self.f.instruction(&I::LocalGet(self.state_local(s)));
    }
    fn set_state(&mut self, s: usize) {
        self.f.instruction(&I::LocalSet(self.state_local(s)));
    }

    fn cmp_op(&mut self, op: Cmp) {
        self.f.instruction(&match op {
            Cmp::Lt => I::F64Lt,
            Cmp::Gt => I::F64Gt,
            Cmp::Le => I::F64Le,
            Cmp::Ge => I::F64Ge,
            Cmp::Eq => I::F64Eq,
            Cmp::Ne => I::F64Ne,
        });
    }

    /// Rust's `f64::min`/`max`: a NaN operand yields the other one. Takes the operands from the
    /// stack and leaves the result.
    fn min_max(&mut self, max: bool) {
        self.f.instruction(&I::LocalSet(T1));
        self.f.instruction(&I::LocalSet(T0));
        self.f.instruction(&I::LocalGet(T0));
        self.f.instruction(&I::LocalGet(T1));
        self.f.instruction(&if max { I::F64Max } else { I::F64Min });
        self.f.instruction(&I::LocalSet(T2));
        // fallback = if a is NaN { b } else { a }
        self.f.instruction(&I::LocalGet(T1));
        self.f.instruction(&I::LocalGet(T0));
        self.f.instruction(&I::LocalGet(T0));
        self.f.instruction(&I::LocalGet(T0));
        self.f.instruction(&I::F64Ne);
        self.f.instruction(&I::Select);
        // result = if r is NaN { fallback } else { r }
        self.f.instruction(&I::LocalGet(T2));
        self.f.instruction(&I::LocalGet(T2));
        self.f.instruction(&I::LocalGet(T2));
        self.f.instruction(&I::F64Ne);
        self.f.instruction(&I::Select);
    }

    /// `if b != 0 { a / b } else { 0 }`, operands on the stack.
    fn guarded_div(&mut self) {
        self.f.instruction(&I::LocalSet(T1));
        self.f.instruction(&I::LocalSet(T0));
        self.f.instruction(&I::LocalGet(T0));
        self.f.instruction(&I::LocalGet(T1));
        self.f.instruction(&I::F64Div);
        self.f.instruction(&I::F64Const(0.0.into()));
        self.f.instruction(&I::LocalGet(T1));
        self.f.instruction(&I::F64Const(0.0.into()));
        self.f.instruction(&I::F64Ne);
        self.f.instruction(&I::Select);
    }

    fn call_bin(&mut self, op: BinOp) {
        // stack: a b
        let a = (T0, T1);
        self.f.instruction(&I::LocalSet(a.1));
        self.f.instruction(&I::LocalSet(a.0));
        self.f.instruction(&I::I32Const(bin_id(op)));
        self.f.instruction(&I::LocalGet(a.0));
        self.f.instruction(&I::LocalGet(a.1));
        self.f.instruction(&I::Call(BIN));
    }

    fn call_fn1(&mut self, f: Fn1) {
        self.f.instruction(&I::LocalSet(T0));
        self.f.instruction(&I::I32Const(fn_id(f)));
        self.f.instruction(&I::LocalGet(T0));
        self.f.instruction(&I::Call(F1));
    }

    fn fn1(&mut self, f: Fn1) {
        match f {
            Fn1::Abs => {
                self.f.instruction(&I::F64Abs);
            }
            Fn1::Floor => {
                self.f.instruction(&I::F64Floor);
            }
            Fn1::Ceil => {
                self.f.instruction(&I::F64Ceil);
            }
            Fn1::Sqrt => {
                self.f.instruction(&I::F64Const(0.0.into()));
                self.min_max(true);
                self.f.instruction(&I::F64Sqrt);
            }
            Fn1::Fract => {
                self.f.instruction(&I::LocalTee(T0));
                self.f.instruction(&I::LocalGet(T0));
                self.f.instruction(&I::F64Floor);
                self.f.instruction(&I::F64Sub);
            }
            Fn1::Sign => {
                self.f.instruction(&I::LocalSet(T0));
                // x > 0 ? 1 : (x < 0 ? -1 : 0)
                self.f.instruction(&I::F64Const(1.0.into()));
                self.f.instruction(&I::F64Const((-1.0).into()));
                self.f.instruction(&I::F64Const(0.0.into()));
                self.f.instruction(&I::LocalGet(T0));
                self.f.instruction(&I::F64Const(0.0.into()));
                self.f.instruction(&I::F64Lt);
                self.f.instruction(&I::Select);
                self.f.instruction(&I::LocalGet(T0));
                self.f.instruction(&I::F64Const(0.0.into()));
                self.f.instruction(&I::F64Gt);
                self.f.instruction(&I::Select);
            }
            f => self.call_fn1(f),
        }
    }
}

/// Distinct skip targets (instruction indices) in the code, or an error for a bad skip.
fn skip_targets(code: &[Instr]) -> Result<Vec<usize>, String> {
    let mut targets = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        if let Instr::SkipIfZero { count, .. } | Instr::CmpSkip { count, .. } = *instr
            && count > 0
        {
            let t = i + 1 + count;
            if t > code.len() {
                return Err("skip past the end".into());
            }
            targets.push(t);
        }
    }
    targets.sort_unstable();
    targets.dedup();
    Ok(targets)
}

pub fn emit(kernel: &Kernel, block: usize) -> Result<Emitted, String> {
    supported(kernel)?;
    let layout = Layout::new(kernel, block);
    for instr in &kernel.code {
        let regs: Vec<usize> = match *instr {
            Instr::Const { dst, .. } | Instr::SampleRate { dst } | Instr::Input { dst, .. } => {
                vec![dst]
            }
            Instr::LoadState { dst, .. } => vec![dst],
            Instr::StoreState { src, .. } => vec![src],
            Instr::Copy { dst, src } => vec![dst, src],
            Instr::Neg { dst, a } | Instr::Call { dst, a, .. } => vec![dst, a],
            Instr::Bin { dst, a, b, .. }
            | Instr::Cmp { dst, a, b, .. }
            | Instr::CmpSkip { dst, a, b, .. } => vec![dst, a, b],
            Instr::MulAdd { dst, a, b, c }
            | Instr::MulSub { dst, a, b, c }
            | Instr::SubMul { dst, a, b, c } => vec![dst, a, b, c],
            Instr::Clamp { dst, x, lo, hi } => vec![dst, x, lo, hi],
            Instr::Select { dst, cond, a, b } => vec![dst, cond, a, b],
            Instr::SkipIfZero { cond, .. } => vec![cond],
            _ => vec![],
        };
        if regs.iter().any(|&r| r >= kernel.registers) {
            return Err("register out of range".into());
        }
    }
    for (_, reg) in &kernel.outputs {
        if *reg >= kernel.registers {
            return Err("output register out of range".into());
        }
    }

    let mut types = TypeSection::new();
    // 0: bin(op, a, b) -> f64; 1: f1(fn, x) -> f64; 2: render(frames); 3: wrap(x, n) -> i32;
    // 4: read(base, n, pos) -> f64
    types
        .ty()
        .function([ValType::I32, ValType::F64, ValType::F64], [ValType::F64]);
    types
        .ty()
        .function([ValType::I32, ValType::F64], [ValType::F64]);
    types.ty().function([ValType::I32], []);
    types
        .ty()
        .function([ValType::F64, ValType::F64], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::F64, ValType::F64], [ValType::F64]);
    let mut imports = ImportSection::new();
    imports.import("env", "bin", wasm_encoder::EntityType::Function(0));
    imports.import("env", "f1", wasm_encoder::EntityType::Function(1));
    imports.import("env", "wrapx", wasm_encoder::EntityType::Function(3));
    let mut functions = FunctionSection::new();
    // function indices: imports 0..3, then 3 render, 4 wrap, 5 read_linear, 6 read_cubic
    functions.function(2);
    functions.function(3);
    functions.function(4);
    functions.function(4);
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: layout.pages,
        maximum: Some(layout.pages),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let mut exports = ExportSection::new();
    exports.export("mem", ExportKind::Memory, 0);
    exports.export("render", ExportKind::Func, 3);

    let targets = skip_targets(&kernel.code)?;
    let n_locals = (kernel.registers + kernel.state_init.len()) as u32;
    let locals = vec![(1, ValType::I32), (2, ValType::I64), (3 + n_locals, ValType::F64)];
    let mut e = Emit {
        f: Function::new(locals),
        layout: &layout,
        stack: plan_stack(kernel, &targets),
    };
    let _ = FRAMES;

    {
        for r in 0..kernel.registers {
            e.f.instruction(&I::I32Const(0));
            e.f.instruction(&I::F64Load(mem(layout.regs + r * 8)));
            e.f.instruction(&I::LocalSet(e.reg_local(r)));
        }
        for s in 0..kernel.state_init.len() {
            e.f.instruction(&I::I32Const(0));
            e.f.instruction(&I::F64Load(mem(layout.state + s * 8)));
            e.f.instruction(&I::LocalSet(e.state_local(s)));
        }
    }

    e.f.instruction(&I::I32Const(0));
    e.f.instruction(&I::LocalSet(SAMPLE));
    e.f.instruction(&I::Block(BlockType::Empty)); // done
    e.f.instruction(&I::Loop(BlockType::Empty)); // top
    e.f.instruction(&I::LocalGet(SAMPLE));
    e.f.instruction(&I::LocalGet(FRAMES));
    e.f.instruction(&I::I32GeU);
    e.f.instruction(&I::BrIf(1));

    for _ in &targets {
        e.f.instruction(&I::Block(BlockType::Empty));
    }
    let mut open: Vec<usize> = targets.clone(); // ascending; innermost is the smallest
    let depth_of = |open: &[usize], t: usize| open.iter().filter(|&&o| o < t).count() as u32;

    for (index, instr) in kernel.code.iter().enumerate() {
        while open.first() == Some(&index) {
            e.f.instruction(&I::End);
            open.remove(0);
        }
        match *instr {
            Instr::Const { dst, value } => {
                        e.f.instruction(&I::F64Const(value.into()));
                e.end_set(dst);
            }
            Instr::Input { dst, port } => {
                if port >= layout.n_ports {
                    return Err("input port out of range".into());
                }
                        e.f.instruction(&I::LocalGet(SAMPLE));
                e.f.instruction(&I::I32Const(2));
                e.f.instruction(&I::I32Shl);
                e.f.instruction(&I::F32Load(MemArg {
                    offset: (layout.inputs + port * block * 4) as u64,
                    align: 2,
                    target: MemArgTarget::Memory(0),
        ordering: None,
                }));
                e.f.instruction(&I::F64PromoteF32);
                e.end_set(dst);
            }
            Instr::SampleRate { dst } => {
                        e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::F64Load(mem(layout.sample_rate)));
                e.end_set(dst);
            }
            Instr::LoadState { dst, slot } => {
                if slot >= layout.n_state {
                    return Err("state slot out of range".into());
                }
                        e.get_state(slot);
                e.end_set(dst);
            }
            Instr::StoreState { slot, src } => {
                if slot >= layout.n_state {
                    return Err("state slot out of range".into());
                }
                e.get(src);
                e.set_state(slot);
            }
            Instr::Copy { dst, src } => {
                        e.get(src);
                e.end_set(dst);
            }
            Instr::Neg { dst, a } => {
                        e.get(a);
                e.f.instruction(&I::F64Neg);
                e.end_set(dst);
            }
            Instr::Bin { dst, op, a, b } => {
                        e.get(a);
                e.get(b);
                match op {
                    BinOp::Add => {
                        e.f.instruction(&I::F64Add);
                    }
                    BinOp::Sub => {
                        e.f.instruction(&I::F64Sub);
                    }
                    BinOp::Mul => {
                        e.f.instruction(&I::F64Mul);
                    }
                    BinOp::Div => e.guarded_div(),
                    BinOp::Min => e.min_max(false),
                    BinOp::Max => e.min_max(true),
                    BinOp::Pow | BinOp::Atan2 | BinOp::Rem => e.call_bin(op),
                }
                e.end_set(dst);
            }
            Instr::MulAdd { dst, a, b, c } => {
                        e.get(a);
                e.get(b);
                e.f.instruction(&I::F64Mul);
                e.get(c);
                e.f.instruction(&I::F64Add);
                e.end_set(dst);
            }
            Instr::MulSub { dst, a, b, c } => {
                        e.get(a);
                e.get(b);
                e.f.instruction(&I::F64Mul);
                e.get(c);
                e.f.instruction(&I::F64Sub);
                e.end_set(dst);
            }
            Instr::SubMul { dst, a, b, c } => {
                        e.get(c);
                e.get(a);
                e.get(b);
                e.f.instruction(&I::F64Mul);
                e.f.instruction(&I::F64Sub);
                e.end_set(dst);
            }
            Instr::Clamp { dst, x, lo, hi } => {
                        e.get(x);
                e.get(lo);
                e.min_max(true);
                e.get(hi);
                e.min_max(false);
                e.end_set(dst);
            }
            Instr::Cmp { dst, op, a, b } => {
                        e.get(a);
                e.get(b);
                e.cmp_op(op);
                e.f.instruction(&I::F64ConvertI32U);
                e.end_set(dst);
            }
            Instr::Select { dst, cond, a, b } => {
                        e.get(a);
                e.get(b);
                e.get(cond);
                e.f.instruction(&I::F64Const(0.0.into()));
                e.f.instruction(&I::F64Ne);
                e.f.instruction(&I::Select);
                e.end_set(dst);
            }
            Instr::Call { dst, f, a } => {
                        e.get(a);
                e.fn1(f);
                e.end_set(dst);
            }
            Instr::Read { dst, list, idx }
                if matches!(list, ArraySource::State(_) | ArraySource::Table(_)) =>
            {
                let (base, len) = list_at(&layout, list)?;
                        e.get(idx);
                e.f.instruction(&I::F64Const((len as f64).into()));
                e.f.instruction(&I::Call(WRAP));
                e.f.instruction(&I::I32Const(3));
                e.f.instruction(&I::I32Shl);
                e.f.instruction(&I::F64Load(mem(base)));
                e.end_set(dst);
            }
            Instr::ReadAt {
                dst,
                list,
                pos,
                interp,
            } if matches!(list, ArraySource::State(_) | ArraySource::Table(_)) => {
                let (base, len) = list_at(&layout, list)?;
                        match interp {
                    Interp::None => {
                        e.get(pos);
                        e.f.instruction(&I::F64Const((len as f64).into()));
                        e.f.instruction(&I::Call(WRAP));
                        e.f.instruction(&I::I32Const(3));
                        e.f.instruction(&I::I32Shl);
                        e.f.instruction(&I::F64Load(mem(base)));
                    }
                    Interp::Linear | Interp::Cubic => {
                        e.f.instruction(&I::I32Const(base as i32));
                        e.f.instruction(&I::F64Const((len as f64).into()));
                        e.get(pos);
                        e.f.instruction(&I::Call(if interp == Interp::Linear {
                            READ_LINEAR
                        } else {
                            READ_CUBIC
                        }));
                    }
                }
                e.end_set(dst);
            }
            Instr::Write {
                list,
                idx,
                src,
                cond,
            } => {
                let Some(&(base, len)) = layout.buffers.get(list) else {
                    return Err("state list out of range".into());
                };
                if let Some(c) = cond {
                    e.get(c);
                    e.f.instruction(&I::F64Const(0.0.into()));
                    e.f.instruction(&I::F64Ne);
                    e.f.instruction(&I::If(BlockType::Empty));
                }
                e.get(idx);
                e.f.instruction(&I::F64Const((len as f64).into()));
                e.f.instruction(&I::Call(WRAP));
                e.f.instruction(&I::I32Const(3));
                e.f.instruction(&I::I32Shl);
                e.get(src);
                e.f.instruction(&I::F64Store(mem(base)));
                if cond.is_some() {
                    e.f.instruction(&I::End);
                }
            }
            Instr::Memo { dst, slot, math, a } => {
                if slot >= kernel.memo_slots {
                    return Err("memo slot out of range".into());
                }
                let at = layout.memo + slot * 32;
                let word = |k: usize| MemArg {
                    offset: (at + k * 8) as u64,
                    align: 3,
                    target: MemArgTarget::Memory(0),
                    ordering: None,
                };
                        // key = bits of the operands; a hit needs the slot filled and both keys equal
                e.get(a);
                e.f.instruction(&I::I64ReinterpretF64);
                e.f.instruction(&I::LocalSet(K0));
                match math {
                    super::MemoMath::Unary(_) => {
                        e.f.instruction(&I::I64Const(0));
                    }
                    super::MemoMath::Binary(_, b) => {
                        e.get(b);
                        e.f.instruction(&I::I64ReinterpretF64);
                    }
                }
                e.f.instruction(&I::LocalSet(K1));
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::I64Load(word(2)));
                e.f.instruction(&I::I64Const(0));
                e.f.instruction(&I::I64Ne);
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::I64Load(word(0)));
                e.f.instruction(&I::LocalGet(K0));
                e.f.instruction(&I::I64Eq);
                e.f.instruction(&I::I32And);
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::I64Load(word(1)));
                e.f.instruction(&I::LocalGet(K1));
                e.f.instruction(&I::I64Eq);
                e.f.instruction(&I::I32And);
                e.f.instruction(&I::If(BlockType::Result(ValType::F64)));
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::F64Load(word(3)));
                e.f.instruction(&I::Else);
                match math {
                    super::MemoMath::Unary(f) => {
                        e.get(a);
                        e.fn1(f);
                    }
                    super::MemoMath::Binary(op, b) => {
                        e.get(a);
                        e.get(b);
                        match op {
                            BinOp::Add => {
                                e.f.instruction(&I::F64Add);
                            }
                            BinOp::Sub => {
                                e.f.instruction(&I::F64Sub);
                            }
                            BinOp::Mul => {
                                e.f.instruction(&I::F64Mul);
                            }
                            BinOp::Div => e.guarded_div(),
                            BinOp::Min => e.min_max(false),
                            BinOp::Max => e.min_max(true),
                            BinOp::Pow | BinOp::Atan2 | BinOp::Rem => e.call_bin(op),
                        }
                    }
                }
                e.f.instruction(&I::LocalSet(T2));
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::LocalGet(K0));
                e.f.instruction(&I::I64Store(word(0)));
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::LocalGet(K1));
                e.f.instruction(&I::I64Store(word(1)));
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::I64Const(1));
                e.f.instruction(&I::I64Store(word(2)));
                e.f.instruction(&I::I32Const(0));
                e.f.instruction(&I::LocalGet(T2));
                e.f.instruction(&I::F64Store(word(3)));
                e.f.instruction(&I::LocalGet(T2));
                e.f.instruction(&I::End);
                e.end_set(dst);
            }
            Instr::SkipIfZero { cond, count } => {
                if count > 0 {
                    e.get(cond);
                    e.f.instruction(&I::F64Const(0.0.into()));
                    e.f.instruction(&I::F64Eq);
                    e.f.instruction(&I::BrIf(depth_of(&open, index + 1 + count)));
                }
            }
            Instr::CmpSkip {
                dst,
                op,
                a,
                b,
                count,
            } => {
                        e.get(a);
                e.get(b);
                e.cmp_op(op);
                e.f.instruction(&I::F64ConvertI32U);
                e.end_set(dst);
                if count > 0 {
                    // skip when the stored result is zero
                    e.get(dst);
                    e.f.instruction(&I::F64Const(0.0.into()));
                    e.f.instruction(&I::F64Eq);
                    e.f.instruction(&I::BrIf(depth_of(&open, index + 1 + count)));
                }
            }
            _ => return Err("instruction needs the interpreter".into()),
        }
    }
    while !open.is_empty() {
        e.f.instruction(&I::End);
        open.remove(0);
    }
    for (k, (_, reg)) in kernel.outputs.iter().enumerate() {
        e.f.instruction(&I::LocalGet(SAMPLE));
        e.f.instruction(&I::I32Const(2));
        e.f.instruction(&I::I32Shl);
        e.get(*reg);
        e.f.instruction(&I::F32DemoteF64);
        e.f.instruction(&I::F32Store(MemArg {
            offset: (layout.outputs + k * block * 4) as u64,
            align: 2,
            target: MemArgTarget::Memory(0),
        ordering: None,
        }));
    }
    e.f.instruction(&I::LocalGet(SAMPLE));
    e.f.instruction(&I::I32Const(1));
    e.f.instruction(&I::I32Add);
    e.f.instruction(&I::LocalSet(SAMPLE));
    e.f.instruction(&I::Br(0));
    e.f.instruction(&I::End); // loop
    e.f.instruction(&I::End); // block

    {
        for r in 0..kernel.registers {
            e.f.instruction(&I::I32Const(0));
            e.f.instruction(&I::LocalGet(e.reg_local(r)));
            e.f.instruction(&I::F64Store(mem(layout.regs + r * 8)));
        }
        for s in 0..kernel.state_init.len() {
            e.f.instruction(&I::I32Const(0));
            e.f.instruction(&I::LocalGet(e.state_local(s)));
            e.f.instruction(&I::F64Store(mem(layout.state + s * 8)));
        }
    }
    e.f.instruction(&I::End);

    let mut code = CodeSection::new();
    code.function(&e.f);
    code.function(&wrap_function());
    code.function(&read_function(false));
    code.function(&read_function(true));
    let mut module = Module::new();
    module
        .section(&types)
        .section(&imports)
        .section(&functions)
        .section(&memories)
        .section(&exports)
        .section(&code);
    Ok(Emitted {
        bytes: module.finish(),
        inline: kernel.code.len(),
        layout,
    })
}

fn list_at(layout: &Layout, list: ArraySource) -> Result<(usize, usize), String> {
    match list {
        ArraySource::State(s) => layout.buffers.get(s).copied(),
        ArraySource::Table(t) => layout.tables.get(t).copied(),
        _ => None,
    }
    .filter(|&(_, len)| len > 0)
    .ok_or_else(|| "empty or unknown list".to_string())
}

/// `wrap(x, n)`: [`crate::wrapped_index`] for the usual cases; the rest ask the host.
fn wrap_function() -> Function {
    let mut f = Function::new([(1, ValType::F64)]);
    let (x, n, fl) = (0, 1, 2);
    let b = BlockType::Result(ValType::I32);
    f.instruction(&I::LocalGet(x));
    f.instruction(&I::F64Floor);
    f.instruction(&I::LocalSet(fl));
    // 0 <= floor < n
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::F64Const(0.0.into()));
    f.instruction(&I::F64Ge);
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Lt);
    f.instruction(&I::I32And);
    f.instruction(&I::If(b));
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::I32TruncSatF64U);
    f.instruction(&I::Else);
    // floor < 0 and 0 <= floor + n < n
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::F64Const(0.0.into()));
    f.instruction(&I::F64Lt);
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Add);
    f.instruction(&I::F64Const(0.0.into()));
    f.instruction(&I::F64Ge);
    f.instruction(&I::I32And);
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Add);
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Lt);
    f.instruction(&I::I32And);
    f.instruction(&I::If(b));
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Add);
    f.instruction(&I::I32TruncSatF64U);
    f.instruction(&I::Else);
    // floor >= n and floor - n < n
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Ge);
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Sub);
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Lt);
    f.instruction(&I::I32And);
    f.instruction(&I::If(b));
    f.instruction(&I::LocalGet(fl));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::F64Sub);
    f.instruction(&I::I32TruncSatF64U);
    f.instruction(&I::Else);
    f.instruction(&I::LocalGet(x));
    f.instruction(&I::LocalGet(n));
    f.instruction(&I::Call(WRAPX));
    f.instruction(&I::End);
    f.instruction(&I::End);
    f.instruction(&I::End);
    f.instruction(&I::End);
    f
}

/// `read(base, n, pos)`: [`super::read_array`] over `n` doubles at byte offset `base`.
fn read_function(cubic: bool) -> Function {
    // params: 0 base (i32), 1 n, 2 pos; locals: 3 b, 4 t, 5..8 y0..y3 (f64)
    let mut f = Function::new([(9, ValType::F64)]);
    let (base, n, pos, b, t, y) = (0, 1, 2, 3, 4, 5);
    let el = |f: &mut Function, index_local_k: f64| {
        // element(wrap(b + k)); the address is base + 8 * index
        f.instruction(&I::LocalGet(base));
        f.instruction(&I::LocalGet(b));
        f.instruction(&I::F64Const(index_local_k.into()));
        f.instruction(&I::F64Add);
        f.instruction(&I::LocalGet(n));
        f.instruction(&I::Call(WRAP));
        f.instruction(&I::I32Const(3));
        f.instruction(&I::I32Shl);
        f.instruction(&I::I32Add);
        f.instruction(&I::F64Load(mem(0)));
    };
    // a non-finite position reads element 0
    f.instruction(&I::LocalGet(pos));
    f.instruction(&I::LocalGet(pos));
    f.instruction(&I::F64Sub);
    f.instruction(&I::F64Const(0.0.into()));
    f.instruction(&I::F64Ne);
    f.instruction(&I::If(BlockType::Empty));
    f.instruction(&I::LocalGet(base));
    f.instruction(&I::F64Load(mem(0)));
    f.instruction(&I::Return);
    f.instruction(&I::End);
    f.instruction(&I::LocalGet(pos));
    f.instruction(&I::F64Floor);
    f.instruction(&I::LocalSet(b));
    f.instruction(&I::LocalGet(pos));
    f.instruction(&I::LocalGet(b));
    f.instruction(&I::F64Sub);
    f.instruction(&I::LocalSet(t));
    if !cubic {
        // 0 <= b and b + 1 < n: two neighbours without wrapping
        f.instruction(&I::LocalGet(b));
        f.instruction(&I::F64Const(0.0.into()));
        f.instruction(&I::F64Ge);
        f.instruction(&I::LocalGet(b));
        f.instruction(&I::F64Const(1.0.into()));
        f.instruction(&I::F64Add);
        f.instruction(&I::LocalGet(n));
        f.instruction(&I::F64Lt);
        f.instruction(&I::I32And);
        f.instruction(&I::If(BlockType::Empty));
        el(&mut f, 0.0);
        f.instruction(&I::LocalSet(y));
        el(&mut f, 1.0);
        f.instruction(&I::LocalSet(y + 1));
        f.instruction(&I::Else);
        el(&mut f, 0.0);
        f.instruction(&I::LocalSet(y));
        el(&mut f, 1.0);
        f.instruction(&I::LocalSet(y + 1));
        f.instruction(&I::End);
        // y0 + (y1 - y0) * t
        f.instruction(&I::LocalGet(y));
        f.instruction(&I::LocalGet(y + 1));
        f.instruction(&I::LocalGet(y));
        f.instruction(&I::F64Sub);
        f.instruction(&I::LocalGet(t));
        f.instruction(&I::F64Mul);
        f.instruction(&I::F64Add);
    } else {
        for (k, off) in [-1.0, 0.0, 1.0, 2.0].into_iter().enumerate() {
            el(&mut f, off);
            f.instruction(&I::LocalSet(y + k as u32));
        }
        let (y0, y1, y2, y3) = (y, y + 1, y + 2, y + 3);
        let term = |f: &mut Function, c: f64, v: u32| {
            f.instruction(&I::F64Const(c.into()));
            f.instruction(&I::LocalGet(v));
            f.instruction(&I::F64Mul);
        };
        // a = -0.5*y0 + 1.5*y1 - 1.5*y2 + 0.5*y3, left to right
        term(&mut f, -0.5, y0);
        term(&mut f, 1.5, y1);
        f.instruction(&I::F64Add);
        term(&mut f, 1.5, y2);
        f.instruction(&I::F64Sub);
        term(&mut f, 0.5, y3);
        f.instruction(&I::F64Add);
        f.instruction(&I::LocalSet(y + 4)); // a
        // b = y0 - 2.5*y1 + 2.0*y2 - 0.5*y3
        f.instruction(&I::LocalGet(y0));
        term(&mut f, 2.5, y1);
        f.instruction(&I::F64Sub);
        term(&mut f, 2.0, y2);
        f.instruction(&I::F64Add);
        term(&mut f, 0.5, y3);
        f.instruction(&I::F64Sub);
        f.instruction(&I::LocalSet(y + 5)); // b
        // c = -0.5*y0 + 0.5*y2
        term(&mut f, -0.5, y0);
        term(&mut f, 0.5, y2);
        f.instruction(&I::F64Add);
        f.instruction(&I::LocalSet(y + 6)); // c
        // ((a*t + b)*t + c)*t + y1
        f.instruction(&I::LocalGet(y + 4));
        f.instruction(&I::LocalGet(t));
        f.instruction(&I::F64Mul);
        f.instruction(&I::LocalGet(y + 5));
        f.instruction(&I::F64Add);
        f.instruction(&I::LocalGet(t));
        f.instruction(&I::F64Mul);
        f.instruction(&I::LocalGet(y + 6));
        f.instruction(&I::F64Add);
        f.instruction(&I::LocalGet(t));
        f.instruction(&I::F64Mul);
        f.instruction(&I::LocalGet(y1));
        f.instruction(&I::F64Add);
    }
    f.instruction(&I::End);
    f
}

/// What an inline instruction reads and writes, in the order its emitted code pushes operands.
struct Info {
    /// Operands pushed in this order, which may already be on the stack.
    ops: Vec<usize>,
    /// Other reads, which always come from locals.
    local_reads: Vec<usize>,
    dst: Option<usize>,
    /// A register that cannot live on the stack (its code reads it twice or out of order).
    forced: Vec<usize>,
}

fn info(instr: &Instr) -> Option<Info> {
    let plain = |ops: Vec<usize>, dst: Option<usize>| Info {
        ops,
        local_reads: vec![],
        dst,
        forced: vec![],
    };
    Some(match *instr {
        Instr::Const { dst, .. }
        | Instr::SampleRate { dst }
        | Instr::Input { dst, .. }
        | Instr::LoadState { dst, .. } => plain(vec![], Some(dst)),
        Instr::StoreState { src, .. } => plain(vec![src], None),
        Instr::Copy { dst, src } => plain(vec![src], Some(dst)),
        Instr::Neg { dst, a } | Instr::Call { dst, a, .. } => plain(vec![a], Some(dst)),
        Instr::Bin { dst, a, b, .. } | Instr::Cmp { dst, a, b, .. } => {
            plain(vec![a, b], Some(dst))
        }
        Instr::MulAdd { dst, a, b, c } | Instr::MulSub { dst, a, b, c } => {
            plain(vec![a, b, c], Some(dst))
        }
        Instr::SubMul { dst, a, b, c } => plain(vec![c, a, b], Some(dst)),
        Instr::Clamp { dst, x, lo, hi } => plain(vec![x, lo, hi], Some(dst)),
        Instr::Select { dst, cond, a, b } => plain(vec![a, b, cond], Some(dst)),
        Instr::SkipIfZero { cond, .. } => plain(vec![cond], None),
        Instr::CmpSkip { dst, a, b, .. } => Info {
            ops: vec![a, b],
            local_reads: vec![],
            dst: Some(dst),
            forced: vec![dst],
        },
        Instr::Memo { dst, math, a, .. } => {
            let mut reads = vec![a];
            if let super::MemoMath::Binary(_, b) = math {
                reads.push(b);
            }
            Info {
                ops: vec![],
                local_reads: reads.clone(),
                dst: Some(dst),
                forced: reads,
            }
        }
        Instr::Read { dst, idx, .. } => plain(vec![idx], Some(dst)),
        Instr::ReadAt {
            dst, pos, interp, ..
        } => {
            if interp == Interp::None {
                plain(vec![pos], Some(dst))
            } else {
                Info {
                    ops: vec![],
                    local_reads: vec![pos],
                    dst: Some(dst),
                    forced: vec![pos],
                }
            }
        }
        Instr::Write {
            idx, src, cond, ..
        } => match cond {
            Some(c) => Info {
                ops: vec![],
                local_reads: vec![c, idx, src],
                dst: None,
                forced: vec![c, idx, src],
            },
            None => Info {
                ops: vec![idx],
                local_reads: vec![src],
                dst: None,
                forced: vec![src],
            },
        },
        _ => return None,
    })
}

/// Registers that can stay on the wasm operand stack instead of a local: written once, read once
/// later in the sample code, and consumed in the order the stack holds them with no block
/// boundary between. Where the emitted code would use such a register in some other way, it stays
/// a local, so this only ever removes `local.set`/`local.get` pairs.
fn plan_stack(kernel: &Kernel, targets: &[usize]) -> std::collections::HashSet<usize> {
    use std::collections::{HashMap, HashSet};
    let mut reads: HashMap<usize, usize> = HashMap::new();
    let mut defs: HashMap<usize, usize> = HashMap::new();
    let mut forced: HashSet<usize> = HashSet::new();
    for section in [&kernel.init, &kernel.block, &kernel.code] {
        for instr in section {
            let Some(i) = info(instr) else {
                return HashSet::new();
            };
            for r in i.ops.iter().chain(&i.local_reads) {
                *reads.entry(*r).or_default() += 1;
            }
            if let Some(d) = i.dst {
                *defs.entry(d).or_default() += 1;
            }
            forced.extend(i.forced);
        }
    }
    for (_, r) in &kernel.outputs {
        *reads.entry(*r).or_default() += 1;
    }
    // defined once, in the sample code, and read exactly once
    let mut candidates: HashSet<usize> = HashSet::new();
    let sample_defs: HashSet<usize> = kernel
        .code
        .iter()
        .filter_map(|i| info(i).and_then(|i| i.dst))
        .collect();
    for (&r, &n) in &reads {
        if n == 1
            && defs.get(&r) == Some(&1)
            && sample_defs.contains(&r)
            && !forced.contains(&r)
            && !kernel.init.iter().chain(&kernel.block).any(|i| {
                info(i).is_some_and(|i| i.ops.iter().chain(&i.local_reads).any(|&x| x == r))
            })
        {
            candidates.insert(r);
        }
    }
    // Simulate the stack; whatever cannot be where the code needs it goes back to a local.
    'again: loop {
        let mut pending: Vec<usize> = Vec::new();
        for (index, instr) in kernel.code.iter().enumerate() {
            if targets.contains(&index) && !pending.is_empty() {
                for r in pending.drain(..) {
                    candidates.remove(&r);
                }
                continue 'again;
            }
            let Some(i) = info(instr) else {
                return HashSet::new();
            };
            let on_stack: Vec<usize> = i
                .ops
                .iter()
                .copied()
                .filter(|r| candidates.contains(r))
                .collect();
            let k = on_stack.len();
            let n = i.ops.len();
            let prefix_ok = i.ops[..k].iter().all(|r| candidates.contains(r))
                && pending.len() >= k
                && pending[pending.len() - k..] == i.ops[..k];
            // an operand that is not on the stack must have been defined earlier (so it is
            // pending) — otherwise the candidate was never produced here
            let defined_before = on_stack.iter().all(|r| pending.contains(r));
            if !(prefix_ok && defined_before) || n < k {
                for r in on_stack {
                    candidates.remove(&r);
                }
                continue 'again;
            }
            pending.truncate(pending.len() - k);
            if let Some(d) = i.dst
                && candidates.contains(&d)
            {
                pending.push(d);
            }
        }
        if !pending.is_empty() {
            for r in pending {
                candidates.remove(&r);
            }
            continue 'again;
        }
        return candidates;
    }
}
