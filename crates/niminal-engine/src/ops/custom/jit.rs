//! Spike: a hand-written aarch64 code generator (dynasm) for the kernels `wasm.rs` also handles.
//!
//! The same register IR, no wasm in between. The generated function runs the whole sample loop
//! for a block directly on the voice's register and state arrays; the hottest registers live in
//! the callee-saved `d8`-`d15` and the caller-saved `d16`-`d31` for the length of the block, the
//! rest stay in memory. Maths that is not an instruction (`sin`, `pow`...) calls the same Rust
//! helpers as the interpreter. Lists (`Read`, `ReadAt`, `Write`) are not generated; a kernel with
//! them stays on the interpreter.
//!
//! Rounding is the interpreter's: separate multiply and add (no `fmadd`), `fminnm`/`fmaxnm` for
//! Rust's NaN-ignoring `min`/`max`, the guarded division, the same comparisons.

// dynasm's expansions convert register numbers with `.into()`, even from `u8` to `u8`
#![allow(clippy::useless_conversion)]

use dynasmrt::aarch64::Assembler;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, ExecutableBuffer, dynasm};

use super::{ArraySource, BinOp, Cmp, Fn1, Instr, Interp, Kernel, MemoMath};

type Entry = unsafe extern "C" fn(
    regs: *mut f64,
    state: *mut f64,
    inputs: *const *const f32,
    outputs: *const *mut f32,
    memo: *mut u64,
    frames: usize,
    lists: *const *mut f64,
    sample_rate: f64,
);

const BINS: [BinOp; 9] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Min,
    BinOp::Max,
    BinOp::Pow,
    BinOp::Atan2,
    BinOp::Rem,
];

fn bin_id(op: BinOp) -> u32 {
    BINS.iter().position(|&b| b == op).unwrap_or(0) as u32
}

fn fn_id(f: Fn1) -> u32 {
    Fn1::ALL.iter().position(|&g| g == f).unwrap_or(0) as u32
}

extern "C" fn host_bin(op: u32, a: f64, b: f64) -> f64 {
    BINS[op as usize].apply(a, b)
}

extern "C" fn host_wrap(x: f64, n: f64) -> u64 {
    crate::wrapped_index(x, n as usize) as u64
}

extern "C" fn host_f1(id: u32, x: f64) -> f64 {
    Fn1::ALL[id as usize].apply(x)
}

/// Condition-code names for `fcmp` results, chosen so that an unordered (NaN) comparison gives
/// what Rust's operators give: false, except for `!=`.
fn holds(op: Cmp) -> &'static str {
    match op {
        Cmp::Lt => "mi",
        Cmp::Gt => "gt",
        Cmp::Le => "ls",
        Cmp::Ge => "ge",
        Cmp::Eq => "eq",
        Cmp::Ne => "ne",
    }
}

/// The exact negation of [`holds`].
fn fails(op: Cmp) -> &'static str {
    match op {
        Cmp::Lt => "pl",
        Cmp::Gt => "le",
        Cmp::Le => "hi",
        Cmp::Ge => "lt",
        Cmp::Eq => "ne",
        Cmp::Ne => "eq",
    }
}

/// A compiled kernel, shared by its voices.
pub struct JitKernel {
    _buffer: ExecutableBuffer,
    entry: Entry,
    pub n_ports: usize,
    pub n_outputs: usize,
    pub memo_slots: usize,
    pub n_lists: usize,
    pub bytes: usize,
    pub allocated: usize,
}

// SAFETY: the buffer is immutable once finalized and the entry point is a plain function pointer
// into it; every call passes its own voice's storage.
unsafe impl Send for JitKernel {}
// SAFETY: as above.
unsafe impl Sync for JitKernel {}

struct Gen<'k> {
    ops: Assembler,
    kernel: &'k Kernel,
    /// FP register of each virtual register (registers, then state slots), if it has one.
    alloc: Vec<Option<u8>>,
    /// Registers that need no load at the start of the block or store at its end.
    temp: Vec<bool>,
    /// Constants set once before the loop instead of every sample.
    hoist: Vec<Option<f64>>,
}

