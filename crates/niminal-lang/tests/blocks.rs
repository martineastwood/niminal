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

#[test]
fn a_scan_crossfades_neighbours_and_preserves_constant_levels() {
    for (centre, expected) in [(0.0, 1.0), (0.25, 2.0), (0.5, 3.0), (1.0, 5.0)] {
        let p = settled_peak(&format!("scan_mix([1, 3, 5], centre: {centre})"));
        assert!((p - expected).abs() < 0.001, "{centre}: {p}");
    }
    for width in [1, 2, 64] {
        for centre in [0.0, 0.5, 1.0] {
            let p = settled_peak(&format!(
                "scan_mix([0.7, 0.7, 0.7], centre: {centre}, width: {width})"
            ));
            assert!((p - 0.7).abs() < 0.001, "{centre}, {width}: {p}");
        }
    }
    assert_eq!(settled_peak("scan_mix([], centre: 0.5)"), 0.0);
}

#[test]
fn a_harmonic_bank_exposes_the_same_partials_it_mixes() {
    let p = settled_peak(
        "b = harmonic_bank(220hz, centre: 0.7, width: 3, focus: 0.8)\n b.mixed - b.partials.sum",
    );
    assert!(p < 1e-6, "{p}");
    // With a narrow, fully focused window the endpoints select fundamental and fourth harmonic.
    for (centre, frequency) in [(0, 220), (1, 880)] {
        let out = render(
            &format!(
                "use std/blocks\ninstr a() {{ harmonic_osc(220hz, count: 4, centre: {centre}, width: 1, focus: 1) }}"
            ),
            400,
        );
        let correlation = out
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                f64::from(x)
                    * (std::f64::consts::TAU * f64::from(frequency) * i as f64 / 48_000.0).sin()
            })
            .sum::<f64>()
            / out.len() as f64;
        assert!(correlation > 0.49, "centre {centre}: {correlation}");
    }
}

#[test]
fn the_stereo_echo_has_a_dry_free_first_repeat_and_a_feedback_tail() {
    let audio = render(
        "use std/blocks\ninstr a() {\n x = env[1 1ms 0]\n e = x.stereo_echo(time: 50ms, feedback: 0.5, skew: 0, diffusion: 0, motion: 0)\n e[0]\n}",
        200,
    );
    assert!(peak(&audio[..2300]) < 1e-6);
    assert!(peak(&audio[2400..2600]) > 0.8);
    assert!(peak(&audio[4800..5000]) > 0.1);
}

#[test]
fn stereo_echo_controls_change_the_returned_sound() {
    for (control, low, high) in [
        ("skew", "-0.5", "0.5"),
        ("colour", "0", "1"),
        ("diffusion", "0", "1"),
        ("motion", "0", "1"),
        ("duck", "0", "1"),
    ] {
        let sound = |value| {
            render(
                &format!(
                    "use std/blocks\ninstr a() {{ e = osc(saw, 220hz).stereo_echo(time: 20ms, {control}: {value})\n e[0] }}"
                ),
                400,
            )
        };
        let (a, b) = (sound(low), sound(high));
        let difference = a.iter().zip(&b).map(|(a, b)| (a - b).abs()).sum::<f32>();
        assert!(difference > 1.0, "{control}: {difference}");
    }
}

