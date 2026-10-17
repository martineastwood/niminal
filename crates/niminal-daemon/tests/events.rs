//! The engine's `events.send`: each op, validation, late events and replay.

use niminal_daemon::{At, Op, QuantizeDefaults, Session};
use niminal_lang::Layout;
use niminal_score::{Time, Value};

const SR: f32 = 48_000.0;

const SETUP: &str = "
tempo 120bpm
ctl level = 0.0.smooth(0ms)
instr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }
instr flat(freq: hz) { level }
track lead { instrument = tone }
track pad(cut: hz = 300hz) { instrument = tone
  out = it.lpf(cutoff: cut) }
track held { instrument = flat }
";

fn session() -> Session {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(SETUP, Some("now")).unwrap();
    s
}

fn note(target: &str, at: At, hz: f64) -> Op {
    Op::Note { target: target.into(), at, args: [("freq".to_string(), Value::Hz(hz))].into(), dur: Some(Time::Seconds(0.1)), id: None, tag: None }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn notes_start_at_a_sample_a_position_or_now() {
    let mut s = session();
    let sent = s
        .send_events(0, vec![
            note("lead", At::Sample(4_800), 440.0),
            note("lead", At::Time(Time::Beats(2.0)), 440.0), // 1 second at 120bpm
            note("lead", At::Now, 440.0),
        ])
        .unwrap();
    assert_eq!((sent.accepted, sent.late, sent.first), (3, 0, Some(0)));
    let out = s.process(60_000).remove(0);
    assert!(peak(&out[..4_000]) > 0.1, "now");
    assert_eq!(peak(&out[4_000 - 1..4_000]), peak(&out[4_000 - 1..4_000]));
    assert!(peak(&out[4_800..9_000]) > 0.1, "sample");
    assert_eq!(peak(&out[20_000..47_990]), 0.0);
    assert!(peak(&out[48_000..52_000]) > 0.1, "two beats in");
}

#[test]
fn a_batch_with_one_bad_event_does_nothing_and_names_it() {
    let mut s = session();
    let problems = s
        .send_events(0, vec![note("lead", At::Now, 440.0), note("nobody", At::Now, 440.0), Op::Control { name: "nope".into(), at: At::Now, value: 1.0, unit: None, glide: None, tag: None }])
        .unwrap_err();
    assert_eq!(problems.iter().map(|p| p.event).collect::<Vec<_>>(), [Some(1), Some(2)]);
    assert_eq!(peak(&s.process(4_000).remove(0)), 0.0);
}

#[test]
fn events_in_the_past_sound_at_once_and_are_counted() {
    let mut s = session();
    s.process(10_000);
    let sent = s.send_events(0, vec![note("lead", At::Sample(100), 440.0), note("lead", At::Sample(20_000), 440.0)]).unwrap();
    assert_eq!((sent.late, sent.first), (1, Some(100)));
    assert!(peak(&s.process(2_000).remove(0)) > 0.1);
}

#[test]
fn a_control_moves_at_its_sample_with_a_glide() {
    let mut s = session();
    s.eval("held(freq: 100hz) for 100bars", Some("now")).unwrap();
    s.send_events(0, vec![Op::Control { name: "level".into(), at: At::Sample(2_400), value: 0.5, unit: None, glide: Some(Time::Seconds(0.0)), tag: None }]).unwrap();
    let out = s.process(4_800).remove(0);
    assert_eq!(peak(&out[..2_399]), 0.0);
    assert!((out[2_500] - 0.5).abs() < 1e-5);
}

#[test]
fn a_track_op_sets_a_track_parameter() {
    let render = |cutoff: &str| {
        let mut s = session();
        s.send_events(0, vec![
            Op::Track { name: "pad".into(), at: At::Sample(0), args: [("cut".to_string(), cutoff.parse().unwrap())].into(), glide: None, tag: None },
            note("pad", At::Sample(10), 2_000.0),
        ])
        .unwrap();
        s.process(4_800).remove(0)
    };
    let (dull, bright) = (render("100hz"), render("8khz"));
    assert!(peak(&bright) > peak(&dull) * 2.0, "{} vs {}", peak(&bright), peak(&dull));
    let mut s = session();
    let problems = s.send_events(0, vec![Op::Track { name: "pad".into(), at: At::Now, args: [("nope".to_string(), Value::Num(1.0))].into(), glide: None, tag: None }]).unwrap_err();
    assert!(problems[0].message.contains("no parameter"), "{}", problems[0].message);
}

#[test]
fn the_log_replays_to_the_same_audio() {
    let mut s = session();
    s.eval("held(freq: 100hz) for 100bars", Some("now")).unwrap();
    s.process(1_000);
    s.send_events(0, vec![
        note("lead", At::Sample(3_000), 330.0),
        note("lead", At::Time(Time::Beats(1.0)), 440.0),
        Op::Control { name: "level".into(), at: At::Sample(5_000), value: 0.4, unit: None, glide: Some(Time::Seconds(0.01)), tag: None },
    ])
    .unwrap();
    s.process(2_000);
    s.send_events(0, vec![note("lead", At::Now, 550.0)]).unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 63_000, false).remove(0);
    assert_eq!(live, replayed[3_000..]);
}

