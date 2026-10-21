use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, ..Default::default() }).unwrap())
}

/// Frequency in hz of the stretch from `from` to `to` seconds, by counting rising zero crossings.
fn hz(out: &[f32], from: f64, to: f64) -> f64 {
    let window = &out[(from * SR) as usize..(to * SR) as usize];
    let rises = window.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    rises as f64 / (to - from)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |p, s| p.max(s.abs()))
}

const SLOW_ATTACK: &str = "env[0 100ms 1 | 10ms 0]";

fn song(options: &str, notes: &str) -> Vec<f32> {
    let settings = options.replace(", ", "\n").replace(": ", " = ");
    raw(&format!("tempo 120bpm\nmono x(freq: hz) {{\n{settings}\nosc(sine, freq) * {SLOW_ATTACK}\n}}\n{notes}"))
}

#[test]
fn legato_keeps_the_envelope_going_where_a_plain_mono_note_starts_it_again() {
    let notes = "x(freq: 220hz) for 4beats\nat 1beat x(freq: 330hz) for 1beat";
    let legato = song("legato: true", notes);
    let plain = song("glide: 0ms", notes);
    // 20ms into the second note (at 0.5s)
    assert!(peak(&legato[(0.52 * SR) as usize..(0.53 * SR) as usize]) > 0.9, "the held note carries on");
    assert!(peak(&plain[(0.52 * SR) as usize..(0.53 * SR) as usize]) < 0.3, "a new note attacks from silence");
    assert!((hz(&legato, 0.6, 0.9) - 330.0).abs() < 10.0);
}

#[test]
fn letting_go_of_the_newest_note_goes_back_to_the_one_still_held() {
    let out = song("legato: true", "x(freq: 220hz) for 4beats\nat 1beat x(freq: 440hz) for 1beat");
    assert!((hz(&out, 0.6, 0.9) - 440.0).abs() < 10.0, "while both are held, the last one sounds");
    assert!((hz(&out, 1.1, 1.4) - 220.0).abs() < 10.0, "after it ends, the first is back");
}

#[test]
fn priority_picks_which_held_note_sounds() {
    let notes = "x(freq: 440hz) for 4beats\nat 1beat x(freq: 220hz) for 1beat";
    assert!((hz(&song("legato: true, priority: lowest", notes), 0.6, 0.9) - 220.0).abs() < 10.0);
    assert!((hz(&song("legato: true, priority: highest", notes), 0.6, 0.9) - 440.0).abs() < 10.0, "the low note joins but is not heard");
}

#[test]
fn the_last_held_note_letting_go_releases_the_voice() {
    let out = song("legato: true", "x(freq: 220hz) for 1beat\nat 0.5beat x(freq: 330hz) for 1beat");
    // the second note ends at 0.75s, and the first (which ended at 0.5s) didn't release it
    assert!(out.len() > (0.7 * SR) as usize && out.len() < (0.85 * SR) as usize, "the voice ended with the last note: {}", out.len());
}

#[test]
fn glide_slides_to_the_new_pitch_over_the_time_given() {
    let notes = "x(freq: 220hz) for 4beats\nat 1beat x(freq: 440hz) for 2beats";
    for options in ["glide: 400ms", "glide: 400ms, legato: true"] {
        let out = song(options, notes);
        let start = hz(&out, 0.5, 0.6);
        let end = hz(&out, 1.2, 1.5);
        assert!(start > 215.0 && start < 380.0, "{options}: starts near the old pitch: {start}");
        assert!((end - 440.0).abs() < 12.0, "{options}: arrives: {end}");
    }
    let jump = song("glide: 0ms", "x(freq: 220hz) for 4beats\nat 1beat x(freq: 440hz) for 2beats");
    assert!((hz(&jump, 0.6, 0.7) - 440.0).abs() < 20.0, "no glide, no slide");
}

#[test]
fn steal_quietest_ends_the_softest_note_not_the_oldest() {
    let notes = "x(freq: 200hz, amp: 0db) for 8beats\nx(freq: 300hz, amp: -40db) for 8beats\nat 0.5beat x(freq: 500hz, amp: -20db) for 8beats";
    let src = |steal: &str| raw(&format!("tempo 120bpm\ninstr x(freq: hz, amp: db) {{ max = 2{steal}\nosc(sine, freq).gain(amp) }}\n{notes}"));
    let late = |out: &[f32]| peak(&out[(1.0 * SR) as usize..(1.5 * SR) as usize]);
    assert!(late(&src("\nsteal = quietest")) > 0.8, "the loud note survives");
    assert!(late(&src("\nsteal = oldest")) < 0.2, "the oldest, loudest note was ended");
}

#[test]
fn mono_only_options_are_checked() {
    let msg = |src: &str| compile(src).err().unwrap()[0].message.clone();
    assert!(msg("instr x(freq: hz) { legato = true\n osc(sine, freq) }").contains("`legato` is for `mono`"));
    assert!(msg("mono x(freq: hz) { priority = lowest\n osc(sine, freq) }").contains("legato voice"));
    assert!(msg("mono x(freq: hz) { priority = middle\nlegato = true\n osc(sine, freq) }").contains("`last`, `lowest` or `highest`"));
    assert!(msg("mono x(f: hz) { glide = 1beat\nlegato = true\n osc(sine, f) }").contains("no `freq` parameter"));
    assert!(compile("mono x(freq: hz) { glide = 1beat\nlegato = true\npriority = highest\n osc(sine, freq) }").is_ok());
}

#[test]
fn two_clips_with_one_instrument_keep_separate_voice_limits() {
    for definition in ["mono tone(freq: hz) { 0.2 * env[1 | 1ms 0] }", "instr tone(freq: hz) { max = 1\nsteal = oldest\n0.2 * env[1 | 1ms 0] }"] {
        let out = raw(&format!("tempo 120bpm\n{definition}\nclip bass {{ instrument: tone\nnotes: [c3,e3] }}\nclip bass2 {{ instrument: tone\nnotes: [g3,b3] }}\nscene both {{ bass, bass2 }}\nboth.start\nat 1bar hush"));
        assert!((out[2400] - 0.4).abs() < 1e-5, "each clip keeps one voice: {}", out[2400]);
    }
}
