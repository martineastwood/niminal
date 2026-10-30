//! The engine's `events.send`: each op, validation, late events and replay.

use niminal_server::{At, Op, Session};
mod support;
use niminal_lang::Layout;
use niminal_score::{Time, Value};
use support::Eval;

const SR: f32 = 48_000.0;

const SETUP: &str = "\nctl level = 0.0.smooth(0ms)\ninstr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\ninstr flat(freq: hz) { level }\nnode lead { instrument = tone }\nbus pad_audio\nfx pad_fx(cut: hz = 300hz) {out = pad_audio.lpf(cutoff: cut)}\nnode pad { instrument = tone\noutput = pad_audio}\nnode held { instrument = flat }\n";

/// A note on the `held` node that lasts the whole test.
fn hold(s: &mut Session) {
    let args = [("freq".to_string(), Value::Hz(100.0))].into();
    s.send_events(
        0,
        vec![Op::Note {
            to: None,
            target: "held".into(),
            at: At::Now,
            args,
            dur: Some(Time::Seconds(200.0)),
            id: None,
            tag: None,
        }],
    )
    .unwrap();
}

fn session() -> Session {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(SETUP, Some("now")).unwrap();
    s
}

fn note(target: &str, at: At, hz: f64) -> Op {
    Op::Note {
        to: None,
        target: target.into(),
        at,
        args: [("freq".to_string(), Value::Hz(hz))].into(),
        dur: Some(Time::Seconds(0.1)),
        id: None,
        tag: None,
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn notes_start_at_a_sample_a_position_or_now() {
    let mut s = session();
    let sent = s
        .send_events(
            0,
            vec![
                note("lead", At::Sample(4_800), 440.0),
                note("lead", At::Sample(48_000), 440.0), // 1 second at 120bpm
                note("lead", At::Now, 440.0),
            ],
        )
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
        .send_events(
            0,
            vec![
                note("lead", At::Now, 440.0),
                note("nobody", At::Now, 440.0),
                Op::Control {
                    name: "nope".into(),
                    at: At::Now,
                    value: 1.0,
                    unit: None,
                    glide: None,
                    tag: None,
                },
            ],
        )
        .unwrap_err();
    assert_eq!(
        problems.iter().map(|p| p.event).collect::<Vec<_>>(),
        [Some(1), Some(2)]
    );
    assert_eq!(peak(&s.process(4_000).remove(0)), 0.0);
}

#[test]
fn events_in_the_past_sound_at_once_and_are_counted() {
    let mut s = session();
    s.process(10_000);
    let sent = s
        .send_events(
            0,
            vec![
                note("lead", At::Sample(100), 440.0),
                note("lead", At::Sample(20_000), 440.0),
            ],
        )
        .unwrap();
    assert_eq!((sent.late, sent.first), (1, Some(100)));
    assert!(peak(&s.process(2_000).remove(0)) > 0.1);
}

#[test]
fn a_control_moves_at_its_sample_with_a_glide() {
    let mut s = session();
    hold(&mut s);
    s.send_events(
        0,
        vec![Op::Control {
            name: "level".into(),
            at: At::Sample(2_400),
            value: 0.5,
            unit: None,
            glide: Some(Time::Seconds(0.0)),
            tag: None,
        }],
    )
    .unwrap();
    let out = s.process(4_800).remove(0);
    assert_eq!(peak(&out[..2_399]), 0.0);
    assert!((out[2_500] - 0.5).abs() < 1e-5);
}

#[test]
fn a_control_op_sets_a_fx_parameter() {
    let render = |cutoff: f64| {
        let mut s = session();
        s.send_events(
            0,
            vec![
                Op::Control {
                    name: "pad_fx::cut".into(),
                    at: At::Sample(0),
                    value: cutoff,
                    unit: Some("hz".into()),
                    glide: None,
                    tag: None,
                },
                note("pad", At::Sample(10), 2_000.0),
            ],
        )
        .unwrap();
        s.process(4_800).remove(0)
    };
    let (dull, bright) = (render(100.0), render(8_000.0));
    assert!(
        peak(&bright) > peak(&dull) * 2.0,
        "{} vs {}",
        peak(&bright),
        peak(&dull)
    );
    let mut s = session();
    let problems = s
        .send_events(
            0,
            vec![Op::Control {
                name: "pad_fx::nope".into(),
                at: At::Now,
                value: 1.0,
                unit: None,
                glide: None,
                tag: None,
            }],
        )
        .unwrap_err();
    assert!(
        problems[0].message.contains("unknown control"),
        "{}",
        problems[0].message
    );
}

#[test]
fn the_log_replays_to_the_same_audio() {
    let mut s = session();
    hold(&mut s);
    s.process(1_000);
    s.send_events(
        0,
        vec![
            note("lead", At::Sample(3_000), 330.0),
            note("lead", At::Sample(24_000), 440.0),
            Op::Control {
                name: "level".into(),
                at: At::Sample(5_000),
                value: 0.4,
                unit: None,
                glide: Some(Time::Seconds(0.01)),
                tag: None,
            },
        ],
    )
    .unwrap();
    s.process(2_000);
    s.send_events(0, vec![note("lead", At::Now, 550.0)])
        .unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, s.log(), 63_000, false).remove(0);
    assert_eq!(live, replayed[3_000..]);
}

const VOICE: &str = "\ninstr organ(freq: hz, cut: hz = 40hz) { osc(saw, freq).lpf(cutoff: cut) * env[1 | 100ms 0] }\nnode lead { instrument = organ }\n";

fn voices() -> Session {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(VOICE, Some("now")).unwrap();
    s
}
fn own(s: &mut Session, client: u64) {
    let source = s.program().source.clone();
    s.send_events(client, vec![define(&source, At::Now)])
        .unwrap();
}

fn held(id: u64, at: At, hz: f64) -> Op {
    Op::Note {
        to: None,
        target: "lead".into(),
        at,
        args: [("freq".to_string(), Value::Hz(hz))].into(),
        dur: None,
        id: Some(id),
        tag: None,
    }
}

fn set(id: u64, at: At, cut: &str, glide: Option<f64>) -> Op {
    Op::Set {
        id,
        at,
        args: [("cut".to_string(), cut.parse().unwrap())].into(),
        glide: glide.map(Time::Seconds),
        tag: None,
    }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_held_note_sounds_until_off_then_releases() {
    let mut s = voices();
    s.send_events(
        0,
        vec![
            held(1, At::Sample(1_000), 220.0),
            Op::Off {
                id: 1,
                at: At::Sample(48_000),
                tag: None,
            },
        ],
    )
    .unwrap();
    let out = s.process(60_000).remove(0);
    assert!(
        rms(&out[40_000..47_000]) > 0.01,
        "still sounding long after it started"
    );
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
            Op::Note {
                to: None,
                target: "lead".into(),
                at: At::Sample(0),
                args: [("freq".to_string(), Value::Hz(220.0))].into(),
                dur: Some(Time::Seconds(0.5)),
                id: Some(1),
                tag: None,
            },
            Op::Note {
                to: None,
                target: "lead".into(),
                at: At::Sample(0),
                args: [("freq".to_string(), Value::Hz(330.0))].into(),
                dur: Some(Time::Seconds(0.5)),
                id: Some(2),
                tag: None,
            },
            Op::Off {
                id: 2,
                at: At::Sample(2_400),
                tag: None,
            },
        ],
    )
    .unwrap();
    let out = s.process(48_000).remove(0);
    assert!(rms(&out[20_000..23_000]) > 0.01);
    assert!(
        rms(&out[36_000..48_000]) < 1e-4,
        "both ended by 0.5sec plus the release"
    );
}

