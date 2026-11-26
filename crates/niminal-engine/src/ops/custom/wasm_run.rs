//! Prototype: runs the module `wasm.rs` emits through wasmtime (native targets only).
//!
//! Compiled modules are shared by every voice of a kernel; each voice has its own instance, so its
//! memory (state, lists, the block's inputs and outputs) is private. Compiling and instantiating
//! happen when a voice is created, never while rendering. Enable with `NIMINAL_WASM=1`; a kernel
//! that cannot be compiled runs on the interpreter.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use wasmtime::{Caller, Config, Engine, InstancePre, Linker, Module, OptLevel, Store, Strategy};

use super::wasm::{self, Layout, bin_from_id};
use super::{Custom, Fn1, Kernel};
use crate::opcode::block_size;

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.strategy(Strategy::Cranelift);
        config.cranelift_opt_level(OptLevel::Speed);
        // linear memory is never grown, so it needs no reserved address space or guard pages
        config.memory_reservation(0);
        config.memory_guard_size(0);
        Engine::new(&config).expect("wasmtime engine config is valid")
    })
}

fn linker() -> Result<Linker<()>, String> {
    let mut linker = Linker::new(engine());
    linker
        .func_wrap("env", "bin", |_: Caller<'_, ()>, id: i32, a: f64, b: f64| {
            bin_from_id(id).apply(a, b)
        })
        .map_err(|e| e.to_string())?;
    linker
        .func_wrap("env", "f1", |_: Caller<'_, ()>, id: i32, x: f64| {
            Fn1::ALL[id as usize].apply(x)
        })
        .map_err(|e| e.to_string())?;
    linker
        .func_wrap("env", "wrapx", |_: Caller<'_, ()>, x: f64, n: f64| {
            crate::wrapped_index(x, n as usize) as i32
        })
        .map_err(|e| e.to_string())?;
    Ok(linker)
}

/// A kernel compiled to wasm, shared by its voices.
pub(super) struct WasmKernel {
    pre: InstancePre<()>,
    layout: Layout,
    pub inline: usize,
    pub bytes: usize,
    pub compile_ms: f64,
}

type Cache = Mutex<Vec<(Weak<Kernel>, Option<Arc<WasmKernel>>)>>;

