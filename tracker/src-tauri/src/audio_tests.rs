use super::*;
use crate::pattern::{EffectSlot, FxLane, Pattern, Track};
use niminal_lang::Layout;
use niminal_server::{EngineServer, Session};
use niminal_ws::{Server, ServerConfig};

const TONE: &str = include_str!("../../src/presets/tone.nml");

fn test_song(note: Option<&str>) -> Song {
    Song {
        bpm: 125.0,
        rows_per_beat: 4,
        tracks: (0..4)
            .map(|i| Track {
                name: format!("Track {i}"),
                instrument: (i == 0).then(|| "tone".into()),
                ..Track::default()
            })
            .collect(),
        patterns: vec![Pattern {
            id: "p0".into(),
            name: "First".into(),
            rows: 32,
            notes: (0..4).map(|_| vec![note.map(str::to_string); 32]).collect(),
            fx: (0..4).map(|_| vec![FxLane::empty(32)]).collect(),
        }],
        sequence: vec!["p0".into()],
    }
}

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

    let song = test_song(Some("C-4"));
    worker
        .command(Command::Play {
            song: song.clone(),
            mode: Mode::Pattern,
            pattern_id: "p0".into(),
            start_position: 0,
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
    let mut song = test_song(None);
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
    song.tracks[0].effects = vec![EffectSlot {
        definition: "half".into(),
        ..EffectSlot::default()
    }];
    worker
        .command(Command::Update { song: song.clone() })
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
    song.tracks[0].volume_db = -6.0;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((render_note(&mut worker) / dry - 0.25 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    song.tracks[0].muted = true;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert_eq!(render_note(&mut worker), 0.0);
    song.tracks[0].muted = false;
    song.tracks[0].effects.clear();
    song.tracks[0].volume_db = 0.0;
    worker.command(Command::Update { song }).unwrap();
    let bypassed = render_note(&mut worker);
    assert!(
        (bypassed / dry - 1.0).abs() < 0.001,
        "dry={dry}, bypassed={bypassed}"
    );
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn song_playback_completes_and_leaves_the_final_release_audible() {
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
            source: format!("\u{a0}\u{a0}{TONE}"),
            kind: Kind::Instr,
        })
        .unwrap();
    let mut song = test_song(Some("C-4"));
    // Exercise the last supported track: its mixer route must exist too.
    song.tracks[0].instrument = None;
    song.tracks.resize(16, Track::default());
    song.tracks[15].instrument = Some("tone".into());
    song.patterns[0].notes.resize(16, vec![None; 32]);
    song.patterns[0].fx.resize(16, vec![FxLane::empty(32)]);
    song.patterns[0].notes[15][0] = Some("C-4".into());
    song.patterns[0].rows = 1;
    let mut variation = song.patterns[0].clone();
    variation.id = "p1".into();
    variation.rows = 2;
    variation.notes[15][0] = Some("D#4".into());
    song.patterns.push(variation);
    song.sequence = vec!["p0".into(), "p1".into(), "p0".into()];
    worker
        .command(Command::Play {
            song: song.clone(),
            mode: Mode::Song,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    let mut timeline_change = song;
    timeline_change.sequence.reverse();
    // A different order cannot retime events that have already been scheduled.
    timeline_change.sequence = vec!["p1".into(), "p0".into()];
    assert!(
        worker
            .command(Command::Update {
                song: timeline_change
            })
            .is_err()
    );
    let mut heard = false;
    for _ in 0..36 {
        heard |= engine.lock().unwrap().session_mut().process(1000)[0]
            .iter()
            .any(|s| s.abs() > 0.01);
        worker.tick().unwrap();
        if !worker.status.playing {
            break;
        }
    }
    assert!(heard);
    assert!(!worker.status.playing);
    assert!(worker.playback.is_none());
    // Natural completion must not panic away the final instrument's release.
    assert!(
        engine.lock().unwrap().session_mut().process(3000)[0]
            .iter()
            .any(|s| s.abs() > 0.001)
    );
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn native_effect_chain_parameter_commands_and_bus_lfo_change_rendered_audio() {
    use crate::pattern::Lfo;
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48000.0, Layout::Stereo).without_limiter(),
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
            source: "instr constant(freq: hz) { [0.25, 0.25].to_layout(stereo) }".into(),
            kind: Kind::Instr,
        })
        .unwrap();
    worker
        .command(Command::Apply {
            id: "level".into(),
            source: "fx level(amount: 0..1 = 1) { out = it * amount }".into(),
            kind: Kind::Fx,
        })
        .unwrap();
    assert_eq!(worker.status.effect_parameters["level"][0].name, "amount");
    let mut song = test_song(Some("C-4"));
    song.tracks[0].effects = vec![
        EffectSlot {
            definition: "level".into(),
            values: BTreeMap::from([("amount".into(), 0.5)]),
            ..EffectSlot::default()
        };
        2
    ];
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    worker
        .command(Command::Audition {
            instrument: "tone".into(),
            note: "C-4".into(),
            track: 0,
        })
        .unwrap();
    let audio = engine.lock().unwrap().session_mut().process(8000);
    let peak = audio[0][2000..6000]
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        (peak - 0.0625).abs() < 0.001,
        "two native gain effects: {peak}"
    );
    song.tracks[0].effects.truncate(1);
    song.tracks[0].effects[0].values.clear();
    song.patterns[0].fx[0][0].target = Some(crate::commands::FxTarget::Effect {
        device: 0,
        parameter: "amount".into(),
    });
    song.patterns[0].fx[0][0].values[0] = Some("0".into());
    song.patterns[0].fx[0][0].values[1] = Some("1".into());
    worker
        .command(Command::Play {
            song: song.clone(),
            mode: Mode::Pattern,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    let origin = worker.playback.as_ref().unwrap().origin;
    let start = engine.lock().unwrap().session_mut().clock();
    let mut rendered = Vec::new();
    for _ in 0..24 {
        rendered.extend(engine.lock().unwrap().session_mut().process(1000).remove(0));
        worker.tick().unwrap();
    }
    let offset = (origin - start) as usize;
    assert!(
        rendered[offset + 2000..offset + 4000]
            .iter()
            .all(|sample| sample.abs() < 0.0001)
    );
    assert!(
        rendered[offset + 7760..offset + 9000]
            .iter()
            .any(|sample| sample.abs() > 0.2)
    );
    worker.command(Command::Stop).unwrap();
    song.patterns[0].fx[0][0].values.fill(None);
    song.tracks[0].effects[0].modulations.insert(
        "amount".into(),
        Lfo {
            enabled: true,
            shape: "square".into(),
            period: 2.0,
            min: 0.0,
            max: 1.0,
        },
    );
    worker
        .command(Command::Play {
            song,
            mode: Mode::Pattern,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    let origin = worker.playback.as_ref().unwrap().origin;
    let start = engine.lock().unwrap().session_mut().clock();
    let mut rendered = Vec::new();
    for _ in 0..24 {
        rendered.extend(engine.lock().unwrap().session_mut().process(1000).remove(0));
        worker.tick().unwrap();
    }
    let offset = (origin - start) as usize;
    let a = rendered[offset + 2000..offset + 4000]
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let b = rendered[offset + 7760..offset + 9000]
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        a.min(b) < 0.0001 && a.max(b) > 0.2,
        "native bus LFO: {a}, {b}"
    );
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn library_effects_expose_controls_and_render_on_mono_and_stereo_engines() {
    let sources = [
        include_str!("../../src/presets/delay.nml"),
        include_str!("../../src/presets/chorus.nml"),
        include_str!("../../src/presets/lowpass.nml"),
        include_str!("../../src/presets/highpass.nml"),
        include_str!("../../src/presets/bandpass.nml"),
        include_str!("../../src/presets/notch.nml"),
    ];
    for layout in [Layout::Mono, Layout::Stereo] {
        let engine = Arc::new(Mutex::new(EngineServer::new(
            Session::new(48000.0, layout).without_limiter(),
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
        for (index, source) in sources.iter().enumerate() {
            let id = format!("library_{index}");
            worker
                .command(Command::Apply {
                    id: id.clone(),
                    source: (*source).into(),
                    kind: Kind::Fx,
                })
                .unwrap();
            assert!(worker.sounds[&id].parameters.len() >= 3);
            engine.lock().unwrap().session_mut().process(2048);
            let mut song = test_song(None);
            song.tracks[0].effects.push(EffectSlot {
                definition: id,
                ..EffectSlot::default()
            });
            worker.command(Command::Update { song }).unwrap();
            worker
                .command(Command::Audition {
                    instrument: "tone".into(),
                    note: "C-4".into(),
                    track: 0,
                })
                .unwrap();
            let audio = engine.lock().unwrap().session_mut().process(24000);
            assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
            assert!(
                audio.iter().flatten().any(|sample| sample.abs() > 0.001),
                "silent preset {index}"
            );
        }
        worker.command(Command::Disconnect).unwrap();
    }
}