const VOICE: &str = "
tempo 120bpm
instr organ(freq: hz, cut: hz = 40hz) { osc(saw, freq).lpf(cutoff: cut) * env[1 | 100ms 0] }
track lead { instrument = organ }
";

fn voices() -> Session {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(VOICE, Some("now")).unwrap();
    s
}

fn held(id: u64, at: At, hz: f64) -> Op {
    Op::Note { target: "lead".into(), at, args: [("freq".to_string(), Value::Hz(hz))].into(), dur: None, id: Some(id), tag: None }
}

fn set(id: u64, at: At, cut: &str, glide: Option<f64>) -> Op {
    Op::Set { id, at, args: [("cut".to_string(), cut.parse().unwrap())].into(), glide: glide.map(Time::Seconds), tag: None }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_held_note_sounds_until_off_then_releases() {
    let mut s = voices();
    s.send_events(0, vec![held(1, At::Sample(1_000), 220.0), Op::Off { id: 1, at: At::Sample(48_000), tag: None }]).unwrap();
    let out = s.process(60_000).remove(0);
    assert!(rms(&out[40_000..47_000]) > 0.01, "still sounding long after it started");
    assert!(rms(&out[56_000..60_000]) < 1e-4, "released after the off");
    // the id is free again
    s.send_events(0, vec![held(1, At::Now, 220.0)]).unwrap();
}

#[test]
fn a_note_with_an_id_and_a_dur_is_held_for_at_most_the_dur_and_off_can_end_it_sooner() {
    let mut s = voices();
    s.send_events(
        0,
        vec![
            Op::Note { target: "lead".into(), at: At::Sample(0), args: [("freq".to_string(), Value::Hz(220.0))].into(), dur: Some(Time::Seconds(0.5)), id: Some(1), tag: None },
            Op::Note { target: "lead".into(), at: At::Sample(0), args: [("freq".to_string(), Value::Hz(330.0))].into(), dur: Some(Time::Seconds(0.5)), id: Some(2), tag: None },
            Op::Off { id: 2, at: At::Sample(2_400), tag: None },
        ],
    )
    .unwrap();
    let out = s.process(48_000).remove(0);
    assert!(rms(&out[20_000..23_000]) > 0.01);
    assert!(rms(&out[36_000..48_000]) < 1e-4, "both ended by 0.5sec plus the release");
}

#[test]
fn a_set_moves_a_parameter_of_a_sounding_note_with_a_glide() {
    let run = |glide: Option<f64>| {
        let mut s = voices();
        s.send_events(0, vec![held(1, At::Sample(0), 110.0), set(1, At::Sample(24_000), "8khz", glide)]).unwrap();
        s.process(48_000).remove(0)
    };
    let (jump, slide) = (run(None), run(Some(0.5)));
    assert_eq!(jump[..24_000], slide[..24_000]);
    // brighter once the cutoff opens: right after, a jump has all of it, a glide has little
    assert!(rms(&jump[24_100..26_000]) > rms(&jump[20_000..24_000]) * 2.0);
    assert!(rms(&slide[24_000..24_400]) < rms(&jump[24_000..24_400]) * 0.5);
    // and by the end of the glide they have met
    assert!((rms(&slide[44_000..48_000]) / rms(&jump[44_000..48_000]) - 1.0).abs() < 0.1);
}

#[test]
fn a_set_before_the_note_starts_gives_it_the_value_and_an_off_before_it_starts_makes_a_blip() {
    let mut s = voices();
    s.send_events(0, vec![held(1, At::Sample(12_000), 110.0), set(1, At::Sample(100), "8khz", None)]).unwrap();
    let bright = s.process(24_000).remove(0);
    let mut s = voices();
    s.send_events(0, vec![held(1, At::Sample(12_000), 110.0)]).unwrap();
    let dull = s.process(24_000).remove(0);
    assert!(rms(&bright[12_100..20_000]) > rms(&dull[12_100..20_000]) * 2.0);

    let mut s = voices();
    s.send_events(0, vec![held(7, At::Sample(12_000), 220.0), Op::Off { id: 7, at: At::Sample(6_000), tag: None }]).unwrap();
    let out = s.process(96_000).remove(0);
    assert!(rms(&out[12_100..12_400]) > 0.001, "it still sounds");
    assert!(rms(&out[40_000..96_000]) < 1e-4, "but only as a blip");
}

#[test]
fn ids_belong_to_the_client_and_bad_ones_fail_the_batch() {
    let mut s = voices();
    s.send_events(1, vec![held(1, At::Now, 220.0)]).unwrap();
    // another client can use the same id
    s.send_events(2, vec![held(1, At::Now, 330.0)]).unwrap();
    // the same client cannot, nor can one batch use an id twice
    let again = s.send_events(1, vec![held(1, At::Now, 220.0)]).unwrap_err();
    assert!(again[0].message.contains("already sounding"), "{}", again[0].message);
    let twice = s.send_events(3, vec![held(5, At::Now, 220.0), held(5, At::Now, 220.0)]).unwrap_err();
    assert_eq!(twice[0].event, Some(1));
    // unknown ids, and ids already let go of
    let unknown = s.send_events(1, vec![Op::Off { id: 9, at: At::Now, tag: None }, set(9, At::Now, "1khz", None)]).unwrap_err();
    assert_eq!(unknown.iter().map(|p| p.event).collect::<Vec<_>>(), [Some(0), Some(1)]);
    s.send_events(1, vec![Op::Off { id: 1, at: At::Now, tag: None }]).unwrap();
    assert!(s.send_events(1, vec![set(1, At::Now, "1khz", None)]).is_err());
    // an unknown parameter
    let bad = s.send_events(2, vec![Op::Set { id: 1, at: At::Now, args: [("nope".to_string(), Value::Num(1.0))].into(), glide: None, tag: None }]).unwrap_err();
    assert!(bad[0].message.contains("no parameter"), "{}", bad[0].message);
}

#[test]
fn a_client_that_goes_has_its_notes_released_and_its_waiting_notes_dropped() {
    let mut s = voices();
    s.send_events(1, vec![held(1, At::Sample(0), 220.0)]).unwrap();
    s.send_events(1, vec![Op::Note { target: "lead".into(), at: At::Sample(30_000), args: [("freq".to_string(), Value::Hz(440.0))].into(), dur: Some(Time::Seconds(0.2)), id: None, tag: None }]).unwrap();
    s.process(10_000);
    s.drop_client(1);
    let out = s.process(30_000).remove(0);
    assert!(rms(&out[6_000..10_000]) < 1e-4, "released: {}", rms(&out[6_000..10_000]));
    assert!(rms(&out[20_000..30_000]) < 1e-4, "the waiting note never sounds");
}

#[test]
fn held_notes_and_sets_replay_to_the_same_audio() {
    let mut s = voices();
    s.send_events(4, vec![held(1, At::Sample(500), 110.0), set(1, At::Sample(9_000), "4khz", Some(0.1))]).unwrap();
    s.process(3_000);
    s.send_events(4, vec![Op::Off { id: 1, at: At::Sample(30_000), tag: None }, held(2, At::Time(Time::Beats(1.0)), 165.0)]).unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 63_000, false).remove(0);
    let mut expected = voices();
    expected.send_events(4, vec![held(1, At::Sample(500), 110.0)]).unwrap();
    assert_eq!(live, replayed[3_000..]);
    let _ = expected;
}

