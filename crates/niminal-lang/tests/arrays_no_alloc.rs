//! The array path must allocate before rendering, never on the audio thread.
#![allow(clippy::unwrap_used)]
// the counting allocator forwards every call to `System` unchanged
#![allow(clippy::undocumented_unsafe_blocks)]
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
    let source = "const wave = harmonics([1, 0.5], size: 4800)\ns = sample(wave).root(440hz).loop(forward)\ninstr a() { s(freq: 440hz, stretch: 1.7, stretch_mode: 2, loop_start: 0.2, loop_end: 0.8, loop_crossfade: 10ms) }";
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

#[test]
fn phase_vocoder_with_dense_overlap_does_not_allocate_while_processing() {
    let source = niminal_lang::expand(
        "use std/spectral\ninstr a() { spectral_warp(osc(saw, 220hz), shift: 7, size: 512, overlap: 8) }",
        std::path::Path::new("."),
    ).unwrap();
    let program = niminal_lang::compile(&source.text).unwrap();
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
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
fn spectral_memory_capture_scan_and_envelope_controls_allocate_before_audio_only() {
    let program=niminal_lang::compile("instr a() {
  source=osc(saw, 220hz) * 0.1
  spectral_memory(source, record: env[1 80ms 1 1ms 0], position: phasor(2hz), shift: 7, formant: -3, preserve: 1, size: 512, frames: 64)
}").unwrap();
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 1];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..256 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output[0].iter().all(|v| v.is_finite()));
    assert!(output[0].iter().any(|v| v.abs() > 0.001));
}

#[test]
fn spectral_edits_morphing_and_partial_tracking_do_not_allocate_on_audio_thread() {
    for expression in [
        "spectral_thin(x,count: 8,size: 512)",
        "spectral_arp(x,position: phasor(2hz),size: 512)",
        "spectral_hold(x,magnitude: phasor(10hz)>0.5,frequency: phasor(7hz)>0.5,size: 512)",
        "spectral_shift(x,shift: 73hz,size: 512)",
        "spectral_morph(x,osc(saw,330hz),size: 512)",
        "spectral_partials(x,count: 8,freeze: phasor(8hz)>0.5,size: 512)",
        "partial_morph(x,osc(saw,330hz)*0.1,magnitude: phasor(3hz),frequency: phasor(2hz),freeze: phasor(8hz)>0.5,count: 8,size: 512)",
    ] {
        let program = niminal_lang::compile(&format!(
            "instr a() {{ x=osc(saw,220hz)*0.1\n {expression} }}"
        ))
        .unwrap();
        let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.0);
        let mut buses = Buses::new(&[]);
        let mut output = vec![vec![0.0; block_size()]; 1];
        ALLOCATIONS.with(|count| count.set(0));
        NODE.with(|node| node.set(true));
        for _ in 0..256 {
            voice.process_blocks(&mut output, block_size(), &mut buses);
        }
        NODE.with(|node| node.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0, "{expression}");
        assert!(output[0].iter().all(|v| v.is_finite()));
    }
}

#[test]
fn stereo_reverb_controls_and_resets_do_not_allocate() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "config {{ channels: stereo }}\nfn effect(x: [num]) {{ {state}reset_on(impulse(100hz)) {{ x.mverb(size: (phasor(20hz) > 0.5), predelay: (phasor(1hz) * 200).ms) }} }}\ninstr a() {{ effect([osc(saw, 110hz), osc(sine, 165hz)]) }}"
        );
        let program = niminal_lang::compile(&source).unwrap_or_else(|e| panic!("{e:?}"));
        let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
        let mut buses = Buses::new(&[]);
        let mut output = vec![vec![0.0; block_size()]; 2];
        ALLOCATIONS.with(|count| count.set(0));
        NODE.with(|node| node.set(true));
        for _ in 0..128 {
            voice.process_blocks(&mut output, block_size(), &mut buses);
        }
        NODE.with(|node| node.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        assert!(output.iter().flatten().all(|x| x.is_finite()));
    }
}