impl Gen<'_> {
    fn n_regs(&self) -> usize {
        self.kernel.registers
    }

    /// Byte offset of a virtual register's home, and whether it is in the state array.
    fn home(&self, v: usize) -> (u32, bool) {
        if v < self.n_regs() {
            ((v * 8) as u32, false)
        } else {
            (((v - self.n_regs()) * 8) as u32, true)
        }
    }

    fn load(&mut self, d: u8, v: usize) {
        let (off, state) = self.home(v);
        if state {
            dynasm!(self.ops ; .arch aarch64 ; ldr D(d), [x20, off]);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; ldr D(d), [x19, off]);
        }
    }

    fn store(&mut self, d: u8, v: usize) {
        let (off, state) = self.home(v);
        if state {
            dynasm!(self.ops ; .arch aarch64 ; str D(d), [x20, off]);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; str D(d), [x19, off]);
        }
    }

    /// The FP register holding `v`: its own, or `scratch` after loading it.
    fn read(&mut self, v: usize, scratch: u8) -> u8 {
        match self.alloc[v] {
            Some(d) => d,
            None => {
                self.load(scratch, v);
                scratch
            }
        }
    }

    /// Where to compute a result for `v`.
    fn target(&self, v: usize, scratch: u8) -> u8 {
        self.alloc[v].unwrap_or(scratch)
    }

    /// After computing into `from` (a [`Self::target`]): store it if `v` has no register.
    fn finish(&mut self, v: usize, from: u8) {
        if self.alloc[v].is_none() {
            self.store(from, v);
        }
    }

    fn fmov(&mut self, dst: u8, src: u8) {
        if dst != src {
            dynasm!(self.ops ; .arch aarch64 ; fmov D(dst), D(src));
        }
    }

    fn constant(&mut self, dst: u8, value: f64) {
        let bits = value.to_bits();
        if bits == 0 {
            dynasm!(self.ops ; .arch aarch64 ; fmov D(dst), xzr);
            return;
        }
        dynasm!(self.ops ; .arch aarch64 ; movz x16, (bits & 0xffff) as u32);
        for shift in [16u32, 32, 48] {
            let part = ((bits >> shift) & 0xffff) as u32;
            if part != 0 {
                match shift {
                    16 => dynasm!(self.ops ; .arch aarch64 ; movk x16, part, lsl 16),
                    32 => dynasm!(self.ops ; .arch aarch64 ; movk x16, part, lsl 32),
                    _ => dynasm!(self.ops ; .arch aarch64 ; movk x16, part, lsl 48),
                }
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; fmov D(dst), x16);
    }

    /// Calls a host function with the operands in `d0`, `d1` and an id in `w0`; the result is in
    /// `d0`. The caller-saved registers that hold virtual registers survive through the frame.
    fn call(&mut self, address: usize, id: u32) {
        let mut hi: Vec<u8> = self.alloc.iter().flatten().copied().filter(|&d| d >= 16).collect();
        hi.sort_unstable();
        hi.dedup();
        for (k, d) in hi.iter().enumerate() {
            let off = 64 + (k as u32) * 8;
            dynasm!(self.ops ; .arch aarch64 ; str D(*d), [sp, off]);
        }
        dynasm!(self.ops ; .arch aarch64 ; movz w0, id);
        self.constant_x(16, address as u64);
        dynasm!(self.ops ; .arch aarch64 ; blr x16);
        for (k, d) in hi.iter().enumerate() {
            let off = 64 + (k as u32) * 8;
            dynasm!(self.ops ; .arch aarch64 ; ldr D(*d), [sp, off]);
        }
    }

    fn constant_x(&mut self, x: u8, bits: u64) {
        dynasm!(self.ops ; .arch aarch64 ; movz X(x), (bits & 0xffff) as u32);
        for shift in [16u32, 32, 48] {
            let part = ((bits >> shift) & 0xffff) as u32;
            if part != 0 {
                match shift {
                    16 => dynasm!(self.ops ; .arch aarch64 ; movk X(x), part, lsl 16),
                    32 => dynasm!(self.ops ; .arch aarch64 ; movk X(x), part, lsl 32),
                    _ => dynasm!(self.ops ; .arch aarch64 ; movk X(x), part, lsl 48),
                }
            }
        }
    }

    /// Byte offset in the list table of a list the spike can read.
    fn list_slot(&self, list: ArraySource) -> Result<(u32, usize), String> {
        let buffers = self.kernel.buffers.len();
        let (slot, len) = match list {
            ArraySource::State(l) => (l, self.kernel.buffers.get(l).map(|b| b.len())),
            ArraySource::Table(t) => (buffers + t, self.kernel.tables_f64.get(t).map(|b| b.len())),
            _ => return Err("a list the spike does not generate".into()),
        };
        match len {
            Some(len) if len > 0 => Ok(((slot * 8) as u32, len)),
            _ => Err("empty or unknown list".into()),
        }
    }

    /// `x9 = wrapped_index(value in D(a), len)`. Uses d0, d1, d6, d7 and x9; the rare out-of-range
    /// case calls the host and keeps every live register.
    fn wrap(&mut self, a: u8, len: usize) {
        let neg = self.ops.new_dynamic_label();
        let inrange = self.ops.new_dynamic_label();
        let fast_sub = self.ops.new_dynamic_label();
        let slow = self.ops.new_dynamic_label();
        let done = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch aarch64 ; frintm d0, D(a));
        self.constant(1, len as f64);
        dynasm!(self.ops
            ; .arch aarch64
            ; fmov d7, xzr
            ; fcmp d0, d7
            ; b.mi =>neg
            ; fcmp d0, d1
            ; b.mi =>inrange
            ; fsub d6, d0, d1
            ; fcmp d6, d1
            ; b.mi =>fast_sub
            ; b =>slow
            ; =>neg
            ; fadd d6, d0, d1
            ; fcmp d6, d7
            ; b.mi =>slow
            ; fcmp d6, d1
            ; b.mi =>fast_sub
            ; b =>slow
            ; =>inrange
            ; fcvtzu x9, d0
            ; b =>done
            ; =>fast_sub
            ; fcvtzu x9, d6
            ; b =>done
            ; =>slow
            ; fmov d0, D(a));
        // everything the call clobbers that is live: the scratch registers, then the call saves
        // the allocated caller-saved ones
        for (k, d) in [2u32, 3, 4, 5, 29, 30, 31].into_iter().enumerate() {
            let off = 288 + (k as u32) * 8;
            dynasm!(self.ops ; .arch aarch64 ; str D(d as u8), [sp, off]);
        }
        self.call(host_wrap as *const () as usize, 0);
        dynasm!(self.ops ; .arch aarch64 ; mov x9, x0);
        for (k, d) in [2u32, 3, 4, 5, 29, 30, 31].into_iter().enumerate() {
            let off = 288 + (k as u32) * 8;
            dynasm!(self.ops ; .arch aarch64 ; ldr D(d as u8), [sp, off]);
        }
        dynasm!(self.ops ; .arch aarch64 ; =>done);
    }

    /// `D(t) = list[x9]`.
    fn element(&mut self, t: u8, slot: u32) {
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x10, [x26, slot]
            ; ldr D(t), [x10, x9, lsl 3]);
    }

    /// `t = f(a)` for the one-argument functions.
    fn unary(&mut self, f: Fn1, a: u8, t: u8) {
                match f {
            Fn1::Abs => dynasm!(self.ops ; .arch aarch64 ; fabs D(t), D(a)),
            Fn1::Floor => dynasm!(self.ops ; .arch aarch64 ; frintm D(t), D(a)),
            Fn1::Ceil => dynasm!(self.ops ; .arch aarch64 ; frintp D(t), D(a)),
            Fn1::Sqrt => dynasm!(self.ops
                ; .arch aarch64
                ; fmov d1, xzr
                ; fmaxnm d0, D(a), d1
                ; fsqrt D(t), d0),
            Fn1::Fract => dynasm!(self.ops
                ; .arch aarch64
                ; frintm d0, D(a)
                ; fsub D(t), D(a), d0),
            Fn1::Sign => dynasm!(self.ops
                ; .arch aarch64
                ; fmov d0, xzr
                ; fmov d1, 1.0
                ; fcmp D(a), 0.0
                ; fcsel d0, d1, d0, gt
                ; fmov d1, -1.0
                ; fcsel D(t), d1, d0, mi),
            f => {
                self.fmov(0, a);
                self.call(host_f1 as *const () as usize, fn_id(f));
                self.fmov(t, 0);
            }
        }
    }

    /// `t = a op b`.
    fn binary(&mut self, op: BinOp, a: u8, b: u8, t: u8) {
        let tt = t;
        match op {
            BinOp::Add => dynasm!(self.ops ; .arch aarch64 ; fadd D(tt), D(a), D(b)),
            BinOp::Sub => dynasm!(self.ops ; .arch aarch64 ; fsub D(tt), D(a), D(b)),
            BinOp::Mul => dynasm!(self.ops ; .arch aarch64 ; fmul D(tt), D(a), D(b)),
            BinOp::Div => dynasm!(self.ops
                ; .arch aarch64
                ; fdiv d0, D(a), D(b)
                ; fmov d1, xzr
                ; fcmp D(b), 0.0
                ; fcsel D(tt), d1, d0, eq),
            BinOp::Min => dynasm!(self.ops ; .arch aarch64 ; fminnm D(tt), D(a), D(b)),
            BinOp::Max => dynasm!(self.ops ; .arch aarch64 ; fmaxnm D(tt), D(a), D(b)),
            BinOp::Pow | BinOp::Atan2 | BinOp::Rem => {
                // operands are never d0 or d1 (scratch registers start at d2)
                self.fmov(0, a);
                self.fmov(1, b);
                self.call(host_bin as *const () as usize, bin_id(op));
                self.fmov(t, 0);
            }
        }
    }
}

