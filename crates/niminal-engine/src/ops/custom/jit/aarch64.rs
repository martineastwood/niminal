//! The aarch64 code generator: the kernel's register IR to machine code with dynasm.
//!
//! The hottest registers live in `d8`-`d28` for the length of the block (`d8`-`d15` survive calls,
//! the rest are saved around the rare call), the rest stay in memory; `d0`-`d7` and `d29`-`d31`
//! are scratch. Rounding is the interpreter's: separate multiply and add (no `fmadd`),
//! `fminnm`/`fmaxnm` for Rust's NaN-ignoring `min`/`max`, the guarded division, the same
//! comparisons.

use dynasmrt::aarch64::Assembler;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, dynasm};

use super::{Analysis, Generated, bin_id, fn_id};
use crate::ops::custom::{ArraySource, BinOp, Cmp, Fn1, Instr, Interp, Kernel, MemoMath};

/// FP registers the allocator may hand out.
pub(super) const ALLOCATABLE: &[u8] = &[
    8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
];

pub(super) type Entry = unsafe extern "C" fn(
    regs: *mut f64,
    state: *mut f64,
    inputs: *const *const f32,
    outputs: *const *mut f32,
    memo: *mut u64,
    frames: usize,
    lists: *const *mut f64,
    sample_rate: f64,
);

extern "C" fn host_bin(op: u32, a: f64, b: f64) -> f64 {
    super::BINS[op as usize].apply(a, b)
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
        let (slot, len) = super::list_slot(self.kernel, list)?;
        Ok(((slot * 8) as u32, len))
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


pub(super) fn generate(kernel: &Kernel, a: &Analysis) -> Result<Generated, String> {
        let (alloc, temp, hoist) = (a.alloc.clone(), a.temp.clone(), a.hoist.clone());
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
        Ok(Generated {
            buffer,
            entry,
            bytes,
        })
}
