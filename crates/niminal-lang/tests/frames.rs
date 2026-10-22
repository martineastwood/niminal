//! Frame scopes: code that runs on a window of samples every half window, with `fft` and `ifft`.

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

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

const SIGNAL: &str = "osc(saw, 220hz) * 0.5 + osc(sine, 1333hz) * 0.25";

#[test]
fn a_frame_that_returns_its_window_gives_the_input_back_one_frame_late() {
    let dry = render(&format!("instr a() {{ {SIGNAL} }}"), 60);
    let wet = render(
        &format!("fn same(x) {{ x.frames(256, fn(w) {{ w }}) }}\ninstr a() {{ same({SIGNAL}) }}"),
        60,
    );
    // nothing is aligned for a single path, so the output is simply late by the frame
    let shifted = (512..dry.len())
        .map(|i| (wet[i] - dry[i - 256]).abs())
        .fold(0.0, f32::max);
    assert!(shifted < 1e-4, "{shifted}");
}

#[test]
fn the_latency_of_a_frame_is_declared_so_a_path_beside_it_is_delayed_to_match() {
    let out = render(
        &format!(
            "fn same(x) {{ x.frames(256, fn(w) {{ w }}) }}\ninstr a() {{ {SIGNAL} - same({SIGNAL}) }}"
        ),
        60,
    );
    assert!(rms(&out[1024..]) < 1e-4, "{}", rms(&out[1024..]));
}

#[test]
fn fft_then_ifft_gives_the_window_back() {
    let wet = render(
        &format!(
            "fn through(x) {{ x.frames(512, fn(w) {{ s = fft(w)\n ifft(s[0], s[1]) }}) }}\ninstr a() {{ through({SIGNAL}) }}"
        ),
        80,
    );
    let dry = render(&format!("instr a() {{ {SIGNAL} }}"), 80);
    let worst = (1024..dry.len())
        .map(|i| (wet[i] - dry[i - 512]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn a_function_can_filter_the_spectrum() {
    // keep the first 40 bins of 513: below about 1.9khz, so the 1333hz tone stays and a 5khz tone goes
    let source = |tone: &str| {
        format!(
            "const keep = [{}]\nfn low(x) {{ x.frames(1024, fn(w) {{ s = fft(w)\n ifft(s[0] * keep, s[1] * keep) }}) }}\ninstr a() {{ low(osc(sine, {tone})) }}",
            (0..513)
                .map(|k| if k < 40 { "1" } else { "0" })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let low = render(&source("1333hz"), 120);
    let high = render(&source("5000hz"), 120);
    assert!(rms(&low[4096..]) > 0.5, "{}", rms(&low[4096..]));
    assert!(rms(&high[4096..]) < 0.02, "{}", rms(&high[4096..]));
}

#[test]
fn state_inside_a_frame_lasts_from_frame_to_frame() {
    // a spectrum that is held while a state array keeps the loudest magnitudes
    let out = render(
        "fn peaks(x) {
  x.frames(512, fn(w) {
    state peak = zeros(257)
    s = fft(w)
    mag = sqrt(s[0] * s[0] + s[1] * s[1])
    peak = max(peak, mag)
    ifft(peak, s[1] * 0)
  })
}
instr a() { peaks(osc(saw, 220hz) * env[1 400ms 0]) }",
        200,
    );
    assert!(out.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn atan2_gives_the_angle() {
    let out = render(
        "fn angle(x) { state held = 0\n atan2(x, 1) }\ninstr a() { angle(osc(sine, 100hz)) }",
        20,
    );
    assert!(out.iter().any(|v| v.abs() > 0.1));
    let flat = render("instr a() { atan2(1, 1) }", 2);
    assert!((flat[0] - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
}

#[test]
fn mistakes_are_reported() {
    let body = |b: &str| format!("fn f(x) {{ {b} }}\ninstr a() {{ f(osc(saw, 110hz)) }}");
    assert!(error(&body("x.frames(100, fn(w) { w })")).contains("power of two"));
    assert!(error(&body("x.frames(256, fn(w) { w[0] })")).contains("256 plain samples"));
    assert!(
        error(&body("x.frames(256, fn(w) { w.frames(256, fn(v) { v }) })"))
            .contains("another frame")
    );
    assert!(error(&body("fft([1, 2, 3])")).contains("power of two"));
    assert!(error(&body("ifft([1, 2, 3, 4], [1, 2])")).contains("same length"));
}

#[test]
fn a_mask_can_be_built_with_map() {
    let out = render(
        "const keep = (0..<513).map(it < 40)\nfn low(x) { x.frames(1024, fn(w) { s = fft(w)\n ifft(s[0] * keep, s[1] * keep) }) }\ninstr a() { low(osc(sine, 1333hz)) }",
        120,
    );
    assert!(rms(&out[4096..]) > 0.5);
}
