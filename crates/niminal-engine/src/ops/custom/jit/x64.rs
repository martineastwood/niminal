//! The x86-64 code generator: the kernel's register IR to machine code with dynasm.
//!
//! SSE2 plus SSE4.1 (`roundsd`, `blendvpd`), which every x86-64 CPU of the last fifteen years has;
//! a CPU without it keeps running the interpreter. The generated function uses the System V
//! calling convention on every operating system (`extern "sysv64"`).
//!
//! `xmm0`-`xmm5` are scratch and the allocator hands out `xmm6`-`xmm15`; everything is caller-saved
//! in System V, so the rare call to a Rust helper saves the live registers around it. Rounding is
//! the interpreter's: separate multiply and add, Rust's NaN-ignoring `min`/`max` (built from
//! `minsd`/`maxsd` and a NaN fix-up), the guarded division, comparisons that are false on NaN
//! except `!=`.
//!
//! Frame (below the six pushed callee-saved registers): 0..128 spill area for `xmm0`-`xmm15`,
//! 128..192 eight temporaries, 192 the input table, 200 the output table, 208 the sample rate.

use dynasmrt::x64::Assembler;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, dynasm};

use super::{Analysis, Generated, bin_id, fn_id, list_slot};
use crate::ops::custom::{ArraySource, BinOp, Cmp, Fn1, Instr, Interp, Kernel, MemoMath};

/// FP registers the allocator may hand out.
pub(super) const ALLOCATABLE: &[u8] = &[6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

pub(super) type Entry = unsafe extern "sysv64" fn(
    regs: *mut f64,
    state: *mut f64,
    inputs: *const *const f32,
    outputs: *const *mut f32,
    memo: *mut u64,
    frames: usize,
    lists: *const *mut f64,
    sample_rate: f64,
);

extern "sysv64" fn host_bin(op: u32, a: f64, b: f64) -> f64 {
    super::BINS[op as usize].apply(a, b)
}

extern "sysv64" fn host_wrap(x: f64, n: f64) -> u64 {
    crate::wrapped_index(x, n as usize) as u64
}

extern "sysv64" fn host_f1(id: u32, x: f64) -> f64 {
    Fn1::ALL[id as usize].apply(x)
}

const FRAME: i32 = 264;
const TEMP: i32 = 128;
const INPUTS: i32 = 192;
const OUTPUTS: i32 = 200;
const SAMPLE_RATE: i32 = 208;

/// `cmpsd` predicates.
const EQ: i8 = 0;
const LT: i8 = 1;
const LE: i8 = 2;
const UNORD: i8 = 3;
const NEQ: i8 = 4;

#[derive(Clone, Copy)]
enum Sse {
    Add,
    Sub,
    Mul,
}

struct Gen<'k> {
    ops: Assembler,
    kernel: &'k Kernel,
    alloc: Vec<Option<u8>>,
    temp: Vec<bool>,
    hoist: Vec<Option<f64>>,
}