/// What an instruction reads and writes, as virtual registers (registers, then state slots).
fn touches(instr: &Instr, r: usize) -> (Vec<usize>, Vec<usize>) {
    match *instr {
        Instr::Const { dst, .. } | Instr::SampleRate { dst } | Instr::Input { dst, .. } => {
            (vec![], vec![dst])
        }
        Instr::LoadState { dst, slot } => (vec![r + slot], vec![dst]),
        Instr::StoreState { slot, src } => (vec![src], vec![r + slot]),
        Instr::Copy { dst, src } => (vec![src], vec![dst]),
        Instr::Neg { dst, a } | Instr::Call { dst, a, .. } => (vec![a], vec![dst]),
        Instr::Bin { dst, a, b, .. } | Instr::Cmp { dst, a, b, .. } | Instr::CmpSkip { dst, a, b, .. } => {
            (vec![a, b], vec![dst])
        }
        Instr::MulAdd { dst, a, b, c }
        | Instr::MulSub { dst, a, b, c }
        | Instr::SubMul { dst, a, b, c } => (vec![a, b, c], vec![dst]),
        Instr::Clamp { dst, x, lo, hi } => (vec![x, lo, hi], vec![dst]),
        Instr::Select { dst, cond, a, b } => (vec![cond, a, b], vec![dst]),
        Instr::SkipIfZero { cond, .. } => (vec![cond], vec![]),
        Instr::Read { dst, idx, .. } => (vec![idx], vec![dst]),
        Instr::ReadAt { dst, pos, .. } => (vec![pos], vec![dst]),
        Instr::Write {
            idx, src, cond, ..
        } => {
            let mut reads = vec![idx, src];
            reads.extend(cond);
            (reads, vec![])
        }
        Instr::Memo { dst, a, math, .. } => {
            let mut reads = vec![a];
            if let MemoMath::Binary(_, b) = math {
                reads.push(b);
            }
            (reads, vec![dst])
        }
        _ => (vec![], vec![]),
    }
}

