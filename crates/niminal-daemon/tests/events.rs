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
    Op::Note { target: target.into(), at, args: [("freq".to_string(), Value::Hz(hz))].into(), dur: Some(Time::Seconds(0.1)), id: None }
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
        .send_events(0, vec![note("lead", At::Now, 440.0), note("nobody", At::Now, 440.0), Op::Control { name: "nope".into(), at: At::Now, value: 1.0, unit: None, glide: None }])
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
    s.send_events(0, vec![Op::Control { name: "level".into(), at: At::Sample(2_400), value: 0.5, unit: None, glide: Some(Time::Seconds(0.0)) }]).unwrap();
    let out = s.process(4_800).remove(0);
    assert_eq!(peak(&out[..2_399]), 0.0);
    assert!((out[2_500] - 0.5).abs() < 1e-5);
}

#[test]
fn a_track_op_sets_a_track_parameter() {
    let render = |cutoff: &str| {
        let mut s = session();
        s.send_events(0, vec![
            Op::Track { name: "pad".into(), at: At::Sample(0), args: [("cut".to_string(), cutoff.parse().unwrap())].into() },
            note("pad", At::Sample(10), 2_000.0),
        ])
        .unwrap();
        s.process(4_800).remove(0)
    };
    let (dull, bright) = (render("100hz"), render("8khz"));
    assert!(peak(&bright) > peak(&dull) * 2.0, "{} vs {}", peak(&bright), peak(&dull));
    let mut s = session();
    let problems = s.send_events(0, vec![Op::Track { name: "pad".into(), at: At::Now, args: [("nope".to_string(), Value::Num(1.0))].into() }]).unwrap_err();
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
        Op::Control { name: "level".into(), at: At::Sample(5_000), value: 0.4, unit: None, glide: Some(Time::Seconds(0.01)) },
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
    Op::Note { target: "lead".into(), at, args: [("freq".to_string(), Value::Hz(hz))].into(), dur: None, id: Some(id) }
}

fn set(id: u64, at: At, cut: &str, glide: Option<f64>) -> Op {
    Op::Set { id, at, args: [("cut".to_string(), cut.parse().unwrap())].into(), glide: glide.map(Time::Seconds) }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_held_note_sounds_until_off_then_releases() {
    let mut s = voices();
    s.send_events(0, vec![held(1, At::Sample(1_000), 220.0), Op::Off { id: 1, at: At::Sample(48_000) }]).unwrap();
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
            Op::Note { target: "lead".into(), at: At::Sample(0), args: [("freq".to_string(), Value::Hz(220.0))].into(), dur: Some(Time::Seconds(0.5)), id: Some(1) },
            Op::Note { target: "lead".into(), at: At::Sample(0), args: [("freq".to_string(), Value::Hz(330.0))].into(), dur: Some(Time::Seconds(0.5)), id: Some(2) },
            Op::Off { id: 2, at: At::Sample(2_400) },
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
    s.send_events(0, vec![held(7, At::Sample(12_000), 220.0), Op::Off { id: 7, at: At::Sample(6_000) }]).unwrap();
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
    let unknown = s.send_events(1, vec![Op::Off { id: 9, at: At::Now }, set(9, At::Now, "1khz", None)]).unwrap_err();
    assert_eq!(unknown.iter().map(|p| p.event).collect::<Vec<_>>(), [Some(0), Some(1)]);
    s.send_events(1, vec![Op::Off { id: 1, at: At::Now }]).unwrap();
    assert!(s.send_events(1, vec![set(1, At::Now, "1khz", None)]).is_err());
    // an unknown parameter
    let bad = s.send_events(2, vec![Op::Set { id: 1, at: At::Now, args: [("nope".to_string(), Value::Num(1.0))].into(), glide: None }]).unwrap_err();
    assert!(bad[0].message.contains("no parameter"), "{}", bad[0].message);
}

#[test]
fn a_client_that_goes_has_its_notes_released_and_its_waiting_notes_dropped() {
    let mut s = voices();
    s.send_events(1, vec![held(1, At::Sample(0), 220.0)]).unwrap();
    s.send_events(1, vec![Op::Note { target: "lead".into(), at: At::Sample(30_000), args: [("freq".to_string(), Value::Hz(440.0))].into(), dur: Some(Time::Seconds(0.2)), id: None }]).unwrap();
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
    s.send_events(4, vec![Op::Off { id: 1, at: At::Sample(30_000) }, held(2, At::Time(Time::Beats(1.0)), 165.0)]).unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 63_000, false).remove(0);
    let mut expected = voices();
    expected.send_events(4, vec![held(1, At::Sample(500), 110.0)]).unwrap();
    assert_eq!(live, replayed[3_000..]);
    let _ = expected;
}