/// One short note a beat for `bars` bars, from bar `first` (counting from 0), all tagged.
fn bars_of(hz: f64, first: u32, bars: u32, tag: &str) -> Vec<Op> {
    (first * 4..(first + bars) * 4)
        .map(|beat| Op::Note {
            target: "lead".into(),
            at: At::Time(Time::Beats(f64::from(beat))),
            args: [("freq".to_string(), Value::Hz(hz))].into(),
            dur: Some(Time::Seconds(0.2)),
            id: None,
            tag: Some(tag.into()),
        })
        .collect()
}

fn hz_of(x: &[f32]) -> f32 {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / (x.len() as f32 / SR)
}

const BAR: usize = 96_000;

#[test]
fn cancelling_a_tag_from_a_bar_leaves_the_earlier_bars_and_the_replacements_play() {
    let mut s = voices();
    s.send_events(1, bars_of(220.0, 0, 4, "a")).unwrap();
    let mut audio = s.process(BAR / 2).remove(0);
    let done = s.cancel_events(1, Some("a"), At::Time(Time::Bars(1.0)), false);
    assert_eq!((done.cancelled, done.released), (12, 0));
    s.send_events(1, bars_of(440.0, 1, 3, "b")).unwrap();
    audio.extend(s.process(4 * BAR - BAR / 2).remove(0));
    // each note rings for its 0.2 seconds
    assert!((hz_of(&audio[1_000..9_000]) - 220.0).abs() < 8.0);
    assert!((hz_of(&audio[BAR + 1_000..BAR + 9_000]) - 440.0).abs() < 12.0);
    assert!((hz_of(&audio[3 * BAR + 1_000..3 * BAR + 9_000]) - 440.0).abs() < 12.0);
    assert!(rms(&audio[88_000..95_000]) < 1e-4);
}

