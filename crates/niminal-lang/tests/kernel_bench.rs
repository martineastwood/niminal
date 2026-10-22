//! How fast `fn` kernels run, to say what a change to them bought. Not run with the tests:
//!
//! ```text
//! cargo test --release -p niminal-lang --test kernel_bench -- --ignored --nocapture
//! ```
//!
//! Each case renders 16 voices for 10 seconds on one thread and reports the cost of a voice's sample.
//! The numbers depend on the machine, so compare runs on one machine; `design/fn-extensions.md` keeps
//! the baseline and what each step changed.

use std::time::Instant;

use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

const VOICES: usize = 16;
const SECONDS: usize = 10;

fn bench(label: &str, source: &str) {
    let expanded = niminal_lang::expand(source, std::path::Path::new("."))
        .unwrap_or_else(|d| panic!("{label}: {}", d.message));
    let program = compile(&expanded.text).unwrap_or_else(|e| panic!("{label}: {e:?}"));
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voices: Vec<Voice> = (0..VOICES)
        .map(|_| Voice::new(instrument.graph.clone(), 48_000.0))
        .collect();
    let mut out = vec![0.0; block_size()];
    let blocks = 48_000 / block_size() * SECONDS;
    let mut loudest = 0.0f32;
    let start = Instant::now();
    for _ in 0..blocks {
        for v in &mut voices {
            v.process(&mut out);
            loudest = out.iter().fold(loudest, |m, s| m.max(s.abs()));
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let samples = (VOICES * SECONDS * 48_000) as f64;
    assert!(loudest > 0.0, "{label} is silent, so it measures nothing");
    println!(
        "{label:<34} {:>8.1} ns/sample {:>8.0} voices in real time",
        seconds * 1e9 / samples,
        samples / (SECONDS as f64 * 48_000.0) / seconds * SECONDS as f64
    );
}

const ONE_POLE: &str = "fn one_pole(x, cutoff: hz) {
  state y = 0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}\n";

const LADDER: &str = "fn ladder(x, cutoff: hz, res: 0..1 = 0.5) {
  state s1 = 0
  state s2 = 0
  state s3 = 0
  state s4 = 0
  g = 1 - exp(-2 * pi * cutoff / sample_rate)
  inp = x - 4 * res * s4
  s1 = s1 + g * (tanh(inp) - tanh(s1))
  s2 = s2 + g * (tanh(s1) - tanh(s2))
  s3 = s3 + g * (tanh(s2) - tanh(s3))
  s4 = s4 + g * (tanh(s3) - tanh(s4))
  s4
}\n";

const FIR: &str = "const taps = harmonics([1, 0.5, 0.25, 0.125], size: 64)
fn fir_loop(x) {
  state buf = zeros(64)
  state pos = 0
  buf[pos] = x
  acc = 0
  for i in 0..<64 { acc = acc + buf[pos - i] * taps[i] }
  pos = (pos + 1).mod(64)
  acc
}\n";

const MULTITAP: &str = "fn multitap(x, size: const int = 8192) {
  state buf = zeros(size)
  state pos = 0
  buf[pos] = x
  acc = 0
  for k in 1..<9 { acc = acc + buf[pos - k * 331] * (1 / k) }
  pos = (pos + 1).mod(size)
  x + acc * 0.3
}\n";

const MODAL: &str = "fn modal(x, freq: init hz) {
  state s1 = zeros(16)
  state s2 = zeros(16)
  acc = 0
  for k in 0..<16 {
    w = 2 * pi * freq * (k + 1) / sample_rate
    y = x * 0.01 + 1.98 * cos(w) * s1[k] - 0.9801 * s2[k]
    s2[k] = s1[k]
    s1[k] = y
    acc = acc + y
  }
  acc
}\n";

const FIR_LANE: &str = "const taps = harmonics([1, 0.5, 0.25, 0.125], size: 64)
fn lane_fir(x) {
  state buf = zeros(64)
  state pos = 0
  buf[pos] = x
  y = buf.fir(taps, pos)
  pos = (pos + 1).mod(64)
  y
}\n";

const MULTITAP_LANE: &str = "fn multitap(x, size: const int = 8192) {
  state buf = zeros(size)
  state pos = 0
  buf[pos] = x
  t = buf.taps([331, 662, 993, 1324, 1655, 1986, 2317, 2648], pos)
  y = x + t.dot([1, 0.5, 0.333, 0.25, 0.2, 0.166, 0.142, 0.125]) * 0.3
  pos = (pos + 1).mod(size)
  y
}\n";