#[test]
fn a_set_moves_a_parameter_of_a_sounding_note_with_a_glide() {
    let run = |glide: Option<f64>| {
        let mut s = voices();
        s.send_events(
            0,
            vec![
                held(1, At::Sample(0), 110.0),
                set(1, At::Sample(24_000), "8khz", glide),
            ],
        )
        .unwrap();
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
    s.send_events(
        0,
        vec![
            held(1, At::Sample(12_000), 110.0),
            set(1, At::Sample(100), "8khz", None),
        ],
    )
    .unwrap();
    let bright = s.process(24_000).remove(0);
    let mut s = voices();
    s.send_events(0, vec![held(1, At::Sample(12_000), 110.0)])
        .unwrap();
    let dull = s.process(24_000).remove(0);
    assert!(rms(&bright[12_100..20_000]) > rms(&dull[12_100..20_000]) * 2.0);

    let mut s = voices();
    s.send_events(
        0,
        vec![
            held(7, At::Sample(12_000), 220.0),
            Op::Off {
                id: 7,
                at: At::Sample(6_000),
                tag: None,
            },
        ],
    )
    .unwrap();
    let out = s.process(96_000).remove(0);
    assert!(rms(&out[12_100..12_400]) > 0.001, "it still sounds");
    assert!(rms(&out[40_000..96_000]) < 1e-4, "but only as a blip");
}

#[test]
fn ids_belong_to_the_client_and_bad_ones_fail_the_batch() {
    let mut s = voices();
    own(&mut s, 3);
    own(&mut s, 2);
    own(&mut s, 1);
    s.send_events(1, vec![held(1, At::Now, 220.0)]).unwrap();
    // another client can use the same id
    s.send_events(2, vec![held(1, At::Now, 330.0)]).unwrap();
    // the same client cannot, nor can one batch use an id twice
    let again = s.send_events(1, vec![held(1, At::Now, 220.0)]).unwrap_err();
    assert!(
        again[0].message.contains("already sounding"),
        "{}",
        again[0].message
    );
    let twice = s
        .send_events(3, vec![held(5, At::Now, 220.0), held(5, At::Now, 220.0)])
        .unwrap_err();
    assert_eq!(twice[0].event, Some(1));
    // unknown ids, and ids already let go of
    let unknown = s
        .send_events(
            1,
            vec![
                Op::Off {
                    id: 9,
                    at: At::Now,
                    tag: None,
                },
                set(9, At::Now, "1khz", None),
            ],
        )
        .unwrap_err();
    assert_eq!(
        unknown.iter().map(|p| p.event).collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
    s.send_events(
        1,
        vec![Op::Off {
            id: 1,
            at: At::Now,
            tag: None,
        }],
    )
    .unwrap();
    assert!(
        s.send_events(1, vec![set(1, At::Now, "1khz", None)])
            .is_err()
    );
    // an unknown parameter
    let bad = s
        .send_events(
            2,
            vec![Op::Set {
                id: 1,
                at: At::Now,
                args: [("nope".to_string(), Value::Num(1.0))].into(),
                glide: None,
                tag: None,
            }],
        )
        .unwrap_err();
    assert!(
        bad[0].message.contains("no parameter"),
        "{}",
        bad[0].message
    );
}

#[test]
fn a_client_that_goes_has_its_notes_released_and_its_waiting_notes_dropped() {
    let mut s = voices();
    own(&mut s, 1);
    s.send_events(1, vec![held(1, At::Sample(0), 220.0)])
        .unwrap();
    s.send_events(
        1,
        vec![Op::Note {
            to: None,
            target: "lead".into(),
            at: At::Sample(30_000),
            args: [("freq".to_string(), Value::Hz(440.0))].into(),
            dur: Some(Time::Seconds(0.2)),
            id: None,
            tag: None,
        }],
    )
    .unwrap();
    s.process(10_000);
    s.drop_client(1);
    let out = s.process(30_000).remove(0);
    assert!(
        rms(&out[6_000..10_000]) < 1e-4,
        "released: {}",
        rms(&out[6_000..10_000])
    );
    assert!(
        rms(&out[20_000..30_000]) < 1e-4,
        "the waiting note never sounds"
    );
}

#[test]
fn held_notes_and_sets_replay_to_the_same_audio() {
    let mut s = voices();
    own(&mut s, 4);
    s.send_events(
        4,
        vec![
            held(1, At::Sample(500), 110.0),
            set(1, At::Sample(9_000), "4khz", Some(0.1)),
        ],
    )
    .unwrap();
    s.process(3_000);
    s.send_events(
        4,
        vec![
            Op::Off {
                id: 1,
                at: At::Sample(30_000),
                tag: None,
            },
            held(2, At::Sample(24_000), 165.0),
        ],
    )
    .unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, s.log(), 63_000, false).remove(0);
    let mut expected = voices();
    own(&mut expected, 4);
    expected
        .send_events(4, vec![held(1, At::Sample(500), 110.0)])
        .unwrap();
    assert_eq!(live, replayed[3_000..]);
    let _ = expected;
}

/// One short note a beat for `bars` bars, from bar `first` (counting from 0), all tagged.
fn bars_of(hz: f64, first: u32, bars: u32, tag: &str) -> Vec<Op> {
    (first * 4..(first + bars) * 4)
        .map(|beat| Op::Note {
            to: None,
            target: "lead".into(),
            at: At::Sample(((f64::from(beat)) * 24000.0).round() as u64),
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
    own(&mut s, 1);
    s.send_events(1, bars_of(220.0, 0, 4, "a")).unwrap();
    let mut audio = s.process(BAR / 2).remove(0);
    let done = s.cancel_events(1, Some("a"), At::Sample(96_000), false);
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
    own(&mut s, 2);
    own(&mut s, 1);
    s.send_events(1, bars_of(220.0, 0, 1, "a")).unwrap();
    s.send_events(2, bars_of(330.0, 0, 1, "a")).unwrap();
    let done = s.cancel_events(1, None, At::Now, false);
    assert_eq!(done.cancelled, 4);
    let out = s.process(BAR).remove(0);
    assert!(
        (hz_of(&out[1_000..9_000]) - 330.0).abs() < 10.0,
        "client 2's notes still play"
    );
}

#[test]
fn cancel_with_release_lets_go_of_held_notes_and_drops_waiting_offs() {
    let mut s = voices();
    own(&mut s, 1);
    let mut ops = vec![held(1, At::Sample(0), 220.0), held(2, At::Sample(0), 330.0)];
    for op in &mut ops {
        if let Op::Note { tag, .. } = op {
            *tag = Some("pad".into());
        }
    }
    ops.push(Op::Off {
        id: 2,
        at: At::Sample(40_000),
        tag: Some("pad".into()),
    });
    s.send_events(1, ops).unwrap();
    s.process(10_000);
    // the waiting off is taken back, and the held notes are released instead
    let done = s.cancel_events(1, Some("pad"), At::Now, true);
    assert_eq!((done.cancelled, done.released), (1, 2));
    let out = s.process(20_000).remove(0);
    assert!(
        rms(&out[6_000..20_000]) < 1e-4,
        "id 1 was released, and id 2 with it: {}",
        rms(&out[6_000..20_000])
    );

    // without `release` a held note carries on
    let mut s = voices();
    own(&mut s, 1);
    s.send_events(1, vec![held(1, At::Sample(0), 220.0)])
        .unwrap();
    s.process(5_000);
    assert_eq!(s.cancel_events(1, None, At::Now, false).released, 0);
    assert!(rms(&s.process(20_000).remove(0)[10_000..]) > 0.01);
}

#[test]
fn a_waiting_control_can_be_cancelled() {
    let mut s = session();
    own(&mut s, 1);
    hold(&mut s);
    s.send_events(
        1,
        vec![Op::Control {
            name: "level".into(),
            at: At::Sample(5_000),
            value: 0.5,
            unit: None,
            glide: Some(Time::Seconds(0.0)),
            tag: Some("x".into()),
        }],
    )
    .unwrap();
    assert_eq!(s.cancel_events(1, Some("y"), At::Now, false).cancelled, 0);
    assert_eq!(s.cancel_events(1, Some("x"), At::Now, false).cancelled, 1);
    assert_eq!(peak(&s.process(10_000).remove(0)), 0.0);
}

fn define(source: &str, at: At) -> Op {
    Op::Define {
        source: source.into(),
        at,
        dir: None,
        tag: None,
    }
}

fn first_sound(x: &[f32]) -> Option<usize> {
    x.iter().position(|s| s.abs() > 1e-4)
}

const SINE: &str =
    "instr a(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\nnode lead { instrument = a }";
const SAW: &str =
    "instr a(freq: hz) { osc(saw, freq) * env[1 | 5ms 0] }\nnode lead { instrument = a }";

fn tone(at: At, hz: f64) -> Op {
    Op::Note {
        to: None,
        target: "lead".into(),
        at,
        args: [("freq".to_string(), Value::Hz(hz))].into(),
        dur: Some(Time::Seconds(0.05)),
        id: None,
        tag: None,
    }
}

#[test]
fn a_define_lands_at_its_sample_and_the_next_note_uses_it() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(SINE, Some("now")).unwrap();
    // beat 2 at 120bpm is sample 48000
    s.send_events(
        0,
        vec![
            define(SAW, At::Sample(48_000)),
            tone(At::Sample(20_000), 110.0),
            tone(At::Sample(48_000), 110.0),
        ],
    )
    .unwrap();
    let out = s.process(70_000).remove(0);

    let mut reference = Session::new(SR, Layout::Mono).without_limiter();
    reference.eval(SAW, Some("now")).unwrap();
    reference
        .send_events(0, vec![tone(At::Sample(48_000), 110.0)])
        .unwrap();
    let saw = reference.process(70_000).remove(0);
    assert_eq!(
        out[48_000..],
        saw[48_000..],
        "the second note is the new instrument from its first sample"
    );
    assert_ne!(
        out[20_000..22_000],
        saw[20_000..22_000],
        "the first note was the old one"
    );
    assert!(first_sound(&out[48_000..]).unwrap() < 10);
}

#[test]
fn a_define_can_be_cancelled_by_its_tag_and_performance_statements_are_refused() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval_as(1, SINE).unwrap();
    let mut op = define(SAW, At::Sample(48_000));
    if let Op::Define { tag, .. } = &mut op {
        *tag = Some("t".into());
    }
    s.send_events(1, vec![op]).unwrap();
    assert_eq!(s.cancel_events(1, Some("t"), At::Now, false).cancelled, 1);
    s.send_events(1, vec![tone(At::Sample(48_000), 110.0)])
        .unwrap();
    let out = s.process(60_000).remove(0);
    let mut reference = Session::new(SR, Layout::Mono).without_limiter();
    reference.eval(SINE, Some("now")).unwrap();
    reference
        .send_events(0, vec![tone(At::Sample(48_000), 110.0)])
        .unwrap();
    assert_eq!(out, reference.process(60_000).remove(0));

    for bad in ["lead.play([c4])", "tempo 90bpm", "lead(freq: c4) for 1beat"] {
        let problems = s
            .send_events(1, vec![tone(At::Now, 110.0), define(bad, At::Now)])
            .unwrap_err();
        assert_eq!(problems[0].event, Some(1), "{bad}");
    }
}

#[test]
fn a_batch_that_fails_leaves_nothing_behind() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(SINE, Some("now")).unwrap();
    let problems = s
        .send_events(
            1,
            vec![
                define(SAW, At::Sample(100)),
                define("instr broken( {", At::Sample(200)),
            ],
        )
        .unwrap_err();
    assert_eq!(problems[0].event, Some(1));
    assert_eq!(s.waiting_changes(), 0, "the first define was taken back");
    let problems = s
        .send_events(
            1,
            vec![
                define(SAW, At::Sample(100)),
                tone(At::Sample(300), -1.0),
                Op::Off {
                    id: 1,
                    at: At::Now,
                    tag: None,
                },
            ],
        )
        .unwrap_err();
    assert!(!problems.is_empty());
    assert_eq!(s.waiting_changes(), 0, "so was the one before a bad note");
}