impl Gen<'_> {
    fn n_regs(&self) -> usize {
        self.kernel.registers
    }

    /// Byte offset of a virtual register's home, and whether it is in the state array.
    fn home(&self, v: usize) -> (i32, bool) {
        if v < self.n_regs() {
            ((v * 8) as i32, false)
        } else {
            (((v - self.n_regs()) * 8) as i32, true)
        }
    }

    fn load(&mut self, x: u8, v: usize) {
        let (off, state) = self.home(v);
        if state {
            dynasm!(self.ops ; .arch x64 ; movsd Rx(x), [r12 + off]);
        } else {
            dynasm!(self.ops ; .arch x64 ; movsd Rx(x), [rbx + off]);
        }
    }

    fn store(&mut self, x: u8, v: usize) {
        let (off, state) = self.home(v);
        if state {
            dynasm!(self.ops ; .arch x64 ; movsd [r12 + off], Rx(x));
        } else {
            dynasm!(self.ops ; .arch x64 ; movsd [rbx + off], Rx(x));
        }
    }

    /// The register holding `v`: its own, or `scratch` after loading it.
    fn read(&mut self, v: usize, scratch: u8) -> u8 {
        match self.alloc[v] {
            Some(x) => x,
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

    fn mov(&mut self, dst: u8, src: u8) {
        if dst != src {
            dynasm!(self.ops ; .arch x64 ; movaps Rx(dst), Rx(src));
        }
    }

    /// `rcx = bits`, for building constants.
    fn gpr(&mut self, bits: u64) {
        dynasm!(self.ops ; .arch x64 ; mov rcx, QWORD bits as i64);
    }

    fn constant(&mut self, x: u8, value: f64) {
        let bits = value.to_bits();
        if bits == 0 {
            dynasm!(self.ops ; .arch x64 ; xorpd Rx(x), Rx(x));
        } else {
            self.gpr(bits);
            dynasm!(self.ops ; .arch x64 ; movq Rx(x), rcx);
        }
    }

    /// Calls a Rust helper: the operands are already in `xmm0`/`xmm1` and `edi`; every other
    /// register survives through the spill area. The result is in `xmm0` or `rax`.
    fn call(&mut self, address: usize, id: u32) {
        for r in 1..16u8 {
            let off = i32::from(r) * 8;
            dynasm!(self.ops ; .arch x64 ; movsd [rsp + off], Rx(r));
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov edi, id as i32
            ; mov rax, QWORD address as i64
            ; call rax);
        for r in 1..16u8 {
            let off = i32::from(r) * 8;
            dynasm!(self.ops ; .arch x64 ; movsd Rx(r), [rsp + off]);
        }
    }

    fn sse(&mut self, op: Sse, dst: u8, src: u8) {
        match op {
            Sse::Add => dynasm!(self.ops ; .arch x64 ; addsd Rx(dst), Rx(src)),
            Sse::Sub => dynasm!(self.ops ; .arch x64 ; subsd Rx(dst), Rx(src)),
            Sse::Mul => dynasm!(self.ops ; .arch x64 ; mulsd Rx(dst), Rx(src)),
        }
    }

    /// `t = a op b` with a destructive two-operand instruction.
    fn arith(&mut self, op: Sse, a: u8, b: u8, t: u8) {
        let commutes = matches!(op, Sse::Add | Sse::Mul);
        if t == a {
            self.sse(op, t, b);
        } else if t != b {
            self.mov(t, a);
            self.sse(op, t, b);
        } else if commutes {
            self.sse(op, t, a);
        } else {
            self.mov(4, a);
            self.sse(op, 4, b);
            self.mov(t, 4);
        }
    }

    /// Rust's `f64::min` / `f64::max` into scratch register `dst` (not `a`, `b` or `xmm0`): a NaN
    /// operand yields the other one.
    fn min_max(&mut self, max: bool, dst: u8, a: u8, b: u8) {
        self.mov(dst, a);
        if max {
            dynasm!(self.ops ; .arch x64 ; maxsd Rx(dst), Rx(b));
        } else {
            dynasm!(self.ops ; .arch x64 ; minsd Rx(dst), Rx(b));
        }
        // minsd/maxsd return their second operand for a NaN; b being NaN must give a
        self.mov(0, b);
        dynasm!(self.ops
            ; .arch x64
            ; cmpsd xmm0, xmm0, UNORD
            ; blendvpd Rx(dst), Rx(a));
    }

    /// `xmm0 = mask(a op b)`, all ones where the comparison holds.
    fn mask(&mut self, op: Cmp, a: u8, b: u8) {
        let (first, second, predicate) = match op {
            Cmp::Lt => (a, b, LT),
            Cmp::Le => (a, b, LE),
            Cmp::Gt => (b, a, LT),
            Cmp::Ge => (b, a, LE),
            Cmp::Eq => (a, b, EQ),
            Cmp::Ne => (a, b, NEQ),
        };
        self.mov(0, first);
        dynasm!(self.ops ; .arch x64 ; cmpsd xmm0, Rx(second), predicate);
    }

    /// `xmm0 = 1.0` where the mask in `xmm0` is set, else 0.0.
    fn mask_to_bool(&mut self) {
        self.gpr(1.0f64.to_bits());
        dynasm!(self.ops ; .arch x64 ; movq xmm1, rcx ; andpd xmm0, xmm1);
    }

    /// `rax = wrapped_index(value in Rx(a), len)`. Clobbers `xmm0`-`xmm3` and `rcx`; the rare
    /// out-of-range case calls the host and keeps everything else.
    fn wrap(&mut self, a: u8, len: usize) {
        let neg = self.ops.new_dynamic_label();
        let inrange = self.ops.new_dynamic_label();
        let fast_sub = self.ops.new_dynamic_label();
        let slow = self.ops.new_dynamic_label();
        let done = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch x64 ; roundsd xmm0, Rx(a), 9);
        self.constant(1, len as f64);
        dynasm!(self.ops
            ; .arch x64
            ; xorpd xmm2, xmm2
            ; ucomisd xmm0, xmm2
            ; jp =>slow
            ; jb =>neg
            ; ucomisd xmm0, xmm1
            ; jb =>inrange
            ; movaps xmm3, xmm0
            ; subsd xmm3, xmm1
            ; ucomisd xmm3, xmm1
            ; jb =>fast_sub
            ; jmp =>slow
            ; =>neg
            ; movaps xmm3, xmm0
            ; addsd xmm3, xmm1
            ; ucomisd xmm3, xmm2
            ; jb =>slow
            ; ucomisd xmm3, xmm1
            ; jb =>fast_sub
            ; jmp =>slow
            ; =>inrange
            ; cvttsd2si rax, xmm0
            ; jmp =>done
            ; =>fast_sub
            ; cvttsd2si rax, xmm3
            ; jmp =>done
            ; =>slow);
        // xmm0 = the floor, xmm1 = n: exactly the host function's arguments
        self.call(host_wrap as *const () as usize, 0);
        dynasm!(self.ops ; .arch x64 ; =>done);
    }

    /// `Rx(t) = list[rax]`.
    fn element(&mut self, t: u8, slot: usize) {
        let off = (slot * 8) as i32;
        dynasm!(self.ops
            ; .arch x64
            ; mov rdx, [r13 + off]
            ; movsd Rx(t), [rdx + rax * 8]);
    }

    /// `t = f(a)` for the one-argument functions.
    fn unary(&mut self, f: Fn1, a: u8, t: u8) {
        match f {
            Fn1::Abs => {
                self.gpr(0x7fff_ffff_ffff_ffff);
                dynasm!(self.ops ; .arch x64 ; movq xmm4, rcx);
                self.mov(5, a);
                dynasm!(self.ops ; .arch x64 ; andpd xmm5, xmm4);
                self.mov(t, 5);
            }
            Fn1::Floor => dynasm!(self.ops ; .arch x64 ; roundsd Rx(t), Rx(a), 9),
            Fn1::Ceil => dynasm!(self.ops ; .arch x64 ; roundsd Rx(t), Rx(a), 10),
            Fn1::Sqrt => {
                // x.max(0.0).sqrt(): a NaN gives 0
                dynasm!(self.ops ; .arch x64 ; xorpd xmm4, xmm4);
                self.min_max(true, 5, a, 4);
                dynasm!(self.ops ; .arch x64 ; sqrtsd Rx(t), xmm5);
            }
            Fn1::Fract => {
                dynasm!(self.ops ; .arch x64 ; roundsd xmm5, Rx(a), 9);
                self.mov(4, a);
                dynasm!(self.ops ; .arch x64 ; subsd xmm4, xmm5);
                self.mov(t, 4);
            }
            Fn1::Sign => {
                // 1 where x > 0, -1 where x < 0, else 0 (a NaN gives 0)
                dynasm!(self.ops ; .arch x64 ; xorpd xmm4, xmm4);
                self.mov(0, 4);
                dynasm!(self.ops ; .arch x64 ; cmpsd xmm0, Rx(a), LT);
                self.gpr(1.0f64.to_bits());
                dynasm!(self.ops ; .arch x64 ; movq xmm5, rcx ; andpd xmm0, xmm5);
                self.mov(5, a);
                dynasm!(self.ops ; .arch x64 ; cmpsd xmm5, xmm4, LT);
                self.gpr((-1.0f64).to_bits());
                dynasm!(self.ops ; .arch x64 ; movq xmm4, rcx ; andpd xmm5, xmm4 ; orpd xmm0, xmm5);
                self.mov(t, 0);
            }
            f => {
                self.mov(0, a);
                self.call(host_f1 as *const () as usize, fn_id(f));
                self.mov(t, 0);
            }
        }
    }

    /// `t = a op b`.
    fn binary(&mut self, op: BinOp, a: u8, b: u8, t: u8) {
        match op {
            BinOp::Add => self.arith(Sse::Add, a, b, t),
            BinOp::Sub => self.arith(Sse::Sub, a, b, t),
            BinOp::Mul => self.arith(Sse::Mul, a, b, t),
            BinOp::Div => {
                // a / b, or 0 where b is 0
                self.mov(4, a);
                dynasm!(self.ops
                    ; .arch x64
                    ; divsd xmm4, Rx(b)
                    ; xorpd xmm5, xmm5
                    ; cmpsd xmm5, Rx(b), EQ
                    ; andnpd xmm5, xmm4);
                self.mov(t, 5);
            }
            BinOp::Min => {
                self.min_max(false, 5, a, b);
                self.mov(t, 5);
            }
            BinOp::Max => {
                self.min_max(true, 5, a, b);
                self.mov(t, 5);
            }
            BinOp::Pow | BinOp::Atan2 | BinOp::Rem => {
                // operands are never xmm0 (scratch operands start at xmm1)
                self.mov(0, a);
                self.mov(1, b);
                self.call(host_bin as *const () as usize, bin_id(op));
                self.mov(t, 0);
            }
        }
    }
}

