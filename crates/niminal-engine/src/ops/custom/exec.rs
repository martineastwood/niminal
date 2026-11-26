//! Kernel interpreter loop, cold instruction handlers, and frame-scope execution.

use realfft::num_complex::Complex;

use crate::opcode::{MAX_INPUTS, ProcessCtx};
use crate::{Array, wrapped_index};

use super::super::lanes as lane_ops;
#[cfg(feature = "kernel-profile")]
use super::profile;
use super::{
    ArraySource, BinOp, Custom, Fn1, Instr, Interp, LaneSrc, MemoEntry, MemoMath, NativeInput,
    Reduce, read_array, read_at,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Init,
    Block,
    Sample,
}

impl Custom {
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
        let carried_init = section == Section::Init && self.carried;
        // most kernels have no reset scope, and then no instruction needs checking for one
        match (self.kernel.resets.is_empty(), carried_init) {
            (true, false) => self.exec::<false, false>(code, section, ctx, input, i),
            (true, true) => self.exec::<false, true>(code, section, ctx, input, i),
            (false, false) => self.exec::<true, false>(code, section, ctx, input, i),
            (false, true) => self.exec::<true, true>(code, section, ctx, input, i),
        }
    }

    fn exec<const RESETS: bool, const CARRIED_INIT: bool>(
        &mut self,
        code: &[Instr],
        section: Section,
        ctx: &ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        i: usize,
    ) {
        let mut instructions = code.iter();
        #[cfg(feature = "kernel-profile")]
        let mut previous = None;
        while let Some(instr) = instructions.next() {
            #[cfg(feature = "kernel-profile")]
            if section == Section::Sample {
                profile::record(
                    instr,
                    code.len() - instructions.as_slice().len() - 1,
                    &mut previous,
                );
            }
            let (regs, state, buffers) = (&mut self.regs, &mut self.state, &mut self.buffers);
            match *instr {
                Instr::Frame {
                    scope,
                    input: x,
                    input2,
                    dst,
                } => {
                    self.run_frame(scope, (x, input2), dst, ctx, input, i);
                }
                Instr::Reset { .. } => {
                    if RESETS {
                        self.exec_cold(instr, section, ctx, input, i);
                    }
                }
                Instr::BeginBranch { .. } | Instr::EndBranch => {
                    unreachable!("branch markers are resolved before rendering")
                }
                Instr::SkipIfZero { cond, count } => {
                    if regs[cond] == 0.0 && count > 0 {
                        instructions.nth(count - 1);
                    }
                }
                Instr::CmpSkip {
                    dst,
                    op,
                    a,
                    b,
                    count,
                } => {
                    let holds = op.holds(regs[a], regs[b]);
                    regs[dst] = f64::from(holds);
                    if !holds && count > 0 {
                        instructions.nth(count - 1);
                    }
                }
                Instr::Const { dst, value } => regs[dst] = value,
                Instr::Input { dst, port } => regs[dst] = f64::from(input(port, i)),
                Instr::SampleRate { dst } => regs[dst] = self.sample_rate,
                Instr::LoadState { dst, slot } => regs[dst] = state[slot],
                Instr::StoreState { slot, src } => {
                    if !CARRIED_INIT {
                        state[slot] = regs[src];
                    }
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Add,
                    a,
                    b,
                } => regs[dst] = regs[a] + regs[b],
                Instr::Bin {
                    dst,
                    op: BinOp::Sub,
                    a,
                    b,
                } => regs[dst] = regs[a] - regs[b],
                Instr::Bin {
                    dst,
                    op: BinOp::Mul,
                    a,
                    b,
                } => {
                    regs[dst] = regs[a] * regs[b];
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Div,
                    a,
                    b,
                } => {
                    regs[dst] = if regs[b] == 0.0 {
                        0.0
                    } else {
                        regs[a] / regs[b]
                    };
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Min,
                    a,
                    b,
                } => regs[dst] = regs[a].min(regs[b]),
                Instr::Bin {
                    dst,
                    op: BinOp::Max,
                    a,
                    b,
                } => regs[dst] = regs[a].max(regs[b]),
                Instr::Bin {
                    dst,
                    op: BinOp::Rem,
                    a,
                    b,
                } => {
                    let y = regs[b];
                    regs[dst] = if y == 0.0 {
                        0.0
                    } else {
                        let x = regs[a];
                        x - y * (x / y).floor()
                    };
                }
                Instr::Bin { dst, op, a, b } => regs[dst] = eval_bin_cold(op, regs[a], regs[b]),
                Instr::MulAdd { dst, a, b, c } => {
                    let product = regs[a] * regs[b];
                    regs[dst] = product + regs[c];
                }
                Instr::MulSub { dst, a, b, c } => {
                    let product = regs[a] * regs[b];
                    regs[dst] = product - regs[c];
                }
                Instr::SubMul { dst, a, b, c } => {
                    let product = regs[a] * regs[b];
                    regs[dst] = regs[c] - product;
                }
                Instr::Clamp { dst, x, lo, hi } => {
                    regs[dst] = regs[x].max(regs[lo]).min(regs[hi]);
                }
                Instr::Cmp { dst, op, a, b } => {
                    regs[dst] = if op.holds(regs[a], regs[b]) { 1.0 } else { 0.0 };
                }
                Instr::Select { dst, cond, a, b } => {
                    regs[dst] = if regs[cond] != 0.0 { regs[a] } else { regs[b] };
                }
                Instr::Memo { dst, slot, math, a } => {
                    let x = regs[a];
                    let y = match math {
                        MemoMath::Unary(_) => 0.0,
                        MemoMath::Binary(_, b) => regs[b],
                    };
                    let key = [x.to_bits(), y.to_bits()];
                    let entry = &mut self.memo[slot];
                    let value = match *entry {
                        Some(hit) if hit.key == key => hit.value,
                        _ => eval_memo_miss(entry, key, math, x, y),
                    };
                    regs[dst] = value;
                }
                Instr::Neg { dst, a } => regs[dst] = -regs[a],
                Instr::Copy { dst, src } => regs[dst] = regs[src],
                Instr::Call {
                    dst,
                    f: Fn1::Floor,
                    a,
                } => regs[dst] = regs[a].floor(),
                Instr::Call {
                    dst,
                    f: Fn1::Fract,
                    a,
                } => {
                    let x = regs[a];
                    regs[dst] = x - x.floor();
                }
                Instr::Call {
                    dst,
                    f: Fn1::Abs,
                    a,
                } => regs[dst] = regs[a].abs(),
                Instr::Call {
                    dst,
                    f: Fn1::Ceil,
                    a,
                } => regs[dst] = regs[a].ceil(),
                Instr::Call {
                    dst,
                    f: Fn1::Sqrt,
                    a,
                } => regs[dst] = regs[a].max(0.0).sqrt(),
                Instr::Call {
                    dst,
                    f: Fn1::Sign,
                    a,
                } => {
                    let x = regs[a];
                    regs[dst] = if x > 0.0 {
                        1.0
                    } else if x < 0.0 {
                        -1.0
                    } else {
                        0.0
                    };
                }
                Instr::Call { dst, f, a } => regs[dst] = eval_fn1_cold(f, regs[a]),
                Instr::Native { .. }
                | Instr::Read {
                    list: ArraySource::Lane(_),
                    ..
                }
                | Instr::ReadAt {
                    list: ArraySource::Values(_) | ArraySource::Lane(_),
                    ..
                }
                | Instr::LaneBin { .. }
                | Instr::LaneCall { .. }
                | Instr::LaneFromRegs { .. }
                | Instr::LaneStore { .. }
                | Instr::Reduce { .. }
                | Instr::Dot { .. }
                | Instr::Fir { .. }
                | Instr::Taps { .. }
                | Instr::Fft { .. }
                | Instr::Ifft { .. }
                | Instr::MatVec { .. } => self.exec_cold(instr, section, ctx, input, i),
                Instr::Read {
                    dst,
                    list: ArraySource::State(s),
                    idx,
                } => {
                    // the usual case: a whole-number position in a state list
                    let data = &buffers[s];
                    regs[dst] = data[wrapped_index(regs[idx], data.len())];
                }
                Instr::Read {
                    dst,
                    list: ArraySource::Table(t),
                    idx,
                } => {
                    let data = &self.kernel.tables_f64[t];
                    regs[dst] = data[wrapped_index(regs[idx], data.len())];
                }
                Instr::Read {
                    dst,
                    list: ArraySource::Values(a),
                    idx,
                } => {
                    let regs_list = &self.kernel.values[a];
                    regs[dst] = regs[regs_list[wrapped_index(regs[idx], regs_list.len())]];
                }
                Instr::ReadAt {
                    dst,
                    list: ArraySource::State(s),
                    pos: idx,
                    interp,
                } => {
                    regs[dst] = read_at(&buffers[s], regs[idx], interp);
                }
                Instr::ReadAt {
                    dst,
                    list: ArraySource::Table(t),
                    pos: idx,
                    interp,
                } => {
                    regs[dst] = read_at(&self.kernel.tables_f64[t], regs[idx], interp);
                }
                Instr::Write {
                    list,
                    idx,
                    src,
                    cond,
                } => {
                    if !CARRIED_INIT && cond.is_none_or(|c| regs[c] != 0.0) {
                        let data = &mut buffers[list];
                        let n = data.len();
                        data[wrapped_index(regs[idx], n)] = regs[src];
                    }
                }
            }
        }
    }

    /// One sample of the kernel's `index`th built-in opcode, its inputs read from the registers and
    /// its results written to `dst` onwards. The sample-section half of [`Instr::Native`]; the code
    /// generators call it for the opcodes a kernel uses.
    pub(super) fn exec_native_sample(&mut self, dst: usize, index: usize, ctx: &ProcessCtx) {
        let call = &self.kernel.natives[index];
        let op = &mut self.natives[index];
        if self.one_sample[index] {
            let mut x = [0.0f32; MAX_INPUTS];
            for (value, input) in x.iter_mut().zip(&call.inputs) {
                *value = match *input {
                    NativeInput::Reg(r) => self.regs[r] as f32,
                    NativeInput::Const(c) => c as f32,
                };
            }
            self.regs[dst] = f64::from(op.process_one(ctx, &x[..call.inputs.len()]));
            return;
        }
        for (value, input) in self.native_inputs.iter_mut().zip(&call.inputs) {
            value[0] = match *input {
                NativeInput::Reg(r) => self.regs[r] as f32,
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
            self.regs[dst + k] = f64::from(output[0]);
        }
    }

    /// The instructions that are rare or heavy, kept out of the hot loop so that it stays small.
    #[inline(never)]
    fn exec_cold(
        &mut self,
        instr: &Instr,
        section: Section,
        ctx: &ProcessCtx,
        input: &impl Fn(usize, usize) -> f32,
        i: usize,
    ) {
        let k = &*self.kernel;
        let (regs, state, buffers, lanes, plans) = (
            &mut self.regs,
            &mut self.state,
            &mut self.buffers,
            &mut self.lanes,
            &mut self.fft_plans,
        );
        match *instr {
            Instr::Reset {
                scope,
                trigger,
                previous,
                guard,
            } => {
                if guard.is_none_or(|g| regs[g] != 0.0) {
                    let positive = regs[trigger] > 0.0;
                    let edge = positive && state[previous] <= 0.0;
                    state[previous] = if positive { 1.0 } else { 0.0 };
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
            }
            Instr::Native { dst, index, guard } => {
                if guard.is_some_and(|g| regs[g] == 0.0) {
                    return;
                }
                let call = &k.natives[index];
                // a native run once per block (a control scope) sees its inputs held through it
                if section != Section::Block {
                    self.exec_native_sample(dst, index, ctx);
                    return;
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
                    for ((c, r), i) in p.spectrum.iter().zip(dre.iter_mut()).zip(dim.iter_mut()) {
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
            _ => unreachable!("handled by the hot loop"),
        }
    }

    /// One sample through a frame scope: the input goes in, the output that is due comes out, and each
    /// hop the scope's code runs on the latest frame.
    fn run_frame(
        &mut self,
        scope: usize,
        sources: (usize, Option<usize>),
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
        run.input[run.pos] = self.regs[sources.0];
        if let Some(x) = sources.1 {
            run.input2[run.pos] = self.regs[x];
        }
        run.pos = (run.pos + 1) % n;
        if !run.pos.is_multiple_of(frame.hop) {
            return;
        }
        let pos = run.pos;
        let window = &mut self.lanes[frame.window];
        for (k, w) in window.iter_mut().enumerate() {
            *w = run.input[(pos + k) % n] * run.window[k];
        }
        if let Some(window2) = frame.window2 {
            for (k, w) in self.lanes[window2].iter_mut().enumerate() {
                *w = run.input2[(pos + k) % n] * run.window[k];
            }
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
                    run.output[(pos + k) % n] += v * run.synthesis[k];
                }
            }
            lane_ops::Operand::Scalar(v) => {
                for k in 0..n {
                    run.output[(pos + k) % n] += v * run.synthesis[k];
                }
            }
        }
    }

    pub(super) fn process_with(
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
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(mut jit) = self.jit.take() {
            let done = jit.render(self, ctx, &input, outs, frames);
            self.jit = Some(jit);
            if done {
                return;
            }
        }
        #[cfg(feature = "wasm-kernel")]
        if let Some(mut wasm) = self.wasm.take() {
            wasm.render(self, &input, outs, frames);
            self.wasm = Some(wasm);
            return;
        }
        let kernel = self.kernel.clone();
        if kernel.resets.is_empty() {
            for i in 0..frames {
                self.exec::<false, false>(&kernel.code, Section::Sample, ctx, &input, i);
                for (out, (_, reg)) in outs.iter_mut().zip(&kernel.outputs) {
                    out[i] = self.regs[*reg] as f32;
                }
            }
        } else {
            for i in 0..frames {
                self.exec::<true, false>(&kernel.code, Section::Sample, ctx, &input, i);
                for (out, (_, reg)) in outs.iter_mut().zip(&kernel.outputs) {
                    out[i] = self.regs[*reg] as f32;
                }
            }
        }
    }
}

#[inline(never)]
fn eval_memo_miss(
    entry: &mut Option<MemoEntry>,
    key: [u64; 2],
    math: MemoMath,
    x: f64,
    y: f64,
) -> f64 {
    let value = match math {
        MemoMath::Unary(f) => f.apply(x),
        MemoMath::Binary(op, _) => op.apply(x, y),
    };
    *entry = Some(MemoEntry { key, value });
    value
}

#[inline(never)]
fn eval_bin_cold(op: BinOp, x: f64, y: f64) -> f64 {
    op.apply(x, y)
}

#[inline(never)]
fn eval_fn1_cold(f: Fn1, x: f64) -> f64 {
    f.apply(x)
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