#[test]
fn notes_after_a_define_are_checked_against_the_program_it_makes() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(SINE, Some("now")).unwrap();
    let new_node =
        "instr b(freq: hz) { osc(saw, freq) * env[1 | 5ms 0] }\nnode extra { instrument = b }";
    let note = |at| Op::Note {
        to: None,
        target: "extra".into(),
        at,
        args: [("freq".to_string(), Value::Hz(110.0))].into(),
        dur: Some(Time::Seconds(0.05)),
        id: None,
        tag: None,
    };
    // the node exists only once the define lands
    assert!(s.send_events(1, vec![note(At::Sample(60_000))]).is_err());
    s.send_events(
        1,
        vec![
            define(new_node, At::Sample(48_000)),
            note(At::Sample(60_000)),
        ],
    )
    .unwrap();
    assert!(peak(&s.process(70_000).remove(0)[60_000..]) > 0.1);
}

#[test]
fn a_forget_drops_every_definition_when_it_lands() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval_as(1, SINE).unwrap();
    s.send_events(
        1,
        vec![Op::Forget {
            at: At::Sample(10_000),
            tag: None,
        }],
    )
    .unwrap();
    assert!(
        s.send_events(1, vec![tone(At::Sample(20_000), 110.0)])
            .is_err(),
        "no `lead` once it has gone"
    );
    s.send_events(1, vec![tone(At::Sample(5_000), 110.0)])
        .unwrap();
    s.process(12_000);
    assert!(s.send_events(1, vec![tone(At::Now, 110.0)]).is_err());
}