/// Assigns FP registers to virtual registers by linear scan over live ranges.
///
/// A register that the sample code writes unconditionally before it reads is a temporary: its
/// value is dead outside `[first write, last read]`, so its FP register can be reused and it is
/// not saved at the end of the block. Everything else (state, registers read before written,
/// written inside a skipped region, set by the init or block code) lives for the whole loop and is
/// loaded at the start of the block and stored at its end.
fn allocate(kernel: &Kernel) -> (Vec<Option<u8>>, Vec<bool>, Vec<Option<f64>>) {
    let r = kernel.registers;
    let n = r + kernel.state_init.len();
    let len = kernel.code.len();
    let mut uses = vec![0usize; n];
    let mut first_write: Vec<Option<usize>> = vec![None; n];
    let mut first_read: Vec<Option<usize>> = vec![None; n];
    let mut last: Vec<usize> = vec![0; n];
    // skip regions: instruction i skips to i + 1 + count
    let region_end: Vec<usize> = kernel
        .code
        .iter()
        .enumerate()
        .map(|(i, instr)| match *instr {
            Instr::SkipIfZero { count, .. } | Instr::CmpSkip { count, .. } if count > 0 => i + 1 + count,
            _ => 0,
        })
        .collect();
    for (index, instr) in kernel.code.iter().enumerate() {
        let (reads, writes) = touches(instr, r);
        for v in reads {
            uses[v] += 1;
            first_read[v].get_or_insert(index);
            last[v] = index;
        }
        for v in writes {
            uses[v] += 1;
            let conditional = region_end[..index].iter().any(|&end| end > index);
            if first_write[v].is_none() && !conditional {
                first_write[v] = Some(index);
            } else if first_write[v].is_none() {
                // written only under a skip: treat like a loop-carried value
                first_read[v].get_or_insert(0);
            }
            last[v] = last[v].max(index);
        }
    }
    for (_, reg) in &kernel.outputs {
        uses[*reg] += 1;
        first_read[*reg].get_or_insert(len);
        last[*reg] = len;
    }
    // a constant written once, unconditionally, before any read is the same every sample: it is
    // put in its register once, before the loop
    let mut writes = vec![0usize; n];
    for instr in &kernel.code {
        for v in touches(instr, r).1 {
            writes[v] += 1;
        }
    }
    let mut hoist: Vec<Option<f64>> = vec![None; n];
    for (index, instr) in kernel.code.iter().enumerate() {
        if let Instr::Const { dst, value } = *instr
            && writes[dst] == 1
            && first_write[dst] == Some(index)
            && first_read[dst].is_none_or(|rd| rd > index)
        {
            hoist[dst] = Some(value);
        }
    }
    let mut temp = vec![false; n];
    let mut intervals: Vec<(usize, usize, usize)> = Vec::new(); // start, end, vreg
    for v in 0..n {
        if uses[v] == 0 {
            continue;
        }
        let is_temp = v < r
            && hoist[v].is_none()
            && match (first_write[v], first_read[v]) {
                (Some(w), Some(rd)) => w < rd,
                (Some(_), None) => true,
                _ => false,
            };
        temp[v] = is_temp;
        let (start, end) = if is_temp { (first_write[v].unwrap_or(0), last[v]) } else { (0, len) };
        intervals.push((start, end, v));
    }
    intervals.sort_by_key(|&(start, end, v)| (start, std::cmp::Reverse(uses[v] * 1000 / (end - start + 1)), v));
    let weight = |v: usize, start: usize, end: usize| uses[v] as f64 / (end - start + 1) as f64;
    let mut alloc: Vec<Option<u8>> = vec![None; n];
    // d29-d31 are scratch for cubic interpolation
    let mut free: Vec<u8> = (8..=28).rev().collect();
    let mut active: Vec<(usize, usize, usize)> = Vec::new();
    for (start, end, v) in intervals {
        // expire
        active.retain(|&(_, e, av)| {
            if e < start {
                if let Some(d) = alloc[av] {
                    free.push(d);
                }
                false
            } else {
                true
            }
        });
        free.sort_unstable_by(|a, b| b.cmp(a));
        if let Some(d) = free.pop() {
            alloc[v] = Some(d);
            active.push((start, end, v));
        } else if let Some((k, &(vs, ve, victim))) = active
            .iter()
            .enumerate()
            .min_by(|a, b| {
                let (x, y) = (a.1, b.1);
                weight(x.2, x.0, x.1).total_cmp(&weight(y.2, y.0, y.1))
            })
            && weight(victim, vs, ve) < weight(v, start, end)
        {
            alloc[v] = alloc[victim].take();
            active.remove(k);
            active.push((start, end, v));
        }
    }
    for v in 0..n {
        if alloc[v].is_none() {
            hoist[v] = None;
        }
    }
    (alloc, temp, hoist)
}

