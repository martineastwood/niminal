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
        "phase", "rate", "levels", "slope", "up", "down", "parity", "heard", "frequencies", "low",
        "band", "high", "notch", "sound", "opening", "modulator", "value", "gate", "fall", "aux", "mid",
    ];
    let controls: String = names.iter().map(|n| format!("ctl {n} = 0.5\n")).collect();
    program(&format!(
        "use std/blocks\n{controls}instr a(freq: hz) {{\n  \
         harmonic_osc(freq, gates: [1, 0.5]) * function_gen(cycle: 4) * (wobble(seed: 1) + clock_div(2)) \
         + complex_osc(freq).low_pass_gate().sound.multimode_filter().band * shift_random(impulse(2hz)).value\n}}"
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

/// The correlation of `x` with a sine at `frequency`: about 0.5 for a full-scale sine in phase, 0 for none.
fn sine_content(x: &[f32], frequency: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let w = std::f64::consts::TAU * frequency * i as f64 / 48_000.0;
        re += f64::from(v) * w.sin();
        im += f64::from(v) * w.cos();
    }
    (re * re + im * im).sqrt() / x.len() as f64
}

fn settled(body: &str) -> Vec<f32> {
    let out = render(&format!("use std/blocks\ninstr a() {{ {body} }}"), 400);
    assert!(out.iter().all(|v| v.is_finite()), "{body}");
    out[out.len() / 2..].to_vec()
}

#[test]
fn the_multimode_filter_gives_each_response_from_the_same_input() {
    // Two sines a long way either side of a 1khz cutoff, read through each output.
    let input = "(osc(sine, 100hz) + osc(sine, 10khz))";
    let read = |output: &str| settled(&format!("{input}.multimode_filter(cutoff: 1khz, res: 0).{output}"));
    let (low, high) = (read("low"), read("high"));
    assert!(sine_content(&low, 100.0) > 0.45 && sine_content(&low, 10_000.0) < 0.01);
    assert!(sine_content(&high, 10_000.0) > 0.45 && sine_content(&high, 100.0) < 0.01);
    // At the cutoff the notch takes a sine out, and the band passes it at half level at resonance 0.
    let notch = settled("osc(sine, 1khz).multimode_filter(cutoff: 1khz, res: 0.5).notch");
    assert!(peak(&notch) < 0.01, "{}", peak(&notch));
    let band = settled("osc(sine, 1khz).multimode_filter(cutoff: 1khz, res: 0).band");
    assert!((peak(&band) - 0.5).abs() < 0.02, "{}", peak(&band));
}

#[test]
fn the_multimode_filter_holds_its_resonance_and_filters_channels_apart() {
    for drive in [0.0, 1.0] {
        for cutoff in ["200hz", "880hz", "3khz"] {
            let p = peak(&settled(&format!(
                "osc(saw, 55hz).multimode_filter(cutoff: {cutoff}, res: 1, drive: {drive}).low"
            )));
            assert!(p > 0.3 && p < 2.0, "{cutoff}, drive {drive}: {p}");
        }
    }
    // A silent right channel stays silent while the left rings.
    let right = settled("[osc(saw, 110hz), 0].multimode_filter(cutoff: 500hz, res: 0.9).low[1]");
    assert_eq!(peak(&right), 0.0);
}

#[test]
fn a_low_pass_gate_strikes_closes_and_darkens_as_it_closes() {
    // The default strikes once as the note starts and has closed by the end.
    let struck = render("use std/blocks\ninstr a() { osc(saw, 220hz).low_pass_gate(decay: 100ms).sound }", 800);
    assert!(peak(&struck[..4800]) > 0.5);
    assert!(peak(&struck[struck.len() - 4800..]) < 1e-3);
    // The opening is 0..1 and level holds it open without a strike.
    let opening = settled("osc(saw, 220hz).low_pass_gate(ping: 0, level: 0.6).opening");
    assert!((peak(&opening) - 0.6).abs() < 0.01);
    // Half open, the filter takes more off the top than the bottom; the gate alone does not.
    let filtered = settled("osc(saw, 220hz).low_pass_gate(ping: 0, level: 0.5, mode: filter).sound");
    let ratio = sine_content(&filtered, 2_200.0) / sine_content(&filtered, 220.0);
    assert!(ratio < 0.02, "{ratio}");
    let gated = settled("osc(saw, 220hz).low_pass_gate(ping: 0, level: 0.5, mode: gate).sound");
    let ratio = sine_content(&gated, 2_200.0) / sine_content(&gated, 220.0);
    assert!(ratio > 0.08, "{ratio}");
}

#[test]
fn a_complex_oscillator_starts_as_a_sine_and_its_controls_add_harmonics() {
    let plain = settled("complex_osc(220hz, index: 0)");
    assert!(sine_content(&plain, 220.0) > 0.49);
    for control in ["index: 3", "index: 0, fold: 0.8", "am: 1", "index: 1, fold: 0.5, symmetry: 1"] {
        let rich = settled(&format!("complex_osc(220hz, {control})"));
        let p = peak(&rich);
        assert!(p <= 1.2, "{control}: {p}");
        assert!(sine_content(&rich, 220.0) < 0.45, "{control}");
    }
    // The modulator is a sine at ratio times the frequency.
    assert!(sine_content(&settled("complex_osc(220hz, ratio: 3).modulator"), 660.0) > 0.49);
}

#[test]
fn a_shift_random_loop_repeats_when_locked_and_changes_when_not() {
    // 48 steps a second, eight to a loop: read the value at each step.
    let steps = |lock: f32, seed: u32| {
        let out = render(
            &format!("use std/blocks\ninstr a() {{ shift_random(clock: impulse(48hz), lock: {lock}, seed: {seed}).value }}"),
            1000,
        );
        assert!(out.iter().all(|v| (-1.0..=1.0).contains(v)));
        (0..32).map(|i| out[i * 1000 + 500]).collect::<Vec<_>>()
    };
    let locked = steps(1.0, 1);
    assert_eq!(locked[..8], locked[8..16]);
    assert_eq!(locked[..8], locked[24..32]);
    assert!(locked[..8].iter().any(|v| *v != locked[0]));
    let free = steps(0.0, 1);
    assert_ne!(free[..8], free[8..16]);
    assert_eq!(steps(0.3, 5), steps(0.3, 5));
    assert_ne!(steps(1.0, 5), steps(1.0, 6));
}

#[test]
fn the_voice_modules_stay_finite_under_extreme_modulation() {
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
    for (body, limit) in [
        ("f = osc(saw, 110hz).multimode_filter(cutoff: x * 1hz, res: y, drive: z)\n f.low + f.band + f.high + f.notch", 50.0),
        ("osc(saw, 110hz).low_pass_gate(ping: x, level: y, decay: z * 1sec, res: x).sound", 50.0),
        ("complex_osc(x * 1hz, ratio: y, index: z, am: x, fold: y, symmetry: z).sound", 1.2),
        ("shift_random(clock: x, lock: y, seed: 2).value", 1.0),
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
                                && out.iter().all(|v| v.is_finite() && v.abs() <= limit),
                            "{body}: {x}, {y}, {z}: {}",
                            peak(&out)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_voice_module_example_in_the_docs_plays() {
    let out = render(
        "use std/blocks\ninstr a() {\n  freq = 130.81hz\n  steps = shift_random(clock: clock_div(1/4), lock: 0.9, seed: 4)\n  voice = complex_osc(freq * (1 + steps.value.max(0)), ratio: 1.5, index: 2, fold: 0.3)\n  gated = voice.low_pass_gate(ping: steps.gate, decay: 180ms)\n  gated.sound.multimode_filter(cutoff: 300hz + gated.opening * 3khz, res: 0.7).band * env[1 | 50ms 0]\n}",
        400,
    );
    assert!(out.iter().all(|v| v.is_finite()));
    let p = peak(&out);
    assert!(p > 0.05 && p < 1.5, "{p}");
}

/// The last sample of a render of a constant expression.
fn settled_value(body: &str) -> f32 {
    let out = render(&format!("use std/blocks\ninstr a() {{ {body} }}"), 4);
    assert!(out.iter().all(|v| v.is_finite()), "{body}");
    out[out.len() - 1]
}

#[test]
fn quantize_snaps_to_the_nearest_scale_note_and_ties_go_down() {
    for (body, expected) in [
        ("quantize(0)", 0.0),
        ("quantize(1)", 0.0),
        ("quantize(1.4)", 2.0),
        ("quantize(5.4)", 5.0),
        ("quantize(11.6)", 12.0),
        ("quantize(-0.6)", -1.0),
        ("quantize(24.4)", 24.0),
        ("quantize(7, root: 2)", 7.0),
        ("quantize(3.2, root: 1)", 3.0),
        ("quantize(4.9, scale: [0, 7])", 7.0),
        ("quantize(9.4, scale: [0, 3, 5, 7, 10])", 10.0),
        ("quantize(1.4, amount: 0)", 1.4),
        ("quantize(1.4, amount: 0.5)", 1.7),
    ] {
        let got = settled_value(body);
        assert!((got - expected).abs() < 1e-4, "{body}: {got}, wanted {expected}");
    }
}

fn stereo_render(body: &str, blocks: usize) -> Vec<f32> {
    let p = program(&format!(
        "use std/blocks\nconfig {{ channels: stereo }}\ninstr a() {{ src = harmonics([1, 0.3, 0.2], size: 512)\n {body} }}"
    ));
    let instrument = p.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut buses = niminal_engine::Buses::new(&[]);
    let mut block = vec![vec![0.0; block_size()]; 2];
    let mut out = Vec::new();
    for _ in 0..blocks {
        voice.process_blocks(&mut block, block_size(), &mut buses);
        out.extend(block[0].iter().chain(&block[1]));
    }
    out
}

#[test]
fn a_texture_stays_at_a_steady_level_however_thick_it_is() {
    for body in [
        "texture(src)",
        "texture(src, density: 5hz, size: 50ms)",
        "texture(src, density: 100hz, size: 500ms, spread: 1, detune: 12)",
        "texture(src, pitch: -24, position: 1)",
    ] {
        let out = stereo_render(body, 300);
        assert!(out.iter().all(|v| v.is_finite()), "{body}");
        let p = peak(&out[out.len() / 2..]);
        assert!(p > 0.01 && p < 1.5, "{body}: {p}");
    }
    // The same seed is the same cloud; another seed is another.
    let a = stereo_render("texture(src, spread: 1, seed: 3)", 50);
    assert_eq!(a, stereo_render("texture(src, spread: 1, seed: 3)", 50));
    assert_ne!(a, stereo_render("texture(src, spread: 1, seed: 4)", 50));
}

#[test]
fn a_vocoder_speaks_the_carrier_through_the_modulator() {
    let silent = settled_peak("vocoder(osc(saw, 110hz), 0 * osc(saw, 200hz), hiss: 0)");
    assert!(silent < 0.01, "{silent}");
    let spoken = settled_peak("vocoder(osc(saw, 110hz), osc(saw, 200hz))");
    assert!(spoken > 0.05 && spoken < 1.5, "{spoken}");
}

#[test]
fn cross_mod_blends_from_the_first_signal_to_the_effect() {
    let plain = settled_peak("cross_mod(osc(sine, 220hz) * 0.5, osc(sine, 330hz), amount: 0).sound");
    assert!((plain - 0.5).abs() < 0.02, "{plain}");
    let silent = settled_peak("cross_mod(osc(sine, 220hz), 0 * osc(sine, 330hz)).aux");
    assert!(silent < 1e-4, "{silent}");
    // silence in `a` is silence out in every mode (so an effect made of it can go quiet), and ring and diode
    // are silent when `b` is
    for mode in ["ring", "fold", "diode"] {
        let body = format!("cross_mod(0 * osc(sine, 220hz), osc(sine, 330hz), mode: {mode}, amount: 1).sound");
        let silent = settled_peak(&body);
        assert!(silent < 1e-4, "{body}: {silent}");
    }
    for mode in ["ring", "diode"] {
        let body = format!("cross_mod(osc(sine, 220hz), 0 * osc(sine, 330hz), mode: {mode}, amount: 1).sound");
        let silent = settled_peak(&body);
        assert!(silent < 1e-4, "{body}: {silent}");
    }
    for mode in ["ring", "fold", "diode"] {
        let p = settled_peak(&format!(
            "cross_mod(osc(sine, 220hz), osc(saw, 331hz), mode: {mode}, amount: 1).sound"
        ));
        assert!(p > 0.05 && p <= 1.2, "{mode}: {p}");
    }
}

#[test]
fn the_processing_modules_stay_finite_under_extreme_modulation() {
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
    for (body, limit, stereo) in [
        ("quantize(x, root: y, amount: z)", 1100.0, false),
        ("vocoder(osc(saw, 110hz), osc(saw, 200hz), low: y * 1hz, high: z * 1hz, width: x, attack: y * 1ms, release: z * 1ms, hiss: x)", 1.5, false),
        ("cross_mod(osc(saw, 110hz) * x, osc(sine, 330hz) * y, amount: z, mode: fold).sound", 1.2, false),
        ("cross_mod(osc(saw, 110hz) * x, osc(sine, 330hz) * y, amount: z, mode: diode).sound", 1.2, false),
        ("texture(src, position: x, size: y * 1ms, density: z * 1hz, pitch: x, spread: y, detune: z, width: x, gain: y)", 4.0, true),
    ] {
        let source = if stereo {
            format!("use std/blocks\nconfig {{ channels: stereo }}\ninstr a(x: num, y: num, z: num) {{ src = harmonics([1, 0.3], size: 512)\n {body} }}")
        } else {
            format!("use std/blocks\ninstr a(x: num, y: num, z: num) {{ {body} }}")
        };
        let p = program(&source);
        let instrument = p.instruments.iter().find(|i| i.name == "a").unwrap();
        for &x in &extremes {
            for &y in &extremes {
                let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
                voice.start_note();
                let mut buses = niminal_engine::Buses::new(&[]);
                let mut block = vec![vec![0.0; block_size()]; if stereo { 2 } else { 1 }];
                for z in extremes {
                    voice.set_param_by_name("x", x);
                    voice.set_param_by_name("y", y);
                    voice.set_param_by_name("z", z);
                    for _ in 0..20 {
                        voice.process_blocks(&mut block, block_size(), &mut buses);
                        assert!(
                            !voice.is_poisoned()
                                && block.iter().flatten().all(|v| v.is_finite() && v.abs() <= limit),
                            "{body}: {x}, {y}, {z}"
                        );
                    }
                }
            }
        }
    }
}

fn last_of(source: &str) -> f32 {
    let out = render(source, 4);
    out[out.len() - 1]
}

#[test]
fn a_choice_can_be_passed_on_to_another_function() {
    // Plain functions, and functions with state (compiled as kernels), in every pairing.
    for (inner_state, outer_state) in [("", ""), ("state _s = 0\n", ""), ("", "state _s = 0\n"), ("state _s = 0\n", "state _s = 0\n")] {
        let program = |call: &str| {
            format!(
                "fn inner(x, mode: (low | high) = low) {{\n {inner_state} if mode == low {{ x * 0.25 }} else {{ x * 0.5 }}\n}}\n\
                 fn outer(x, mode: (low | high) = low) {{\n {outer_state} inner(x, mode: mode)\n}}\n\
                 instr a(pick: (low | high) = high) {{ {call} }}"
            )
        };
        let tag = format!("inner {inner_state:?}, outer {outer_state:?}");
        assert_eq!(last_of(&program("outer(1, mode: high)")), 0.5, "{tag}");
        assert_eq!(last_of(&program("outer(1, mode: low)")), 0.25, "{tag}");
        assert_eq!(last_of(&program("outer(1)")), 0.25, "default, {tag}");
        // an instrument's own choice, and one passed through two functions
        assert_eq!(last_of(&program("outer(1, mode: pick)")), 0.5, "instr, {tag}");
        assert_eq!(last_of(&program("outer(outer(1, mode: pick), mode: pick)")), 0.25, "twice, {tag}");
    }
}

#[test]
fn a_passed_on_choice_is_matched_by_name_not_by_position() {
    let source = "fn inner(x, mode: (high | low) = low) { if mode == low { x * 0.25 } else { x * 0.5 } }\n\
                  fn outer(x, mode: (low | high) = low) { inner(x, mode: mode) }\n\
                  instr a() { outer(1, mode: high) }";
    assert_eq!(last_of(source), 0.5);
}

#[test]
fn a_passed_on_choice_the_callee_does_not_have_is_an_error() {
    let source = "fn inner(x, mode: (low | mid) = low) { x }\n\
                  fn outer(x, mode: (low | high) = low) { inner(x, mode: mode) }\n\
                  instr a() { outer(1, mode: high) }";
    let Err(error) = compile(source) else { panic!("it should not compile") };
    assert_eq!(error[0].message, "`mode` is one of low, mid");
    assert!(error[0].help.as_deref().is_some_and(|h| h.contains("`mode` is `high` here")), "{:?}", error[0].help);
    // the options that do exist still pass
    assert_eq!(
        last_of("fn inner(x, mode: (low | mid) = low) { if mode == low { x * 0.25 } else { x } }\nfn outer(x, mode: (low | high) = low) { inner(x, mode: mode) }\ninstr a() { outer(1, mode: low) }"),
        0.25
    );
}

/// The last sample after holding each value of the control `x` for `blocks` blocks, in turn.
fn after_holding(body: &str, values: &[f32], blocks: usize) -> Vec<f32> {
    let p = program(&format!("use std/blocks\ninstr a(x: num) {{ {body} }}"));
    let instrument = p.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    voice.start_note();
    let mut out = vec![0.0; block_size()];
    values
        .iter()
        .map(|&value| {
            voice.set_param_by_name("x", value);
            for _ in 0..blocks {
                voice.process(&mut out);
            }
            out[out.len() - 1]
        })
        .collect()
}

#[test]
fn slew_rises_and_falls_at_their_own_speeds() {
    let n = (4 * block_size()) as f32;
    // linear: a full 1.0 takes the whole time, so after n samples it has covered n / (time * rate)
    let up = after_holding("slew(x, rise: 2sec, fall: 1sec, shape: linear)", &[1.0], 4)[0];
    assert!((up - n / 96_000.0).abs() < 1e-4, "{up}");
    // smooth with no time follows at once, and with a long time barely moves
    assert_eq!(after_holding("slew(x, rise: 0sec, fall: 0sec)", &[1.0], 4)[0], 1.0);
    assert!(after_holding("slew(x, rise: 30sec)", &[1.0], 4)[0] < 0.01);
    // it falls at the fall time, not the rise time: quick up, slow down
    let both = after_holding("slew(x, rise: 1ms, fall: 5sec)", &[1.0, 0.0], 40);
    assert!(both[0] > 0.99, "{both:?}");
    assert!(both[1] > 0.9, "falls slowly: {both:?}");
    let both = after_holding("slew(x, rise: 5sec, fall: 1ms)", &[1.0, 0.0], 40);
    assert!(both[0] < 0.1 && both[1] < 0.1, "{both:?}");
    // a stereo signal keeps its channels apart
    let out = stereo_render("slew([1, 0], rise: 100ms)", 20);
    let (mut left, mut right) = (0.0f32, 0.0f32);
    for chunk in out.chunks(2 * block_size()) {
        left = left.max(peak(&chunk[..block_size()]));
        right = right.max(peak(&chunk[block_size()..]));
    }
    assert!(left > 0.05 && right == 0.0, "{left} {right}");
}

#[test]
fn morph_osc_starts_as_a_sine_and_stays_within_one() {
    let plain = settled_peak("morph_osc(220hz, shape: 0).sound - osc(sine, 220hz)");
    assert!(plain < 1e-4, "{plain}");
    for shape in [0.0, 0.2, 0.33, 0.5, 0.66, 0.8, 1.0] {
        for width in [0.05, 0.5, 0.95] {
            let p = settled_peak(&format!("morph_osc(220hz, shape: {shape}, width: {width}).sound"));
            assert!(p > 0.3 && p < 1.2, "{shape}, {width}: {p}");
        }
    }
    // the sub is an octave down: half as many zero crossings as the sound at shape 0
    let crossings = |body: &str| {
        let out = render(&format!("use std/blocks\ninstr a() {{ {body} }}"), 400);
        out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32
    };
    let ratio = crossings("morph_osc(220hz).sound") / crossings("morph_osc(220hz).sub");
    assert!((ratio - 2.0).abs() < 0.1, "{ratio}");
}

#[test]
fn a_wavefolder_leaves_a_quiet_signal_alone_and_adds_overtones_when_pushed() {
    let alone = settled_peak("wavefolder(osc(sine, 110hz) * 0.5, amount: 0) - osc(sine, 110hz) * 0.5");
    assert!(alone < 1e-4, "{alone}");
    assert!(settled_peak("wavefolder(0 * osc(sine, 110hz), amount: 1)") < 1e-4);
    // pushed hard, a sine gains many more zero crossings than it began with
    let crossings = |body: &str| {
        let out = render(&format!("use std/blocks\ninstr a() {{ {body} }}"), 400);
        out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    };
    assert!(crossings("wavefolder(osc(sine, 110hz), amount: 1)") > 2 * crossings("osc(sine, 110hz)"));
    let p = settled_peak("wavefolder(osc(saw, 110hz) * 8, amount: 1, symmetry: 1)");
    assert!(p > 0.3 && p <= 1.2, "{p}");
}

#[test]
fn a_multi_lfo_has_four_outputs_a_quarter_turn_apart() {
    let all = settled_peak("m = multi_lfo(rate: 5hz, shift: 0.25, smooth: 0)\n m.a + m.b * 0 + m.c * 0");
    assert!(all > 0.9 && all <= 1.0001, "{all}");
    // a triangle half a cycle apart is the opposite sign
    assert!(settled_peak("m = multi_lfo(rate: 5hz, shape: 0.5, smooth: 0, shift: 0.25)\n m.a + m.c") < 1e-3);
    // shift 0 gives four copies; quarter turns do not
    assert!(settled_peak("m = multi_lfo(rate: 5hz, shift: 0)\n m.a - m.d") < 1e-5);
    assert!(settled_peak("m = multi_lfo(rate: 5hz, shift: 0.25)\n m.a - m.b") > 0.1);
}

#[test]
fn a_formant_filter_passes_its_peaks_and_not_a_steady_level() {
    assert!(settled_value("f = osc(saw, 110hz) * 0 + 1\n f.formant_filter().sound").abs() < 0.05);
    // a sine at the lowest peak comes out of the low output, not the high one
    let low = settled_peak("osc(sine, 600hz).formant_filter(freq: 600hz, res: 0.8).low");
    let high = settled_peak("osc(sine, 600hz).formant_filter(freq: 600hz, res: 0.8).high");
    assert!(low > 0.3 && high < low / 3.0, "{low} {high}");
    for res in [0.0, 0.5, 1.0] {
        let p = settled_peak(&format!("osc(saw, 110hz).formant_filter(freq: 500hz, span: 0.7, res: {res}).sound"));
        assert!(p > 0.05 && p <= 2.0, "{res}: {p}");
    }
}

#[test]
fn the_oscillator_and_modulation_blocks_stay_finite_under_extreme_modulation() {
    let extremes = [-1e30, -1.0, 0.0, 0.5, 1.0, 1e30, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    for (body, limit) in [
        ("slew(osc(saw, 3hz), rise: x * 1ms, fall: y * 1ms, shape: linear)", 1.2),
        ("slew(osc(saw, 3hz) * z, rise: x * 1ms, fall: y * 1ms)", 1e6),
        ("morph_osc(x * 1hz, shape: y, width: z).sound", 1.3),
        ("morph_osc(x * 1hz, shape: y, width: z).sub", 1.05),
        ("wavefolder(osc(saw, 110hz) * x, amount: y, symmetry: z)", 1.2),
        ("multi_lfo(rate: x * 1hz, shape: y, smooth: z, shift: x).d", 1.0001),
        ("osc(saw, 110hz).formant_filter(freq: x * 1hz, span: y, res: z, tilt: x).sound", 2.0),
    ] {
        let p = program(&format!("use std/blocks\ninstr a(x: num, y: num, z: num) {{ {body} }}"));
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
                    for _ in 0..20 {
                        voice.process(&mut out);
                        assert!(
                            !voice.is_poisoned() && out.iter().all(|v| v.is_finite() && v.abs() <= limit),
                            "{body}: {x}, {y}, {z}: {}",
                            peak(&out)
                        );
                    }
                }
            }
        }
    }
}