#[test]
fn timestamped_definitions_and_notes_replay_to_the_same_audio() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(SINE, Some("now")).unwrap();
    s.send_events(
        1,
        vec![
            define(SAW, At::Sample(48_000)),
            tone(At::Sample(144_000), 220.0),
        ],
    )
    .unwrap();
    s.process(20_000);
    s.send_events(1, vec![tone(At::Sample(72_000), 165.0)])
        .unwrap();
    let live = s.process(150_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, s.log(), 170_000, false).remove(0);
    assert_eq!(live, replayed[20_000..]);
}

#[test]
fn a_timed_panic_silences_voices_at_its_sample_and_later_events_still_play() {
    let mut s = voices();
    own(&mut s, 1);
    s.send_events(
        1,
        vec![
            held(1, At::Sample(0), 220.0),
            held(2, At::Sample(30_000), 330.0),
            Op::Panic {
                at: At::Sample(10_000),
                tag: None,
            },
        ],
    )
    .unwrap();
    let out = s.process(50_000).remove(0);
    assert!(rms(&out[8_000..10_000]) > 0.01, "sounding right up to it");
    assert_eq!(peak(&out[10_000..30_000]), 0.0, "silent after it");
    assert!(
        rms(&out[40_000..50_000]) > 0.003,
        "a note that starts after it plays"
    );
    // and a panic that was asked for can be taken back
    let mut s = voices();
    own(&mut s, 1);
    s.send_events(
        1,
        vec![
            held(1, At::Sample(0), 220.0),
            Op::Panic {
                at: At::Sample(10_000),
                tag: Some("p".into()),
            },
        ],
    )
    .unwrap();
    assert_eq!(s.cancel_events(1, Some("p"), At::Now, false).cancelled, 1);
    assert!(rms(&s.process(20_000).remove(0)[12_000..]) > 0.01);
}