impl JitKernel {
    pub fn compile(kernel: &Kernel) -> Result<Self, String> {
        if !kernel.resets.is_empty() {
            return Err("reset scopes".into());
        }
        for instr in &kernel.code {
            match instr {
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
                | Instr::Read { .. }
                | Instr::ReadAt { .. }
                | Instr::Write { .. }
                | Instr::SkipIfZero { .. }
                | Instr::CmpSkip { .. } => {}
                _ => return Err("an instruction the spike does not generate".into()),
            }
        }
        if kernel.registers + kernel.state_init.len() > 4000 {
            return Err("too many registers for the spike's immediate offsets".into());
        }
        let (alloc, temp, hoist) = allocate(kernel);
        let allocated = alloc.iter().flatten().count();
        let mut g = Gen {
            ops: Assembler::new().map_err(|e| e.to_string())?,
            kernel,
            alloc,
            temp,
            hoist,
        };
        let len = kernel.code.len();
        let labels: Vec<DynamicLabel> = (0..=len).map(|_| g.ops.new_dynamic_label()).collect();
        let top = g.ops.new_dynamic_label();
        let done = g.ops.new_dynamic_label();
        let entry = g.ops.offset();

        // frame: 0..64 callee-saved x registers, 64..192 spill area for d16-d31, 192..256 d8-d15,
        // 256 the sample rate; x29/x30 above
        dynasm!(g.ops
            ; .arch aarch64
            ; stp x29, x30, [sp, -384]!
            ; mov x29, sp
            ; stp x19, x20, [sp, 16]
            ; stp x21, x22, [sp, 32]
            ; stp x23, x24, [sp, 48]
            ; str x25, [sp, 64 + 128]
            ; str x26, [sp, 200]
            ; stp d8, d9, [sp, 208]
            ; stp d10, d11, [sp, 224]
            ; stp d12, d13, [sp, 240]
            ; stp d14, d15, [sp, 256]
            ; str d0, [sp, 272]
            ; mov x19, x0
            ; mov x20, x1
            ; mov x21, x2
            ; mov x22, x3
            ; mov x25, x4
            ; mov x23, x5
            ; mov x26, x6
            ; movz x24, 0
        );
        // registers and state into their FP registers
        for v in 0..g.alloc.len() {
            if let (Some(d), false) = (g.alloc[v], g.temp[v]) {
                match g.hoist[v] {
                    Some(value) => g.constant(d, value),
                    None => g.load(d, v),
                }
            }
        }
        dynasm!(g.ops
            ; .arch aarch64
            ; cbz x23, =>done
            ; =>top
        );
        for (index, instr) in kernel.code.iter().enumerate() {
            dynasm!(g.ops ; .arch aarch64 ; =>labels[index]);
            match *instr {
                Instr::Const { dst, .. } if g.hoist[dst].is_some() => {}
                Instr::Const { dst, value } => {
                    let t = g.target(dst, 0);
                    g.constant(t, value);
                    g.finish(dst, t);
                }
                Instr::Input { dst, port } => {
                    let t = g.target(dst, 0);
                    let off = (port * 8) as u32;
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; ldr x9, [x21, off]
                        ; ldr s0, [x9, x24, lsl 2]
                        ; fcvt D(t), s0);
                    g.finish(dst, t);
                }
                Instr::SampleRate { dst } => {
                    let t = g.target(dst, 0);
                    dynasm!(g.ops ; .arch aarch64 ; ldr D(t), [sp, 272]);
                    g.finish(dst, t);
                }
                Instr::LoadState { dst, slot } => {
                    let s = g.read(kernel.registers + slot, 2);
                    let t = g.target(dst, 0);
                    g.fmov(t, s);
                    g.finish(dst, t);
                }
                Instr::StoreState { slot, src } => {
                    let s = g.read(src, 2);
                    let v = kernel.registers + slot;
                    match g.alloc[v] {
                        Some(d) => g.fmov(d, s),
                        None => g.store(s, v),
                    }
                }
                Instr::Copy { dst, src } => {
                    let s = g.read(src, 2);
                    let t = g.target(dst, 0);
                    g.fmov(t, s);
                    g.finish(dst, t);
                }
                Instr::Neg { dst, a } => {
                    let x = g.read(a, 2);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops ; .arch aarch64 ; fneg D(t), D(x));
                    g.finish(dst, t);
                }
                Instr::Bin { dst, op, a, b } => {
                    let x = g.read(a, 2);
                    let y = g.read(b, 3);
                    let t = g.target(dst, 0);
                    g.binary(op, x, y, t);
                    g.finish(dst, t);
                }
                Instr::MulAdd { dst, a, b, c } | Instr::MulSub { dst, a, b, c } => {
                    let x = g.read(a, 2);
                    let y = g.read(b, 3);
                    let z = g.read(c, 4);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops ; .arch aarch64 ; fmul d0, D(x), D(y));
                    if matches!(instr, Instr::MulAdd { .. }) {
                        dynasm!(g.ops ; .arch aarch64 ; fadd D(t), d0, D(z));
                    } else {
                        dynasm!(g.ops ; .arch aarch64 ; fsub D(t), d0, D(z));
                    }
                    g.finish(dst, t);
                }
                Instr::SubMul { dst, a, b, c } => {
                    let x = g.read(a, 2);
                    let y = g.read(b, 3);
                    let z = g.read(c, 4);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; fmul d0, D(x), D(y)
                        ; fsub D(t), D(z), d0);
                    g.finish(dst, t);
                }
                Instr::Clamp { dst, x, lo, hi } => {
                    let a = g.read(x, 2);
                    let l = g.read(lo, 3);
                    let h = g.read(hi, 4);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; fmaxnm d0, D(a), D(l)
                        ; fminnm D(t), d0, D(h));
                    g.finish(dst, t);
                }
                Instr::Cmp { dst, op, a, b } => {
                    let x = g.read(a, 2);
                    let y = g.read(b, 3);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; fmov d1, xzr
                        ; fmov d6, 1.0
                        ; fcmp D(x), D(y));
                    match holds(op) {
                        "mi" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, mi),
                        "gt" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, gt),
                        "ls" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, ls),
                        "ge" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, ge),
                        "eq" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, eq),
                        _ => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, ne),
                    }
                    g.finish(dst, t);
                }
                Instr::Select { dst, cond, a, b } => {
                    let x = g.read(a, 2);
                    let y = g.read(b, 3);
                    let c = g.read(cond, 4);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; fcmp D(c), 0.0
                        ; fcsel D(t), D(x), D(y), ne);
                    g.finish(dst, t);
                }
                Instr::Call { dst, f, a } => {
                    let x = g.read(a, 2);
                    let t = g.target(dst, 0);
                    g.unary(f, x, t);
                    g.finish(dst, t);
                }
                Instr::SkipIfZero { cond, count } => {
                    if count > 0 {
                        let c = g.read(cond, 2);
                        let target = labels[index + 1 + count];
                        dynasm!(g.ops
                            ; .arch aarch64
                            ; fcmp D(c), 0.0
                            ; b.eq =>target);
                    }
                }
                Instr::CmpSkip {
                    dst,
                    op,
                    a,
                    b,
                    count,
                } => {
                    let x = g.read(a, 2);
                    let y = g.read(b, 3);
                    let t = g.target(dst, 0);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; fmov d1, xzr
                        ; fmov d6, 1.0
                        ; fcmp D(x), D(y));
                    match holds(op) {
                        "mi" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, mi),
                        "gt" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, gt),
                        "ls" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, ls),
                        "ge" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, ge),
                        "eq" => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, eq),
                        _ => dynasm!(g.ops ; .arch aarch64 ; fcsel D(t), d6, d1, ne),
                    }
                    g.finish(dst, t);
                    if count > 0 {
                        // the flags still hold the comparison (a store does not touch them)
                        let target = labels[index + 1 + count];
                        match fails(op) {
                            "pl" => dynasm!(g.ops ; .arch aarch64 ; b.pl =>target),
                            "le" => dynasm!(g.ops ; .arch aarch64 ; b.le =>target),
                            "hi" => dynasm!(g.ops ; .arch aarch64 ; b.hi =>target),
                            "lt" => dynasm!(g.ops ; .arch aarch64 ; b.lt =>target),
                            "ne" => dynasm!(g.ops ; .arch aarch64 ; b.ne =>target),
                            _ => dynasm!(g.ops ; .arch aarch64 ; b.eq =>target),
                        }
                    }
                }
                Instr::Read { dst, list, idx } => {
                    let (slot, len) = g.list_slot(list)?;
                    let a = g.read(idx, 2);
                    g.wrap(a, len);
                    let t = g.target(dst, 0);
                    g.element(t, slot);
                    g.finish(dst, t);
                }
                Instr::ReadAt {
                    dst,
                    list,
                    pos,
                    interp,
                } => {
                    let (slot, len) = g.list_slot(list)?;
                    let p = g.read(pos, 2);
                    let t = g.target(dst, 0);
                    if interp == Interp::None {
                        g.wrap(p, len);
                        g.element(t, slot);
                    } else {
                        let nonfinite = g.ops.new_dynamic_label();
                        let done = g.ops.new_dynamic_label();
                        dynasm!(g.ops
                            ; .arch aarch64
                            ; fsub d0, D(p), D(p)
                            ; fcmp d0, 0.0
                            ; b.ne =>nonfinite
                            ; frintm d3, D(p)
                            ; fsub d4, D(p), d3);
                        if interp == Interp::Linear {
                            g.wrap(3, len);
                            g.element(5, slot);
                            dynasm!(g.ops ; .arch aarch64 ; fmov d2, 1.0 ; fadd d2, d3, d2);
                            g.wrap(2, len);
                            g.element(2, slot);
                            dynasm!(g.ops
                                ; .arch aarch64
                                ; fsub d2, d2, d5
                                ; fmul d2, d2, d4
                                ; fadd D(t), d5, d2);
                        } else {
                            // y0..y3 are the entries at floor-1 ..= floor+2
                            for (k, (offset, y)) in [(-1.0f64, 5u8), (0.0, 29), (1.0, 30), (2.0, 31)]
                                .into_iter()
                                .enumerate()
                            {
                                let _ = k;
                                match offset as i32 {
                                    -1 => dynasm!(g.ops ; .arch aarch64 ; fmov d2, -1.0 ; fadd d2, d3, d2),
                                    0 => dynasm!(g.ops ; .arch aarch64 ; fmov d2, d3),
                                    1 => dynasm!(g.ops ; .arch aarch64 ; fmov d2, 1.0 ; fadd d2, d3, d2),
                                    _ => dynasm!(g.ops ; .arch aarch64 ; fmov d2, 2.0 ; fadd d2, d3, d2),
                                }
                                g.wrap(2, len);
                                g.element(y, slot);
                            }
                            // a = -0.5*y0 + 1.5*y1 - 1.5*y2 + 0.5*y3 (left to right, no fma)
                            dynasm!(g.ops
                                ; .arch aarch64
                                ; fmov d1, -0.5
                                ; fmul d0, d1, d5
                                ; fmov d1, 1.5
                                ; fmul d6, d1, d29
                                ; fadd d0, d0, d6
                                ; fmul d6, d1, d30
                                ; fsub d0, d0, d6
                                ; fmov d1, 0.5
                                ; fmul d6, d1, d31
                                ; fadd d0, d0, d6
                                // b = y0 - 2.5*y1 + 2.0*y2 - 0.5*y3
                                ; fmov d1, 2.5
                                ; fmul d6, d1, d29
                                ; fsub d7, d5, d6
                                ; fmov d1, 2.0
                                ; fmul d6, d1, d30
                                ; fadd d7, d7, d6
                                ; fmov d1, 0.5
                                ; fmul d6, d1, d31
                                ; fsub d7, d7, d6
                                // c = -0.5*y0 + 0.5*y2
                                ; fmov d1, -0.5
                                ; fmul d6, d1, d5
                                ; fmov d1, 0.5
                                ; fmul d1, d1, d30
                                ; fadd d1, d6, d1
                                // ((a*t + b)*t + c)*t + y1
                                ; fmul d0, d0, d4
                                ; fadd d0, d0, d7
                                ; fmul d0, d0, d4
                                ; fadd d0, d0, d1
                                ; fmul d0, d0, d4
                                ; fadd D(t), d0, d29);
                        }
                        dynasm!(g.ops ; .arch aarch64 ; b =>done ; =>nonfinite);
                        dynasm!(g.ops ; .arch aarch64 ; ldr x10, [x26, slot] ; ldr D(t), [x10]);
                        dynasm!(g.ops ; .arch aarch64 ; =>done);
                    }
                    g.finish(dst, t);
                }
                Instr::Write {
                    list,
                    idx,
                    src,
                    cond,
                } => {
                    let (slot, len) = g.list_slot(ArraySource::State(list))?;
                    let skip = g.ops.new_dynamic_label();
                    if let Some(c) = cond {
                        let c = g.read(c, 4);
                        dynasm!(g.ops ; .arch aarch64 ; fcmp D(c), 0.0 ; b.eq =>skip);
                    }
                    let a = g.read(idx, 2);
                    g.wrap(a, len);
                    let s = g.read(src, 3);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; ldr x10, [x26, slot]
                        ; str D(s), [x10, x9, lsl 3]
                        ; =>skip);
                }
                Instr::Memo { dst, slot, math, a } => {
                    let at = (slot * 32) as u32;
                    let x = g.read(a, 2);
                    let miss = g.ops.new_dynamic_label();
                    let end = g.ops.new_dynamic_label();
                    let y = match math {
                        MemoMath::Binary(_, b) => Some(g.read(b, 3)),
                        MemoMath::Unary(_) => None,
                    };
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; fmov x9, D(x)
                        ; ldr x10, [x25, at]
                        ; cmp x9, x10
                        ; b.ne =>miss);
                    match y {
                        Some(y) => dynasm!(g.ops
                            ; .arch aarch64
                            ; fmov x11, D(y)
                            ; ldr x12, [x25, at + 8]
                            ; cmp x11, x12
                            ; b.ne =>miss),
                        None => dynasm!(g.ops
                            ; .arch aarch64
                            ; ldr x12, [x25, at + 8]
                            ; cbnz x12, =>miss),
                    }
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; ldr x13, [x25, at + 16]
                        ; cbz x13, =>miss
                        ; ldr d0, [x25, at + 24]
                        ; b =>end
                        ; =>miss);
                    match math {
                        MemoMath::Unary(f) => g.unary(f, x, 0),
                        MemoMath::Binary(op, _) => {
                            let y = y.unwrap_or(1);
                            g.binary(op, x, y, 0)
                        }
                    }
                    // the keys again: a call clobbers the scratch registers and x9-x13, so the
                    // operands are read back from where they live
                    let x = g.read(a, 2);
                    dynasm!(g.ops ; .arch aarch64 ; fmov x9, D(x));
                    match math {
                        MemoMath::Binary(_, b) => {
                            let y = g.read(b, 3);
                            dynasm!(g.ops ; .arch aarch64 ; fmov x11, D(y));
                        }
                        MemoMath::Unary(_) => dynasm!(g.ops ; .arch aarch64 ; movz x11, 0),
                    }
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; str x9, [x25, at]
                        ; str x11, [x25, at + 8]
                        ; movz x13, 1
                        ; str x13, [x25, at + 16]
                        ; str d0, [x25, at + 24]
                        ; =>end);
                    let t = g.target(dst, 0);
                    g.fmov(t, 0);
                    g.finish(dst, t);
                }
                _ => return Err("unsupported instruction".into()),
            }
        }
        dynasm!(g.ops ; .arch aarch64 ; =>labels[len]);
        for (k, (_, reg)) in kernel.outputs.iter().enumerate() {
            let r = g.read(*reg, 2);
            let off = (k * 8) as u32;
            dynasm!(g.ops
                ; .arch aarch64
                ; fcvt s0, D(r)
                ; ldr x9, [x22, off]
                ; str s0, [x9, x24, lsl 2]);
        }
        dynasm!(g.ops
            ; .arch aarch64
            ; add x24, x24, 1
            ; cmp x24, x23
            ; b.lo =>top
            ; =>done);
        // FP registers back to their homes
        for v in 0..g.alloc.len() {
            if let (Some(d), false) = (g.alloc[v], g.temp[v]) {
                g.store(d, v);
            }
        }
        dynasm!(g.ops
            ; .arch aarch64
            ; ldp d8, d9, [sp, 208]
            ; ldp d10, d11, [sp, 224]
            ; ldp d12, d13, [sp, 240]
            ; ldp d14, d15, [sp, 256]
            ; ldr x26, [sp, 200]
            ; ldr x25, [sp, 64 + 128]
            ; ldp x23, x24, [sp, 48]
            ; ldp x21, x22, [sp, 32]
            ; ldp x19, x20, [sp, 16]
            ; ldp x29, x30, [sp], 384
            ; ret);
        let bytes = g.ops.offset().0;
        let buffer = g.ops.finalize().map_err(|_| "could not finalize".to_string())?;
        // SAFETY: the code at `entry` was assembled above with exactly the `Entry` signature
        // and AAPCS64 calling convention, and the buffer keeps it alive.
        let entry: Entry = unsafe { std::mem::transmute(buffer.ptr(entry)) };
        Ok(JitKernel {
            _buffer: buffer,
            entry,
            n_ports: kernel.ports.len(),
            n_outputs: kernel.outputs.len(),
            memo_slots: kernel.memo_slots,
            n_lists: kernel.buffers.len() + kernel.tables_f64.len(),
            bytes,
            allocated,
        })
    }

    /// Runs `frames` samples.
    ///
    /// # Safety
    /// `regs` and `state` must hold the kernel's registers and state slots, `inputs` one pointer
    /// per port to `frames` samples, `outputs` one pointer per output to room for `frames`
    /// samples, `memo` four words per memo slot, and `lists` the base address of each state list
    /// followed by each shared table, each as long as the kernel says.
    #[allow(clippy::too_many_arguments)] // mirrors the generated function's C signature
    pub unsafe fn run(
        &self,
        regs: *mut f64,
        state: *mut f64,
        inputs: *const *const f32,
        outputs: *const *mut f32,
        memo: *mut u64,
        frames: usize,
        lists: *const *mut f64,
        sample_rate: f64,
    ) {
        // SAFETY: forwarded from the caller's contract.
        unsafe { (self.entry)(regs, state, inputs, outputs, memo, frames, lists, sample_rate) }
    }
}

