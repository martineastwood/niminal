//! Language features for performance: notes that know their length and place,
//! shared control values, tuning, the clock, and more.

use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(
        render_with(
            &program(src),
            &[],
            &RenderOptions {
                limiter: false,
                until: None,
                ..Default::default()
            },
        )
        .unwrap(),
    )
}

fn error(src: &str) -> String {
    compile(src)
        .err()
        .map(|e| {
            e.iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn at(t: f64) -> usize {
    (t * SR) as usize
}

#[test]
fn note_dur_is_the_scheduled_length() {
    // at 120bpm a beat is half a second
    let out = raw(
        "instr a() { tail = 10ms\n note.dur.num * 0.1 * env[1 | 10ms 1] }\na() for 1beat\nat 1beat a() for 2beats",
    );
    assert!((out[at(0.25)] - 0.05).abs() < 1e-4, "{}", out[at(0.25)]);
    assert!((out[at(0.75)] - 0.1).abs() < 1e-4, "{}", out[at(0.75)]);
}

#[test]
fn note_index_counts_the_notes_of_an_instrument() {
    let out = raw(
        "instr a() { tail = 10ms\n (note.index + 1) * 0.1 * env[1 | 10ms 1] }\ninstr b() { 0 }\na() for 1beat\nat 1beat b() for 1beat\nat 2beats a() for 1beat\nat 3beats a() for 1beat",
    );
    assert!((out[at(0.25)] - 0.1).abs() < 1e-4);
    assert!((out[at(1.25)] - 0.2).abs() < 1e-4);
    assert!((out[at(1.75)] - 0.3).abs() < 1e-4);
}

#[test]
fn note_start_is_one_sample_long() {
    let out = raw("instr a() { (note.start + 0.1) * env[1 | 10ms 1] }\na() for 1beat");
    assert!((out[0] - 1.1).abs() < 1e-6);
    assert!(out[1..at(0.4)].iter().all(|s| (*s - 0.1).abs() < 1e-6));
}

#[test]
fn a_write_inside_an_if_only_happens_when_it_is_taken() {
    // a note counts itself once, then reads the count
    let src = "buffer n = zeros(1)\ninstr a() { tail = 10ms\n if note.start { n[0] = n[0] + 1 }\n n[0] * 0.1 * env[1 | 10ms 1] }\na() for 1beat\nat 1beat a() for 1beat\nat 2beats a() for 1beat";
    let out = raw(src);
    assert!((out[at(0.25)] - 0.1).abs() < 1e-4, "{}", out[at(0.25)]);
    assert!((out[at(0.75)] - 0.2).abs() < 1e-4, "{}", out[at(0.75)]);
    assert!((out[at(1.25)] - 0.3).abs() < 1e-4, "{}", out[at(1.25)]);
}

#[test]
fn clock_tells_beat_bar_and_tempo() {
    let out = raw(
        "tempo 120bpm\ninstr a() { tail = 10ms\n clock.beat * 0.1 * env[1 | 10ms 1] }\ninstr b() { tail = 10ms\n clock.bpm * 0.001 * env[1 | 10ms 1] }\na() for 4beats\nb() for 1beat",
    );
    // 2 beats in after 1 second
    assert!((out[at(1.0)] - 0.2).abs() < 1e-3, "{}", out[at(1.0)]);
    assert!(
        (out[at(0.25)] - (0.5 * 0.1 + 0.12)).abs() < 1e-3,
        "{}",
        out[at(0.25)]
    );
    let bar = raw("instr a() { tail = 10ms\n clock.bar * 0.5 * env[1 | 10ms 1] }\na() for 4beats");
    assert!((bar[at(1.0)] - 0.25).abs() < 1e-3, "{}", bar[at(1.0)]);
    // a function, and a control scope, read the clock for themselves
    let in_fn = raw("fn count() { clock.beat }\ninstr a() { tail = 10ms\n count() * 0.1 * env[1 | 10ms 1] }\na() for 4beats");
    assert!((in_fn[at(1.0)] - 0.2).abs() < 1e-3, "{}", in_fn[at(1.0)]);
    let in_control = raw("instr a() { tail = 10ms\n control { clock.beat } * 0.1 * env[1 | 10ms 1] }\na() for 4beats");
    assert!((in_control[at(1.0)] - 0.2).abs() < 1e-2, "{}", in_control[at(1.0)]);
    assert!(error("instr a() { clock.beet }").contains("has no `beet`"));
}

/// What a note name comes out as, in hz: through an expression, and through a note's own `freq`.
fn hz_of(config: &str, note: &str) -> (f32, f32) {
    let expr = raw(&format!(
        "{config}\ninstr a() {{ tail = 10ms\n{note}.num * 0.001 * env[1 | 10ms 1] }}\na() for 1beat"
    ));
    let played = raw(&format!(
        "{config}\ninstr a(freq: hz) {{ tail = 10ms\nfreq.num * 0.001 * env[1 | 10ms 1] }}\na(freq: {note}) for 1beat"
    ));
    (expr[at(0.1)] * 1000.0, played[at(0.1)] * 1000.0)
}

#[test]
fn cents_are_hundredths_of_a_semitone() {
    let out = raw(
        "instr a(f: hz) { tail = 10ms\n (f + 100ct).num * 0.001 * env[1 | 10ms 1] }\na(f: 440hz) for 1beat",
    );
    let semitone = 440.0 * 2f32.powf(1.0 / 12.0);
    assert!((out[at(0.1)] * 1000.0 - semitone).abs() < 0.05);
    let half = raw(
        "instr a(f: hz) { tail = 10ms\n (f + 50ct).num * 0.001 * env[1 | 10ms 1] }\na(f: 440hz) for 1beat",
    );
    assert!((half[at(0.1)] * 1000.0 - 440.0 * 2f32.powf(0.5 / 12.0)).abs() < 0.05);
}

#[test]
fn a_tuning_changes_what_note_names_mean() {
    let (a, b) = hz_of("", "a4");
    assert!((a - 440.0).abs() < 0.05 && (b - 440.0).abs() < 0.05);
    let (a, b) = hz_of("config { tuning: { a4: 432hz } }", "a4");
    assert!(
        (a - 432.0).abs() < 0.05 && (b - 432.0).abs() < 0.05,
        "{a} {b}"
    );
    // a fifth above a4 in equal temperament vs the tempered 432 reference
    let (a, _) = hz_of("config { tuning: { a4: 432hz } }", "e5");
    assert!((a - 432.0 * 2f32.powf(7.0 / 12.0)).abs() < 0.05);
    // just intonation is measured from c, with a4 where it was
    let (a, b) = hz_of("config { tuning: { scale: just } }", "e4");
    assert!(
        (a - 440.0 * 3.0 / 5.0 * 5.0 / 4.0).abs() < 0.1 && (b - a).abs() < 0.01,
        "{a} {b}"
    );
    let (a, _) = hz_of("config { tuning: { scale: just } }", "c5");
    assert!((a - 880.0 * 3.0 / 5.0).abs() < 0.1, "{a}");
    let (a, _) = hz_of("config { tuning: { scale: just } }", "c3");
    assert!((a - 440.0 * 3.0 / 5.0 / 2.0).abs() < 0.1, "{a}");
}

#[test]
fn a_scale_can_have_any_number_of_degrees_and_period() {
    // a pentatonic scale of five notes per octave: c4 is the root, the sixth note number is c5
    let cfg = "config { tuning: { scale: ratios(1, 9/8, 5/4, 3/2, 5/3) } }";
    let (c5, _) = hz_of(cfg, "f4");
    let (c4, _) = hz_of(cfg, "c4");
    assert!((c5 / c4 - 2.0).abs() < 1e-3, "{c4} {c5}");
    // Bohlen-Pierce: thirteen steps to a 3/1
    let cfg = "config { tuning: { scale: cents(0, 146.3, 292.6, 438.9, 585.2, 731.5, 877.8, 1024.1, 1170.4, 1316.7, 1463.0, 1609.3, 1755.6), period: 1902ct } }";
    let (a, _) = hz_of(cfg, "c4");
    let (b, _) = hz_of(cfg, "c#5");
    assert!(a > 0.0 && (b / a - 3.0).abs() < 0.01, "{a} {b}");
}

#[test]
fn tuning_mistakes_are_explained() {
    assert!(error("config { tuning: { a4: 432 } }").contains("`a4` is a frequency"));
    assert!(error("config { tuning: { scale: pure } }").contains("unknown scale `pure`"));
    assert!(error("config { tuning: { scale: cents(10, 20) } }").contains("starts at the root"));
    assert!(error("config { tuning: { pitch: 3 } }").contains("unknown tuning setting"));
}

fn level(src: &str, t: f64) -> f32 {
    raw(src).get(at(t)).copied().unwrap_or(0.0)
}

#[test]
fn a_range_steps_through_values_that_are_not_whole() {
    let sum = |expr: &str| {
        level(
            &format!(
                "instr a() {{ tail = 10ms\n({expr}).sum * 0.01 * env[1 | 10ms 1] }}\na() for 1beat"
            ),
            0.1,
        ) * 100.0
    };
    assert!(
        (sum("(0..1).step(0.25)") - 2.5).abs() < 1e-4,
        "inclusive: 0 + 0.25 + 0.5 + 0.75 + 1"
    );
    assert!(
        (sum("(0..<1).step(0.25)") - 1.5).abs() < 1e-4,
        "the end left out"
    );
    assert!((sum("(0..1).step(0.25).len") - 5.0).abs() < 1e-4);
    // a range with units steps in those units
    let hz = level(
        "instr a() { tail = 10ms\n (200hz..400hz).step(100hz).sum.num * 0.001 * env[1 | 10ms 1] }\na() for 1beat",
        0.1,
    ) * 1000.0;
    assert!((hz - 900.0).abs() < 0.1, "{hz}");
    // also in a function with state, and in a table
    let kernel = level(
        "fn f(x) { state s = 0\ns = s + x\n(0..1).step(0.5).sum + s * 0 }\ninstr a() { tail = 10ms\n f(1) * 0.01 * env[1 | 10ms 1] }\na() for 1beat",
        0.1,
    ) * 100.0;
    assert!((kernel - 1.5).abs() < 1e-4, "{kernel}");
    let table = level(
        "const t = (0..1).step(0.5)\ninstr a() { tail = 10ms\n t.sum * 0.01 * env[1 | 10ms 1] }\na() for 1beat",
        0.1,
    ) * 100.0;
    assert!((table - 1.5).abs() < 1e-4);
    assert!(error("instr a() { (0..1).step(0) }").contains("above 0"));
    assert!(error("instr a() { (0..1).step(1hz) }").contains("same unit"));
}

#[test]
fn active_switches_off_the_entries_beyond_a_count_that_can_change() {
    // sixteen partials, of which `count` sound: each one is worth 0.01
    let sum = |count: &str| {
        level(
            &format!(
                "instr a(count: 0..16 = 4) {{ tail = 10ms\n(for n in 1..16 {{ 0.01 }}).active(count).sum * env[1 | 10ms 1] }}\na(count: {count}) for 1beat"
            ),
            0.1,
        )
    };
    assert!((sum("4") - 0.04).abs() < 1e-5);
    assert!((sum("10") - 0.10).abs() < 1e-5);
    assert!((sum("0")).abs() < 1e-6);
    assert!((sum("16") - 0.16).abs() < 1e-5);
}

#[test]
fn late_is_the_block_late_send_and_prev_is_always_one_sample() {
    assert!(error("bus fb\nfx t { out = fb\n fb += prev(out) }").contains("a bus runs in blocks"));
    assert!(error("instr a() { late(osc(sine, 1hz)) }").contains("only for a send to a bus"));
}

#[test]
fn match_picks_a_choice_like_an_if_chain() {
    let src = |mode: &str| {
        format!(
            "fn level(mode: (low | mid | high | other) = low) {{\n  match mode {{\n    low: 0.1\n    mid | high: 0.2\n    else: 0.3\n  }}\n}}\ninstr a() {{ tail = 10ms\nlevel(mode: {mode}) * env[1 | 10ms 1] }}\na() for 1beat"
        )
    };
    assert!((raw(&src("low"))[at(0.1)] - 0.1).abs() < 1e-4);
    assert!((raw(&src("high"))[at(0.1)] - 0.2).abs() < 1e-4);
    assert!((raw(&src("other"))[at(0.1)] - 0.3).abs() < 1e-4);
    // as a value it needs an else, like `if`
    assert!(
        !error("fn g(m: (x | y) = x) { match m { x: 1 } }\ninstr a() { g() }\na() for 1beat")
            .is_empty()
    );
}

#[test]
fn pitch_option_names_the_parameter_notes_set() {
    let out = raw(
        "instr a(tone: hz = 220hz) { pitch = tone\ntail = 10ms\n osc(sine, tone) * env[1 | 10ms 1] * 0.5 }\na() for 1beat\nat 1beat a(tone: a5) for 1beat",
    );
    let zero_crossings = |from: usize, to: usize| {
        out[from..to]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count()
    };
    // 220hz for a half second is 110 cycles; a5 (880hz) for the next half second is 440
    assert!((zero_crossings(at(0.05), at(0.45)) as f64 - 88.0).abs() < 3.0);
    assert!((zero_crossings(at(0.55), at(0.95)) as f64 - 352.0).abs() < 3.0);
    assert!(
        error("instr a(tone: hz = 220hz) { pitch = nope\n 0 }").contains("no parameter `nope`")
    );
    assert!(error("instr a(g: num = 1) { pitch = g\n 0 }").contains("in `hz`"));
}

#[test]
fn clips_play_through_the_renamed_pitch() {
    let play = |def: &str| {
        let p = program(&format!(
            "{def}\nc = [c4 e4]\nclip t {{ instrument: a, notes: c }}\nt.start"
        ));
        mono(
            render_with(
                &p,
                &[],
                &RenderOptions {
                    limiter: false,
                    until: Some(2.0),
                    ..Default::default()
                },
            )
            .unwrap(),
        )
    };
    let renamed = play(
        "instr a(tone: hz = 220hz) { pitch = tone\ntail = 10ms\n osc(sine, tone) * env[1 | 10ms 1] * 0.5 }",
    );
    let plain =
        play("instr a(freq: hz = 220hz) { tail = 10ms\n osc(sine, freq) * env[1 | 10ms 1] * 0.5 }");
    assert!(plain.iter().any(|s| s.abs() > 0.1));
    assert_eq!(renamed, plain);
}

#[test]
fn after_starts_a_layer_late_with_fresh_state() {
    // the second layer is silent for 250ms, then an envelope that starts then
    let out = raw(
        "instr a() { tail = 10ms\n 0.1 * env[1 | 10ms 1] + after(250ms) { 0.3 * env[0 100ms 1 | 10ms 1] } }\na() for 1beat",
    );
    assert!((out[at(0.2)] - 0.1).abs() < 1e-4);
    // 50ms into the late envelope: half way up
    assert!(
        (out[at(0.3)] - (0.1 + 0.15)).abs() < 5e-3,
        "{}",
        out[at(0.3)]
    );
    assert!((out[at(0.4)] - 0.4).abs() < 5e-3, "{}", out[at(0.4)]);
}

#[test]
fn finish_when_ends_a_note_early() {
    let out = raw("instr a() { tail = 10ms\n 0.2.finish_when(note.time > 100ms) }\na() for 4beats");
    assert!(
        out.len() < at(0.3),
        "the note ended itself after 100ms, not the written 2s: {}",
        out.len()
    );
}

#[test]
fn noise_is_the_same_on_every_run_and_differs_by_source_and_note() {
    let src = "instr a() { tail = 10ms\n noise() * env[1 | 10ms 1] }\ninstr b() { tail = 10ms\n (noise() - noise()) * env[1 | 10ms 1] }\ninstr c() { tail = 10ms\n noise(seed: 7) * env[1 | 10ms 1] }\n";
    let note = |body: &str| raw(&format!("{src}{body}"));
    // the same program gives the same samples
    assert_eq!(note("a() for 1beat"), note("a() for 1beat"));
    // two sources in one instrument are not copies of each other
    assert!(note("b() for 1beat")[..1000].iter().any(|s| s.abs() > 0.1));
    // each note of an instrument has its own noise...
    let two = note("a() for 1beat\nat 2beats a() for 1beat");
    assert_ne!(two[..1000], two[at(1.0)..at(1.0) + 1000]);
    // ...unless it is given a seed
    let seeded = note("c() for 1beat\nat 2beats c() for 1beat");
    assert_eq!(seeded[..1000], seeded[at(1.0)..at(1.0) + 1000]);
}