#[test]
fn a_kit_sample_in_a_note_crosses_the_wire() {
    let op = Op::Note {
        to: None,
        target: "drums".into(),
        at: At::Sample(0),
        args: [("sample".to_string(), Value::Sample(4))].into(),
        dur: Some(Time::Seconds(0.5)),
        id: None,
        tag: None,
    };
    let text = serde_json::to_string(&op).unwrap();
    assert_eq!(serde_json::from_str::<Op>(&text).unwrap(), op);
}

#[test]
fn an_off_and_a_note_with_the_same_id_at_one_sample_end_the_old_note_and_start_the_new_one() {
    let mut s = voices();
    s.send_events(
        0,
        vec![
            held(1, At::Sample(0), 220.0),
            Op::Off {
                id: 1,
                at: At::Sample(24_000),
                tag: None,
            },
            held(1, At::Sample(24_000), 330.0),
        ],
    )
    .unwrap();
    let out = s.process(48_000).remove(0);
    // the old note (220hz) ends at the off, and the new one (330hz) is still going at the end
    assert!(rms(&out[10_000..20_000]) > 0.01);
    assert!(
        rms(&out[30_000..47_000]) > 0.003,
        "the new note keeps sounding: the off belongs to the old one"
    );
}

#[test]
fn controls_at_one_sample_apply_in_the_order_they_were_sent() {
    let level = |value: f64| Op::Control {
        name: "level".into(),
        at: At::Sample(2_400),
        value,
        unit: None,
        glide: Some(Time::Seconds(0.0)),
        tag: None,
    };
    for (first, last) in [(1.0, 0.25), (0.25, 1.0)] {
        let mut s = session();
        hold(&mut s);
        s.send_events(0, vec![level(first), level(last)]).unwrap();
        let out = s.process(4_800).remove(0);
        assert!((out[2_500] - last as f32).abs() < 1e-5, "{}", out[2_500]);
    }
}

