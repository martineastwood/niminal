use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

fn render(source: &str, name: &str, blocks: usize) -> Vec<f32> {
    let program = compile(source).unwrap_or_else(|errors| panic!("{errors:?}"));
    let instrument = program.instruments.iter().find(|i| i.name == name).unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut out = vec![0.0; blocks * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    out
}

fn error(source: &str) -> String {
    match compile(source) {
        Ok(_) => panic!("expected an error"),
        Err(errors) => errors[0].message.clone(),
    }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn convolving_with_an_impulse_gives_back_the_input() {
    let dry = render("instr a() { osc(saw, 220hz) }", "a", 20);
    let wet = render("instr a() { osc(saw, 220hz).convolve([1]) }", "a", 20);
    for (d, w) in dry.iter().zip(&wet) {
        assert!((d - w).abs() < 1e-3, "{d} {w}");
    }
}

#[test]
fn convolving_with_a_delayed_impulse_delays_the_input() {
    let n = 300;
    let mut ir = vec!["0"; n];
    ir[n - 1] = "1";
    let source = format!(
        "instr a() {{ osc(saw, 220hz).convolve([{}]) }}",
        ir.join(", ")
    );
    let dry = render("instr a() { osc(saw, 220hz) }", "a", 20);
    let wet = render(&source, "a", 20);
    for i in n..dry.len() {
        assert!((wet[i] - dry[i - (n - 1)]).abs() < 1e-3, "sample {i}");
    }
}

#[test]
fn convolve_needs_an_impulse_response() {
    assert!(error("instr a() { osc(saw, 220hz).convolve() }").contains("ir"));
}

#[test]
fn spectral_freeze_holds_a_note_after_it_stops() {
    let held = render(
        "instr a() { (osc(sine, 440hz) * env[1 100ms 0]).spectral_freeze(freeze: 1, size: 256) }",
        "a",
        200,
    );
    let tail = &held[held.len() - 4096..];
    assert!(rms(tail) > 0.05, "{}", rms(tail));
    let plain = render(
        "instr a() { (osc(sine, 440hz) * env[1 100ms 0]).spectral_freeze(freeze: 0, size: 256) }",
        "a",
        200,
    );
    assert!(rms(&plain[plain.len() - 4096..]) < 1e-3);
}

#[test]
fn spectral_delay_delays_sound() {
    let out = render(
        "instr a() { (osc(sine, 440hz) * env[1 50ms 0]).spectral_delay(time: 100ms) }",
        "a",
        300,
    );
    let sr = 48_000;
    let before = &out[..sr / 20];
    let after = &out[sr / 10..sr / 4];
    assert!(rms(before) < 0.02, "{}", rms(before));
    assert!(rms(after) > 0.02, "{}", rms(after));
}

#[test]
fn spectral_effects_check_their_settings() {
    assert!(
        error("instr a() { osc(saw, 220hz).spectral_freeze(size: 100) }").contains("power of two")
    );
    assert!(
        error("instr a() { osc(saw, 220hz).spectral_delay(time: 100ms, max: 60sec) }")
            .contains("at most")
    );
}

#[test]
fn a_function_with_state_can_use_per_sample_opcodes_with_tables_but_not_block_ones() {
    // spectral effects run a sample at a time, so a kernel can call them
    let out = render(
        "fn f(x) {\n state held = 0\n x.spectral_freeze(freeze: 1, size: 256)\n}\ninstr a() { f(osc(sine, 440hz)) }",
        "a",
        40,
    );
    assert!(out.iter().any(|v| *v != 0.0));
    // convolution needs a whole block
    let message = error(
        "fn f(x) {\n state held = 0\n x.convolve([1, 0.5])\n}\ninstr a() { f(osc(sine, 440hz)) }",
    );
    assert!(message.contains("whole blocks"), "{message}");
}

#[test]
fn tables_pass_into_functions_for_waveshapers_oscillators_and_convolution() {
    // a waveshaper from a table, in a function with state and in one without
    for state in ["", " state held = 0\n"] {
        let out = render(
            &format!(
                "fn shape(x, curve: [num]) {{\n{state} x.waveshape(curve)\n}}\ninstr a() {{ shape(osc(sine, 220hz), cheby([1, 0.3])) }}"
            ),
            "a",
            20,
        );
        assert!(out.iter().any(|v| v.abs() > 0.1), "state: {state:?}");
    }
    // an oscillator from a table, and a bank of tables morphed by position, inside a function with state
    let out = render(
        "fn tone(freq: hz, wave: [num]) {\n state held = 0\n osc(wave, freq)\n}\ninstr a() { tone(220hz, harmonics([1, 0.5, 0.25], size: 2048)) }",
        "a",
        20,
    );
    assert!(out.iter().any(|v| v.abs() > 0.1));
    // an impulse response given to a function
    let out = render(
        "fn verb(x, ir: [num]) {\n x.convolve(ir, mix: 1)\n}\ninstr a() { verb(osc(saw, 220hz), [1, 0, 0.5, 0, 0.25]) }",
        "a",
        20,
    );
    assert!(out.iter().any(|v| v.abs() > 0.1));
}