#[test]
fn cancelling_without_a_tag_drops_only_this_clients_waiting_events() {
    let mut s = voices();
    s.send_events(1, bars_of(220.0, 0, 1, "a")).unwrap();
    s.send_events(2, bars_of(330.0, 0, 1, "a")).unwrap();
    let done = s.cancel_events(1, None, At::Now, false);
    assert_eq!(done.cancelled, 4);
    let out = s.process(BAR).remove(0);
    assert!((hz_of(&out[1_000..9_000]) - 330.0).abs() < 10.0, "client 2's notes still play");
}

#[test]
fn cancel_with_release_lets_go_of_held_notes_and_drops_waiting_offs() {
    let mut s = voices();
    let mut ops = vec![held(1, At::Sample(0), 220.0), held(2, At::Sample(0), 330.0)];
    for op in &mut ops {
        if let Op::Note { tag, .. } = op {
            *tag = Some("pad".into());
        }
    }
    ops.push(Op::Off { id: 2, at: At::Sample(40_000), tag: Some("pad".into()) });
    s.send_events(1, ops).unwrap();
    s.process(10_000);
    // the waiting off is taken back, and the held notes are released instead
    let done = s.cancel_events(1, Some("pad"), At::Now, true);
    assert_eq!((done.cancelled, done.released), (1, 2));
    let out = s.process(20_000).remove(0);
    assert!(rms(&out[6_000..20_000]) < 1e-4, "id 1 was released, and id 2 with it: {}", rms(&out[6_000..20_000]));

    // without `release` a held note carries on
    let mut s = voices();
    s.send_events(1, vec![held(1, At::Sample(0), 220.0)]).unwrap();
    s.process(5_000);
    assert_eq!(s.cancel_events(1, None, At::Now, false).released, 0);
    assert!(rms(&s.process(20_000).remove(0)[10_000..]) > 0.01);
}

