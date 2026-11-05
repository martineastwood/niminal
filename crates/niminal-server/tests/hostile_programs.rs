//! Programs written by a user, not the shelf: whatever they do, what a default session plays is finite and no
//! louder than the limiter's ceiling, and a long run of redefinitions, tempo changes and panics keeps it so.
#![allow(clippy::unwrap_used)]

use niminal_engine::DEFAULT_CEILING;
use niminal_lang::Layout;
use niminal_score::Time;
use niminal_server::{At, Op, Session};

fn define(source: &str) -> Op {
    Op::Define {
        source: source.into(),
        at: At::Now,
        dir: None,
        tag: None,
    }
}

fn note(target: &str) -> Op {
    Op::Note {
        target: target.into(),
        to: None,
        at: At::Now,
        args: Default::default(),
        dur: Some(Time::Seconds(1.0)),
        id: None,
        tag: None,
    }
}

fn tempo(bpm: f64) -> Op {
    Op::Tempo {
        at: At::Now,
        bpm,
        meter: None,
        beat: None,
        tag: None,
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |a, b| f32::max(a, b.abs()))
}

/// Why `audio` is not safe to send to a speaker, if it is not.
fn unsafe_audio(audio: &[f32]) -> Option<String> {
    if audio.iter().any(|v| !v.is_finite()) {
        return Some("not finite".into());
    }
    (peak(audio) > DEFAULT_CEILING + 1e-4)
        .then(|| format!("peak {} is over the ceiling", peak(audio)))
}

/// Effect bodies a user could write, each one hostile in a different way.
const HOSTILE_EFFECTS: [(&str, &str); 14] = [
    ("runaway feedback", "in.delay(10ms, feedback: 5, max: 1sec)"),
    (
        "negative delay time",
        "in + in.delay(0sec - 1sec, max: 1sec)",
    ),
    (
        "delay far past its maximum",
        "in.delay(1000000sec, feedback: 0.9, max: 1sec)",
    ),
    ("division by zero", "in / 0"),
    ("division by silence", "1 / (in - in)"),
    ("root of a negative", "pow(0 - 2, 0.5) + sqrt(0 - in)"),
    ("log of nothing", "log(in - in) * in"),
    ("huge gain", "in * 1000000000"),
    (
        "ladder at extremes",
        "in.ladder(10hz, res: 100, drive: 1000000)",
    ),
    (
        "scatter at extremes",
        "scatter(in, 1, 1sec, slices: 1000000, seed: 0 - 1000000, reverse: 5, repeat: 5, drop: 0 - 5, detune: 1000, blend: 0sec, max: 2sec)",
    ),
    (
        "scatter with no length",
        "scatter(in, 1, 0sec, slices: 0, blend: 1sec, max: 2sec)",
    ),
    (
        "freeze at extremes",
        "freeze(in, 1, length: 0sec, grain: 0sec, density: 1000000, position: 9, spread: 9, pitch: 1000, detune: 1000, blend: 0sec, max: 2sec)",
    ),
    (
        "freeze with a huge grain",
        "freeze(in, 1, length: 100sec, grain: 100sec, density: 0, max: 2sec)",
    ),
    (
        "gate and feedback together",
        "(in + in.delay(5ms, feedback: 3, max: 1sec)) * 1000000",
    ),
];

fn effect_program(body: &str) -> String {
    format!(
        "fx hostile {{ out = {body} }}\ninstr src() {{ osc(saw, 220hz) * 0.4 + noise() * 0.2 }}\nnode n {{ instrument = src\n output = hostile }}"
    )
}

#[test]
fn hostile_effects_cannot_be_heard_as_anything_but_finite_and_limited() {
    let (mut failures, mut played) = (Vec::new(), 0);
    for (label, body) in HOSTILE_EFFECTS {
        let mut session = Session::new(48_000.0, Layout::Mono);
        let sent = session.send_events(1, vec![define(&effect_program(body)), note("n")]);
        if let Err(e) = sent {
            // refusing out loud is allowed, but the body must be one we meant to be playable
            failures.push(format!("{label}: refused: {e:?}"));
            continue;
        }
        played += 1;
        for _ in 0..4 {
            let audio = session.process(12_000).remove(0);
            if let Some(why) = unsafe_audio(&audio) {
                failures.push(format!("{label}: {why}"));
                break;
            }
        }
    }
    eprintln!("{played} hostile effects played");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A small deterministic generator, so a failure can be run again.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n.max(1) as u64) as usize
    }
}

/// Redefining effects (good and hostile) under playing notes, moving the tempo through silly values, and
/// panicking and forgetting, over and over: the output never leaves the safe range.
#[test]
fn a_long_session_of_redefinitions_and_tempo_changes_stays_finite_and_limited() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut session = Session::new(48_000.0, Layout::Mono);
    let tempos = [120.0, 20.0, 999.0, 1e-9, 1e9, 0.0, -90.0, 60.0];
    let cases: u64 = std::env::var("NIMINAL_SOAK_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let mut failures = Vec::new();
    let mut active = String::new();
    for round in 0..cases {
        let mut events = Vec::new();
        let mut what = String::new();
        match rng.below(6) {
            0 | 1 => {
                let body = if rng.below(2) == 0 {
                    "in * 0.5".to_string()
                } else {
                    HOSTILE_EFFECTS[rng.below(HOSTILE_EFFECTS.len())]
                        .1
                        .to_string()
                };
                what = body.clone();
                active = body.clone();
                events.push(define(&effect_program(&body)));
            }
            2 => {
                let bpm = tempos[rng.below(tempos.len())];
                what = format!("tempo {bpm}");
                events.push(tempo(bpm));
            }
            3 => events.push(Op::Panic {
                at: At::Now,
                tag: None,
            }),
            4 if rng.below(8) == 0 => events.push(Op::Forget {
                at: At::Now,
                tag: None,
            }),
            _ => {}
        }
        events.push(note("n"));
        // a refused definition or tempo is fine; what is played afterwards must still be safe
        let _ = session.send_events(1, events);
        let audio = session.process(2_400).remove(0);
        if let Some(why) = unsafe_audio(&audio) {
            failures.push(format!("round {round} ({what}; fx {active}): {why}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
