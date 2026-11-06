//! The blocks of `std/blocks` that follow the transport, at the sample level (120 bpm: a beat is 500ms).
use super::rhythm_gate::{audio_body, level};

fn signal(expression: &str) -> Vec<f32> {
    audio_body(&format!("out = source * ({expression})"))
}

#[test]
fn a_function_generator_rises_and_falls_against_the_beat() {
    // A cycle of 4 beats (2s), rising for half of it: 0 at the start, 1 at one second.
    let out = signal("function_gen(cycle: 4, rise: 0.5)");
    for (time, expected) in [(0.0, 0.0), (0.25, 0.25), (0.5, 0.5), (1.0, 1.0)] {
        level(&out, time, expected);
    }
    // A bend holds the bottom longer, and a short rise is steep.
    level(&signal("function_gen(cycle: 4, rise: 0.5, curve: 2)"), 0.5, 0.25);
    level(&signal("function_gen(cycle: 4, rise: 0.25)"), 0.25, 0.5);
    // One that has fallen starts over: cycle of 1 beat is back at the start after 500ms.
    let fast = signal("function_gen(cycle: 1, rise: 0.5)");
    level(&fast, 0.25, 1.0);
    level(&fast, 0.5, 0.0);
    level(&fast, 0.75, 1.0);
}

#[test]
fn a_clock_division_is_a_gate_that_follows_the_beat() {
    let eighths = signal("clock_div(1)"); // high for the first half of every beat
    for (time, expected) in [(0.1, 1.0), (0.3, 0.0), (0.6, 1.0), (0.8, 0.0)] {
        level(&eighths, time, expected);
    }
    // Three halves of a beat is a period of 750ms, so it drifts against the beat.
    let dotted = signal("clock_div(3/2, width: 0.5)");
    for (time, expected) in [(0.1, 1.0), (0.5, 0.0), (0.8, 1.0), (1.05, 1.0)] {
        level(&dotted, time, expected);
    }
}

#[test]
fn stepped_wobble_holds_each_value_for_its_beats_and_the_seed_is_a_run() {
    let a = signal("wobble(beats: 1, seed: 1)");
    // a value lasts a whole beat: the same near the start and the end of the first beat
    assert!((a[2400] - a[22_000]).abs() < 1e-6, "{} {}", a[2400], a[22_000]);
    // and the next beat has another
    assert!((a[2400] - a[26_000]).abs() > 1e-4, "{} {}", a[2400], a[26_000]);
    assert!(a.iter().all(|v| v.is_finite() && v.abs() <= 1.001));
    // the same seed gives the same run, another seed another
    assert_eq!(a, signal("wobble(beats: 1, seed: 1)"));
    assert_ne!(a, signal("wobble(beats: 1, seed: 2)"));
}

#[test]
fn smooth_wobble_slides_between_its_values() {
    let a = signal("wobble(beats: 1, shape: smooth, seed: 3)");
    assert!(a.iter().all(|v| v.is_finite() && v.abs() <= 1.001));
    // no jumps: the biggest step between samples is tiny next to the range it travels
    let step = a.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(step < 0.01, "{step}");
    let (lo, hi) = a.iter().fold((1.0f32, -1.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(hi - lo > 0.1, "it moves: {lo}..{hi}");
}
