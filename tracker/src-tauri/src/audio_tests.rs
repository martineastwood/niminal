use super::*;
use crate::pattern::{Device, FxLane, NoteColumn, Output, Pattern, Track};
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
            notes: (0..4)
                .map(|_| {
                    vec![NoteColumn {
                        notes: vec![note.map(str::to_string); 32],
                        ..NoteColumn::default()
                    }]
                })
                .collect(),
            fx: (0..4).map(|_| vec![FxLane::empty(32)]).collect(),
        }],
        loop_range: None,
        matrix: vec![],
        sequence: vec!["p0".into()],
        master: Track::default(),
        returns: vec![],
        instrument_values: Default::default(),
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
            dir: None,
        })
        .unwrap();
    let rejected = worker.command(Command::Apply {
        id: "tone".into(),
        source: TONE.replace("saw", "sawz"),
        kind: Kind::Instr,
        dir: None,
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
            dir: None,
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
            source: "fn half_level(x) { x * 0.5 }\nfx half { out = half_level(in) }".into(),
            kind: Kind::Fx,
            dir: None,
        })
        .unwrap();
    worker
        .command(Command::Apply {
            id: "tone".into(),
            source: "instr flat(freq: hz) { 0.1 }".into(),
            kind: Kind::Instr,
            dir: None,
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
    song.tracks[0].fx = vec![Device {
        id: "d0".into(),
        definition: "half".into(),
        ..Device::default()
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
                source: "fx half { out = in.no_such_op }".into(),
                kind: Kind::Fx,
                dir: None,
            })
            .is_err()
    );
    assert!((render_note(&mut worker) / dry - 0.5).abs() < 0.001);

    worker
        .command(Command::Apply {
            id: "half".into(),
            source: "fx half { out = in * 0.25 }".into(),
            kind: Kind::Fx,
            dir: None,
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
    song.tracks[0].fx.clear();
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

/// A constant-level mono WAV, so any playback of it is easy to hear in the render.
fn write_wav(path: &std::path::Path, frames: u32, level: i16) {
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend([1, 0, 1, 0]);
    bytes.extend(48000u32.to_le_bytes());
    bytes.extend((48000u32 * 2).to_le_bytes());
    bytes.extend([2, 0, 16, 0]);
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for _ in 0..frames {
        bytes.extend(level.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn the_editor_finds_samples_beside_documents_in_the_project_folder() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let dir = crate::files::project_dir(&base, "song").unwrap();
    let original = base.join("level.wav");
    write_wav(&original, 100, 1000);
    let file = crate::files::import_sample(&dir, &original).unwrap();
    let source = format!(
        "smp_a = sample(\"{file}\").root(c4)\ninstr level(freq: hz) {{ smp_a(freq: freq) }}"
    );
    let diagnostics = |uri: &str| {
        let mut server = niminal_lsp::Server::new();
        let replies = server.handle(&json!({"method":"textDocument/didOpen", "params": {
            "textDocument": {"uri":uri, "text":source, "languageId":"niminal", "version":1}
        }}));
        replies[0]["params"]["diagnostics"].clone()
    };
    assert_eq!(
        diagnostics(&format!("file://{}/level.nml", dir.display())),
        json!([])
    );
    assert_ne!(diagnostics("file:///niminal-tracker/level.nml"), json!([]));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn each_channel_and_the_master_have_a_scope() {
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
            source: "instr flat(freq: hz) { 0.25 }".into(),
            kind: Kind::Instr,
            dir: None,
        })
        .unwrap();
    worker
        .command(Command::Audition {
            instrument: "tone".into(),
            note: "C-4".into(),
            track: 0,
        })
        .unwrap();
    // The engine renders in blocks; scopes reach the client as it renders and the client asks.
    let peak = |worker: &mut Worker, id: &str| {
        for _ in 0..40 {
            engine.lock().unwrap().session_mut().process(512);
            std::thread::sleep(std::time::Duration::from_millis(25));
            worker.pump().unwrap();
            if let Some(frames) = worker
                .scopes
                .get(id)
                .filter(|f| f.iter().any(|v| v.abs() > 0.01))
            {
                return frames.iter().fold(0.0_f32, |p, v| p.max(v.abs()));
            }
        }
        0.0
    };
    assert!(
        (peak(&mut worker, "s_0") - 0.25).abs() < 0.01,
        "the track's own signal"
    );
    assert!(
        (peak(&mut worker, "s_m") - 0.25).abs() < 0.01,
        "and the master's"
    );
    assert_eq!(
        worker.scopes["s_1"]
            .iter()
            .fold(0.0_f32, |p, v| p.max(v.abs())),
        0.0,
        "other tracks are quiet"
    );
    assert_eq!(worker.scopes["s_0"].len(), SCOPE_FRAMES);
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn the_master_has_its_own_level_and_effects_and_solo_silences_other_tracks() {
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
    for (id, source, kind) in [
        ("tone", "instr flat(freq: hz) { 0.1 }", Kind::Instr),
        ("half", "fx half { out = in * 0.5 }", Kind::Fx),
    ] {
        worker
            .command(Command::Apply {
                id: id.into(),
                source: source.into(),
                kind,
                dir: None,
            })
            .unwrap();
    }
    let level = |worker: &mut Worker, track: usize| {
        worker
            .command(Command::Audition {
                instrument: "tone".into(),
                note: "C-4".into(),
                track,
            })
            .unwrap();
        let audio = engine.lock().unwrap().session_mut().process(16000);
        audio[0][2048..4096].iter().map(|s| s.abs()).sum::<f32>() / 2048.0
    };
    let mut song = test_song(None);
    let dry = level(&mut worker, 0);
    assert!(
        dry > 0.09,
        "tracks reach the output through the master: {dry}"
    );

    song.master.volume_db = -6.0;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);

    song.master.volume_db = 0.0;
    song.master.fx = vec![Device {
        id: "m1".into(),
        definition: "half".into(),
        ..Device::default()
    }];
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!(
        (level(&mut worker, 0) / dry - 0.5).abs() < 0.001,
        "the master chain processes every track"
    );
    assert!((level(&mut worker, 1) / dry - 0.5).abs() < 0.001);
    song.master.fx[0].bypass = true;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 1) / dry - 1.0).abs() < 0.001);

    // Soloing one track silences the others; a muted track stays silent even when soloed elsewhere.
    song.tracks[1].solo = true;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert_eq!(level(&mut worker, 0), 0.0);
    assert!(level(&mut worker, 1) > 0.09);
    song.tracks[1].solo = false;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!(level(&mut worker, 0) > 0.09);
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn tracks_send_to_returns_that_have_their_own_effects_level_and_mute() {
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
    for (id, source, kind) in [
        ("tone", "instr flat(freq: hz) { 0.1 }", Kind::Instr),
        ("half", "fx half { out = in * 0.5 }", Kind::Fx),
        ("pass", "fx pass { out = in }", Kind::Fx),
    ] {
        worker
            .command(Command::Apply {
                id: id.into(),
                source: source.into(),
                kind,
                dir: None,
            })
            .unwrap();
    }
    let level = |worker: &mut Worker, track: usize| {
        worker
            .command(Command::Audition {
                instrument: "tone".into(),
                note: "C-4".into(),
                track,
            })
            .unwrap();
        let audio = engine.lock().unwrap().session_mut().process(16000);
        audio[0][2048..4096].iter().map(|s| s.abs()).sum::<f32>() / 2048.0
    };
    let mut song = test_song(None);
    let dry = level(&mut worker, 0);
    let send = |definition: &str| Device {
        id: "s1".into(),
        definition: definition.into(),
        target: Some(0),
        ..Device::default()
    };

    // A send adds the track's signal again through the return.
    song.returns = vec![Track {
        name: "Verb".into(),
        ..Track::default()
    }];
    song.tracks[0].fx = vec![send("pass")];
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 2.0).abs() < 0.001, "dry plus send");
    assert!((level(&mut worker, 1) / dry - 1.0).abs() < 0.001, "other tracks do not send");

    // The return's effect chain processes only what is sent.
    song.returns[0].fx = vec![Device {
        id: "r1".into(),
        definition: "half".into(),
        ..Device::default()
    }];
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 1.5).abs() < 0.001);

    // A send device's own effect sets the send level; return mute and track mute take the send away.
    song.tracks[0].fx = vec![send("half")];
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 1.25).abs() < 0.001);
    song.returns[0].muted = true;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 1.0).abs() < 0.001);
    song.returns[0].muted = false;
    song.tracks[0].muted = true;
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert_eq!(level(&mut worker, 0), 0.0, "a muted track sends nothing");

    // Removing the return silences its sends.
    song.tracks[0].muted = false;
    song.returns.clear();
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 1.0).abs() < 0.001);
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn outputs_route_tracks_into_returns_and_sends_tap_the_chain_where_they_sit() {
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
    for (id, source, kind) in [
        ("tone", "instr flat(freq: hz) { 0.1 }", Kind::Instr),
        ("half", "fx half { out = in * 0.5 }", Kind::Fx),
        ("pass", "fx pass { out = in }", Kind::Fx),
    ] {
        worker
            .command(Command::Apply {
                id: id.into(),
                source: source.into(),
                kind,
                dir: None,
            })
            .unwrap();
    }
    let level = |worker: &mut Worker, track: usize| {
        worker
            .command(Command::Audition {
                instrument: "tone".into(),
                note: "C-4".into(),
                track,
            })
            .unwrap();
        let audio = engine.lock().unwrap().session_mut().process(16000);
        audio[0][2048..4096].iter().map(|s| s.abs()).sum::<f32>() / 2048.0
    };
    let device = |id: &str, definition: &str, target: Option<usize>| Device {
        id: id.into(),
        definition: definition.into(),
        target,
        ..Device::default()
    };
    let mut song = test_song(None);
    let dry = level(&mut worker, 0);
    let ratio = |worker: &mut Worker, song: &Song, track| {
        worker.command(Command::Update { song: song.clone() }).unwrap();
        level(worker, track) / dry
    };

    // A track output into a return is heard only through the return's effects.
    song.returns = vec![Track {
        name: "Drums".into(),
        fx: vec![device("r1", "half", None)],
        ..Track::default()
    }];
    song.tracks[0].output = Output::Return { index: 0 };
    assert!((ratio(&mut worker, &song, 0) - 0.5).abs() < 0.001, "wet only");
    assert!((ratio(&mut worker, &song, 1) - 1.0).abs() < 0.001, "other tracks still reach the master");

    // No output leaves only the sends; a muted track and a missing return are silent.
    song.tracks[0].output = Output::None;
    assert_eq!(ratio(&mut worker, &song, 0), 0.0);
    song.tracks[0].fx = vec![device("s1", "pass", Some(0))];
    assert!((ratio(&mut worker, &song, 0) - 0.5).abs() < 0.001, "send only, through the return's effect");
    song.tracks[0].output = Output::Return { index: 0 };
    song.returns.clear();
    assert!(ratio(&mut worker, &song, 0) < 0.001, "no return, no sound");

    // A pre-fader send takes the chain's signal at its position and leaves the chain as it was.
    let pre = |mut device: Device| {
        device.pre = true;
        device
    };
    song.returns = vec![Track::default()];
    song.tracks[0].output = Output::Master;
    song.tracks[0].fx = vec![device("e1", "half", None), pre(device("s1", "pass", Some(0)))];
    let r = ratio(&mut worker, &song, 0);
    assert!((r - 1.0).abs() < 0.001, "half dry plus half sent: {r}");
    song.tracks[0].fx = vec![pre(device("s1", "pass", Some(0))), device("e1", "half", None)];
    assert!((ratio(&mut worker, &song, 0) - 1.5).abs() < 0.001, "half dry plus all of it sent");

    // The fader scales a post-fader send but not a pre-fader one.
    song.tracks[0].volume_db = -20.0;
    assert!((ratio(&mut worker, &song, 0) - 1.05).abs() < 0.001, "tenth of the dry half, all of the send");
    song.tracks[0].fx = vec![device("s1", "pass", Some(0))];
    assert!((ratio(&mut worker, &song, 0) - 0.2).abs() < 0.001, "post-fader: dry and send both at a tenth");
    song.tracks[0].fx = vec![pre(device("s1", "pass", Some(0)))];
    assert!((ratio(&mut worker, &song, 0) - 1.1).abs() < 0.001, "pre-fader: a tenth of the dry, all of the send");

    // Muting the source leaves only what the sends carry, wherever they sit.
    let mute = |mut device: Device| {
        device.mute_source = true;
        pre(device)
    };
    song.tracks[0].fx = vec![mute(device("s1", "pass", Some(0)))];
    assert!((ratio(&mut worker, &song, 0) - 1.0).abs() < 0.001, "only the send");
    song.tracks[0].fx = vec![device("e1", "half", None), mute(device("s1", "pass", Some(0)))];
    assert!((ratio(&mut worker, &song, 0) - 0.5).abs() < 0.001, "only the send, after the effect");
    song.tracks[0].fx = vec![mute(device("s1", "pass", Some(0))), device("e1", "half", None), pre(device("s2", "pass", Some(0)))];
    assert!((ratio(&mut worker, &song, 0) - 1.0).abs() < 0.001, "nothing after the cut carries sound, not even a later send");
    song.tracks[0].fx[0].mute_source = false;
    assert!((ratio(&mut worker, &song, 0) - 1.55).abs() < 0.001, "taking the cut away restores the chain");
    song.tracks[0].volume_db = 0.0;

    // Re-pointing a send keeps it audible; a bypassed send carries nothing.
    song.returns = vec![Track::default(), Track::default()];
    song.tracks[0].fx = vec![pre(device("s1", "pass", Some(0))), device("e1", "half", None)];
    song.tracks[0].fx[0].target = Some(1);
    assert!((ratio(&mut worker, &song, 0) - 1.5).abs() < 0.001);
    song.tracks[0].fx[0].bypass = true;
    let r = ratio(&mut worker, &song, 0);
    assert!((r - 0.5).abs() < 0.001, "{r}");

    // Returns feed later returns, by output or by their own sends.
    song.tracks[0].fx.clear();
    song.tracks[0].output = Output::Return { index: 0 };
    song.returns[1].fx = vec![device("r1", "half", None)];
    song.returns[0].output = Output::Return { index: 1 };
    assert!((ratio(&mut worker, &song, 0) - 0.5).abs() < 0.001, "track into A into B's effect");
    song.returns[0].output = Output::None;
    song.returns[0].fx = vec![device("r2", "half", None), device("s2", "pass", Some(1))];
    assert!((ratio(&mut worker, &song, 0) - 0.25).abs() < 0.001, "only A's send reaches B");
    assert!(song.validate().is_ok());
    song.returns[1].output = Output::Return { index: 0 };
    assert!(song.validate().is_err(), "a return cannot feed an earlier one");
    song.returns[1].output = Output::Master;
    song.returns[1].fx = vec![device("s3", "pass", Some(1))];
    assert!(song.validate().is_err(), "nor send to itself");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn a_held_note_sounds_until_its_off_and_a_chord_adds_up() {
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
            source: "instr flat(freq: hz) { 0.1 }".into(),
            kind: Kind::Instr,
            dir: None,
        })
        .unwrap();
    // 125 bpm, 4 rows a beat: a row is 5760 samples. Hold two rows, then silence.
    let mut song = test_song(None);
    let held = &mut song.patterns[0].notes[0][0];
    held.notes[0] = Some("C-4".into());
    held.notes[2] = Some("OFF".into());
    let mut second = NoteColumn::blank(32);
    second.notes[0] = Some("E-4".into());
    second.notes[2] = Some("OFF".into());
    song.patterns[0].notes[0].push(second);
    worker
        .command(Command::Play {
            song,
            mode: Mode::Song,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    let audio = engine.lock().unwrap().session_mut().process(48000);
    let level = |range: std::ops::Range<usize>| {
        audio[0][range.clone()].iter().map(|s| s.abs()).sum::<f32>() / range.len() as f32
    };
    let start = audio[0].iter().position(|s| s.abs() > 0.01).expect("sound");
    let sounding = level(start + 500..start + 10000);
    let after = level(start + 13000..start + 20000);
    assert!(
        (sounding - 0.2).abs() < 0.02,
        "two notes of 0.1 add up: {sounding}"
    );
    assert!(after < 0.001, "silent after the OFF: {after}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn instrument_parameters_are_exposed_and_reach_the_notes_that_play() {
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
            source: "instr level(freq: hz, amp: 0..1 = 1, mode: (a | b) = a) { 0.2 * amp }".into(),
            kind: Kind::Instr,
            dir: None,
        })
        .unwrap();
    let parameters = &worker.status.instrument_parameters["tone"];
    assert_eq!(
        parameters
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["amp"],
        "pitch and choices are not controls"
    );
    let play = |worker: &mut Worker, amp: Option<f64>| {
        let mut song = test_song(None);
        song.patterns[0].notes[0][0].notes[0] = Some("C-4".into());
        if let Some(amp) = amp {
            song.instrument_values
                .insert("tone".into(), [("amp".to_string(), amp)].into());
        }
        worker
            .command(Command::Play {
                song,
                mode: Mode::Pattern,
                pattern_id: "p0".into(),
                start_position: 0,
            })
            .unwrap();
        let audio = engine.lock().unwrap().session_mut().process(24000);
        worker.command(Command::Stop).unwrap();
        let start = audio[0].iter().position(|s| s.abs() > 0.01).expect("sound");
        audio[0][start + 500..start + 4000]
            .iter()
            .map(|s| s.abs())
            .sum::<f32>()
            / 3500.0
    };
    let full = play(&mut worker, None);
    let half = play(&mut worker, Some(0.5));
    assert!((full - 0.2).abs() < 0.01, "{full}");
    assert!((half / full - 0.5).abs() < 0.02, "full {full}, half {half}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn imported_samples_play_from_the_project_folder() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-sample-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let dir = crate::files::project_dir(&base, "song").unwrap();
    let original = base.join("level.wav");
    write_wav(&original, 24000, 16384);
    let file = crate::files::import_sample(&dir, &original).unwrap();

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
    // Without the project folder the relative path cannot be found.
    let source = format!(
        "smp_a = sample(\"{file}\").root(c4)\ninstr level(freq: hz) {{ smp_a(freq: freq) }}"
    );
    assert!(
        worker
            .command(Command::Apply {
                id: "level".into(),
                source: source.clone(),
                kind: Kind::Instr,
                dir: None,
            })
            .is_err()
    );
    worker
        .command(Command::Apply {
            id: "level".into(),
            source,
            kind: Kind::Instr,
            dir: Some(dir.to_string_lossy().into_owned()),
        })
        .unwrap();
    worker
        .command(Command::Audition {
            instrument: "level".into(),
            note: "C-4".into(),
            track: 0,
        })
        .unwrap();
    let audio = engine.lock().unwrap().session_mut().process(8000);
    let level = audio[0][2000..6000].iter().map(|s| s.abs()).sum::<f32>() / 4000.0;
    assert!(level > 0.2, "the sample sounds: {level}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn device_chain_runs_in_order_and_can_be_edited_while_playing() {
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
    for (id, source, kind) in [
        ("tone", "instr flat(freq: hz) { 0.1 }", Kind::Instr),
        ("half", "fx half { out = in * 0.5 }", Kind::Fx),
        ("clip", "fx clip { out = in.clamp(-0.05..0.05) }", Kind::Fx),
    ] {
        worker
            .command(Command::Apply {
                id: id.into(),
                source: source.into(),
                kind,
                dir: None,
            })
            .unwrap();
    }
    let device = |id: &str, definition: &str| Device {
        id: id.into(),
        definition: definition.into(),
        ..Device::default()
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
    let mut song = test_song(None);
    let dry = render_note(&mut worker);
    assert!(dry > 0.09);
    let update = |worker: &mut Worker, song: &Song| {
        worker
            .command(Command::Update { song: song.clone() })
            .unwrap();
    };
    // Halve then clip leaves 0.05; clip then halve leaves 0.025, so order is audible.
    song.tracks[0].fx = vec![device("a", "half"), device("b", "clip")];
    update(&mut worker, &song);
    assert!((render_note(&mut worker) / dry - 0.5).abs() < 0.001);
    song.tracks[0].fx.reverse();
    update(&mut worker, &song);
    assert!((render_note(&mut worker) / dry - 0.25).abs() < 0.001);
    // A bypassed device drops out of the chain; clearing the bypass restores it.
    song.tracks[0].fx[0].bypass = true;
    update(&mut worker, &song);
    assert!((render_note(&mut worker) / dry - 0.5).abs() < 0.001);
    song.tracks[0].fx[0].bypass = false;
    update(&mut worker, &song);
    assert!((render_note(&mut worker) / dry - 0.25).abs() < 0.001);
    song.tracks[0].fx.remove(0);
    update(&mut worker, &song);
    assert!((render_note(&mut worker) / dry - 0.5).abs() < 0.001);
    song.tracks[0].fx.clear();
    update(&mut worker, &song);
    assert!((render_note(&mut worker) / dry - 1.0).abs() < 0.001);

    // Adding and removing devices is allowed while the song plays.
    let mut playing = test_song(Some("C-4"));
    worker
        .command(Command::Play {
            song: playing.clone(),
            mode: Mode::Pattern,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    assert!(worker.status.playing);
    playing.tracks[0].fx = vec![device("a", "half")];
    update(&mut worker, &playing);
    playing.tracks[0].fx.clear();
    update(&mut worker, &playing);
    assert!(worker.status.playing);
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
            dir: None,
        })
        .unwrap();
    let mut song = test_song(Some("C-4"));
    // Exercise the last supported track: its mixer route must exist too.
    song.tracks[0].instrument = None;
    song.tracks.resize(16, Track::default());
    song.tracks[15].instrument = Some("tone".into());
    song.patterns[0]
        .notes
        .resize(16, vec![NoteColumn::blank(32)]);
    song.patterns[0].fx.resize(16, vec![FxLane::empty(32)]);
    song.patterns[0].notes[15][0].notes[0] = Some("C-4".into());
    song.patterns[0].rows = 1;
    let mut variation = song.patterns[0].clone();
    variation.id = "p1".into();
    variation.rows = 2;
    variation.notes[15][0].notes[0] = Some("D#4".into());
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
fn track_processor_named_automation_and_bus_lfo_change_rendered_audio() {
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
            dir: None,
        })
        .unwrap();
    worker
        .command(Command::Apply {
            id: "level".into(),
            source: "fx level(amount: 0..1 = 1) { out = in * amount }".into(),
            kind: Kind::Fx,
            dir: None,
        })
        .unwrap();
    assert_eq!(worker.status.effect_parameters["level"][0].name, "amount");
    let mut song = test_song(Some("C-4"));
    song.tracks[0].fx = vec![Device {
        id: "d0".into(),
        definition: "level".into(),
        values: BTreeMap::from([("amount".into(), 0.5)]),
        ..Device::default()
    }];
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
        (peak - 0.125).abs() < 0.001,
        "one native gain processor: {peak}"
    );

    song.tracks[0].fx[0].values.clear();
    song.patterns[0].fx[0][0].target = Some(crate::commands::FxTarget::Effect {
        device: "d0".into(),
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
    song.tracks[0].fx[0].modulations.insert(
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
    // Removing an exposed parameter while playing retains its lane and LFO data,
    // frees the obsolete modulation resources, and keeps scheduling audio.
    worker
        .command(Command::Apply {
            id: "level".into(),
            source: "fx level() { shaped = in * 0.5\n out = shaped * 0.5 }".into(),
            kind: Kind::Fx,
            dir: None,
        })
        .unwrap();
    let retained = worker.playback.as_ref().unwrap().song.clone();
    assert!(matches!(
        retained.patterns[0].fx[0][0].target,
        Some(crate::commands::FxTarget::Effect { .. })
    ));
    worker
        .command(Command::Update {
            song: retained.clone(),
        })
        .unwrap();
    for _ in 0..24 {
        engine.lock().unwrap().session_mut().process(1000);
        worker.tick().unwrap();
    }
    worker.command(Command::Stop).unwrap();
    worker
        .command(Command::Play {
            song: retained,
            mode: Mode::Pattern,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    for _ in 0..24 {
        engine.lock().unwrap().session_mut().process(1000);
        worker.tick().unwrap();
    }
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

/// The definitions in one of the library modules that come with niminal, each of which stands alone.
fn library_definitions(text: &'static str) -> Vec<&'static str> {
    text.split("\n\n")
        .filter(|block| block.lines().any(|l| l.starts_with("fx ") || l.starts_with("instr ")))
        .collect()
}

#[test]
fn library_instruments_expose_controls_and_sound_on_mono_and_stereo_engines() {
    let sources = library_definitions(include_str!("../../../crates/niminal-lang/std/instruments.nml"));
    assert_eq!(sources.len(), 8);
    for layout in [Layout::Mono, Layout::Stereo] {
        let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, layout).without_limiter())));
        let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
        let mut worker = Worker::default();
        worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
        for (index, source) in sources.iter().enumerate() {
            let id = format!("library_instrument_{index}");
            worker.command(Command::Apply { id: id.clone(), source: (*source).into(), kind: Kind::Instr, dir: None }).unwrap();
            assert!(!worker.sounds[&id].parameters.is_empty(), "{id} exposes its controls");
            worker.command(Command::Audition { instrument: id, note: "C-3".into(), track: 0 }).unwrap();
            let audio = engine.lock().unwrap().session_mut().process(24000);
            assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
            assert!(audio.iter().flatten().any(|sample| sample.abs() > 0.001), "silent instrument {index}");
        }
        worker.command(Command::Disconnect).unwrap();
    }
}

#[test]
fn library_effects_expose_controls_and_render_on_mono_and_stereo_engines() {
    let sources = library_definitions(include_str!("../../../crates/niminal-lang/std/fx.nml"));
    assert!(sources.len() >= 14);
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
                dir: None,
            })
            .unwrap();
        for (index, source) in sources.iter().enumerate() {
            let id = format!("library_{index}");
            worker
                .command(Command::Apply {
                    id: id.clone(),
                    source: (*source).into(),
                    kind: Kind::Fx,
                    dir: None,
                })
                .unwrap();
            assert!(worker.sounds[&id].parameters.len() >= 2);
            engine.lock().unwrap().session_mut().process(2048);
            let mut song = test_song(None);
            song.tracks[0].fx = vec![Device {
                id: "d0".into(),
                definition: id,
                ..Device::default()
            }];
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
                "silent effect {index}"
            );
        }
        worker.command(Command::Disconnect).unwrap();
    }
}