fn tempo(at: u64, bpm: f64) -> Op {
    Op::Tempo {
        at: At::Sample(at),
        bpm,
        meter: None,
        beat: None,
        tag: None,
    }
}

fn hold_on(s: &mut Session, client: u64) {
    s.send_events(
        client,
        vec![Op::Note {
            to: None,
            target: "beat_node".into(),
            at: At::Now,
            args: [("freq".to_string(), Value::Hz(100.0))].into(),
            dur: Some(Time::Seconds(200.0)),
            id: None,
            tag: None,
        }],
    )
    .unwrap();
}

const CLOCKED: &str =
    "instr beat_sig(freq: hz) { clock.beat * 0.001 }\nnode beat_node { instrument = beat_sig }\n";

#[test]
fn clock_beat_follows_the_connections_tempo_without_jumping() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(CLOCKED, Some("now")).unwrap();
    hold_on(&mut s, 0);
    // 120 bpm for the first second: beat 2; then 60 bpm, which carries on from there at one beat a second
    s.send_events(0, vec![tempo(SR as u64, 60.0)]).unwrap();
    let out = s.process(3 * SR as usize).remove(0);
    let beat = |sec: f64| f64::from(out[(sec * f64::from(SR)) as usize]) * 1000.0;
    assert!((beat(0.5) - 1.0).abs() < 1e-3, "{}", beat(0.5));
    assert!((beat(1.5) - 2.5).abs() < 1e-3, "{}", beat(1.5));
    assert!((beat(2.5) - 3.5).abs() < 1e-3, "{}", beat(2.5));
}

