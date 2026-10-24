//! The array path must allocate before rendering, never on the audio thread.
use niminal_engine::{Buses, Voice, block_size};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static NODE: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Allocator;
fn count() {
    if NODE.try_with(Cell::get).unwrap_or(false) {
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
    let source = "fn voices(xs: [num], pos) { state history = zeros(64)\nhistory[pos] = xs[pos]\n(xs + history).map(it * 0.5) }\ninstr a(freq: hz = 100hz) { signals = osc(saw, (0..<64).map(freq * (1 + it * 0.001)))\nvoices(signals, osc(saw, 1hz) * 64).at(0.5, interp: cubic) }";
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
    let mut output = vec![vec![0.0; block_size()]; 1];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..128 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|value| value.is_finite()));
}

#[test]
fn reset_scopes_callbacks_and_completion_do_not_allocate_while_processing() {
    let source = "fn apply(x, make: fn) { state unused = 0\nmake(x) }\nfn custom(t, x) { reset_on(t) { state buf = zeros(128)\nbuf[0] = x\napply(buf[0], fn(v) { v.delay(1ms) }).finish_when(0) } }\ninstr a(attack: sec = 10ms) { trigger = osc(square, 1khz)\nout = custom(trigger, 1)\nout += reset_on(trigger) { osc(saw, 440hz).reverb() * env[1 attack 0] } }";
    let program = niminal_lang::compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("composition", source))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 1];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..128 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|value| value.is_finite()));
}

#[test]
fn vector_instructions_do_not_allocate_while_processing() {
    let source = "const modes = [1, 2, 3, 4, 5, 6, 7, 8]
const m = [1, 0.5, 0.25, 0.125, 1, 0.5, 0.25, 0.125]
fn all(x, freq: init hz) {
  state ring = zeros(512)
  state pos = 0
  state s1 = zeros(8)
  state s2 = zeros(8)
  ring[pos] = x
  w = 2 * pi * freq / sample_rate * modes
  y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2
  s2 = s1
  s1 = y
  t = ring.taps([10, 20.5, 30, 40], pos)
  mixed = m.matvec(t)
  pos = (pos + 1).mod(512)
  y.sum + ring.fir([1, 0.5, 0.25], pos) + t.sum + mixed.sum + s1.dot(s2)
}
instr a() { impulse(5hz).all(220hz) }";
    let program = niminal_lang::compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("lanes", source))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 1];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..128 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|value| value.is_finite()));
    assert!(output[0].iter().any(|value| *value != 0.0));
}

#[test]
fn frame_scopes_and_ffts_do_not_allocate_while_processing() {
    let source = "fn spectral(x) {
  x.frames(512, fn(w) {
    state peak = zeros(257)
    s = fft(w)
    mag = sqrt(s[0] * s[0] + s[1] * s[1])
    peak = max(peak * 0.99, mag)
    ifft(peak, atan2(s[1], s[0]))
  })
}
instr a() { spectral(osc(saw, 220hz)) }";
    let program = niminal_lang::compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("frames", source))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 1];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..256 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|value| value.is_finite()));
}

#[test]
fn wsola_sample_alignment_does_not_allocate_while_processing() {
    let source = "const wave = harmonics([1, 0.5], size: 4800)\ns = sample(wave).root(440hz).loop(forward)\ninstr a() { s(freq: 440hz, stretch: 1.7, stretch_mode: 2, loop_start: 0.2, loop_end: 0.8) }";
    let program = niminal_lang::compile(source).unwrap();
    let graph = &program
        .instruments
        .iter()
        .find(|i| i.name == "a")
        .unwrap()
        .graph;
    let mut voice = Voice::new(graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 1];
    let mut audible = false;
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    // Cross several grain boundaries so the correlation search actually runs.
    for _ in 0..128 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
        audible |= output[0].iter().any(|value| value.abs() > 0.01);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|value| value.is_finite()));
    assert!(audible);
}
