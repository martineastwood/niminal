//! The array path must allocate before rendering, never on the audio thread.
use niminal_engine::{BLOCK, Buses, Voice};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Allocator;
fn count() {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn large_computed_arrays_and_buffers_do_not_allocate_while_processing() {
    let source = "fn bank(xs: [num], pos) { state history = zeros(64)\nhistory[pos] = xs[pos]\n(xs + history).map(it * 0.5) }\ninstr a(freq: hz = 100hz) { signals = osc(saw, (0..<64).map(freq * (1 + it * 0.001)))\nbank(signals, osc(saw, 1hz) * 64).at(0.5, interp: cubic) }";
    let program = niminal_lang::compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("arrays", source))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = [[0.0; BLOCK]; 1];
    ALLOCATIONS.with(|count| count.set(0));
    TRACK.with(|track| track.set(true));
    for _ in 0..128 {
        voice.process_blocks(&mut output, BLOCK, &mut buses);
    }
    TRACK.with(|track| track.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|value| value.is_finite()));
}