#[test]
fn scanning_and_stereo_echo_stay_finite_under_extreme_modulation() {
    let extremes = [
        -1e30,
        -1.0,
        0.0,
        0.5,
        1.0,
        1e30,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    for body in [
        "harmonic_bank(220hz, centre: x, width: y, focus: z, gates: [x, y, z]).mixed",
        "scan_mix([osc(sine, 110hz), osc(sine, 220hz)], centre: x, width: y)",
        "e = osc(saw, 110hz).stereo_echo(time: x * 1sec, feedback: y, colour: z, skew: x, diffusion: y, motion: z, duck: x)\n e[0] + e[1]",
    ] {
        let p = program(&format!(
            "use std/blocks\ninstr a(x: num, y: num, z: num) {{ {body} }}"
        ));
        let instrument = p.instruments.iter().find(|i| i.name == "a").unwrap();
        for &x in &extremes {
            for &y in &extremes {
                let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
                voice.start_note();
                let mut out = vec![0.0; block_size()];
                for z in extremes {
                    voice.set_param_by_name("x", x);
                    voice.set_param_by_name("y", y);
                    voice.set_param_by_name("z", z);
                    for _ in 0..40 {
                        voice.process(&mut out);
                        assert!(
                            !voice.is_poisoned()
                                && out.iter().all(|v| v.is_finite() && v.abs() < 50.0),
                            "{body}: {x}, {y}, {z}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn stereo_echo_preserves_separate_inputs_and_skew_separates_mono_repeats() {
    let silent = render(
        "use std/blocks\ninstr a() { e = [env[1 1ms 0], 0].stereo_echo(time: 50ms)\n e[1] }",
        300,
    );
    assert_eq!(peak(&silent), 0.0);
    let difference = render(
        "use std/blocks\ninstr a() { e = env[1 1ms 0].stereo_echo(time: 50ms, skew: 1, motion: 0, diffusion: 0, feedback: 0)\n e[0] - e[1] }",
        300,
    );
    assert!(peak(&difference[..1100]) < 1e-6);
    assert!(difference[1200..1300].iter().any(|x| *x > 0.8));
    assert!(difference[3600..3700].iter().any(|x| *x < -0.8));
}

#[test]
fn a_held_trigger_runs_one_contour_with_sample_pulses_at_its_turns() {
    let read = |field| {
        render(
            &format!(
                "use std/blocks\ninstr a() {{ contour(trigger: 1, rise: 10ms, fall: 20ms).{field} }}"
            ),
            40,
        )
    };
    let (value, eor, eoc) = (read("value"), read("eor"), read("eoc"));
    for (pulse, expected) in [(&eor, 480), (&eoc, 1440)] {
        let hits: Vec<_> = pulse
            .iter()
            .enumerate()
            .filter(|(_, x)| **x > 0.5)
            .collect();
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].0.abs_diff(expected) <= 2,
            "{} vs {expected}",
            hits[0].0
        );
        assert_eq!(*hits[0].1, 1.0);
    }
    assert!((value[239] - 0.5).abs() < 0.005);
    assert!(value[479] > 0.995);
    assert!(value[1600..].iter().all(|x| *x == 0.0));
}

#[test]
fn contour_retrigger_rises_from_the_present_level_and_cycle_off_finishes() {
    let p = program(
        "use std/blocks\ninstr a(t: num, cycling: num) { contour(trigger: t, rise: 10ms, fall: 20ms, cycle: cycling) }",
    );
    let mut voice = Voice::new(p.instruments[0].graph.clone(), 48_000.0);
    voice.set_param_by_name("t", 1.0);
    let mut first = vec![0.0; 480];
    for chunk in first.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    voice.set_param_by_name("t", 0.0);
    let mut falling = vec![0.0; 480];
    for chunk in falling.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    let before = falling[479];
    voice.set_param_by_name("t", 1.0);
    let mut retriggered = vec![0.0; 100];
    for chunk in retriggered.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    assert!((retriggered[0] - before).abs() < 0.003);
    assert!(retriggered[99] > before + 0.15);

    voice.set_param_by_name("t", 0.0);
    voice.set_param_by_name("cycling", 1.0);
    for chunk in vec![0.0; 3000].chunks_mut(block_size()) {
        voice.process(chunk);
    }
    voice.set_param_by_name("cycling", 0.0);
    let mut finished = vec![0.0; 3000];
    for chunk in finished.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    assert!(finished[2000..].iter().all(|x| *x == 0.0));
}

#[test]
fn contours_cycle_chain_and_keep_timing_when_the_curve_changes() {
    let cycle = render(
        "use std/blocks\ninstr a() { contour(rise: 0ms, fall: 0ms, cycle: 1).eoc }",
        2,
    );
    assert!(cycle.iter().all(|x| *x == 0.0 || *x == 1.0));
    assert_eq!(cycle.iter().filter(|x| **x == 1.0).count(), cycle.len() / 2);
    let chained = render(
        "use std/blocks\ninstr a() { first = contour(trigger: 1, rise: 10ms, fall: 20ms)\n contour(trigger: first.eoc, rise: 10ms, fall: 20ms) }",
        40,
    );
    assert!(peak(&chained[..1438]) == 0.0);
    assert!(peak(&chained[1440..2000]) > 0.99);
    for curve in [0.1, 1.0, 10.0] {
        let eor = render(
            &format!(
                "use std/blocks\ninstr a() {{ contour(trigger: 1, rise: 10ms, curve: {curve}).eor }}"
            ),
            10,
        );
        assert_eq!(eor.iter().filter(|x| **x == 1.0).count(), 1);
        assert!(eor[478..482].contains(&1.0));
    }
}

#[test]
fn space_reverb_returns_a_tail_and_its_controls_do_audible_work() {
    let sound = |control: &str| {
        render(
            &format!(
                "use std/blocks\ninstr a() {{ w = (noise(seed: 3) * env[1 10ms 0]).space_reverb({control})\n w[0] }}"
            ),
            1600,
        )
    };
    let energy = |x: &[f32]| x.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
    let short = sound("decay: 0.2");
    let long = sound("decay: 0.85");
    assert!(
        peak(&long[..800]) == 0.0,
        "predelay must separate the sound from its return"
    );
    assert!(energy(&long[48_000..]) > energy(&short[48_000..]) * 5.0);
    for (control, low, high) in [
        ("size", "0.1", "0.9"),
        ("absorption", "0", "1"),
        ("diffusion", "0", "1"),
        ("motion", "0", "1"),
        ("rate", "0.01hz", "4hz"),
        ("predelay", "0ms", "200ms"),
        ("tilt", "-1", "1"),
    ] {
        let (a, b) = (
            sound(&format!("{control}: {low}")),
            sound(&format!("{control}: {high}")),
        );
        let delta = a
            .iter()
            .zip(&b)
            .map(|(a, b)| f64::from(a - b).powi(2))
            .sum::<f64>();
        assert!(delta > 1e-5, "{control}: {delta}");
    }
}

#[test]
fn ducking_responds_to_opposite_polarity_stereo_input() {
    for block in ["stereo_echo(time: 20ms", "space_reverb(predelay: 0ms"] {
        let sound = |duck| {
            render(
                &format!(
                    "use std/blocks\ninstr a() {{ s = osc(saw, 220hz)\n w = [s, 0-s].{block}, duck: {duck})\n w[0] }}"
                ),
                500,
            )
        };
        let (dry, ducked) = (sound(0), sound(1));
        let energy = |x: &[f32]| {
            x[x.len() / 2..]
                .iter()
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>()
        };
        assert!(energy(&ducked) < energy(&dry) * 0.2, "{block}");
    }
}

#[test]
fn contour_and_space_survive_independent_extreme_controls() {
    let extremes = [
        -1e30,
        -1.0,
        0.0,
        0.5,
        1.0,
        1e30,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    for body in [
        "c = contour(trigger: x, cycle: y, rise: z * 1sec, fall: x * 1sec, curve: y)\n c.value + c.eor + c.eoc",
        "w = osc(saw, 110hz).space_reverb(size: x, decay: y, absorption: z, diffusion: x, motion: y, rate: z * 1hz, predelay: x * 1sec, tilt: y, duck: z)\n w[0] + w[1]",
    ] {
        let p = program(&format!(
            "use std/blocks\ninstr a(x: num, y: num, z: num) {{ {body} }}"
        ));
        for x in extremes {
            for y in extremes {
                let mut voice = Voice::new(p.instruments[0].graph.clone(), 48_000.0);
                let mut out = vec![0.0; block_size()];
                for z in extremes {
                    voice.set_param_by_name("x", x);
                    voice.set_param_by_name("y", y);
                    voice.set_param_by_name("z", z);
                    for _ in 0..40 {
                        voice.process(&mut out);
                        assert!(
                            !voice.is_poisoned()
                                && out.iter().all(|v| v.is_finite() && v.abs() < 50.0),
                            "{body}: {x}, {y}, {z}"
                        );
                    }
                }
            }
        }
    }
}