#[test]
fn sustained_waveguides_and_resets_allocate_before_audio_only() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "fn model() {{ {state} reset_on(impulse(3hz)) {{ bowed(220hz, position: phasor(0.5hz)) + reed(330hz, stiffness: phasor(0.7hz)) }} }}\ninstr a() {{ model() }}"
        );
        let program = niminal_lang::compile(&source).unwrap();
        let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.);
        let mut buses = Buses::new(&[]);
        let mut output = vec![vec![0.; block_size()]; 1];
        ALLOCATIONS.with(|count| count.set(0));
        NODE.with(|node| node.set(true));
        for _ in 0..800 {
            voice.process_blocks(&mut output, block_size(), &mut buses);
        }
        NODE.with(|node| node.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        assert!(output[0].iter().all(|x| x.is_finite()));
    }
}

#[test]
fn modular_blocks_do_not_allocate_while_rendering_or_completing_contours() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n shape = contour(cycle: 1, rise: 5ms, fall: 10ms)\n accent = contour(trigger: shape.eoc, rise: 1ms, fall: 20ms)\n bank = harmonic_bank(110hz, centre: shape.value, focus: 0.8)\n voice = scan_mix(bank.partials, centre: accent.value)\n voice.stereo_echo(time: 20ms).space_reverb(size: 0.3 + shape.value * 0.4)\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output.iter().flatten().all(|value| value.is_finite()));
}

#[test]
fn the_voice_modules_do_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n steps = shift_random(clock: impulse(40hz), lock: 0.5, seed: 3)\n voice = complex_osc(110hz * (1 + steps.value * 0.5), index: 2, fold: 0.5, am: 0.3)\n gated = voice.low_pass_gate(ping: steps.gate, decay: 30ms)\n f = [gated.sound, voice.modulator].multimode_filter(cutoff: 300hz + gated.opening * 4khz, res: 0.9, drive: 0.5)\n [f.low[0] + f.band[1], f.high[1] + f.notch[0]]\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output.iter().flatten().all(|value| value.is_finite()));
    assert!(output.iter().flatten().any(|value| value.abs() > 1e-4));
}

#[test]
fn the_processing_modules_do_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n src = harmonics([1, 0.4, 0.2], size: 512)\n cloud = texture(src, position: phasor(0.3hz), spread: 0.5, detune: 3)\n spoken = vocoder(osc(saw, 110hz), osc(saw, 180hz) * 0.5, bands: 12)\n mangled = cross_mod(spoken, osc(sine, quantize(60 + shift_random(clock: impulse(20hz)).value * 12) * 1hz), mode: fold, amount: 0.7).sound\n [cloud[0] + mangled, cloud[1] + mangled]\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output.iter().flatten().all(|value| value.is_finite()));
    assert!(output.iter().flatten().any(|value| value.abs() > 1e-4));
}

#[test]
fn the_oscillator_and_modulation_blocks_do_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n lfos = multi_lfo(rate: 0.7hz, shift: 0.2)\n random = random_source(clock: impulse(30hz), steps: 0.4, deja_vu: 0.6, seed: 4)\n tone = multi_voice(110hz + random.value * 20hz, model: lfos.a * 0.5 + 0.5, timbre: lfos.b * 0.5 + 0.5, morph: lfos.c * 0.5 + 0.5)\n shaped = wavefolder(slew(tone, rise: 5ms, fall: 80ms), amount: lfos.d * 0.5 + 0.5)\n voiced = shaped.formant_filter(freq: 500hz + lfos.a * 100hz).sound\n [voiced * random.gate, morph_osc(220hz, shape: lfos.b * 0.5 + 0.5).sound]\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output.iter().flatten().all(|value| value.is_finite()));
    assert!(output.iter().flatten().any(|value| value.abs() > 1e-4));
}

