//! Native code for kernels: a dynasm code generator per CPU architecture over the same register IR.
//!
//! The generated function runs a whole block directly on the voice's register and state arrays,
//! its state lists and shared tables, and calls the same Rust helpers as the interpreter for maths
//! that is not an instruction (`sin`, `pow`...). A kernel that calls a built-in opcode, runs a
//! frame scope or a vector instruction, or has reset scopes is not generated and stays on the
//! interpreter. Output is bit-identical to the interpreter's; `tests/it/wasm_kernel.rs` fuzzes it.

// dynasm's expansions convert register numbers with `.into()`, even from `u8` to `u8`
#![allow(clippy::useless_conversion)]

use dynasmrt::ExecutableBuffer;

use super::{ArraySource, BinOp, Fn1, Instr, Kernel, MemoMath, NativeInput};

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
use aarch64 as arch;
#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "x86_64")]
use x64 as arch;

pub(super) const BINS: [BinOp; 9] = [
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

/// What a generated function needs to call a built-in opcode: the voice it belongs to and the
/// block's context. Built on the stack for each call of `JitVoice::render`.
pub(super) struct NativeState {
    custom: *mut super::Custom,
    ctx: crate::opcode::ProcessCtx,
}

/// The generated code's call into a built-in opcode: sample `index`'s result goes to `dst` onwards
/// in the voice's registers.
///
/// # Safety
/// `state` is the live [`NativeState`] of the `render` call that is running the generated code.
unsafe fn native_call(state: *mut NativeState, dst: u32, index: u32) {
    // SAFETY: the caller's contract; the generated code only calls this while `render` is running
    // it, and `render` does not touch the voice until the generated code returns.
    let state = unsafe { &mut *state };
    // SAFETY: as above; `custom` points to the voice being rendered.
    let custom = unsafe { &mut *state.custom };
    custom.exec_native_sample(dst as usize, index as usize, &state.ctx);
}

/// What a backend hands back.
struct Generated {
    buffer: ExecutableBuffer,
    entry: arch::Entry,
    bytes: usize,
}

/// What the register allocator decided, which every backend follows.
pub(super) struct Analysis {
    pub alloc: Vec<Option<u8>>,
    /// Registers that need no load at the start of the block or store at its end.
    pub temp: Vec<bool>,
    /// Constants set once before the loop instead of every sample.
    pub hoist: Vec<Option<f64>>,
}

/// Slot in the list table and length of a state list or shared table the generators can read.
fn list_slot(kernel: &Kernel, list: ArraySource) -> Result<(usize, usize), String> {
    let buffers = kernel.buffers.len();
    let (slot, len) = match list {
        ArraySource::State(l) => (l, kernel.buffers.get(l).map(|b| b.len())),
        ArraySource::Table(t) => (buffers + t, kernel.tables_f64.get(t).map(|b| b.len())),
        _ => return Err("a list the code generator does not handle".into()),
    };
    match len {
        Some(len) if len > 0 => Ok((slot, len)),
        _ => Err("empty or unknown list".into()),
    }
}

/// A compiled kernel, shared by its voices.
pub struct JitKernel {
    _buffer: ExecutableBuffer,
    entry: arch::Entry,
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

/// What an instruction reads and writes, as virtual registers (registers, then state slots).
fn touches(kernel: &Kernel, instr: &Instr) -> (Vec<usize>, Vec<usize>) {
    let r = kernel.registers;
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
        Instr::Native { dst, index, guard } => {
            let call = &kernel.natives[index];
            let mut reads: Vec<usize> = call
                .inputs
                .iter()
                .filter_map(|input| match *input {
                    NativeInput::Reg(r) => Some(r),
                    NativeInput::Const(_) => None,
                })
                .collect();
            let writes: Vec<usize> = (dst..dst + call.op.outputs()).collect();
            if let Some(g) = guard {
                // skipped when the guard is zero, which leaves the results as they were
                reads.push(g);
                reads.extend(&writes);
            }
            (reads, writes)
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
fn allocate(kernel: &Kernel, registers: &[u8]) -> Analysis {
    let r = kernel.registers;
    let n = r + kernel.state_init.len();
    let len = kernel.code.len();
    let mut uses = vec![0usize; n];
    let mut first_write: Vec<Option<usize>> = vec![None; n];
    let mut first_read: Vec<Option<usize>> = vec![None; n];
    let mut last: Vec<usize> = vec![0; n];
    let mut write_at: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut read_at: Vec<Vec<usize>> = vec![Vec::new(); n];
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
        let (reads, writes) = touches(kernel, instr);
        for v in reads {
            uses[v] += 1;
            read_at[v].push(index);
            first_read[v].get_or_insert(index);
            last[v] = index;
        }
        for v in writes {
            uses[v] += 1;
            write_at[v].push(index);
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
        read_at[*reg].push(len);
        first_read[*reg].get_or_insert(len);
        last[*reg] = len;
    }
    // The first instruction that cannot run once the one at `w` has: the nearest end of a skip that
    // covers `w`. A read before that point runs only if `w` did, so it sees this sample's write.
    let covered_until: Vec<usize> = (0..len)
        .map(|w| {
            region_end[..w]
                .iter()
                .filter(|&&end| end > w)
                .copied()
                .min()
                .unwrap_or(usize::MAX)
        })
        .collect();
    // a constant written once, unconditionally, before any read is the same every sample: it is
    // put in its register once, before the loop
    let mut writes = vec![0usize; n];
    for instr in &kernel.code {
        for v in touches(kernel, instr).1 {
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
        // a temporary: every read is of a value this sample wrote, whichever branch ran
        let is_temp = v < r
            && hoist[v].is_none()
            && !write_at[v].is_empty()
            && read_at[v].iter().all(|&rd| {
                write_at[v]
                    .iter()
                    .any(|&w| w < rd && rd < covered_until[w])
            });
        temp[v] = is_temp;
        let (start, end) = if is_temp {
            (write_at[v].iter().copied().min().unwrap_or(0), last[v])
        } else {
            (0, len)
        };
        intervals.push((start, end, v));
    }
    intervals.sort_by_key(|&(start, end, v)| (start, std::cmp::Reverse(uses[v] * 1000 / (end - start + 1)), v));
    let weight = |v: usize, start: usize, end: usize| uses[v] as f64 / (end - start + 1) as f64;
    let mut alloc: Vec<Option<u8>> = vec![None; n];
        let mut free: Vec<u8> = registers.iter().rev().copied().collect();
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
    Analysis { alloc, temp, hoist }
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
                | Instr::Native { .. }
                | Instr::Read { .. }
                | Instr::ReadAt { .. }
                | Instr::Write { .. }
                | Instr::SkipIfZero { .. }
                | Instr::CmpSkip { .. } => {}
                other => {
                    let name = format!("{other:?}");
                    let name = name.split([' ', '{', '(']).next().unwrap_or("an instruction");
                    return Err(format!("{name} is not generated"));
                }
            }
        }
        if kernel.registers + kernel.state_init.len() > 4000 {
            return Err("too many registers for the generated code's immediate offsets".into());
        }
        if std::env::var("NIMINAL_JIT_DUMP").as_deref() == Ok(&kernel.name) {
            for (i, instr) in kernel.code.iter().enumerate() {
                eprintln!("{i:3} {instr:?}");
            }
        }
        let analysis = allocate(kernel, arch::ALLOCATABLE);
        let allocated = analysis.alloc.iter().flatten().count();
        let generated = arch::generate(kernel, &analysis)?;
        Ok(JitKernel {
            _buffer: generated.buffer,
            entry: generated.entry,
            n_ports: kernel.ports.len(),
            n_outputs: kernel.outputs.len(),
            memo_slots: kernel.memo_slots,
            n_lists: kernel.buffers.len() + kernel.tables_f64.len(),
            bytes: generated.bytes,
            allocated,
        })
    }

    /// Runs `frames` samples.
    ///
    /// # Safety
    /// `regs` and `state` must hold the kernel's registers and state slots, `inputs` one pointer
    /// per port to `frames` samples, `outputs` one pointer per output to room for `frames`
    /// samples, `memo` four words per memo slot, and `lists` the base address of each state list
    /// followed by each shared table, each as long as the kernel says, and `native` a live
    /// [`NativeState`] for the voice being rendered.
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
        native: *mut NativeState,
        sample_rate: f64,
    ) {
        // SAFETY: forwarded from the caller's contract.
        unsafe {
            (self.entry)(
                regs, state, inputs, outputs, memo, frames, lists, native, sample_rate,
            )
        }
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
    /// The most samples one call may render: what the input buffers hold.
    capacity: usize,
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
            capacity: block,
            kernel,
        })
    }

    /// Renders `frames` samples, or returns `false` without touching anything when the buffers
    /// are not what the generated code assumes (more samples than the block size this voice was
    /// made for, or fewer output buffers than outputs): the caller then runs the interpreter,
    /// which works on the same registers, state and lists.
    pub fn render(
        &mut self,
        custom: &mut super::Custom,
        ctx: &crate::opcode::ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        outs: &mut [Vec<f32>],
        frames: usize,
    ) -> bool {
        if frames > self.capacity
            || outs.len() < self.out_ptrs.len()
            || outs.iter().take(self.out_ptrs.len()).any(|o| o.len() < frames)
        {
            return false;
        }
        for (port, samples) in self.inputs.iter_mut().enumerate() {
            for (i, sample) in samples.iter_mut().enumerate().take(frames) {
                *sample = input(port, i);
            }
            self.in_ptrs[port] = samples.as_ptr() as usize;
        }
        for (k, out) in outs.iter_mut().enumerate().take(self.out_ptrs.len()) {
            self.out_ptrs[k] = out.as_mut_ptr() as usize;
        }
        let sample_rate = custom.sample_rate;
        let buffers = custom.buffers.len();
        for (l, list) in custom.buffers.iter_mut().enumerate() {
            self.list_ptrs[l] = list.as_mut_ptr() as usize;
        }
        for (t, table) in custom.kernel.tables_f64.iter().enumerate() {
            self.list_ptrs[buffers + t] = table.as_ptr() as usize;
        }
        // every pointer the generated code gets comes from this one, so none of them is invalidated
        // by another being made
        let raw = std::ptr::from_mut(custom);
        let mut native = NativeState {
            custom: raw,
            ctx: *ctx,
        };
        // SAFETY: the pointer tables hold one valid pointer per port and output, each to at
        // least `frames` samples (the inputs were just written; `outs` are block-sized); the
        // register and state vectors are the kernel's own, and the memo has four words per slot.
        // `usize` and pointers have the same layout.
        unsafe {
            self.kernel.run(
                (*raw).regs.as_mut_ptr(),
                (*raw).state.as_mut_ptr(),
                self.in_ptrs.as_ptr().cast::<*const f32>(),
                self.out_ptrs.as_ptr().cast::<*mut f32>(),
                self.memo.as_mut_ptr(),
                frames,
                self.list_ptrs.as_ptr().cast::<*mut f64>(),
                &mut native,
                sample_rate,
            );
        }
        true
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