#[test]
fn the_length_of_a_beat_and_a_bar_follow_the_tempo() {
    // seconds: 0.5 and 2 at 120 bpm, then 1 and 4 at 60
    for (field, before, after) in [("beat_length", 0.5, 1.0), ("bar_length", 2.0, 4.0)] {
        let source = format!(
            "instr beat_sig(freq: hz) {{ clock.{field} / 1sec * 0.001 }}\nnode beat_node {{ instrument = beat_sig }}\n"
        );
        let mut s = Session::new(SR, Layout::Mono).without_limiter();
        s.eval(&source, Some("now")).unwrap();
        hold_on(&mut s, 0);
        s.send_events(0, vec![tempo(SR as u64, 60.0)]).unwrap();
        let out = s.process(3 * SR as usize).remove(0);
        let seconds = |at: usize| f64::from(out[at]) * 1000.0;
        assert!(
            (seconds(SR as usize / 2) - before).abs() < 1e-3,
            "{field}: {}",
            seconds(SR as usize / 2)
        );
        assert!(
            (seconds(3 * SR as usize - 1) - after).abs() < 1e-3,
            "{field}: {}",
            seconds(3 * SR as usize - 1)
        );
    }
}

#[test]
fn a_synced_lfo_stays_locked_to_the_beat_through_a_tempo_change() {
    let source = "instr beat_sig(freq: hz) { synced_lfo(beats: 1, shape: saw) * 0.001 }\nnode beat_node { instrument = beat_sig }\n";
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(source, Some("now")).unwrap();
    hold_on(&mut s, 0);
    s.send_events(0, vec![tempo(SR as u64, 60.0)]).unwrap();
    let out = s.process(3 * SR as usize).remove(0);
    let at = |sec: f64| f64::from(out[(sec * f64::from(SR)) as usize]) * 1000.0;
    // the saw is -1 at a half beat past the start of a cycle: beat 0.75 is -0.5, and after the change
    // (a beat a second from beat 2) beat 3.25 is +0.5
    assert!((at(0.375) + 0.5).abs() < 0.02, "{}", at(0.375));
    assert!((at(2.25) - 0.5).abs() < 0.02, "{}", at(2.25));
}

#[test]
fn a_filter_swept_by_a_synced_lfo_plays() {
    let source = "instr beat_sig(freq: hz = 220hz) {\n  wobble = synced_lfo(beats: 1/2).unipolar.range(300hz..3khz).smooth(5ms)\n  osc(saw, freq).lpf(cutoff: wobble) * 0.3\n}\nnode beat_node { instrument = beat_sig }\n";
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(source, Some("now")).unwrap();
    hold_on(&mut s, 0);
    let out = s.process(SR as usize).remove(0);
    assert!(out.iter().all(|v| v.is_finite()) && out.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn two_connections_keep_their_own_tempo() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    for client in [1, 2] {
        s.send_events(
            client,
            vec![Op::Define {
                source: CLOCKED.into(),
                at: At::Now,
                dir: None,
                tag: None,
            }],
        )
        .unwrap();
    }
    s.send_events(2, vec![tempo(0, 240.0)]).unwrap();
    for client in [1, 2] {
        hold_on(&mut s, client);
    }
    let out = s.process(SR as usize).remove(0);
    // both notes sum on the master: 2 beats a second and 4 beats a second at one second
    let beat = f64::from(out[SR as usize - 1]) * 1000.0;
    assert!((beat - 6.0).abs() < 0.05, "{beat}");
}

#[test]
fn a_tempo_change_replays_to_the_same_audio() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(CLOCKED, Some("now")).unwrap();
    hold_on(&mut s, 0);
    s.send_events(0, vec![tempo(10_000, 90.0)]).unwrap();
    s.process(5_000);
    s.send_events(0, vec![tempo(30_000, 150.0)]).unwrap();
    let live = s.process(60_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, s.log(), 65_000, false).remove(0);
    assert_eq!(live, replayed[5_000..]);
    assert!(live[59_999] > live[20_000]);
}

#[test]
fn a_tempo_in_the_batch_that_defines_what_reads_it_reaches_it() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.send_events(
        1,
        vec![
            Op::Define {
                source: CLOCKED.into(),
                at: At::Sample(0),
                dir: None,
                tag: None,
            },
            tempo(0, 240.0),
        ],
    )
    .unwrap();
    hold_on(&mut s, 1);
    let out = s.process(SR as usize).remove(0);
    let beat = f64::from(out[SR as usize - 1]) * 1000.0;
    assert!((beat - 4.0).abs() < 0.05, "{beat}");
}

#[test]
fn a_problem_after_clock_beat_is_pointed_at_where_it_was_written() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    let source = "instr a(freq: hz) { clock.beat + nope(1) }";
    let problems = s
        .send_events(
            1,
            vec![Op::Define {
                source: source.into(),
                at: At::Now,
                dir: None,
                tag: None,
            }],
        )
        .unwrap_err();
    let at = source.find("nope").unwrap();
    let span = problems[0].span.expect("a span");
    assert_eq!(&source[span.start..span.end], "nope", "{problems:?}");
    assert_eq!(problems[0].column, Some(at + 1));
}

