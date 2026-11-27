//! A live session: start it, step it, change it while it runs.
#![allow(clippy::unwrap_used)]

use niminal_web::live::{LiveFailure, LiveSession};

const PIECE: &str = "tempo 120bpm\ninstr tone(freq: hz) { out = osc(sine, freq) * env[0 2ms 1 | 150ms 0] }\nclip c {\n  instrument: tone\n  length: 1bar\n  notes: [a3 ~ c4 ~]\n}\nc.start\n";

fn loudest(session: &LiveSession) -> f32 {
    session.out.iter().flatten().fold(0.0, |p, s| p.max(s.abs()))
}

#[test]
fn a_session_starts_plays_and_hushes() {
    let Ok((mut session, landed)) = LiveSession::start(PIECE, 48_000.0) else {
        panic!("the piece should start");
    };
    assert!(landed.bar >= 1);
    // two seconds in steps of 1024 frames: it makes sound
    let mut peak = 0.0f32;
    for _ in 0..94 {
        session.process(1024);
        peak = peak.max(loudest(&session));
    }
    assert!(peak > 0.05, "silent: {peak}");
    let status: serde_json::Value = serde_json::from_str(&session.status()).unwrap();
    assert_eq!(status["bpm"], 120.0);
    assert_eq!(status["clips"][0]["name"], "c");
    assert_eq!(status["clips"][0]["running"], true);
    // hush: what is sounding dies away and nothing new starts
    session.hush();
    for _ in 0..94 {
        session.process(1024);
    }
    session.process(1024);
    assert!(loudest(&session) < 1e-3, "still sounding after a hush");
}

#[test]
fn code_run_in_a_live_session_changes_it() {
    let Ok((mut session, _)) = LiveSession::start(PIECE, 48_000.0) else {
        panic!("the piece should start");
    };
    for _ in 0..10 {
        session.process(1024);
    }
    // a new clip on the same instrument, started now
    let landed = session.eval("clip d {\n  instrument: tone\n  length: 1bar\n  notes: [e4]\n}\nd.start\n", Some("now")).ok().unwrap();
    assert!(!landed.changes.is_empty());
    let status: serde_json::Value = serde_json::from_str(&session.status()).unwrap();
    assert_eq!(status["clips"].as_array().unwrap().len(), 2);
}

#[test]
fn mistakes_are_reported_and_change_nothing() {
    let Ok((mut session, _)) = LiveSession::start(PIECE, 48_000.0) else {
        panic!("the piece should start");
    };
    let Err(LiveFailure::Snippet(problems)) = session.eval("instr bad() { out = nonsense(1) }\n", Some("now")) else {
        panic!("expected a problem in the snippet");
    };
    assert!(problems[0].span.is_some());
    // and the session carries on
    session.process(1024);
}

#[test]
fn a_piece_with_a_mistake_does_not_start() {
    let Err(LiveFailure::Source(errors)) = LiveSession::start("instr a() { out = nonsense(1) }\n", 48_000.0).map(|_| ()) else {
        panic!("expected located errors");
    };
    assert!(!errors.is_empty());
}

#[test]
fn a_piece_that_uses_the_standard_library_starts() {
    let source = "use std/instruments.{bowed_string}\n\ntempo 96bpm\n\nbowed_string(freq: a3) for 2beats\n";
    let (mut session, _) = LiveSession::start(source, 48_000.0).ok().expect("a piece with a `use` line starts");
    session.process(4800);
    assert!(loudest(&session) > 0.0);
}

/// Every example the Studio offers must start as a live session, not only render.
#[test]
fn the_studio_examples_start() {
    for name in ["functions", "kernels", "acid", "physical_models", "modal", "array_bank"] {
        let path = format!("{}/../../examples/{name}.nml", env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(&path).unwrap();
        if let Err(failure) = LiveSession::start(&source, 48_000.0) {
            let LiveFailure::Source(d) = &failure else { panic!("{name} did not start") };
            panic!("{name} did not start: {:?}", d.first().map(|d| d.message.clone()));
        }
    }
}

#[test]
fn a_control_moves_while_the_piece_plays() {
    let source = "tempo 120bpm\nctl volume = -40db\ninstr tone(freq: hz) { out = osc(sine, freq) * 0.5 * volume }\nclip c {\n  instrument: tone\n  length: 1bar\n  notes: [a3 a3 a3 a3]\n}\nc.start\n";
    let (mut session, _) = LiveSession::start(source, 48_000.0).ok().expect("starts");
    session.process(4800);
    let quiet = loudest(&session);
    session.set_control("volume", -6.0, Some("db")).expect("the control moves");
    session.process(24_000);
    session.process(4800);
    assert!(loudest(&session) > quiet * 4.0, "the control did not change the level");
    assert!(session.set_control("nonsense", 1.0, None).is_err());
}