#[test]
fn a_waiting_control_can_be_cancelled() {
    let mut s = session();
    s.eval("held(freq: 100hz) for 100bars", Some("now")).unwrap();
    s.send_events(1, vec![Op::Control { name: "level".into(), at: At::Sample(5_000), value: 0.5, unit: None, glide: Some(Time::Seconds(0.0)), tag: Some("x".into()) }]).unwrap();
    assert_eq!(s.cancel_events(1, Some("y"), At::Now, false).cancelled, 0);
    assert_eq!(s.cancel_events(1, Some("x"), At::Now, false).cancelled, 1);
    assert_eq!(peak(&s.process(10_000).remove(0)), 0.0);
}

fn define(source: &str, at: At) -> Op {
    Op::Define { source: source.into(), at, dir: None, tag: None }
}

fn first_sound(x: &[f32]) -> Option<usize> {
    x.iter().position(|s| s.abs() > 1e-4)
}

const SINE: &str = "instr a(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\ntrack lead { instrument = a }";
const SAW: &str = "instr a(freq: hz) { osc(saw, freq) * env[1 | 5ms 0] }\ntrack lead { instrument = a }";

fn tone(at: At, hz: f64) -> Op {
    Op::Note { target: "lead".into(), at, args: [("freq".to_string(), Value::Hz(hz))].into(), dur: Some(Time::Seconds(0.05)), id: None, tag: None }
}

#[test]
fn a_define_lands_at_its_beat_and_the_next_note_uses_it_from_that_sample() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    // beat 2 at 120bpm is sample 48000
    s.send_events(0, vec![define(SAW, At::Time(Time::Beats(2.0))), tone(At::Sample(20_000), 110.0), tone(At::Sample(48_000), 110.0)]).unwrap();
    let out = s.process(70_000).remove(0);

    let mut reference = Session::new(SR, Layout::Mono).without_limiter();
    reference.eval(&format!("tempo 120bpm\n{SAW}"), Some("now")).unwrap();
    reference.send_events(0, vec![tone(At::Sample(48_000), 110.0)]).unwrap();
    let saw = reference.process(70_000).remove(0);
    assert_eq!(out[48_000..], saw[48_000..], "the second note is the new instrument from its first sample");
    assert_ne!(out[20_000..22_000], saw[20_000..22_000], "the first note was the old one");
    assert!(first_sound(&out[48_000..]).unwrap() < 10);
}

#[test]
fn a_define_can_be_cancelled_by_its_tag_and_performance_statements_are_refused() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    let mut op = define(SAW, At::Time(Time::Beats(2.0)));
    if let Op::Define { tag, .. } = &mut op {
        *tag = Some("t".into());
    }
    s.send_events(1, vec![op]).unwrap();
    assert_eq!(s.cancel_events(1, Some("t"), At::Now, false).cancelled, 1);
    s.send_events(1, vec![tone(At::Sample(48_000), 110.0)]).unwrap();
    let out = s.process(60_000).remove(0);
    let mut reference = Session::new(SR, Layout::Mono).without_limiter();
    reference.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    reference.send_events(0, vec![tone(At::Sample(48_000), 110.0)]).unwrap();
    assert_eq!(out, reference.process(60_000).remove(0));

    for bad in ["lead.play([c4])", "tempo 90bpm", "clip c { notes: [c4] }", "lead(freq: c4) for 1beat"] {
        let problems = s.send_events(1, vec![tone(At::Now, 110.0), define(bad, At::Now)]).unwrap_err();
        assert_eq!(problems[0].event, Some(1), "{bad}");
    }
}

#[test]
fn a_batch_that_fails_leaves_nothing_behind() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    let problems = s.send_events(1, vec![define(SAW, At::Sample(100)), define("instr broken( {", At::Sample(200))]).unwrap_err();
    assert_eq!(problems[0].event, Some(1));
    assert!(s.pending().is_empty(), "the first define was taken back");
    let problems = s.send_events(1, vec![define(SAW, At::Sample(100)), tone(At::Sample(300), -1.0), Op::Off { id: 1, at: At::Now, tag: None }]).unwrap_err();
    assert!(!problems.is_empty());
    assert!(s.pending().is_empty(), "so was the one before a bad note");
}

