//! `std/blocks`: signals in, a sound out, and nothing that can go wrong however the controls are set.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};

fn program(source: &str) -> niminal_lang::Program {
    let expanded =
        expand(source, Path::new(".")).unwrap_or_else(|d| panic!("{}: {source}", d.message));
    compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("blocks", &expanded.text))
                .collect::<String>()
        )
    })
}

fn render(source: &str, blocks: usize) -> Vec<f32> {
    let program = program(source);
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut out = vec![0.0; blocks * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// A mono signal's level over the settled second half of a render.
fn settled_peak(body: &str) -> f32 {
    let out = render(&format!("use std/blocks\ninstr a() {{ {body} }}"), 400);
    assert!(out.iter().all(|v| v.is_finite()), "{body}");
    peak(&out[out.len() / 2..])
}

#[test]
fn a_harmonic_oscillator_sums_its_partials_to_at_most_one() {
    for brightness in [0.0, 0.5, 1.0] {
        let p = settled_peak(&format!(
            "harmonic_osc(220hz, brightness: {brightness}, count: 16)"
        ));
        assert!(p > 0.3 && p <= 1.001, "brightness {brightness}: {p}");
    }
}

#[test]
fn a_gate_closes_its_partial() {
    // Brightness 1 makes the levels 1 and 2^-0.1; with the fundamental gated off only the second remains.
    let level2 = 2f32.powf(-0.1);
    let total = 1.0 + level2;
    let closed = settled_peak("harmonic_osc(220hz, count: 2, brightness: 1, gates: [0, 1])");
    assert!((closed - level2 / total).abs() < 0.01, "{closed}");
    // A gate that is half open halves its partial: the fundamental alone, at half level.
    let half = settled_peak("harmonic_osc(220hz, count: 2, brightness: 1, gates: [0.5, 0])");
    assert!((half - 0.5 / total).abs() < 0.01, "{half}");
    // Partials without a gate stay open: three partials, one gate, shut.
    let ungated = settled_peak("harmonic_osc(220hz, count: 3, brightness: 1, gates: [0])");
    assert!(ungated > 0.5, "{ungated}");
}

#[test]
fn odd_even_picks_the_odd_or_the_even_partials() {
    // Only odd: the fundamental alone out of two, so the whole of the (scaled) sum is the fundamental.
    let odd = settled_peak("harmonic_osc(220hz, count: 2, odd_even: 1)");
    assert!((odd - 1.0).abs() < 0.01, "{odd}");
    // Only even: the second partial alone.
    let even = settled_peak("harmonic_osc(220hz, count: 2, odd_even: -1)");
    assert!((even - 1.0).abs() < 0.01, "{even}");
}

#[test]
fn partials_that_would_alias_are_left_out() {
    // 15 kHz: the second partial is 30 kHz, past 45% of the sample rate, so only the fundamental sounds.
    let p = settled_peak("harmonic_osc(15khz, count: 2)");
    assert!((p - 1.0).abs() < 0.01, "{p}");
}

#[test]
fn a_harmonic_oscillator_survives_extreme_controls() {
    let program = program(
        "use std/blocks\ninstr a(freq: hz, brightness: 0..1, odd_even: -1..1, smoothing: 0..1) {\n  \
         harmonic_osc(freq, count: 32, brightness: brightness, odd_even: odd_even, gate_smooth: smoothing * 1sec, gates: [1, 0, 0.5])\n}",
    );
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let extremes = [0.0, 1.0, -1.0, 0.5, 10.0, -10.0, 1e6, -1e6, 1e30, -1e30];
    let mut failures = Vec::new();
    for freq in [0.0f32, 1.0, 55.0, 880.0, 20_000.0, 1e6, -440.0] {
        for &x in &extremes {
            let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
            for name in ["brightness", "odd_even", "smoothing"] {
                voice.set_param_by_name(name, x as f32);
            }
            voice.set_param_by_name("freq", freq);
            voice.start_note();
            let mut out = vec![0.0; 40 * block_size()];
            for chunk in out.chunks_mut(block_size()) {
                voice.process(chunk);
            }
            if voice.is_poisoned() || !out.iter().all(|v| v.is_finite()) {
                failures.push(format!("{freq} Hz, controls {x}: NaN or infinity"));
            } else if peak(&out) > 1.5 {
                failures.push(format!("{freq} Hz, controls {x}: peak {}", peak(&out)));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn control_blocks_survive_extreme_controls_and_stay_within_their_range() {
    // Each block is read through an instrument whose controls are set far outside what they declare.
    let blocks = [
        ("function_gen(cycle: x, rise: y, curve: z)", 0.0, 1.0),
        ("wobble(beats: x, seed: 3)", -1.0, 1.0),
        ("wobble(beats: x, shape: smooth, seed: 3)", -1.0, 1.0),
        ("clock_div(x, width: y)", 0.0, 1.0),
    ];
    let extremes = [
        0.0,
        1.0,
        -1.0,
        0.5,
        10.0,
        -10.0,
        1e6,
        -1e6,
        1e30,
        -1e30,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut failures = Vec::new();
    for (call, lo, hi) in blocks {
        let program = program(&format!(
            "use std/blocks\ninstr a(x: 0..1, y: 0..1, z: 0..1) {{ {call} }}"
        ));
        let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
        for &x in &extremes {
            for &y in &extremes {
                for &z in &[0.0, 1.0, 1e30, f32::NAN] {
                    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
                    voice.set_param_by_name("x", x);
                    voice.set_param_by_name("y", y);
                    voice.set_param_by_name("z", z);
                    voice.start_note();
                    let mut out = vec![0.0; 20 * block_size()];
                    for chunk in out.chunks_mut(block_size()) {
                        voice.process(chunk);
                    }
                    let (min, max) = out
                        .iter()
                        .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
                    if voice.is_poisoned() || !out.iter().all(|v| v.is_finite()) {
                        failures.push(format!("{call} with {x}, {y}, {z}: NaN or infinity"));
                    } else if min < lo - 0.001 || max > hi + 0.001 {
                        failures.push(format!("{call} with {x}, {y}, {z}: {min}..{max}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures[..failures.len().min(20)].join("\n"));
}

#[test]
fn a_blocks_own_names_never_clash_with_your_controls() {
    // The names a block uses inside itself (phase, rate, levels, slope, ...) are also good names for controls.
    let names = [
        "phase", "rate", "levels", "slope", "up", "down", "parity", "heard", "frequencies",
    ];
    let controls: String = names.iter().map(|n| format!("ctl {n} = 0.5\n")).collect();
    program(&format!(
        "use std/blocks\n{controls}instr a(freq: hz) {{\n  \
         harmonic_osc(freq, gates: [1, 0.5]) * function_gen(cycle: 4) * (wobble(seed: 1) + clock_div(2))\n}}"
    ));
}

#[test]
fn an_echo_loop_survives_extreme_controls() {
    let mut failures = Vec::new();
    for mode in ["off", "on"] {
        let program = program(&format!(
            "use std/blocks\ninstr a(x: 0..1, y: 0..1, z: 0..1) {{\n  \
             w = osc(saw, 110hz).echo_loop(time: x * 1sec, feedback: y, filter: z * 1hz, duck: y, ping_pong: {mode})\n  w[0] + w[w.channels.len - 1]\n}}"
        ));
        let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
        for &x in &[0.0, 0.001, 1.0, 1e6, -1.0, f32::NAN, f32::INFINITY] {
            for &y in &[0.0, 0.95, 1.0, 10.0, -10.0, f32::NAN] {
                for &z in &[0.0, 20.0, 20_000.0, 1e9, -1.0, f32::NAN] {
                    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
                    voice.set_param_by_name("x", x);
                    voice.set_param_by_name("y", y);
                    voice.set_param_by_name("z", z);
                    voice.start_note();
                    let mut out = vec![0.0; 200 * block_size()];
                    for chunk in out.chunks_mut(block_size()) {
                        voice.process(chunk);
                    }
                    if voice.is_poisoned() || !out.iter().all(|v| v.is_finite() && v.abs() < 50.0) {
                        failures.push(format!("ping_pong {mode} with {x}, {y}, {z}: peak {}", peak(&out)));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures[..failures.len().min(20)].join("\n"));
}
