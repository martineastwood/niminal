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
    Op::Note { target: target.into(), at, args: [("freq".to_string(), Value::Hz(hz))].into(), dur: Time::Seconds(0.1) }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn notes_start_at_a_sample_a_position_or_now() {
    let mut s = session();
    let sent = s
        .send_events(vec![
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
        .send_events(vec![note("lead", At::Now, 440.0), note("nobody", At::Now, 440.0), Op::Control { name: "nope".into(), at: At::Now, value: 1.0, unit: None, glide: None }])
        .unwrap_err();
    assert_eq!(problems.iter().map(|p| p.event).collect::<Vec<_>>(), [Some(1), Some(2)]);
    assert_eq!(peak(&s.process(4_000).remove(0)), 0.0);
}

#[test]
fn events_in_the_past_sound_at_once_and_are_counted() {
    let mut s = session();
    s.process(10_000);
    let sent = s.send_events(vec![note("lead", At::Sample(100), 440.0), note("lead", At::Sample(20_000), 440.0)]).unwrap();
    assert_eq!((sent.late, sent.first), (1, Some(100)));
    assert!(peak(&s.process(2_000).remove(0)) > 0.1);
}

#[test]
fn a_control_moves_at_its_sample_with_a_glide() {
    let mut s = session();
    s.eval("held(freq: 100hz) for 100bars", Some("now")).unwrap();
    s.send_events(vec![Op::Control { name: "level".into(), at: At::Sample(2_400), value: 0.5, unit: None, glide: Some(Time::Seconds(0.0)) }]).unwrap();
    let out = s.process(4_800).remove(0);
    assert_eq!(peak(&out[..2_399]), 0.0);
    assert!((out[2_500] - 0.5).abs() < 1e-5);
}

#[test]
fn a_track_op_sets_a_track_parameter() {
    let render = |cutoff: &str| {
        let mut s = session();
        s.send_events(vec![
            Op::Track { name: "pad".into(), at: At::Sample(0), args: [("cut".to_string(), cutoff.parse().unwrap())].into() },
            note("pad", At::Sample(10), 2_000.0),
        ])
        .unwrap();
        s.process(4_800).remove(0)
    };
    let (dull, bright) = (render("100hz"), render("8khz"));
    assert!(peak(&bright) > peak(&dull) * 2.0, "{} vs {}", peak(&bright), peak(&dull));
    let mut s = session();
    let problems = s.send_events(vec![Op::Track { name: "pad".into(), at: At::Now, args: [("nope".to_string(), Value::Num(1.0))].into() }]).unwrap_err();
    assert!(problems[0].message.contains("no parameter"), "{}", problems[0].message);
}

#[test]
fn the_log_replays_to_the_same_audio() {
    let mut s = session();
    s.eval("held(freq: 100hz) for 100bars", Some("now")).unwrap();
    s.process(1_000);
    s.send_events(vec![
        note("lead", At::Sample(3_000), 330.0),
        note("lead", At::Time(Time::Beats(1.0)), 440.0),
        Op::Control { name: "level".into(), at: At::Sample(5_000), value: 0.4, unit: None, glide: Some(Time::Seconds(0.01)) },
    ])
    .unwrap();
    s.process(2_000);
    s.send_events(vec![note("lead", At::Now, 550.0)]).unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 63_000, false).remove(0);
    assert_eq!(live, replayed[3_000..]);
}
