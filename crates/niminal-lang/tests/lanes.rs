//! Vector forms against the scalar loops they replace: the same sound, in far fewer instructions.

use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

fn render(source: &str, blocks: usize) -> Vec<f32> {
    let program = compile(source).unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut out = vec![0.0; blocks * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    out
}

fn error(source: &str) -> String {
    match compile(source) {
        Ok(_) => panic!("expected an error: {source}"),
        Err(errors) => errors[0].message.clone(),
    }
}

/// Both sources must make the same sound.
fn same(vector: &str, scalar: &str) {
    let (v, s) = (render(vector, 60), render(scalar, 60));
    assert!(v.iter().any(|x| x.abs() > 1e-4), "silent: {vector}");
    for (i, (a, b)) in v.iter().zip(&s).enumerate() {
        assert!(
            (a - b).abs() < 1e-4,
            "sample {i}: vector {a}, scalar {b}\n{vector}\n{scalar}"
        );
    }
}

const TAPS: &str = "const taps = harmonics([1, 0.5, 0.25, 0.125], size: 64)\n";

#[test]
fn fir_is_the_loop_over_a_ring() {
    same(
        &format!(
            "{TAPS}fn f(x) {{\n state buf = zeros(64)\n state pos = 0\n buf[pos] = x\n y = buf.fir(taps, pos)\n pos = (pos + 1).mod(64)\n y\n}}\ninstr a() {{ osc(saw, 110hz).f() }}"
        ),
        &format!(
            "{TAPS}fn f(x) {{\n state buf = zeros(64)\n state pos = 0\n buf[pos] = x\n acc = 0\n for i in 0..<64 {{ acc = acc + buf[pos - i] * taps[i] }}\n pos = (pos + 1).mod(64)\n acc\n}}\ninstr a() {{ osc(saw, 110hz).f() }}"
        ),
    );
}

#[test]
fn taps_are_many_reads_of_one_delay_line() {
    same(
        "fn f(x) {\n state buf = zeros(8192)\n state pos = 0\n buf[pos] = x\n t = buf.taps([331, 662.5, 1000, 2047.25], pos)\n y = x + t.sum * 0.25\n pos = (pos + 1).mod(8192)\n y\n}\ninstr a() { osc(saw, 110hz).f() }",
        "fn f(x) {\n state buf = zeros(8192)\n state pos = 0\n buf[pos] = x\n acc = buf.at(pos - 331, interp: linear) + buf.at(pos - 662.5, interp: linear) + buf.at(pos - 1000, interp: linear) + buf.at(pos - 2047.25, interp: linear)\n y = x + acc * 0.25\n pos = (pos + 1).mod(8192)\n y\n}\ninstr a() { osc(saw, 110hz).f() }",
    );
}

#[test]
fn a_resonator_bank_is_the_loop_over_its_modes() {
    let setup = "const modes = [1, 2, 3, 4, 5, 6, 7, 8]\n";
    same(
        &format!(
            "{setup}fn f(x, freq: hz) {{\n state s1 = zeros(8)\n state s2 = zeros(8)\n w = 2 * pi * freq / sample_rate * modes\n y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2\n s2 = s1\n s1 = y\n y.sum\n}}\ninstr a() {{ impulse(3hz).f(220hz) }}"
        ),
        &format!(
            "{setup}fn f(x, freq: hz) {{\n state s1 = zeros(8)\n state s2 = zeros(8)\n acc = 0\n for k in 0..<8 {{\n  w = 2 * pi * freq * (k + 1) / sample_rate\n  y = x * 0.01 + 1.98 * cos(w) * s1[k] - 0.9801 * s2[k]\n  s2[k] = s1[k]\n  s1[k] = y\n  acc = acc + y\n }}\n acc\n}}\ninstr a() {{ impulse(3hz).f(220hz) }}"
        ),
    );
}

#[test]
fn dot_and_matvec_multiply_arrays() {
    same(
        "const m = [1, 0.5, -0.5, 1]\nfn f(x) {\n state v = zeros(2)\n y = m.matvec(v)\n v = [x, y[0] * 0.5]\n v.dot([1, 2])\n}\ninstr a() { osc(saw, 110hz).f() }",
        "fn f(x) {\n state v0 = 0\n state v1 = 0\n y0 = 1 * v0 + 0.5 * v1\n y1 = -0.5 * v0 + 1 * v1\n v0 = x\n v1 = y0 * 0.5\n v0 * 1 + v1 * 2\n}\ninstr a() { osc(saw, 110hz).f() }",
    );
}

#[test]
fn a_whole_array_is_assigned_and_a_single_value_fills_it() {
    let out = render(
        "fn f(x) {\n state buf = zeros(4)\n buf = buf + x\n buf = buf * 0.5\n buf.sum\n}\ninstr a() { osc(saw, 110hz).f() }",
        40,
    );
    assert!(out.iter().any(|v| *v != 0.0));
    let cleared = render(
        "fn f(x) {\n state buf = zeros(4)\n buf = x\n buf = 0\n buf.sum + 1\n}\ninstr a() { osc(saw, 110hz).f() }",
        4,
    );
    assert!(cleared.iter().all(|v| (*v - 1.0).abs() < 1e-6));
}

#[test]
fn mistakes_are_reported() {
    let long = |body: &str| {
        format!(
            "fn f(x) {{\n state buf = zeros(4)\n {body}\n}}\ninstr a() {{ osc(saw, 110hz).f() }}"
        )
    };
    assert!(error(&long("buf = [1, 2, 3]\n buf.sum")).contains("has 4 entries"));
    assert!(error(&long("buf = 3hz\n buf.sum")).contains("holds"));
    assert!(error(&long("buf.dot([1, 2])")).contains("same length"));
    assert!(error(&long("buf.fir([1, 1, 1, 1, 1], 0)")).contains("more than"));
    assert!(error(&long("x.fir([1], 0)")).contains("state array"));
}

#[test]
fn vector_functions_work_in_a_function_without_other_state() {
    // `dot` alone makes a function a kernel, since it is not a graph node
    let out = render(
        "fn f(x) {\n [x, x * 2].dot([1, 0.5])\n}\ninstr a() { osc(saw, 110hz).f() }",
        20,
    );
    assert!(out.iter().any(|v| *v != 0.0));
}

#[test]
fn a_function_cannot_take_the_name_of_a_built_in_or_a_unit() {
    let define = |name: &str| {
        error(&format!(
            "fn {name}(x) {{ x }}\ninstr a() {{ osc(saw, 110hz) }}"
        ))
    };
    for name in [
        "dot", "fir", "taps", "matvec", "frames", "fft", "atan2", "bank", "hold", "lpf", "sin",
    ] {
        assert_eq!(
            define(name),
            format!("`{name}` is a built-in function, so it can't be defined again")
        );
    }
    for name in ["bar", "bars", "hz", "st", "num"] {
        assert_eq!(
            define(name),
            format!("`{name}` is a unit, so it can't be the name of a function")
        );
    }
    // ordinary names, and the prelude's, are free
    assert!(
        compile(
            "fn shimmer(x) { x }\nfn lfo(r: hz) { osc(sine, r) }\ninstr a() { osc(saw, 110hz) }"
        )
        .is_ok()
    );
}