/// One voice's use of a [`JitKernel`]: the coefficient cache and the pointer tables it runs on.
pub struct JitVoice {
    kernel: std::sync::Arc<JitKernel>,
    memo: Vec<u64>,
    inputs: Vec<Vec<f32>>,
    // addresses, so the struct stays `Send` and `Sync`
    in_ptrs: Vec<usize>,
    out_ptrs: Vec<usize>,
    list_ptrs: Vec<usize>,
}

type Cache = std::sync::Mutex<Vec<(std::sync::Weak<Kernel>, Option<std::sync::Arc<JitKernel>>)>>;

impl JitVoice {
    /// `None` if `NIMINAL_JIT=0` or the kernel cannot be generated (the interpreter runs it).
    pub fn new(kernel: &std::sync::Arc<Kernel>) -> Option<Self> {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("NIMINAL_JIT").as_deref() != Ok("0")) {
            return None;
        }
        Self::with(JitKernel::cached(kernel)?)
    }

    /// Generates code whatever the environment says, for tests.
    pub fn forced(kernel: &Kernel) -> Result<Self, String> {
        let compiled = std::sync::Arc::new(JitKernel::compile(kernel)?);
        Self::with(compiled).ok_or_else(|| "no voice".to_string())
    }

    fn with(kernel: std::sync::Arc<JitKernel>) -> Option<Self> {
        let block = crate::opcode::block_size();
        Some(JitVoice {
            memo: vec![0; kernel.memo_slots * 4],
            inputs: vec![vec![0.0; block]; kernel.n_ports],
            in_ptrs: vec![0; kernel.n_ports],
            out_ptrs: vec![0; kernel.n_outputs],
            list_ptrs: vec![0; kernel.n_lists],
            kernel,
        })
    }

    pub fn render(
        &mut self,
        custom: &mut super::Custom,
        input: &impl Fn(usize, usize) -> f32,
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        for (port, samples) in self.inputs.iter_mut().enumerate() {
            for (i, sample) in samples.iter_mut().enumerate().take(frames) {
                *sample = input(port, i);
            }
            self.in_ptrs[port] = samples.as_ptr() as usize;
        }
        for (k, out) in outs.iter_mut().enumerate().take(self.out_ptrs.len()) {
            self.out_ptrs[k] = out.as_mut_ptr() as usize;
        }
        let buffers = custom.buffers.len();
        for (l, list) in custom.buffers.iter_mut().enumerate() {
            self.list_ptrs[l] = list.as_mut_ptr() as usize;
        }
        for (t, table) in custom.kernel.tables_f64.iter().enumerate() {
            self.list_ptrs[buffers + t] = table.as_ptr() as usize;
        }
        // SAFETY: the pointer tables hold one valid pointer per port and output, each to at
        // least `frames` samples (the inputs were just written; `outs` are block-sized); the
        // register and state vectors are the kernel's own, and the memo has four words per slot.
        // `usize` and pointers have the same layout.
        unsafe {
            self.kernel.run(
                custom.regs.as_mut_ptr(),
                custom.state.as_mut_ptr(),
                self.in_ptrs.as_ptr().cast::<*const f32>(),
                self.out_ptrs.as_ptr().cast::<*mut f32>(),
                self.memo.as_mut_ptr(),
                frames,
                self.list_ptrs.as_ptr().cast::<*mut f64>(),
                custom.sample_rate,
            );
        }
    }
}