pub(super) fn generate(kernel: &Kernel, a: &Analysis) -> Result<Generated, String> {
    if !is_x86_feature_detected!("sse4.1") {
        return Err("the CPU has no SSE4.1".into());
    }
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
    // rbx regs, r12 state, r13 lists, r14 sample, r15 frames, rbp memo
    let lists_arg = FRAME + 6 * 8 + 8;
    dynasm!(g.ops
        ; .arch x64
        ; push rbp
        ; push rbx
        ; push r12
        ; push r13
        ; push r14
        ; push r15
        ; sub rsp, FRAME
        ; mov rbx, rdi
        ; mov r12, rsi
        ; mov [rsp + INPUTS], rdx
        ; mov [rsp + OUTPUTS], rcx
        ; mov rbp, r8
        ; mov r15, r9
        ; mov r13, [rsp + lists_arg]
        ; movsd [rsp + SAMPLE_RATE], xmm0
        ; xor r14, r14);
    for v in 0..g.alloc.len() {
        if let (Some(x), false) = (g.alloc[v], g.temp[v]) {
            match g.hoist[v] {
                Some(value) => g.constant(x, value),
                None => g.load(x, v),
            }
        }
    }
    dynasm!(g.ops
        ; .arch x64
        ; test r15, r15
        ; jz =>done
        ; =>top);
    for (index, instr) in kernel.code.iter().enumerate() {
        dynasm!(g.ops ; .arch x64 ; =>labels[index]);
        match *instr {
            Instr::Const { dst, .. } if g.hoist[dst].is_some() => {}
            Instr::Const { dst, value } => {
                let t = g.target(dst, 0);
                g.constant(t, value);
                g.finish(dst, t);
            }
            Instr::Input { dst, port } => {
                let t = g.target(dst, 0);
                let off = (port * 8) as i32;
                dynasm!(g.ops
                    ; .arch x64
                    ; mov rax, [rsp + INPUTS]
                    ; mov rax, [rax + off]
                    ; movss xmm1, [rax + r14 * 4]
                    ; cvtss2sd Rx(t), xmm1);
                g.finish(dst, t);
            }
            Instr::SampleRate { dst } => {
                let t = g.target(dst, 0);
                dynasm!(g.ops ; .arch x64 ; movsd Rx(t), [rsp + SAMPLE_RATE]);
                g.finish(dst, t);
            }
            Instr::LoadState { dst, slot } => {
                let s = g.read(kernel.registers + slot, 1);
                let t = g.target(dst, 0);
                g.mov(t, s);
                g.finish(dst, t);
            }
            Instr::StoreState { slot, src } => {
                let s = g.read(src, 1);
                let v = kernel.registers + slot;
                match g.alloc[v] {
                    Some(x) => g.mov(x, s),
                    None => g.store(s, v),
                }
            }
            Instr::Copy { dst, src } => {
                let s = g.read(src, 1);
                let t = g.target(dst, 0);
                g.mov(t, s);
                g.finish(dst, t);
            }
            Instr::Neg { dst, a } => {
                let x = g.read(a, 1);
                let t = g.target(dst, 0);
                g.gpr(0x8000_0000_0000_0000);
                dynasm!(g.ops ; .arch x64 ; movq xmm4, rcx);
                g.mov(5, x);
                dynasm!(g.ops ; .arch x64 ; xorpd xmm5, xmm4);
                g.mov(t, 5);
                g.finish(dst, t);
            }
            Instr::Bin { dst, op, a, b } => {
                let x = g.read(a, 1);
                let y = g.read(b, 2);
                let t = g.target(dst, 0);
                g.binary(op, x, y, t);
                g.finish(dst, t);
            }
            Instr::MulAdd { dst, a, b, c } | Instr::MulSub { dst, a, b, c } => {
                let x = g.read(a, 1);
                let y = g.read(b, 2);
                let z = g.read(c, 3);
                let t = g.target(dst, 0);
                g.mov(4, x);
                dynasm!(g.ops ; .arch x64 ; mulsd xmm4, Rx(y));
                if matches!(instr, Instr::MulAdd { .. }) {
                    dynasm!(g.ops ; .arch x64 ; addsd xmm4, Rx(z));
                } else {
                    dynasm!(g.ops ; .arch x64 ; subsd xmm4, Rx(z));
                }
                g.mov(t, 4);
                g.finish(dst, t);
            }
            Instr::SubMul { dst, a, b, c } => {
                let x = g.read(a, 1);
                let y = g.read(b, 2);
                let z = g.read(c, 3);
                let t = g.target(dst, 0);
                g.mov(4, x);
                dynasm!(g.ops ; .arch x64 ; mulsd xmm4, Rx(y));
                g.mov(5, z);
                dynasm!(g.ops ; .arch x64 ; subsd xmm5, xmm4);
                g.mov(t, 5);
                g.finish(dst, t);
            }
            Instr::Clamp { dst, x, lo, hi } => {
                let a = g.read(x, 1);
                let l = g.read(lo, 2);
                let h = g.read(hi, 3);
                let t = g.target(dst, 0);
                g.min_max(true, 4, a, l);
                g.min_max(false, 5, 4, h);
                g.mov(t, 5);
                g.finish(dst, t);
            }
            Instr::Cmp { dst, op, a, b } => {
                let x = g.read(a, 1);
                let y = g.read(b, 2);
                let t = g.target(dst, 0);
                g.mask(op, x, y);
                g.mask_to_bool();
                g.mov(t, 0);
                g.finish(dst, t);
            }
            Instr::Select { dst, cond, a, b } => {
                let x = g.read(a, 1);
                let y = g.read(b, 2);
                let c = g.read(cond, 3);
                let t = g.target(dst, 0);
                dynasm!(g.ops ; .arch x64 ; xorpd xmm0, xmm0 ; cmpsd xmm0, Rx(c), NEQ);
                g.mov(4, y);
                dynasm!(g.ops ; .arch x64 ; blendvpd xmm4, Rx(x));
                g.mov(t, 4);
                g.finish(dst, t);
            }
            Instr::Call { dst, f, a } => {
                let x = g.read(a, 1);
                let t = g.target(dst, 0);
                g.unary(f, x, t);
                g.finish(dst, t);
            }
            Instr::SkipIfZero { cond, count } => {
                if count > 0 {
                    let c = g.read(cond, 1);
                    let target = labels[index + 1 + count];
                    dynasm!(g.ops
                        ; .arch x64
                        ; xorpd xmm0, xmm0
                        ; cmpsd xmm0, Rx(c), EQ
                        ; movq rax, xmm0
                        ; test rax, rax
                        ; jnz =>target);
                }
            }
            Instr::CmpSkip {
                dst,
                op,
                a,
                b,
                count,
            } => {
                let x = g.read(a, 1);
                let y = g.read(b, 2);
                let t = g.target(dst, 0);
                g.mask(op, x, y);
                dynasm!(g.ops ; .arch x64 ; movq rax, xmm0);
                g.mask_to_bool();
                g.mov(t, 0);
                g.finish(dst, t);
                if count > 0 {
                    let target = labels[index + 1 + count];
                    dynasm!(g.ops ; .arch x64 ; test rax, rax ; jz =>target);
                }
            }
            Instr::Read { dst, list, idx } => {
                let (slot, len) = list_slot(kernel, list)?;
                let a = g.read(idx, 1);
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
                let (slot, len) = list_slot(kernel, list)?;
                let p = g.read(pos, 1);
                let t = g.target(dst, 0);
                if interp == Interp::None {
                    g.wrap(p, len);
                    g.element(t, slot);
                } else {
                    let nonfinite = g.ops.new_dynamic_label();
                    let done = g.ops.new_dynamic_label();
                    let (b_slot, t_slot, y) = (TEMP, TEMP + 8, TEMP + 16);
                    // a non-finite position reads entry 0
                    dynasm!(g.ops
                        ; .arch x64
                        ; movaps xmm0, Rx(p)
                        ; subsd xmm0, Rx(p)
                        ; xorpd xmm2, xmm2
                        ; ucomisd xmm0, xmm2
                        ; jp =>nonfinite
                        ; roundsd xmm0, Rx(p), 9
                        ; movsd [rsp + b_slot], xmm0
                        ; movaps xmm2, Rx(p)
                        ; subsd xmm2, xmm0
                        ; movsd [rsp + t_slot], xmm2);
                    let offsets: &[f64] = if interp == Interp::Linear {
                        &[0.0, 1.0]
                    } else {
                        &[-1.0, 0.0, 1.0, 2.0]
                    };
                    for (k, offset) in offsets.iter().enumerate() {
                        let at = y + (k as i32) * 8;
                        g.constant(1, *offset);
                        dynasm!(g.ops
                            ; .arch x64
                            ; movsd xmm0, [rsp + b_slot]
                            ; addsd xmm0, xmm1);
                        g.wrap(0, len);
                        g.element(0, slot);
                        dynasm!(g.ops ; .arch x64 ; movsd [rsp + at], xmm0);
                    }
                    if interp == Interp::Linear {
                        // y0 + (y1 - y0) * t
                        dynasm!(g.ops
                            ; .arch x64
                            ; movsd xmm1, [rsp + y + 8]
                            ; movsd xmm2, [rsp + y]
                            ; subsd xmm1, xmm2
                            ; mulsd xmm1, [rsp + t_slot]
                            ; addsd xmm2, xmm1);
                        g.mov(t, 2);
                    } else {
                        let mul = |g: &mut Gen<'_>, x: u8, c: f64, slot: i32| {
                            g.constant(x, c);
                            dynasm!(g.ops ; .arch x64 ; mulsd Rx(x), [rsp + slot]);
                        };
                        let (y0, y1, y2, y3) = (y, y + 8, y + 16, y + 24);
                        // a = -0.5*y0 + 1.5*y1 - 1.5*y2 + 0.5*y3, left to right
                        mul(&mut g, 0, -0.5, y0);
                        mul(&mut g, 1, 1.5, y1);
                        dynasm!(g.ops ; .arch x64 ; addsd xmm0, xmm1);
                        mul(&mut g, 1, 1.5, y2);
                        dynasm!(g.ops ; .arch x64 ; subsd xmm0, xmm1);
                        mul(&mut g, 1, 0.5, y3);
                        dynasm!(g.ops ; .arch x64 ; addsd xmm0, xmm1);
                        // b = y0 - 2.5*y1 + 2.0*y2 - 0.5*y3
                        dynasm!(g.ops ; .arch x64 ; movsd xmm2, [rsp + y0]);
                        mul(&mut g, 1, 2.5, y1);
                        dynasm!(g.ops ; .arch x64 ; subsd xmm2, xmm1);
                        mul(&mut g, 1, 2.0, y2);
                        dynasm!(g.ops ; .arch x64 ; addsd xmm2, xmm1);
                        mul(&mut g, 1, 0.5, y3);
                        dynasm!(g.ops ; .arch x64 ; subsd xmm2, xmm1);
                        // c = -0.5*y0 + 0.5*y2
                        mul(&mut g, 3, -0.5, y0);
                        mul(&mut g, 1, 0.5, y2);
                        dynasm!(g.ops ; .arch x64 ; addsd xmm3, xmm1);
                        // ((a*t + b)*t + c)*t + y1
                        dynasm!(g.ops
                            ; .arch x64
                            ; movsd xmm4, [rsp + t_slot]
                            ; mulsd xmm0, xmm4
                            ; addsd xmm0, xmm2
                            ; mulsd xmm0, xmm4
                            ; addsd xmm0, xmm3
                            ; mulsd xmm0, xmm4
                            ; addsd xmm0, [rsp + y1]);
                        g.mov(t, 0);
                    }
                    dynasm!(g.ops ; .arch x64 ; jmp =>done ; =>nonfinite);
                    let off = (slot * 8) as i32;
                    dynasm!(g.ops
                        ; .arch x64
                        ; mov rdx, [r13 + off]
                        ; movsd Rx(t), [rdx]
                        ; =>done);
                }
                g.finish(dst, t);
            }
            Instr::Write {
                list,
                idx,
                src,
                cond,
            } => {
                let (slot, len) = list_slot(kernel, ArraySource::State(list))?;
                let skip = g.ops.new_dynamic_label();
                if let Some(c) = cond {
                    let c = g.read(c, 3);
                    let go = g.ops.new_dynamic_label();
                    dynasm!(g.ops
                        ; .arch x64
                        ; xorpd xmm0, xmm0
                        ; ucomisd Rx(c), xmm0
                        ; jp =>go
                        ; je =>skip
                        ; =>go);
                }
                let a = g.read(idx, 1);
                g.wrap(a, len);
                let s = g.read(src, 2);
                let off = (slot * 8) as i32;
                dynasm!(g.ops
                    ; .arch x64
                    ; mov rdx, [r13 + off]
                    ; movsd [rdx + rax * 8], Rx(s)
                    ; =>skip);
            }
            Instr::Memo { dst, slot, math, a } => {
                let at = (slot * 32) as i32;
                let x = g.read(a, 1);
                let y = match math {
                    MemoMath::Binary(_, b) => Some(g.read(b, 2)),
                    MemoMath::Unary(_) => None,
                };
                let miss = g.ops.new_dynamic_label();
                let end = g.ops.new_dynamic_label();
                dynasm!(g.ops
                    ; .arch x64
                    ; movq rax, Rx(x)
                    ; cmp rax, [rbp + at]
                    ; jne =>miss);
                match y {
                    Some(y) => dynasm!(g.ops
                        ; .arch x64
                        ; movq rax, Rx(y)
                        ; cmp rax, [rbp + at + 8]
                        ; jne =>miss),
                    None => dynasm!(g.ops
                        ; .arch x64
                        ; cmp QWORD [rbp + at + 8], 0
                        ; jne =>miss),
                }
                dynasm!(g.ops
                    ; .arch x64
                    ; cmp QWORD [rbp + at + 16], 0
                    ; je =>miss
                    ; movsd xmm0, [rbp + at + 24]
                    ; jmp =>end
                    ; =>miss);
                match math {
                    MemoMath::Unary(f) => g.unary(f, x, 0),
                    MemoMath::Binary(op, _) => g.binary(op, x, y.unwrap_or(2), 0),
                }
                // a call may have clobbered the scratch registers: the operands are read back
                // from where they live, and the result is kept in xmm0 through the stores
                let x = g.read(a, 1);
                dynasm!(g.ops ; .arch x64 ; movq rax, Rx(x));
                match math {
                    MemoMath::Binary(_, b) => {
                        let y = g.read(b, 2);
                        dynasm!(g.ops ; .arch x64 ; movq rcx, Rx(y));
                    }
                    MemoMath::Unary(_) => dynasm!(g.ops ; .arch x64 ; xor ecx, ecx),
                }
                dynasm!(g.ops
                    ; .arch x64
                    ; mov [rbp + at], rax
                    ; mov [rbp + at + 8], rcx
                    ; mov QWORD [rbp + at + 16], 1
                    ; movsd [rbp + at + 24], xmm0
                    ; =>end);
                let t = g.target(dst, 0);
                g.mov(t, 0);
                g.finish(dst, t);
            }
            _ => return Err("unsupported instruction".into()),
        }
    }
    dynasm!(g.ops ; .arch x64 ; =>labels[len]);
    for (k, (_, reg)) in kernel.outputs.iter().enumerate() {
        let r = g.read(*reg, 1);
        let off = (k * 8) as i32;
        dynasm!(g.ops
            ; .arch x64
            ; cvtsd2ss xmm0, Rx(r)
            ; mov rax, [rsp + OUTPUTS]
            ; mov rax, [rax + off]
            ; movss [rax + r14 * 4], xmm0);
    }
    dynasm!(g.ops
        ; .arch x64
        ; add r14, 1
        ; cmp r14, r15
        ; jb =>top
        ; =>done);
    for v in 0..g.alloc.len() {
        if let (Some(x), false) = (g.alloc[v], g.temp[v]) {
            g.store(x, v);
        }
    }
    dynasm!(g.ops
        ; .arch x64
        ; add rsp, FRAME
        ; pop r15
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbx
        ; pop rbp
        ; ret);
    let bytes = g.ops.offset().0;
    let buffer = g
        .ops
        .finalize()
        .map_err(|_| "could not finalize".to_string())?;
    // SAFETY: the code at `entry` was assembled above with exactly the `Entry` signature and the
    // System V calling convention, and the buffer keeps it alive.
    let entry: Entry = unsafe { std::mem::transmute(buffer.ptr(entry)) };
    Ok(Generated {
        buffer,
        entry,
        bytes,
    })
}