impl WasmKernel {
    pub(super) fn compile(kernel: &Kernel) -> Result<Self, String> {
        let start = std::time::Instant::now();
        let emitted = wasm::emit(kernel, block_size())?;
        let module = Module::new(engine(), &emitted.bytes).map_err(|e| e.to_string())?;
        let pre = linker()?
            .instantiate_pre(&module)
            .map_err(|e| e.to_string())?;
        Ok(WasmKernel {
            pre,
            layout: emitted.layout,
            inline: emitted.inline,
            bytes: emitted.bytes.len(),
            compile_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// The compiled form of `kernel`, compiling on first use. `None` if it cannot be (the
    /// interpreter runs it).
    fn of(kernel: &Arc<Kernel>) -> Option<Arc<WasmKernel>> {
        static CACHE: OnceLock<Cache> = OnceLock::new();
        let mut cache = CACHE.get_or_init(|| Mutex::new(Vec::new())).lock().ok()?;
        cache.retain(|(weak, _)| weak.strong_count() > 0);
        if let Some((_, hit)) = cache
            .iter()
            .find(|(weak, _)| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(kernel)))
        {
            return hit.clone();
        }
        let verbose = std::env::var("NIMINAL_WASM_VERBOSE").is_ok();
        let compiled = match WasmKernel::compile(kernel) {
            Ok(k) => {
                if verbose {
                    eprintln!(
                        "wasm {}: {} instructions, {} bytes, {:.2} ms",
                        kernel.name, k.inline, k.bytes, k.compile_ms
                    );
                }
                Some(Arc::new(k))
            }
            Err(error) => {
                if verbose {
                    eprintln!("wasm {}: interpreted ({error})", kernel.name);
                }
                None
            }
        };
        cache.push((Arc::downgrade(kernel), compiled.clone()));
        compiled
    }
}

/// The base of an instance's linear memory.
struct Memory(*mut u8);

// SAFETY: the pointer is only dereferenced by the voice that owns the instance, while wasm is not
// running, so moving or sharing the voice between threads cannot race on it.
unsafe impl Send for Memory {}
// SAFETY: as above; a shared reference never dereferences it.
unsafe impl Sync for Memory {}

/// One voice's instance.
pub(super) struct WasmVoice {
    kernel: Arc<WasmKernel>,
    store: Store<()>,
    render: wasmtime::TypedFunc<i32, ()>,
    mem: Memory,
    /// The interpreter's copy of the state lists is newer than wasm's: push it before rendering.
    pub(super) dirty: bool,
}

impl WasmVoice {
    /// `None` unless `NIMINAL_WASM=1` and the kernel can be compiled.
    pub(super) fn new(kernel: &Arc<Kernel>) -> Option<Self> {
        static ON: OnceLock<bool> = OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("NIMINAL_WASM").as_deref() == Ok("1")) {
            return None;
        }
        Self::instantiate(kernel, WasmKernel::of(kernel)?)
    }

    /// Compiles `kernel` whatever the environment says, for tests and benchmarks.
    pub(super) fn forced(kernel: &Arc<Kernel>) -> Result<Self, String> {
        let compiled = Arc::new(WasmKernel::compile(kernel)?);
        Self::instantiate(kernel, compiled).ok_or_else(|| "could not instantiate".to_string())
    }

    fn instantiate(kernel: &Arc<Kernel>, compiled: Arc<WasmKernel>) -> Option<Self> {
        let mut store = Store::new(engine(), ());
        let instance = compiled.pre.instantiate(&mut store).ok()?;
        let render = instance
            .get_typed_func::<i32, ()>(&mut store, "render")
            .ok()?;
        let memory = instance.get_memory(&mut store, "mem")?;
        let ptr = memory.data_ptr(&store);
        // shared tables are copied in once per voice
        for (table, &(at, len)) in kernel.tables_f64.iter().zip(&compiled.layout.tables) {
            // SAFETY: `at..at + len` doubles lie inside the instance's memory (see `Layout`).
            unsafe {
                std::ptr::copy_nonoverlapping(table.as_ptr(), ptr.add(at).cast::<f64>(), len)
            };
        }
        Some(WasmVoice {
            kernel: compiled,
            store,
            render,
            mem: Memory(ptr),
            dirty: true,
        })
    }

    /// Copies the state lists wasm is running on back to `buffers`.
    pub(super) fn pull_buffers(&self, buffers: &mut [crate::Array<f64>]) {
        for (list, &(at, len)) in buffers.iter_mut().zip(&self.kernel.layout.buffers) {
            // SAFETY: `at..at + len` doubles lie inside this instance's memory (see `Layout`),
            // and wasm is not running.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.mem.0.add(at).cast::<f64>(),
                    list.as_mut_ptr(),
                    len,
                )
            };
        }
    }

    /// Renders `frames` samples of `custom`'s sample section. The caller has run the init and
    /// block sections already.
    pub(super) fn render(
        &mut self,
        custom: &mut Custom,
        input: &impl Fn(usize, usize) -> f32,
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        let layout = &self.kernel.layout;
        let mem = self.mem.0;
        // SAFETY: `mem` is this instance's linear memory, sized by `layout` and never grown. Wasm
        // is not running, so the host has it to itself; every offset below is inside `layout`.
        unsafe {
            if self.dirty {
                for (list, &(at, len)) in custom.buffers.iter().zip(&layout.buffers) {
                    std::ptr::copy_nonoverlapping(list.as_ptr(), mem.add(at).cast::<f64>(), len);
                }
                self.dirty = false;
            }
            for port in 0..layout.n_ports {
                let base = mem
                    .add(layout.inputs + port * layout.block * 4)
                    .cast::<f32>();
                for i in 0..frames {
                    *base.add(i) = input(port, i);
                }
            }
            std::ptr::copy_nonoverlapping(
                custom.regs.as_ptr(),
                mem.add(layout.regs).cast::<f64>(),
                layout.n_regs,
            );
            std::ptr::copy_nonoverlapping(
                custom.state.as_ptr(),
                mem.add(layout.state).cast::<f64>(),
                layout.n_state,
            );
            *mem.add(layout.sample_rate).cast::<f64>() = custom.sample_rate;
        }
        if self.render.call(&mut self.store, frames as i32).is_err() {
            // a trap cannot happen in code that only does float math and bounded loads; if it
            // does, leave the block silent rather than unwinding through the audio thread
            for out in outs.iter_mut() {
                out[..frames].fill(0.0);
            }
            return;
        }
        // SAFETY: as above; wasm has returned.
        unsafe {
            std::ptr::copy_nonoverlapping(
                mem.add(layout.regs).cast::<f64>(),
                custom.regs.as_mut_ptr(),
                layout.n_regs,
            );
            std::ptr::copy_nonoverlapping(
                mem.add(layout.state).cast::<f64>(),
                custom.state.as_mut_ptr(),
                layout.n_state,
            );
            for (k, out) in outs.iter_mut().enumerate().take(layout.n_outputs) {
                std::ptr::copy_nonoverlapping(
                    mem.add(layout.outputs + k * layout.block * 4).cast::<f32>(),
                    out.as_mut_ptr(),
                    frames,
                );
            }
        }
    }
}