impl JitKernel {
    fn cached(kernel: &std::sync::Arc<Kernel>) -> Option<std::sync::Arc<JitKernel>> {
        static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
        let mut cache = CACHE.get_or_init(|| std::sync::Mutex::new(Vec::new())).lock().ok()?;
        cache.retain(|(weak, _)| weak.strong_count() > 0);
        if let Some((_, hit)) = cache
            .iter()
            .find(|(weak, _)| std::ptr::eq(weak.as_ptr(), std::sync::Arc::as_ptr(kernel)))
        {
            return hit.clone();
        }
        let started = std::time::Instant::now();
        let compiled = match JitKernel::compile(kernel) {
            Ok(k) => {
                if std::env::var("NIMINAL_JIT_VERBOSE").is_ok() {
                    eprintln!(
                        "jit {}: {} instructions, {} of {} registers in FP registers, {} bytes, {:.3} ms",
                        kernel.name,
                        kernel.code.len(),
                        k.allocated,
                        kernel.registers + kernel.state_init.len(),
                        k.bytes,
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                Some(std::sync::Arc::new(k))
            }
            Err(error) => {
                if std::env::var("NIMINAL_JIT_VERBOSE").is_ok() {
                    eprintln!("jit {}: interpreted ({error})", kernel.name);
                }
                None
            }
        };
        cache.push((std::sync::Arc::downgrade(kernel), compiled.clone()));
        compiled
    }
}