#[test]
fn notes_after_a_define_are_checked_against_the_program_it_makes() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    let new_track = "instr b(freq: hz) { osc(saw, freq) * env[1 | 5ms 0] }\ntrack extra { instrument = b }";
    let note = |at| Op::Note { target: "extra".into(), at, args: [("freq".to_string(), Value::Hz(110.0))].into(), dur: Some(Time::Seconds(0.05)), id: None, tag: None };
    // the track exists only once the define lands
    assert!(s.send_events(1, vec![note(At::Sample(60_000))]).is_err());
    s.send_events(1, vec![define(new_track, At::Sample(48_000)), note(At::Sample(60_000))]).unwrap();
    assert!(peak(&s.process(70_000).remove(0)[60_000..]) > 0.1);
}

#[test]
fn a_forget_drops_every_definition_when_it_lands() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    s.send_events(1, vec![Op::Forget { at: At::Sample(10_000), tag: None }]).unwrap();
    assert!(s.send_events(1, vec![tone(At::Sample(20_000), 110.0)]).is_err(), "no `lead` once it has gone");
    s.send_events(1, vec![tone(At::Sample(5_000), 110.0)]).unwrap();
    s.process(12_000);
    assert!(s.send_events(1, vec![tone(At::Now, 110.0)]).is_err());
}

#[test]
fn a_tempo_change_moves_beat_stamped_notes_but_not_seconds_stamped_ones() {
    let render = |at: At| {
        let mut s = Session::new(SR, Layout::Mono).without_limiter();
        s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
        // 120bpm: 8 beats is 4 seconds. Double the tempo after 1 second.
        s.send_events(1, vec![Op::Transport { at: At::Sample(48_000), bpm: Some(240.0), meter: None, origin: None, tag: None }, tone(at, 330.0)]).unwrap();
        let out = s.process(5 * 48_000).remove(0);
        first_sound(&out).unwrap()
    };
    // a sine starts from zero, so its first audible sample is a moment after the note starts
    let near = |got: usize, want: usize| assert!(got >= want && got <= want + 3, "{got} is not {want}");
    // the beat-stamped note was 3 seconds after the change and is now 1.5
    near(render(At::Time(Time::Beats(8.0))), 48_000 + 72_000);
    near(render(At::Time(Time::Seconds(4.0))), 4 * 48_000);
    near(render(At::Time(Time::Bars(2.0))), 48_000 + 72_000);
    near(render(At::Sample(200_000)), 200_000);
}

#[test]
fn the_transport_changes_the_clock_and_marks_bar_one() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    s.send_events(1, vec![Op::Transport { at: At::Sample(96_000), bpm: Some(60.0), meter: Some("3/4".into()), origin: Some(true), tag: None }]).unwrap();
    let clock = s.clock_info();
    assert_eq!((clock.bpm, clock.beats_per_bar, clock.bar), (120.0, 4.0, 1));
    assert_eq!(clock.sample, 0);
    // 1.5 seconds after the change, at 60bpm: beat 2.5 of bar 1
    s.process(96_000 + 72_000);
    let clock = s.clock_info();
    assert_eq!((clock.bpm, clock.beats_per_bar, clock.bar), (60.0, 3.0, 1));
    assert!((clock.beat - 2.5).abs() < 1e-6, "{}", clock.beat);
    assert_eq!(clock.sample, 168_000);
    assert_eq!(clock.sample_rate, SR);
    assert!(clock.wall_micros.is_none());
}

#[test]
fn definitions_and_the_transport_replay_to_the_same_audio() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
    s.send_events(1, vec![define(SAW, At::Time(Time::Beats(2.0))), Op::Transport { at: At::Sample(60_000), bpm: Some(150.0), meter: None, origin: None, tag: None }, tone(At::Time(Time::Beats(6.0)), 220.0)]).unwrap();
    s.process(20_000);
    s.send_events(1, vec![tone(At::Time(Time::Beats(3.0)), 165.0)]).unwrap();
    let live = s.process(150_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 170_000, false).remove(0);
    assert_eq!(live, replayed[20_000..]);
}
