//! Offline timings of audio-rate and control-rate modulation. Run with
//! `cargo test --release -p niminal-lang --test control_bench -- --ignored --nocapture`.

use std::time::Instant;

use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

/// Long enough that the clock speed and core a run lands on stop mattering.
const SECONDS: usize = 60;

/// The best of a few runs, since a busy machine only ever slows one down.
fn seconds(source: &str, voices: usize, secs: usize) -> f64 {
    let program = compile(source).unwrap_or_else(|errors| panic!("{errors:?}"));
    let graph = program.instruments[0].graph.clone();
    (0..3)
        .map(|_| {
            let mut all: Vec<Voice> = (0..voices)
                .map(|_| Voice::new(graph.clone(), 48_000.0))
                .collect();
            let mut block = vec![0.0; block_size()];
            let blocks = 48_000 * secs / block_size();
            let start = Instant::now();
            let mut sink = 0.0;
            for _ in 0..blocks {
                for v in &mut all {
                    v.process(&mut block);
                    sink += block[0];
                }
            }
            assert!(sink.is_finite());
            start.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min)
}

#[test]
#[ignore]
fn modulation_cost() {
    let cases = [
        ("static cutoff", "instr a(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: 1khz, res: 0.3) }"),
        (
            "audio-rate lfo",
            "instr a(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: osc(sine, 0.3hz).range(300hz..3khz), res: 0.3) }",
        ),
        (
            "control lfo",
            "instr a(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: control { osc(sine, 0.3hz).range(300hz..3khz) }.smooth, res: 0.3) }",
        ),
        (
            "control lfo, held",
            "instr a(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: control { osc(sine, 0.3hz).range(300hz..3khz) }, res: 0.3) }",
        ),
        (
            "8 audio lfos",
            "instr a(freq: hz = 110hz) { x = osc(saw, freq)\n [0.1hz, 0.2hz, 0.3hz, 0.4hz, 0.5hz, 0.6hz, 0.7hz, 0.8hz].map(fn(r) { x.lpf(cutoff: osc(sine, r).range(300hz..3khz), res: 0.3) }).sum }",
        ),
        (
            "8 control lfos",
            "instr a(freq: hz = 110hz) { x = osc(saw, freq)\n [0.1hz, 0.2hz, 0.3hz, 0.4hz, 0.5hz, 0.6hz, 0.7hz, 0.8hz].map(fn(r) { x.lpf(cutoff: control { osc(sine, r).range(300hz..3khz) }.smooth, res: 0.3) }).sum }",
        ),
    ];
    let kernel_cases = [
        (
            "graph: saw+lpf",
            "instr a(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: 1khz, res: 0.3).lpf(cutoff: 2khz).lpf(cutoff: 3khz) }",
        ),
        (
            "kernel: saw+lpf",
            "fn f(x) { state n = 0\n n = n + 1\n x.lpf(cutoff: 1khz, res: 0.3).lpf(cutoff: 2khz).lpf(cutoff: 3khz) + n * 0 }\ninstr a(freq: hz = 110hz) { f(osc(saw, freq)) }",
        ),
        (
            "kernel: hand one-pole x3",
            "fn p(x, c: hz) { state y = 0\n a = exp(-2 * pi * c / sample_rate)\n y = x * (1 - a) + y * a\n y }\ninstr a(freq: hz = 110hz) { osc(saw, freq).p(1khz).p(2khz).p(3khz) }",
        ),
    ];
    for (name, source) in kernel_cases.into_iter().chain(cases) {
        let t = seconds(source, 16, SECONDS);
        println!("{name:>24}: {t:6.3}s for 16 voices x {SECONDS}s ({:.1}x realtime per voice)", SECONDS as f64 / t);
    }
}

/// A long run of one stateful fn, for attaching a profiler: `PROFILE=1 cargo test --release ...`.
#[test]
#[ignore]
fn profile_kernel() {
    let source = "fn f(x) { state n = 0\n n = n + 1\n x.lpf(cutoff: 1khz, res: 0.3).lpf(cutoff: 2khz).lpf(cutoff: 3khz) + n * 0 }\ninstr a(freq: hz = 110hz) { f(osc(saw, freq)) }";
    println!("{:.2}s", seconds(source, 16, 300));
}