#[test]
fn the_hold_chaos_resonator_and_shifter_blocks_do_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n wild = chaos(rate: 3hz)\n steps = rungler(osc(saw, 3.1hz), osc(saw, 7.3hz))\n gate = comparator(wild.x, threshold: 0.1).gate\n held = track_hold(steps.value, gate)\n tone = osc(saw, 110hz * pow(2, held))\n rung = resonant_body(tone * steps.gate, freq: 220hz + wild.y * 50hz, structure: wild.z * 0.5 + 0.5)\n shifted = freq_shifter(rung, shift: 60hz)\n echoes = shimmer_echo(shifted.up, time: 120ms, shift: 7)\n mixed = vca_mix([shifted.up, shifted.down], [wild.x * 0.5 + 0.5, 0.4], curve: exponential)\n [mixed + echoes[0] * 0.3, mixed + echoes[1] * 0.3]\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(output.iter().flatten().all(|value| value.is_finite()));
    assert!(output.iter().flatten().any(|value| value.abs() > 1e-4));
}

#[test]
fn the_clock_and_logic_blocks_do_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n ticks = impulse(6hz)\n fast = clock_mult(ticks, factor: 4, swing: 0.2)\n coin = bernoulli(fast, probability: 0.6, seed: 2)\n ratchet = burst(coin.a, count: 3, rate: 90hz, accelerate: 0.1)\n held = gate_length(ratchet, length: 30ms)\n late = gate_delay(coin.b, time: 40ms)\n either = gate_logic(held, late, mode: xor)\n half = flip_flop(either)\n inside = window_comparator(osc(sine, 0.7hz), low: -0.3, high: 0.4)\n pitch_step = stepper([0, 3, 7, 12], half.q, reset: inside.outside)\n tone = sync_osc(110hz * pow(2, pitch_step.value / 12), ratio: mux([1.5, 2.5, 3.5], pitch_step.step), shape: 0.3)\n voiced = tone * (held + late * 0.5).smooth(5ms)\n [voiced, voiced * half.not_q]\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    let mut loudest = 0.0f32;
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
        assert!(output.iter().flatten().all(|value| value.is_finite()));
        loudest = output.iter().flatten().fold(loudest, |m, v| m.max(v.abs()));
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(loudest > 1e-3, "{loudest}");
}

#[test]
fn the_row_sequencer_does_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n seq = row_sequencer(impulse(8hz), reset: osc(square, 0.3hz), values: [0, 3, 7, 12], extras: [0.2, 0.5], durations: [2, 1, 1], repeats: [1, 2], modes: [1, 2, 3], gate_length: 0.4)\n tone = osc(saw, 110hz * pow(2, seq.value / 12)) * 0.3\n voiced = tone * seq.gate.smooth(5ms) * (0.5 + seq.extra)\n [voiced, voiced]\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    let mut loudest = 0.0f32;
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
        assert!(output.iter().flatten().all(|value| value.is_finite()));
        loudest = output.iter().flatten().fold(loudest, |m, v| m.max(v.abs()));
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(loudest > 1e-3, "{loudest}");
}

#[test]
fn the_tap_delay_does_not_allocate_while_rendering() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() {\n hit = osc(saw, 220hz) * env[1 | 30ms 0] * impulse(3hz).smooth(2ms)\n tap_delay(hit + noise() * 0.01, time: 90ms, taps: 16, shifts: [0, 7, 12, -5], step: 2, decay: 0.85, curve: 0.8, spread: 0.7)\n}";
    let expanded = niminal_lang::expand(source, std::path::Path::new(".")).unwrap();
    let program = niminal_lang::compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = vec![vec![0.0; block_size()]; 2];
    ALLOCATIONS.with(|count| count.set(0));
    NODE.with(|node| node.set(true));
    let mut loudest = 0.0f32;
    for _ in 0..1024 {
        voice.process_blocks(&mut output, block_size(), &mut buses);
        assert!(output.iter().flatten().all(|value| value.is_finite()));
        loudest = output.iter().flatten().fold(loudest, |m, v| m.max(v.abs()));
    }
    NODE.with(|node| node.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert!(loudest > 1e-3, "{loudest}");
}