const TIMED: &str = "ctl pause = 0.5sec.smooth(0ms)\ninstr pause_sig(freq: hz) { pause / 1sec * 0.1 }\nnode pause_node { instrument = pause_sig }\n";

fn pause_at(s: &mut Session, from: usize, len: usize) -> f64 {
    let out = s.process(from + len).remove(0);
    f64::from(out[from + len - 1]) * 10.0
}

fn set_pause(at: u64, value: f64, unit: &str) -> Op {
    Op::Control {
        name: "pause".into(),
        at: At::Sample(at),
        value,
        unit: Some(unit.into()),
        glide: None,
        tag: None,
    }
}

fn hold_pause(s: &mut Session) {
    s.send_events(
        0,
        vec![Op::Note {
            to: None,
            target: "pause_node".into(),
            at: At::Now,
            args: [("freq".to_string(), Value::Hz(100.0))].into(),
            dur: Some(Time::Seconds(200.0)),
            id: None,
            tag: None,
        }],
    )
    .unwrap();
}

#[test]
fn a_control_set_in_beats_follows_the_tempo_and_one_set_in_seconds_does_not() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(TIMED, Some("now")).unwrap();
    hold_pause(&mut s);
    // a quarter of a beat at 120 bpm is 0.125 sec; at 60 bpm it is 0.25
    s.send_events(0, vec![set_pause(100, 0.25, "beats")])
        .unwrap();
    assert!((pause_at(&mut s, 0, 4_800) - 0.125).abs() < 1e-4);
    s.send_events(0, vec![tempo(10_000, 60.0)]).unwrap();
    assert!((pause_at(&mut s, 4_800, 9_600) - 0.25).abs() < 1e-4);
    // a bar is as many beats as the meter says
    s.send_events(0, vec![set_pause(20_000, 0.5, "bars")])
        .unwrap();
    assert!((pause_at(&mut s, 14_400, 9_600) - 2.0).abs() < 1e-4);
    // set in seconds, it stays put through the next change
    s.send_events(0, vec![set_pause(30_000, 0.3, "sec"), tempo(40_000, 120.0)])
        .unwrap();
    assert!((pause_at(&mut s, 24_000, 24_000) - 0.3).abs() < 1e-4);
}

#[test]
fn another_connections_tempo_does_not_move_a_control_kept_to_beats() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.send_events(
        1,
        vec![Op::Define {
            source: TIMED.into(),
            at: At::Now,
            dir: None,
            tag: None,
        }],
    )
    .unwrap();
    s.send_events(
        1,
        vec![Op::Control {
            name: "pause".into(),
            at: At::Sample(0),
            value: 1.0,
            unit: Some("beats".into()),
            glide: None,
            tag: None,
        }],
    )
    .unwrap();
    s.send_events(2, vec![tempo(100, 30.0)]).unwrap();
    s.send_events(
        1,
        vec![Op::Note {
            to: None,
            target: "pause_node".into(),
            at: At::Now,
            args: [("freq".to_string(), Value::Hz(100.0))].into(),
            dur: Some(Time::Seconds(200.0)),
            id: None,
            tag: None,
        }],
    )
    .unwrap();
    assert!((pause_at(&mut s, 0, 4_800) - 0.5).abs() < 1e-4);
}

#[test]
fn a_control_kept_to_beats_replays_to_the_same_audio() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.eval(TIMED, Some("now")).unwrap();
    hold_pause(&mut s);
    s.send_events(
        0,
        vec![
            set_pause(100, 0.25, "beats"),
            tempo(10_000, 60.0),
            tempo(20_000, 90.0),
        ],
    )
    .unwrap();
    let live = s.process(30_000).remove(0);
    let replayed = Session::replay(SR, Layout::Mono, s.log(), 30_000, false).remove(0);
    assert_eq!(live, replayed);
    assert!((f64::from(live[29_999]) * 10.0 - 0.1666).abs() < 1e-3);
}

#[test]
fn a_control_can_be_set_in_the_batch_that_defines_it() {
    let mut s = Session::new(SR, Layout::Mono).without_limiter();
    s.send_events(
        0,
        vec![
            Op::Define {
                source: TIMED.into(),
                at: At::Sample(2_000),
                dir: None,
                tag: None,
            },
            set_pause(2_000, 0.25, "beats"),
        ],
    )
    .unwrap();
}