const MODAL_LANE: &str = "const modes = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
fn modal(x, freq: init hz) {
  state s1 = zeros(16)
  state s2 = zeros(16)
  w = 2 * pi * freq / sample_rate * modes
  y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2
  s2 = s1
  s1 = y
  y.sum
}\n";

fn modal_lane(modes: usize) -> String {
    let list: Vec<String> = (1..=modes).map(|k| k.to_string()).collect();
    format!(
        "const modes = [{}]
fn modal(x, freq: init hz) {{
  state s1 = zeros({modes})
  state s2 = zeros({modes})
  w = 2 * pi * freq / sample_rate * modes
  y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2
  s2 = s1
  s1 = y
  y.sum
}}\ninstr a() {{ impulse(2hz).modal(220hz) }}",
        list.join(", ")
    )
}

const FRAMES: &str = "fn spectral(x) {
  x.frames(1024, fn(w) {
    s = fft(w)
    ifft(s[0], s[1])
  })
}\ninstr a() { spectral(osc(saw, 110hz)) }";

#[test]
#[ignore = "a benchmark"]
fn library() {
    bench(
        "std ladder",
        "use std/filters\ninstr a() { osc(saw, 110hz).ladder(800hz) }",
    );
    bench(
        "std biquad_lp",
        "use std/filters\ninstr a() { osc(saw, 110hz).biquad_lp(800hz) }",
    );
    bench(
        "std string",
        "use std/physical\ninstr a() { impulse(2hz).string(220hz) }",
    );
    bench(
        "std drum (8 modes)",
        "use std/physical\ninstr a() { impulse(2hz).drum(150hz) }",
    );
    bench(
        "std multitap (3 taps)",
        "use std/delays\ninstr a() { osc(saw, 110hz).multitap([100ms, 200ms, 300ms], [0.5, 0.3, 0.2]) }",
    );
    bench(
        "std fdn reverb (8 lines)",
        "use std/reverb\ninstr a() { osc(saw, 110hz).fdn() }",
    );
    bench(
        "std spectral blur (1024)",
        "use std/spectral\ninstr a() { osc(saw, 110hz).blur() }",
    );
}

#[test]
#[ignore = "a benchmark"]
fn frames() {
    bench("frames(1024): fft then ifft", FRAMES);
}

#[test]
#[ignore = "a benchmark"]
fn lane_sizes() {
    for modes in [4, 16, 64, 256] {
        bench(
            &format!("lane {modes}-mode resonator bank"),
            &modal_lane(modes),
        );
    }
}

#[test]
#[ignore = "a benchmark"]
fn kernels() {
    bench("built-in osc", "instr a() { osc(saw, 110hz) }");
    bench(
        "built-in lpf",
        "instr a() { osc(saw, 110hz).lpf(cutoff: 800hz) }",
    );
    bench(
        "fn one-pole",
        &format!("{ONE_POLE}instr a() {{ osc(saw, 110hz).one_pole(800hz) }}"),
    );
    bench(
        "fn ladder (4 poles, tanh)",
        &format!("{LADDER}instr a() {{ osc(saw, 110hz).ladder(800hz) }}"),
    );
    bench(
        "fn 64-tap fir",
        &format!("{FIR}instr a() {{ osc(saw, 110hz).fir_loop() }}"),
    );
    bench(
        "lane 64-tap fir",
        &format!("{FIR_LANE}instr a() {{ osc(saw, 110hz).lane_fir() }}"),
    );
    bench(
        "lane 8-tap multitap delay",
        &format!("{MULTITAP_LANE}instr a() {{ osc(saw, 110hz).multitap() }}"),
    );
    bench(
        "lane 16-mode resonator bank",
        &format!("{MODAL_LANE}instr a() {{ impulse(2hz).modal(220hz) }}"),
    );
    bench(
        "fn 8-tap multitap delay",
        &format!("{MULTITAP}instr a() {{ osc(saw, 110hz).multitap() }}"),
    );
    bench(
        "fn 16-mode resonator bank",
        &format!("{MODAL}instr a() {{ impulse(2hz).modal(220hz) }}"),
    );
}
