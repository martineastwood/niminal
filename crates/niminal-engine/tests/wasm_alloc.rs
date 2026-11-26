//! Rendering through wasm must not allocate (the audio thread may not).
#![cfg(feature = "wasm-kernel")]
#![allow(clippy::unwrap_used)]

use niminal_engine::ops::{ArraySource, BinOp, Custom, Instr, Kernel};
use niminal_engine::{Array, Opcode, Port, ProcessCtx};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Counting = Counting;

#[test]
fn rendering_does_not_allocate() {
    // a one-pole into a delay line
    let kernel = Arc::new(Kernel {
        ports: vec![Port::required("x")],
        registers: 7,
        state_init: vec![0.0],
        buffers: vec![Array::from(vec![0.0; 64])],
        init: vec![Instr::Const { dst: 1, value: 0.99 }, Instr::Const { dst: 2, value: 3.0 }],
        code: vec![
            Instr::Input { dst: 0, port: 0 },
            Instr::LoadState { dst: 3, slot: 0 },
            Instr::Bin { dst: 4, op: BinOp::Mul, a: 3, b: 1 },
            Instr::Bin { dst: 5, op: BinOp::Add, a: 4, b: 0 },
            Instr::StoreState { slot: 0, src: 5 },
            Instr::Write { list: 0, idx: 2, src: 5, cond: None },
            Instr::Read { dst: 6, list: ArraySource::State(0), idx: 2 },
        ],
        outputs: vec![("out".into(), 6)],
        ..Kernel::default()
    });
    let mut custom = Custom::new_wasm(kernel).unwrap();
    let ctx = ProcessCtx::new(48_000.0, true);
    let input = vec![0.5f32; 64];
    let mut out = vec![vec![0.0f32; 64]];
    for _ in 0..4 {
        custom.process_outputs(&ctx, &[&input], &mut out, 64);
    }
    let before = ALLOCS.load(Relaxed);
    for _ in 0..100 {
        custom.process_outputs(&ctx, &[&input], &mut out, 64);
    }
    assert_eq!(ALLOCS.load(Relaxed), before, "wasm rendering allocated");
}
