//! Functions with state. The language compiles a `fn` body to a small register
//! program (a [`Kernel`]) which [`Custom`] runs once per sample, so state
//! carries from one sample to the next exactly as in a hand-written filter.
//! Work that only depends on note-start or per-block values runs at those rates.

mod exec;
mod ir;
mod math;
#[cfg(feature = "wasm-kernel")]
pub mod wasm;
#[cfg(all(feature = "jit-dynasm", target_arch = "aarch64"))]
mod jit;
#[cfg(feature = "wasm-kernel")]
mod wasm_run;
#[cfg(feature = "kernel-profile")]
pub mod profile;

pub use ir::*;
pub use math::*;

use std::sync::Arc;

use crate::Array;
use crate::opcode::{MAX_INPUTS, Opcode, Port, ProcessCtx, blocks};
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The input and output of one frame scope while it runs.
struct FrameRun {
    input: Vec<f64>,
    input2: Vec<f64>,
    output: Vec<f64>,
    pos: usize,
    window: Vec<f64>,
    synthesis: Vec<f64>,
}

impl FrameRun {
    fn new(frame: &FrameScope) -> Self {
        let size = frame.size;
        let window: Vec<f64> = (0..size)
            .map(|i| 0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / size as f64).cos()))
            .collect();
        // Half overlap uses analysis-only Hann. Denser overlaps use a tapered synthesis window,
        // normalized against all overlapping analysis windows for unity reconstruction.
        let synthesis = if frame.hop == size / 2 {
            vec![1.0; size]
        } else {
            (0..size)
                .map(|i| {
                    let weight: f64 = (0..size)
                        .step_by(frame.hop)
                        .map(|j| window[(i + j) % size].powi(2))
                        .sum();
                    window[i] / weight
                })
                .collect()
        };
        FrameRun {
            input2: vec![0.0; if frame.window2.is_some() { size } else { 0 }],
            input: vec![0.0; size],
            output: vec![0.0; size],
            pos: 0,
            window,
            synthesis,
        }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.input2.fill(0.0);
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

#[derive(Clone, Copy)]
struct MemoEntry {
    key: [u64; 2],
    value: f64,
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
    memo: Vec<Option<MemoEntry>>,
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
    #[cfg(feature = "wasm-kernel")]
    wasm: Option<wasm_run::WasmVoice>,
    #[cfg(all(feature = "jit-dynasm", target_arch = "aarch64"))]
    jit: Option<jit::JitVoice>,
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
        #[cfg(feature = "wasm-kernel")]
        let wasm = wasm_run::WasmVoice::new(&kernel);
        #[cfg(all(feature = "jit-dynasm", target_arch = "aarch64"))]
        let jit = jit::JitVoice::new(&kernel);
        Custom {
            #[cfg(feature = "wasm-kernel")]
            wasm,
            #[cfg(all(feature = "jit-dynasm", target_arch = "aarch64"))]
            jit,
            native_inputs: blocks(inputs),
            native_outputs: blocks(outputs),
            single: blocks(1),
            native_indices: (0..inputs).collect(),
            state: kernel.state_init.clone(),
            buffers: kernel.buffers.clone(),
            lanes: kernel.lanes.iter().map(|&n| vec![0.0; n]).collect(),
            frame_runs: kernel.frames.iter().map(FrameRun::new).collect(),
            fft_plans: kernel.ffts.iter().map(|&n| FftPlan::new(n)).collect(),
            regs: vec![0.0; kernel.registers],
            memo: vec![None; kernel.memo_slots],
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
}

#[cfg(feature = "wasm-kernel")]
impl Custom {
    /// A copy that renders through wasm whatever `NIMINAL_WASM` says, for tests and benchmarks.
    pub fn new_wasm(kernel: Arc<Kernel>) -> Result<Self, String> {
        let mut custom = Custom::new(kernel.clone());
        custom.wasm = Some(wasm_run::WasmVoice::forced(&kernel)?);
        Ok(custom)
    }

}

#[cfg(any(feature = "wasm-kernel", all(feature = "jit-dynasm", target_arch = "aarch64")))]
impl Custom {
    /// A copy that renders through the interpreter whatever `NIMINAL_WASM` or `NIMINAL_JIT` say.
    pub fn new_interpreted(kernel: Arc<Kernel>) -> Self {
        let mut custom = Custom::new(kernel);
        #[cfg(feature = "wasm-kernel")]
        {
            custom.wasm = None;
        }
        #[cfg(all(feature = "jit-dynasm", target_arch = "aarch64"))]
        {
            custom.jit = None;
        }
        custom
    }
}

#[cfg(all(feature = "jit-dynasm", target_arch = "aarch64"))]
impl Custom {
    /// A copy that runs on the dynasm code generator whatever `NIMINAL_JIT` says, for tests.
    pub fn new_jit(kernel: Arc<Kernel>) -> Result<Self, String> {
        let mut custom = Custom::new_interpreted(kernel.clone());
        custom.jit = Some(jit::JitVoice::forced(&kernel)?);
        Ok(custom)
    }
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
        #[cfg(feature = "wasm-kernel")]
        if let Some(w) = &mut self.wasm {
            w.dirty = true;
        }
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
            #[cfg(feature = "wasm-kernel")]
            {
                if let Some(w) = old.wasm.as_ref() {
                    w.pull_buffers(&mut old.buffers);
                }
                if let Some(w) = &mut self.wasm {
                    w.dirty = true;
                }
            }
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
