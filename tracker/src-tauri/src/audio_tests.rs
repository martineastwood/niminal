use super::*;
use crate::pattern::Track;
use niminal_lang::Layout;
use niminal_server::{EngineServer, Session};
use niminal_ws::{Server, ServerConfig};

const TONE: &str = include_str!("../../src/presets/tone.nml");

#[test]
fn real_engine_plays_last_good_source_and_stop_cancels_the_loop() {
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48000.0, Layout::Mono).without_limiter(),
    )));
    let server = Server::start(
        engine.clone(),
        ServerConfig {
            port: 0,
            ..ServerConfig::default()
        },
    )
    .unwrap();
    let mut worker = Worker::default();
    worker
        .command(Command::Connect {
            port: server.port(),
            token: None,
        })
        .unwrap();
    worker
        .command(Command::Apply {
            id: "tone".into(),
            source: TONE.into(),
            kind: Kind::Instr,
        })
        .unwrap();
    let rejected = worker.command(Command::Apply {
        id: "tone".into(),
        source: TONE.replace("saw", "sawz"),
        kind: Kind::Instr,
    });
    assert!(rejected.is_err());
    assert_eq!(worker.sounds["tone"].engine_source, TONE);
    worker
        .command(Command::Audition {
            instrument: "tone".into(),
            note: "C-4".into(),
            track: 0,
        })
        .unwrap();
    let audio = engine.lock().unwrap().session_mut().process(24000);
    assert!(audio[0].iter().any(|sample| sample.abs() > 0.01));

    let pattern = Pattern {
        bpm: 125.0,
        rows_per_beat: 4,
        tracks: (0..4)
            .map(|i| Track {
                name: format!("Track {i}"),
                effect: None,
                muted: false,
                volume_db: 0.0,
                instrument: Some("tone".into()),
                notes: (0..32).map(|_| Some("C-4".into())).collect(),
            })
            .collect(),
    };
    worker
        .command(Command::Play {
            pattern: pattern.clone(),
        })
        .unwrap();
    let audio = engine.lock().unwrap().session_mut().process(10000);
    assert!(audio[0].iter().any(|sample| sample.abs() > 0.01));
    worker.tick().unwrap();
    worker.command(Command::Stop).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(24000);
    assert!(audio[0][2048..].iter().all(|sample| sample.abs() < 0.00001));
    assert!(!worker.status.playing);
    assert!(worker.playback.is_none());

    // Defining another slot must preserve the first one's namespace.
    worker
        .command(Command::Apply {
            id: "second".into(),
            source: TONE.replace("tone(", "other("),
            kind: Kind::Instr,
        })
        .unwrap();
    worker
        .command(Command::Audition {
            instrument: "tone".into(),
            note: "D#4".into(),
            track: 0,
        })
        .unwrap();
    assert!(
        engine.lock().unwrap().session_mut().process(24000)[0]
            .iter()
            .any(|s| s.abs() > 0.01)
    );
    worker.command(Command::Disconnect).unwrap();
    assert!(!worker.status.connected);
    drop(worker);
    server.stop();
}

#[test]
fn track_effects_gain_and_mute_change_rendered_audio_and_reject_bad_fx_edits() {
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48000.0, Layout::Mono).without_limiter(),
    )));
    let server = Server::start(
        engine.clone(),
        ServerConfig {
            port: 0,
            ..ServerConfig::default()
        },
    )
    .unwrap();
    let mut worker = Worker::default();
    worker
        .command(Command::Connect {
            port: server.port(),
            token: None,
        })
        .unwrap();
    // An effect can be applied before any instrument, and may use helpers.
    worker
        .command(Command::Apply {
            id: "half".into(),
            source: "fn half_level(x) { x * 0.5 }\nfx half { out = half_level(it) }".into(),
            kind: Kind::Fx,
        })
        .unwrap();
    worker
        .command(Command::Apply {
            id: "tone".into(),
            source: "instr flat(freq: hz) { 0.1 }".into(),
            kind: Kind::Instr,
        })
        .unwrap();
    let mut pattern = Pattern {
        bpm: 125.0,
        rows_per_beat: 4,
        tracks: (0..4)
            .map(|i| Track {
                name: format!("Track {i}"),
                instrument: (i == 0).then(|| "tone".into()),
                effect: None,
                muted: false,
                volume_db: 0.0,
                notes: vec![None; 32],
            })
            .collect(),
    };
    let render_note = |worker: &mut Worker| {
        worker
            .command(Command::Audition {
                instrument: "tone".into(),
                note: "C-4".into(),
                track: 0,
            })
            .unwrap();
        let audio = engine.lock().unwrap().session_mut().process(16000);
        audio[0][2048..4096].iter().map(|s| s.abs()).sum::<f32>() / 2048.0
    };
    let dry = render_note(&mut worker);
    assert!(dry > 0.09);
    pattern.tracks[0].effect = Some("half".into());
    worker
        .command(Command::Update {
            pattern: pattern.clone(),
        })
        .unwrap();
    let wet = render_note(&mut worker);
    assert!((wet / dry - 0.5).abs() < 0.001, "dry {dry} wet {wet}");
    assert!(
        worker
            .command(Command::Apply {
                id: "half".into(),
                source: "fx half { out = it.no_such_op }".into(),
                kind: Kind::Fx
            })
            .is_err()
    );
    assert!((render_note(&mut worker) / dry - 0.5).abs() < 0.001);

    worker
        .command(Command::Apply {
            id: "half".into(),
            source: "fx half { out = it * 0.25 }".into(),
            kind: Kind::Fx,
        })
        .unwrap();
    assert!((render_note(&mut worker) / dry - 0.25).abs() < 0.001);
    pattern.tracks[0].volume_db = -6.0;
    worker
        .command(Command::Update {
            pattern: pattern.clone(),
        })
        .unwrap();
    assert!((render_note(&mut worker) / dry - 0.25 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    pattern.tracks[0].muted = true;
    worker
        .command(Command::Update {
            pattern: pattern.clone(),
        })
        .unwrap();
    assert_eq!(render_note(&mut worker), 0.0);
    pattern.tracks[0].muted = false;
    pattern.tracks[0].effect = None;
    pattern.tracks[0].volume_db = 0.0;
    worker.command(Command::Update { pattern }).unwrap();
    let bypassed = render_note(&mut worker);
    assert!(
        (bypassed / dry - 1.0).abs() < 0.001,
        "dry={dry}, bypassed={bypassed}"
    );
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}