#[test]
fn graphical_envelopes_render_native_linear_ramps_at_fractional_rows() {
    use crate::{
        commands::FxTarget,
        pattern::{Envelope, EnvelopePoint},
    };
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
            dir: None,
        })
        .unwrap();
    worker
        .command(Command::Apply {
            id: "gain".into(),
            source: "fx gain(amount: 0..1 = 1) { out = in * amount }".into(),
            kind: Kind::Fx,
            dir: None,
        })
        .unwrap();
    let mut song = test_song(None);
    song.bpm = 20.0;
    song.rows_per_beat = 1;
    song.patterns[0].notes[0][0].notes[0] = Some("C-4".into());
    song.tracks[0].fx = vec![Device {
        id: "d0".into(),
        definition: "gain".into(),
        ..Device::default()
    }];
    let lane = &mut song.patterns[0].fx[0][0];
    lane.target = Some(FxTarget::Effect {
        device: "d0".into(),
        parameter: "amount".into(),
    });
    lane.values[0] = Some("1".into()); // Envelope takes priority over row values.
    lane.envelope = Some(Envelope {
        enabled: true,
        mode: "linear".into(),
        points: vec![
            EnvelopePoint {
                row: 0.0,
                value: 0.0,
            },
            EnvelopePoint {
                row: 0.5,
                value: 1.0,
            },
        ],
    });
    worker
        .command(Command::Play {
            song: song.clone(),
            mode: Mode::Pattern,
            pattern_id: "p0".into(),
            start_position: 0,
        })
        .unwrap();
    let origin = worker.playback.as_ref().unwrap().origin as usize;
    let audio = engine.lock().unwrap().session_mut().process(origin + 80000);
    for channel in audio {
        assert!(channel[origin + 1].abs() < 0.0001);
        assert!(
            (channel[origin + 36000] - 0.125).abs() < 0.001,
            "midpoint {}",
            channel[origin + 36000]
        );
        assert!(
            (channel[origin + 72000] - 0.25).abs() < 0.001,
            "endpoint {}",
            channel[origin + 72000]
        );
        assert!(
            channel[origin + 100..origin + 71000]
                .windows(2)
                .all(|s| (s[1] - s[0]).abs() < 0.0001)
        );
    }
    worker.command(Command::Stop).unwrap();
    worker.command(Command::Disconnect).unwrap();
}
