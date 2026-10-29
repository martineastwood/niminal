use super::*;
use crate::pattern::{Device, Envelope, EnvelopePoint, FxLane, NoteColumn, Output, Pattern, Track};
use niminal_lang::Layout;
use niminal_server::{EngineServer, Session};
use niminal_ws::{Server, ServerConfig};

const TONE: &str = include_str!("../../src/presets/tone.nml");

/// Gives every pattern parameter lanes for each return the song has now.
fn sync_lanes(song: &mut Song) {
    for pattern in &mut song.patterns {
        pattern.return_fx = vec![vec![FxLane::empty()]; song.returns.len()];
    }
}

/// Gives the engine the strips of a four-track song, as the tracker's first update does before anything sounds.
fn build_strips(worker: &mut Worker) {
    let mut song = test_song(None);
    for column in song.patterns.iter_mut().flat_map(|p| p.notes.iter_mut().flatten()) {
        column.inst.clear();
    }
    worker.command(Command::Update { song }).unwrap();
}

fn test_song(note: Option<&str>) -> Song {
    Song {
        bpm: 125.0,
        rows_per_beat: 4,
        swing: 0.0,
        tracks: (0..4)
            .map(|i| Track {
                name: format!("Track {i}"),
                ..Track::default()
            })
            .collect(),
        patterns: vec![Pattern {
            timing: None,
            swing: None,
            id: "p0".into(),
            name: "First".into(),
            rows: 32,
            // only the first track has an instrument set on its notes
            notes: (0..4)
                .map(|i| {
                    vec![NoteColumn {
                        notes: vec![note.map(str::to_string); 32],
                        inst: if i == 0 { vec![Some("tone".into()); 32] } else { vec![] },
                        ..NoteColumn::default()
                    }]
                })
                .collect(),
            fx: (0..4).map(|_| vec![FxLane::empty()]).collect(),
            return_fx: vec![],
            master_fx: vec![FxLane::empty()],
            commands: vec![],
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
fn idle_engine_loss_is_published_so_the_window_can_reconnect() {
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48000.0, Layout::Mono).without_limiter(),
    )));
    let server = Server::start(engine, ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let audio = Audio::new();
    let (reply, response) = mpsc::channel();
    audio.sender.send((Command::Connect { port: server.port(), token: None }, reply)).unwrap();
    assert!(response.recv_timeout(Duration::from_secs(5)).unwrap().unwrap().connected);
    server.stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while audio.status().connected && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    let status = audio.status();
    audio.quit();
    assert!(!status.connected);
    assert!(status.error.is_some());
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
    build_strips(&mut worker);
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
            ms: 200,
            commands: vec![],
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
    let mut slower = song.clone();
    slower.bpm = 80.0;
    slower.rows_per_beat = 3;
    worker.command(Command::Update { song: slower }).unwrap();
    assert!(worker.status.playing);
    assert_eq!(worker.playback.as_ref().unwrap().song.bpm, 80.0);
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
            ms: 200,
            commands: vec![],
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
    build_strips(&mut worker);
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
            ms: 200,
                commands: vec![],
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
    build_strips(&mut worker);
    // Strip scopes are enabled only while the mixer needs them.
    worker.command(Command::Scopes { strips: true }).unwrap();
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
            ms: 200,
            commands: vec![],
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
    build_strips(&mut worker);
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
            ms: 200,
                commands: vec![],
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
    build_strips(&mut worker);
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
            ms: 200,
                commands: vec![],
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
    sync_lanes(&mut song);
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
    sync_lanes(&mut song);
    worker
        .command(Command::Update { song: song.clone() })
        .unwrap();
    assert!((level(&mut worker, 0) / dry - 1.0).abs() < 0.001);
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn strips_come_and_go_with_the_tracks_and_returns_a_song_has() {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Mono).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: "instr flat(freq: hz) { 0.1 }".into(), kind: Kind::Instr, dir: None }).unwrap();
    worker.command(Command::Apply { id: "half".into(), source: "fx half { out = in * 0.5 }".into(), kind: Kind::Fx, dir: None }).unwrap();
    let level = |worker: &mut Worker, track: usize| -> Result<f32, String> {
        worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track, ms: 200, commands: vec![] })?;
        let audio = engine.lock().unwrap().session_mut().process(16000);
        Ok(audio[0][2048..4096].iter().map(|s| s.abs()).sum::<f32>() / 2048.0)
    };
    let tracks = |count: usize| {
        let mut song = test_song(None);
        song.tracks.truncate(count);
        for pattern in &mut song.patterns {
            pattern.notes.truncate(count);
            pattern.fx.truncate(count);
        }
        for column in song.patterns.iter_mut().flat_map(|p| p.notes.iter_mut().flatten()) {
            column.inst.clear();
        }
        song
    };

    // only the tracks a song has can be heard; one added later is heard, one taken away is gone, and one that comes back is heard again
    worker.command(Command::Update { song: tracks(2) }).unwrap();
    let dry = level(&mut worker, 0).unwrap();
    assert!(dry > 0.01);
    assert!(level(&mut worker, 3).is_err(), "there is no fourth track yet");
    worker.command(Command::Update { song: tracks(4) }).unwrap();
    assert!((level(&mut worker, 3).unwrap() - dry).abs() < 0.001, "a track added after connecting sounds");
    worker.command(Command::Update { song: tracks(2) }).unwrap();
    assert!(level(&mut worker, 3).is_err(), "a track taken away is gone");
    assert!((level(&mut worker, 1).unwrap() - dry).abs() < 0.001, "the others still sound");
    worker.command(Command::Update { song: tracks(4) }).unwrap();
    assert!((level(&mut worker, 3).unwrap() - dry).abs() < 0.001, "a track that comes back sounds");

    // a return added after the track that outputs to it, taken away, and added again
    let mut song = tracks(4);
    song.returns = vec![Track { name: "Drums".into(), fx: vec![Device { id: "r1".into(), definition: "half".into(), ..Device::default() }], ..Track::default() }];
    sync_lanes(&mut song);
    song.tracks[3].output = Output::Return { index: 0 };
    worker.command(Command::Update { song: song.clone() }).unwrap();
    assert!((level(&mut worker, 3).unwrap() / dry - 0.5).abs() < 0.001, "heard through the return");
    let mut without = tracks(4);
    without.tracks[3].output = Output::Master;
    worker.command(Command::Update { song: without }).unwrap();
    assert!((level(&mut worker, 3).unwrap() / dry - 1.0).abs() < 0.001, "back on the master once the return is gone");
    worker.command(Command::Update { song }).unwrap();
    assert!((level(&mut worker, 3).unwrap() / dry - 0.5).abs() < 0.001, "heard through the return again");
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
    build_strips(&mut worker);
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
            ms: 200,
                commands: vec![],
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
    sync_lanes(&mut song);
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
    sync_lanes(&mut song);
    assert!(ratio(&mut worker, &song, 0) < 0.001, "no return, no sound");

    // A pre-fader send takes the chain's signal at its position and leaves the chain as it was.
    let pre = |mut device: Device| {
        device.pre = true;
        device
    };
    song.returns = vec![Track::default()];
    sync_lanes(&mut song);
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
    sync_lanes(&mut song);
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
    second.inst = vec![Some("tone".into()); 32];
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
    build_strips(&mut worker);
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
            ms: 200,
            commands: vec![],
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
    build_strips(&mut worker);
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
            ms: 200,
                commands: vec![],
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
    song.patterns[0].notes[0][0].inst.clear();
    song.tracks.resize(16, Track::default());
    song.patterns[0]
        .notes
        .resize(16, vec![NoteColumn::blank(32)]);
    song.patterns[0].fx.resize(16, vec![FxLane::empty()]);
    song.patterns[0].notes[15][0].notes[0] = Some("C-4".into());
    song.patterns[0].notes[15][0].inst = vec![Some("tone".into()); 32];
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
fn macro_knob_drives_linked_parameters_and_follows_updates() {
    use crate::pattern::{Macro, MacroLink};
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48000.0, Layout::Stereo).without_limiter(),
    )));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply {
        id: "tone".into(),
        source: "instr constant(freq: hz) { [0.25, 0.25].to_layout(stereo) }".into(),
        kind: Kind::Instr,
        dir: None,
    }).unwrap();
    worker.command(Command::Apply {
        id: "level".into(),
        source: "fx level(amount: 0..1 = 1) { out = in * amount }".into(),
        kind: Kind::Fx,
        dir: None,
    }).unwrap();
    let mut song = test_song(Some("C-4"));
    song.tracks[0].fx = vec![Device { id: "d0".into(), definition: "level".into(), ..Device::default() }];
    song.tracks[0].macros = vec![Macro {
        name: "Gain".into(),
        value: 0.5,
        links: vec![MacroLink { device: "d0".into(), parameter: "amount".into(), min: 0.0, max: 1.0 }],
    }];
    let peak_of = |worker: &mut Worker| {
        worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
        let audio = engine.lock().unwrap().session_mut().process(8000);
        audio[0][2000..6000].iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
    };
    worker.command(Command::Update { song: song.clone() }).unwrap();
    let peak = peak_of(&mut worker);
    assert!((peak - 0.125).abs() < 0.001, "the macro sits at half: {peak}");
    song.tracks[0].macros[0].value = 1.0;
    worker.command(Command::Update { song: song.clone() }).unwrap();
    let peak = peak_of(&mut worker);
    assert!((peak - 0.25).abs() < 0.001, "the macro moved to full: {peak}");
    song.tracks[0].macros[0].links[0].min = 1.0;
    song.tracks[0].macros[0].links[0].max = 0.0;
    worker.command(Command::Update { song: song.clone() }).unwrap();
    let peak = peak_of(&mut worker);
    assert!(peak < 0.001, "a backwards link turns it off at full: {peak}");
    song.tracks[0].macros.clear();
    worker.command(Command::Update { song }).unwrap();
    let peak = peak_of(&mut worker);
    assert!((peak - 0.25).abs() < 0.001, "without the macro the parameter holds its own value: {peak}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn language_macro_scaling_reaches_audio_and_survives_effect_replacement() {
    use crate::pattern::{Macro, MacroLink};
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..Default::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: "instr constant(freq: hz) { [0.25, 0.25].to_layout(stereo) }".into(), kind: Kind::Instr, dir: None }).unwrap();
    let source = "fx probe(cutoff: 200hz..4khz = 200hz, level: -60db..0db = 0db) { out = in * (cutoff / 4khz) * level }";
    let apply = || Command::Apply { id: "probe".into(), source: source.into(), kind: Kind::Fx, dir: None };
    worker.command(apply()).unwrap();
    let mut song = test_song(Some("C-4"));
    song.tracks[0].fx = vec![Device { id: "d0".into(), definition: "probe".into(), ..Default::default() }];
    song.tracks[0].macros = vec![Macro { name: "Sweep".into(), value: 0.5, links: vec![
        MacroLink { device: "d0".into(), parameter: "cutoff".into(), min: 0.0, max: 1.0 },
        MacroLink { device: "d0".into(), parameter: "level".into(), min: 0.0, max: 1.0 },
    ] }];
    worker.command(Command::Update { song: song.clone() }).unwrap();
    let expected = 0.25 * (200.0_f32 * 4000.0).sqrt() / 4000.0 * 10.0_f32.powf(-30.0 / 20.0);
    let peak = |worker: &mut Worker| {
        worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
        engine.lock().unwrap().session_mut().process(10000)[0][2000..6000].iter().copied().fold(0.0_f32, f32::max)
    };
    assert!((peak(&mut worker) - expected).abs() < 1e-5);
    worker.command(apply()).unwrap();
    assert!((peak(&mut worker) - expected).abs() < 1e-5, "Apply preserves the macro's mapped values");
    // A held automation lane uses the same evaluator instead of a hidden macro node.
    song.tracks[0].macros[0].value = 0.0;
    song.patterns[0].fx[0][0].target = Some(crate::commands::FxTarget::Macro { index: 0 });
    song.patterns[0].fx[0][0].held(&[(0, 0.5)]);
    worker.command(Command::Play { song, mode: Mode::Pattern, pattern_id: "p0".into(), start_position: 0 }).unwrap();
    let lead = (worker.playback.as_ref().unwrap().origin - worker.clock().unwrap()) as usize;
    let audio = engine.lock().unwrap().session_mut().process(lead + 4000);
    let actual = audio[0][lead + 2000..lead + 4000].iter().copied().fold(0.0_f32, f32::max);
    assert!((actual - expected).abs() < 1e-5, "the lane uses language scaling: {actual} != {expected}");
    worker.command(Command::Disconnect).unwrap();
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
            ms: 200,
            commands: vec![],
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
    song.patterns[0].fx[0][0].held(&[(0, 0.0), (1, 1.0)]);
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
    song.patterns[0].fx[0][0].envelope = None;
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

/// The presets on one shelf of the library that comes with niminal, each of which stands alone.
fn library_definitions(shelf: &str) -> Vec<&'static str> {
    niminal_lang::presets::of_shelf(shelf).map(|p| p.source.as_str()).collect()
}

#[test]
fn library_instruments_expose_controls_and_sound_on_mono_and_stereo_engines() {
    let sources = library_definitions("std/instruments");
    assert_eq!(sources.len(), 8);
    for layout in [Layout::Mono, Layout::Stereo] {
        let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, layout).without_limiter())));
        let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
        let mut worker = Worker::default();
        worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
        build_strips(&mut worker);
        for (index, source) in sources.iter().enumerate() {
            let id = format!("library_instrument_{index}");
            worker.command(Command::Apply { id: id.clone(), source: (*source).into(), kind: Kind::Instr, dir: None }).unwrap();
            assert!(!worker.sounds[&id].parameters.is_empty(), "{id} exposes its controls");
            worker.command(Command::Audition { instrument: id, note: "C-3".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
            let audio = engine.lock().unwrap().session_mut().process(24000);
            assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
            assert!(audio.iter().flatten().any(|sample| sample.abs() > 0.001), "silent instrument {index}");
        }
        worker.command(Command::Disconnect).unwrap();
    }
}

/// The drum presets as the tracker adds them: the preset renamed, with its own copy of the `drum` function.
#[test]
fn library_drum_presets_carry_their_voice_and_sound_on_mono_and_stereo_engines() {
    let presets = library_definitions("std/drums");
    let voice = niminal_lang::presets::of_shelf("std/drums").next().expect("a drum preset").helpers.as_str();
    assert!(voice.contains("fn drum("), "the drum voice comes with the presets");
    assert_eq!(presets.len(), 21);
    for layout in [Layout::Mono, Layout::Stereo] {
        let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, layout).without_limiter())));
        let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
        let mut worker = Worker::default();
        worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
        build_strips(&mut worker);
        for (index, preset) in presets.iter().enumerate() {
            let id = format!("library_drum_{index}");
            let name = preset.lines().find_map(|l| l.strip_prefix("instr ")).and_then(|l| l.split('(').next()).unwrap();
            let source = format!(
                "{}\n{}",
                preset.replacen(&format!("instr {name}"), &format!("instr {id}"), 1).replace("drum(", &format!("{id}_voice(")),
                voice.replacen("fn drum", &format!("fn {id}_voice"), 1)
            );
            worker.command(Command::Apply { id: id.clone(), source, kind: Kind::Instr, dir: None }).unwrap();
            assert!(!worker.sounds[&id].parameters.is_empty(), "{name} exposes its controls");
            worker.command(Command::Audition { instrument: id, note: "C-3".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
            let audio = engine.lock().unwrap().session_mut().process(24000);
            assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
            assert!(audio.iter().flatten().any(|sample| sample.abs() > 0.001), "silent drum {name}");
        }
        worker.command(Command::Disconnect).unwrap();
    }
}

/// The sound of the tone through the library's overdrive with its drive at `drive`.
fn overdriven_loudness(drive: f64) -> f32 {
    let source = library_definitions("std/fx")
        .into_iter()
        .find(|b| b.lines().any(|l| l.starts_with("fx overdrive(")))
        .expect("the overdrive");
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Mono).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: TONE.into(), kind: Kind::Instr, dir: None }).unwrap();
    worker.command(Command::Apply { id: "overdrive".into(), source: source.into(), kind: Kind::Fx, dir: None }).unwrap();
    let mut song = test_song(None);
    song.tracks[0].fx = vec![Device {
        id: "d0".into(),
        definition: "overdrive".into(),
        values: [("drive".to_string(), drive), ("mix".to_string(), 1.0), ("tone".to_string(), 1.0)].into(),
        ..Device::default()
    }];
    worker.command(Command::Update { song }).unwrap();
    worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(24000);
    worker.command(Command::Disconnect).unwrap();
    let samples: Vec<f32> = audio.iter().flatten().copied().collect();
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// How much louder the left side is than the right (as a ratio of their loudness) for a stereo sound
/// that is four times louder on the left, with the library's compressor on its track set to `link`, or
/// with no compressor at all.
fn left_over_right(compressor_link: Option<f64>) -> f32 {
    let source = library_definitions("std/fx")
        .into_iter()
        .find(|b| b.lines().any(|l| l.starts_with("fx compressor(")))
        .expect("the compressor");
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    let tilted = "instr tilted(freq: hz) { [osc(sine, freq) * 0.4, osc(sine, freq) * 0.1] }";
    worker.command(Command::Apply { id: "tilted".into(), source: tilted.into(), kind: Kind::Instr, dir: None }).unwrap();
    worker.command(Command::Apply { id: "comp".into(), source: source.into(), kind: Kind::Fx, dir: None }).unwrap();
    let mut song = test_song(None);
    song.patterns[0].notes[0][0].inst.clear(); // the note is played by audition, not from the pattern
    if let Some(link) = compressor_link {
        song.tracks[0].fx = vec![Device {
            id: "c0".into(),
            definition: "comp".into(),
            values: [("threshold".to_string(), -40.0), ("ratio".to_string(), 8.0), ("link".to_string(), link), ("knee".to_string(), 0.0), ("makeup".to_string(), 0.0)].into(),
            ..Device::default()
        }];
    }
    worker.command(Command::Update { song }).unwrap();
    worker.command(Command::Audition { instrument: "tilted".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(24000);
    worker.command(Command::Disconnect).unwrap();
    let rms = |c: usize| (audio[c][4000..].iter().map(|s| s * s).sum::<f32>() / (audio[c].len() - 4000) as f32).sqrt();
    rms(0) / rms(1)
}

#[test]
fn a_linked_compressor_keeps_the_stereo_image_and_an_unlinked_one_narrows_it() {
    let (plain, linked, unlinked) = (left_over_right(None), left_over_right(Some(1.0)), left_over_right(Some(0.0)));
    assert!(plain > 3.0, "the sound is louder on the left: {plain}");
    assert!((linked / plain - 1.0).abs() < 0.03, "linked: the left stays {plain} times the right, not {linked}");
    assert!(unlinked < plain * 0.8, "unlinked: each side squeezed on its own, so the difference shrinks: {unlinked} against {plain}");
}

#[test]
fn the_overdrive_drive_control_changes_the_sound() {
    let (soft, hard) = (overdriven_loudness(0.0), overdriven_loudness(1.0));
    assert!(soft > 0.001 && hard > 0.001, "both are audible: {soft}, {hard}");
    assert!((hard / soft - 1.0).abs() > 0.1, "drive 0 and drive 1 sound the same: {soft} and {hard}");
}

#[test]
fn library_effects_expose_controls_and_render_on_mono_and_stereo_engines() {
    let sources = library_definitions("std/fx");
    assert!(sources.len() >= 15);
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
            ms: 200,
                    commands: vec![],
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

/// The loudness of a tone through the library's glitch on a track, with `stutter` set to `hold`.
fn glitched_loudness(hold: f64) -> f32 {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: TONE.into(), kind: Kind::Instr, dir: None }).unwrap();
    let source = library_definitions("std/fx").into_iter().find(|s| s.contains("fx glitch(")).unwrap();
    worker.command(Command::Apply { id: "glitch".into(), source: source.into(), kind: Kind::Fx, dir: None }).unwrap();
    let mut song = test_song(None);
    song.tracks[0].fx = vec![Device {
        id: "s0".into(),
        definition: "glitch".into(),
        values: [("stutter".to_string(), hold)].into(),
        ..Device::default()
    }];
    worker.command(Command::Update { song }).unwrap();
    worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(24000);
    worker.command(Command::Disconnect).unwrap();
    (audio[0][4000..].iter().map(|s| s * s).sum::<f32>() / (audio[0].len() - 4000) as f32).sqrt()
}

#[test]
fn the_glitch_stutter_control_freezes_the_sound_on_a_track() {
    let (open, held) = (glitched_loudness(0.0), glitched_loudness(1.0));
    assert!(open > 0.01, "with no hold the tone passes: {open}");
    // held from the start there is nothing yet to repeat, so the effect is silent
    assert!(held < open * 0.1, "holding the first moment repeats silence: {held} against {open}");
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

#[test]
fn parameter_lanes_on_the_master_and_a_return_change_the_sound() {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Mono).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    for (id, source, kind) in [("tone", "instr flat(freq: hz) { 0.1 }", Kind::Instr), ("pass", "fx pass { out = in }", Kind::Fx)] {
        worker.command(Command::Apply { id: id.into(), source: source.into(), kind, dir: None }).unwrap();
    }
    // the level of what track 0 plays, with the lanes `shape` puts in the song
    let mut level = |shape: &dyn Fn(&mut Song)| {
        let mut song = test_song(Some("C-4"));
        song.returns = vec![Track { name: "Verb".into(), ..Track::default() }];
        sync_lanes(&mut song);
        song.patterns[0].return_fx = vec![vec![FxLane::empty()]];
        song.tracks[0].fx = vec![Device { id: "s1".into(), definition: "pass".into(), target: Some(0), ..Device::default() }];
        shape(&mut song);
        worker.command(Command::Play { song, mode: Mode::Song, pattern_id: "p0".into(), start_position: 0 }).unwrap();
        let audio = engine.lock().unwrap().session_mut().process(24000);
        worker.command(Command::Stop).unwrap();
        audio[0][8000..20000].iter().map(|s| s.abs()).sum::<f32>() / 12000.0
    };
    let plain = level(&|_| {});
    // the track's own signal plus the same through the send and the return
    assert!(plain > 0.05, "something sounds: {plain}");
    // a return's volume lane silences what its send adds
    let quiet_return = level(&|song| song.patterns[0].return_fx[0][0].held(&[(0, -60.0)]));
    assert!((quiet_return / plain - 0.5).abs() < 0.01, "the send added half of it, and is now silent: {quiet_return} against {plain}");
    // the master's volume lane scales everything: 6 dB down is about half
    let quiet_master = level(&|song| song.patterns[0].master_fx[0].held(&[(0, -6.0)]));
    assert!((quiet_master / plain - 0.501).abs() < 0.01, "{quiet_master} against {plain}");
    // an envelope on the master moves it as the rows pass
    let swept = level(&|song| {
        let lane = &mut song.patterns[0].master_fx[0];
        lane.envelope = Some(Envelope { enabled: true, mode: "hold".into(), points: vec![EnvelopePoint { row: 0.0, value: -6.0 }] });
    });
    assert!((swept / plain - 0.501).abs() < 0.01, "{swept} against {plain}");
}

#[test]
fn a_cut_silences_a_note_at_once_where_an_off_lets_its_release_ring() {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Mono).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    let ring = "instr ring(freq: hz) { osc(sine, freq) * env[1 | 2sec 0] * 0.3 }";
    worker.command(Command::Apply { id: "tone".into(), source: ring.into(), kind: Kind::Instr, dir: None }).unwrap();
    // 125 bpm, 4 rows a beat: a row is 5760 samples, so the note lasts 11520 samples from a start a little after the Play; look well after the end
    let mut after = |end: &str| {
        let mut song = test_song(None);
        song.patterns[0].notes[0][0].notes[0] = Some("C-4".into());
        song.patterns[0].notes[0][0].notes[2] = Some(end.into());
        worker.command(Command::Play { song, mode: Mode::Song, pattern_id: "p0".into(), start_position: 0 }).unwrap();
        let audio = engine.lock().unwrap().session_mut().process(30000);
        worker.command(Command::Stop).unwrap();
        let during = audio[0][4000..10000].iter().map(|s| s.abs()).sum::<f32>() / 6000.0;
        (during, audio[0][24000..30000].iter().map(|s| s.abs()).sum::<f32>() / 6000.0)
    };
    let (held, released) = after("OFF");
    assert!(held > 0.05, "the note sounds while it is held: {held}");
    assert!(released > 0.05, "an OFF lets the release ring: {released}");
    let (held, cut) = after("CUT");
    assert!(held > 0.05, "the note sounds while it is held: {held}");
    assert!(cut < 0.005, "a CUT stops it with no tail: {cut}");
}

/// The samples of a WAV the engine wrote: 32-bit floats after an 80-byte header.
fn float_wav(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    bytes[80..].chunks(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
}

#[test]
fn a_song_exports_to_a_wav_with_a_stem_for_each_sounding_track() {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo))));
    let server = Server::start(engine, ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: TONE.into(), kind: Kind::Instr, dir: None }).unwrap();
    let mut song = test_song(Some("C-4"));
    song.tracks[1].name = "Lead 2".into();
    song.patterns[0].notes[1][0].notes = vec![Some("E-4".into()); 32];
    song.patterns[0].notes[1][0].inst = vec![Some("tone".into()); 32];
    song.tracks[1].muted = true;
    let dir = std::env::temp_dir().join(format!("niminal-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("song.wav");
    worker.command(Command::Export { song, path: path.to_string_lossy().into(), stems: true }).unwrap();

    // 32 rows at 125 bpm, 4 rows a beat is 3.84 s, and the tail of the last note follows
    let mix = float_wav(&path);
    assert!(mix.len() >= 2 * (3.84 * 48000.0) as usize, "{} samples", mix.len());
    assert!(mix.iter().any(|s| s.abs() > 0.01));
    let stem = float_wav(&dir.join("song-01-Track_0.wav"));
    assert!(stem.iter().any(|s| s.abs() > 0.01));
    assert!(!dir.join("song-02-Lead_2.wav").exists(), "a muted track has no stem");
    assert!(dir.join("song-02-Track_2.wav").exists());
    assert!(!worker.status.playing);
    let _ = std::fs::remove_dir_all(&dir);
}

fn engine_worker() -> (Arc<Mutex<EngineServer>>, Server, Worker) {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Mono).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    (engine, server, worker)
}

/// A song of one pattern with one note column, `rows` rows of 5760 samples.
fn one_column(rows: usize, cells: &[(usize, &str)], commands: &[(usize, &str)]) -> Song {
    let mut song = test_song(None);
    let column = &mut song.patterns[0].notes[0][0];
    column.inst = vec![Some("tone".into()); 32];
    column.fx = vec![vec![None; 32]];
    for (row, note) in cells {
        column.notes[*row] = Some((*note).into());
    }
    // the commands are written as S80, B00, R41, G10, U10 or D10, one to a row
    for (row, text) in commands {
        column.fx[0][*row] = Some((*text).into());
    }
    song.patterns[0].rows = rows;
    song
}

/// The first channel of the next `samples` of engine output, scheduling the song as the engine's clock goes.
fn run(engine: &Arc<Mutex<EngineServer>>, worker: &mut Worker, samples: usize) -> Vec<f32> {
    let mut audio = Vec::new();
    while audio.len() < samples {
        let chunk = engine.lock().unwrap().session_mut().process(2400.min(samples - audio.len()));
        audio.extend(&chunk[0]);
        worker.tick().unwrap();
    }
    audio
}

fn play_song(worker: &mut Worker, song: Song) {
    worker.command(Command::Play { song, mode: Mode::Song, pattern_id: "p0".into(), start_position: 0 }).unwrap();
}

#[test]
fn a_glide_bends_the_sounding_pitch_to_the_new_note() {
    let (engine, server, mut worker) = engine_worker();
    worker.command(Command::Apply { id: "tone".into(), source: "instr pure(freq: hz) { osc(sine, freq) * 0.2 }".into(), kind: Kind::Instr, dir: None }).unwrap();
    // C-4 from row 0, gliding to C-5 over a row from row 4, held to the end
    play_song(&mut worker, one_column(10, &[(0, "C-4"), (4, "C-5")], &[(4, "G10")]));
    let audio = engine.lock().unwrap().session_mut().process(10 * 5760 + 9000);
    let start = audio[0].iter().position(|s| s.abs() > 0.001).unwrap_or(0);
    let crossings = |from: usize, to: usize| audio[0][start + from..start + to].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64;
    let before = crossings(5760, 5760 * 3) / 0.24;
    let after = crossings(5760 * 6, 5760 * 8) / 0.24;
    assert!((before / 261.63 - 1.0).abs() < 0.05, "C-4 rings at first: {before} crossings a second");
    assert!((after / 523.25 - 1.0).abs() < 0.05, "and C-5 once the glide is over: {after}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

/// A mono WAV that ramps from silence to nearly full level over `frames`.
fn write_ramp(path: &std::path::Path, frames: u32) {
    write_wav(path, frames, 0);
    let mut bytes = std::fs::read(path).unwrap();
    for i in 0..frames as usize {
        let value = (i as f32 / frames as f32 * 30000.0) as i16;
        bytes[44 + i * 2..46 + i * 2].copy_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn offset_reverse_and_slices_choose_what_part_of_a_sample_plays() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-slices-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = crate::files::project_dir(&base, "song").unwrap();
    let ramp = base.join("ramp.wav");
    write_ramp(&ramp, 4800);
    let file = crate::files::import_sample(&dir, &ramp).unwrap();
    let (engine, server, mut worker) = engine_worker();
    let apply = |worker: &mut Worker, source: String| {
        worker.command(Command::Apply { id: "tone".into(), source, kind: Kind::Instr, dir: Some(dir.to_string_lossy().into_owned()) })
    };
    // a plain sampler gets offset and reverse through the parameters its wrapper declares
    apply(&mut worker, format!("smp = sample(\"{file}\").root(c4)\ninstr ramp(freq: hz, begin: 0..1 = 0, rate: -4..4 = 1) {{ smp(freq: freq, begin: begin, rate: rate) }}")).unwrap();
    assert!(worker.status.instrument_parameters["tone"].iter().any(|p| p.name == "begin"));
    assert!(worker.status.instrument_slices.is_empty());
    play_song(&mut worker, one_column(12, &[(0, "C-4"), (4, "C-4"), (8, "C-4")], &[(4, "S80"), (8, "B00")]));
    let audio = [run(&engine, &mut worker, 12 * 5760 + 7200)];
    // playback starts 0.15 s after the clock it was asked at
    let from = 7200;
    let first = |row: usize| audio[0][from + row * 5760 + 50..from + row * 5760 + 90].iter().sum::<f32>() / 40.0;
    let (plain, offset, reversed) = (first(0), first(4), first(8));
    assert!(plain < 0.1, "the sample starts quietly: {plain}");
    assert!((offset - 0.45).abs() < 0.1, "S80 starts half way up the ramp: {offset}");
    assert!(reversed > 0.8, "B00 starts at the loud end: {reversed}");
    worker.command(Command::Stop).unwrap();
    // entering a command in the grid plays the note with it
    let heard = |worker: &mut Worker, command: &str| {
        worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![command.into()] }).unwrap();
        let audio = engine.lock().unwrap().session_mut().process(4800);
        let start = audio[0].iter().position(|s| s.abs() > 0.01).unwrap_or(0);
        audio[0][start + 20..start + 60].iter().sum::<f32>() / 40.0
    };
    assert!((heard(&mut worker, "S80") - 0.45).abs() < 0.1);
    assert!(heard(&mut worker, "B00") > 0.8);

    // a sliced sample plays the piece for each key; the ramp's fourth quarter is its loudest
    apply(&mut worker, format!("smp = sample(\"{file}\").root(c4)\nchops = smp.slices(4)\ninstr chop(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1) {{ chops(sample: sample, begin: begin, rate: rate) }}")).unwrap();
    assert_eq!(worker.status.instrument_slices["tone"], 4);
    assert!(worker.status.instrument_parameters["tone"].iter().all(|p| p.name != "sample"));
    play_song(&mut worker, one_column(12, &[(0, "C-4"), (4, "D#4"), (8, "E-4")], &[]));
    let audio = [run(&engine, &mut worker, 12 * 5760 + 7200)];
    let level = |row: usize| audio[0][from + row * 5760 + 300..from + row * 5760 + 340].iter().sum::<f32>() / 40.0;
    assert!(level(0) < 0.4, "C-4 is the first piece: {}", level(0));
    assert!(level(4) > 0.7, "D#4 is the fourth: {}", level(4));
    assert!(audio[0][from + 8 * 5760..from + 9 * 5760].iter().all(|s| s.abs() < 0.0001), "E-4 is past the last piece");
    // and cut at chosen places: the second piece starts three quarters of the way up the ramp
    worker.command(Command::Stop).unwrap();
    apply(&mut worker, format!("smp = sample(\"{file}\").root(c4)\nchops = smp.slices([0, 0.75])\ninstr chop(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1) {{ chops(sample: sample, begin: begin, rate: rate) }}")).unwrap();
    assert_eq!(worker.status.instrument_slices["tone"], 2);
    play_song(&mut worker, one_column(8, &[(0, "C-4"), (4, "C#4")], &[]));
    let audio = [run(&engine, &mut worker, 8 * 5760 + 7200)];
    let level = |row: usize| audio[0][from + row * 5760 + 300..from + row * 5760 + 340].iter().sum::<f32>() / 40.0;
    assert!(level(0) < 0.1, "the first piece starts at the start: {}", level(0));
    assert!(level(4) > 0.7, "the second starts at 75%: {}", level(4));
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn midi_preview_holds_releases_and_keeps_channels_independent_with_velocity() {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Mono).without_limiter())));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: "instr flat(freq: hz, amp: 0..1 = 0.2) { amp * (1 - note.released) }".into(), kind: Kind::Instr, dir: None }).unwrap();
    let mut song = test_song(None);
    song.instrument_values.insert("tone".into(), BTreeMap::from([("amp".into(), 0.1)]));
    worker.command(Command::Update { song }).unwrap();
    let level = || {
        let audio = engine.lock().unwrap().session_mut().process(24000);
        audio[0][22000..].iter().copied().sum::<f32>() / 2000.0
    };
    let on = |key, velocity| Command::MidiOn { key, instrument: "tone".into(), note: "C-4".into(), track: 0, velocity };
    worker.command(on(60, 127)).unwrap(); assert!((level() - 0.1).abs() < 0.001); // still held after 500ms
    worker.command(on(188, 64)).unwrap(); assert!((level() - (0.1 + 0.1 * 64.0 / 127.0)).abs() < 0.001);
    worker.command(Command::MidiOff { key: Some(60) }).unwrap(); assert!((level() - 0.1 * 64.0 / 127.0).abs() < 0.001);
    // A repeated key replaces its own preview; another channel's same pitch is independent.
    worker.command(on(188, 127)).unwrap(); assert!((level() - 0.1).abs() < 0.001);
    worker.command(Command::MidiOff { key: None }).unwrap(); assert!(level().abs() < 0.001);
    worker.command(Command::MidiOff { key: Some(188) }).unwrap(); // late/duplicate note-off is harmless
    assert!(worker.command(on(60, 0)).is_err());
    worker.command(on(60, 127)).unwrap(); worker.command(Command::Stop).unwrap(); assert!(level().abs() < 0.001);
    assert!(worker.midi_keys.is_empty());
    worker.command(Command::Disconnect).unwrap();
}

#[test]
fn an_export_is_as_long_as_the_patterns_own_tempo_makes_it() {
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo))));
    let server = Server::start(engine, ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default();
    worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: TONE.into(), kind: Kind::Instr, dir: None }).unwrap();
    let mut song = test_song(Some("C-4"));
    // 32 rows at 60 bpm, 4 rows a beat is 8 s; the song's own 125 bpm would make it 3.84 s
    song.patterns[0].timing = Some(crate::pattern::Timing { bpm: 60.0, rows_per_beat: 4, beats_per_bar: 4, beat_unit: 4, tempo: vec![] });
    let path = std::env::temp_dir().join(format!("niminal-export-tempo-{}.wav", std::process::id()));
    worker.command(Command::Export { song, path: path.to_string_lossy().into(), stems: false }).unwrap();
    let seconds = float_wav(&path).len() as f64 / 2.0 / 48000.0;
    assert!((8.0..9.5).contains(&seconds), "{seconds} s");
    let _ = std::fs::remove_file(&path);
}


#[test]
fn tracker_glides_and_slides_are_even_in_semitones() {
    for (notes, commands, row, from, to) in [
        (vec![(0, "A-4"), (4, "A-5")], vec![(4, "G10")], 4, 440.0_f32, 880.0_f32),
        (vec![(0, "A-5"), (4, "A-4")], vec![(4, "G10")], 4, 880.0, 440.0),
        (vec![(0, "A-4")], vec![(2, "U10")], 2, 440.0, 880.0),
        (vec![(0, "A-5")], vec![(2, "D10")], 2, 880.0, 440.0),
    ] {
        let (engine, server, mut worker) = engine_worker();
        worker.command(Command::Apply { id: "tone".into(), source: "instr pitch_probe(freq: hz) { freq.num / 2000 }".into(), kind: Kind::Instr, dir: None }).unwrap();
        play_song(&mut worker, one_column(10, &notes, &commands));
        let origin = worker.playback.as_ref().unwrap().origin as usize;
        let audio = run(&engine, &mut worker, origin + 10 * 5760);
        let start = origin + row * 5760;
        let middle = audio[start + 2879] * 2000.0;
        assert!((middle - (from * to).sqrt()).abs() < 0.01, "musical midpoint: {middle}");
        assert!((audio[start + 5759] * 2000.0 - to).abs() < 0.01);
        worker.command(Command::Disconnect).unwrap();
        drop(worker); server.stop();
    }
}

#[test]
fn a_tempo_ramp_times_every_note_of_an_export_by_the_integral_of_the_tempo() {
    let (_engine, server, mut worker) = engine_worker();
    worker.command(Command::Apply { id: "tone".into(), source: "instr click(freq: hz) { osc(sine, freq) * env[1 | 30ms 0] }".into(), kind: Kind::Instr, dir: None }).unwrap();
    // a short note every 8 rows of a 32-row pattern whose tempo falls from 120 to 60 bpm across it, 4 rows a beat
    let mut song = one_column(32, &[(0, "C-5"), (1, "OFF"), (8, "C-5"), (9, "OFF"), (16, "C-5"), (17, "OFF"), (24, "C-5"), (25, "OFF")], &[]);
    let point = |row, value| crate::pattern::EnvelopePoint { row, value };
    song.patterns[0].timing = Some(crate::pattern::Timing { bpm: 120.0, rows_per_beat: 4, beats_per_bar: 4, beat_unit: 4, tempo: vec![point(0.0, 120.0), point(32.0, 60.0)] });
    let path = std::env::temp_dir().join(format!("niminal-export-ramp-{}.wav", std::process::id()));
    worker.command(Command::Export { song, path: path.to_string_lossy().into(), stems: false }).unwrap();
    let audio = float_wav(&path);
    let left: Vec<f32> = audio.iter().step_by(2).copied().collect();
    let mut onsets: Vec<f64> = Vec::new();
    for (i, s) in left.iter().enumerate() {
        if s.abs() > 0.05 && onsets.last().is_none_or(|last| i as f64 / 48000.0 - last > 0.3) {
            onsets.push(i as f64 / 48000.0);
        }
    }
    // seconds from the start to row r: 15 * r * ln(bpm(r) / 120) / (bpm(r) - 120), with bpm(r) = 120 - 60 r / 32
    let at = |r: f64| { let bpm = 120.0 - 60.0 * r / 32.0; if r == 0.0 { 0.0 } else { 15.0 * r * (bpm / 120.0).ln() / (bpm - 120.0) } };
    assert_eq!(onsets.len(), 4, "{onsets:?}");
    for (onset, row) in onsets.iter().zip([0.0, 8.0, 16.0, 24.0]) {
        assert!(((onset - onsets[0]) - at(row)).abs() < 0.01, "row {row}: {} s from the first, expected {}", onset - onsets[0], at(row));
    }
    let _ = std::fs::remove_file(&path);
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn a_lane_on_an_instrument_parameter_changes_the_note_that_is_sounding() {
    let (engine, server, mut worker) = engine_worker();
    worker.command(Command::Apply { id: "tone".into(), source: "instr steady(freq: hz, level: 0..1 = 0.1) { level }".into(), kind: Kind::Instr, dir: None }).unwrap();
    let mut song = one_column(8, &[(0, "C-4")], &[]);
    let lane = &mut song.patterns[0].fx[0][0];
    lane.target = Some(crate::commands::FxTarget::Instrument { instrument: "tone".into(), parameter: "level".into() });
    lane.held(&[(3, 0.5)]);
    play_song(&mut worker, song);
    let audio = run(&engine, &mut worker, 8 * 5760 + 7200);
    let level = |from: usize, to: usize| audio[7200 + from..7200 + to].iter().sum::<f32>() / (to - from) as f32;
    assert!((level(2 * 5760, 3 * 5760 - 200) - 0.1).abs() < 0.01, "before the row: {}", level(2 * 5760, 3 * 5760 - 200));
    assert!((level(3 * 5760 + 200, 6 * 5760) - 0.5).abs() < 0.01, "from the row on: {}", level(3 * 5760 + 200, 6 * 5760));
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
}

#[test]
fn a_sampler_with_a_gate_follows_its_note_and_one_without_plays_out() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = crate::files::project_dir(&base, "song").unwrap();
    let level = base.join("level.wav");
    write_wav(&level, 24000, 16384);
    let file = crate::files::import_sample(&dir, &level).unwrap();
    let (engine, server, mut worker) = engine_worker();
    let source = format!("smp = sample(\"{file}\").root(c4)\ninstr hit(freq: hz, gate: 0..1 = 1, release: 0ms..2sec = 10ms) {{ smp(freq: freq) * (1 - gate + gate * env[1 | release 0]) }}");
    worker.command(Command::Apply { id: "tone".into(), source, kind: Kind::Instr, dir: Some(dir.to_string_lossy().into_owned()) }).unwrap();
    // the note is held two rows (11520 samples) and then ended by an OFF; the sample is half a second (24000 samples) long
    let heard = |worker: &mut Worker, gate: &str| {
        let mut song = one_column(8, &[(0, "C-4"), (2, "OFF")], &[]);
        song.instrument_values.insert("tone".into(), [("gate".to_string(), gate.parse().unwrap())].into());
        play_song(worker, song);
        let audio = run(&engine, worker, 7200 + 8 * 5760);
        worker.command(Command::Stop).unwrap();
        let mean = |from: usize, to: usize| audio[7200 + from..7200 + to].iter().map(|s| s.abs()).sum::<f32>() / (to - from) as f32;
        (mean(5000, 9000), mean(14000, 20000))
    };
    let (during, after) = heard(&mut worker, "1");
    assert!(during > 0.4 && after < 0.01, "gate on: {during} then {after}");
    let (during, after) = heard(&mut worker, "0");
    assert!(during > 0.4 && after > 0.4, "gate off: {during} then {after}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_kit_folder_plays_one_sample_on_each_key() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-kit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = crate::files::project_dir(&base, "song").unwrap();
    let (quiet, loud) = (base.join("quiet.wav"), base.join("Loud one.wav"));
    write_wav(&quiet, 12000, 4000);
    write_wav(&loud, 12000, 20000);
    let (folder, members) = crate::files::import_kit(&dir, &[quiet, loud], "drums").unwrap();
    assert_eq!(members, ["Loud_one", "quiet"]);
    let (engine, server, mut worker) = engine_worker();
    // the code the tracker writes for a kit
    let source = format!("drums_kit = kit(\"{folder}\")\ninstr drums(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1, gate: 0..1 = 0, release: 0ms..2sec = 10ms) {{\n  drums_kit(sample: sample, begin: begin, rate: rate) * (1 - gate + gate * env[1 | release 0])\n}}\n");
    worker.command(Command::Apply { id: "tone".into(), source, kind: Kind::Instr, dir: Some(dir.to_string_lossy().into_owned()) }).unwrap();
    assert_eq!(worker.status.instrument_slices["tone"], 2, "one key for each file in the folder");
    play_song(&mut worker, one_column(12, &[(0, "C-4"), (4, "C#4"), (8, "D-4")], &[]));
    let audio = run(&engine, &mut worker, 7200 + 12 * 5760);
    let level = |row: usize| audio[7200 + row * 5760 + 300..7200 + row * 5760 + 340].iter().sum::<f32>() / 40.0;
    assert!((level(0) - 0.61).abs() < 0.05, "C-4 is the first sample, by file name: {}", level(0));
    assert!((level(4) - 0.12).abs() < 0.05, "C#4 the second: {}", level(4));
    assert!(audio[7200 + 8 * 5760..7200 + 9 * 5760].iter().all(|s| s.abs() < 0.0001), "D-4 is past the last");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_sampler_fitted_to_the_tempo_lasts_as_long_as_the_song_tempo_makes_it() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-fit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = crate::files::project_dir(&base, "song").unwrap();
    let level = base.join("level.wav");
    write_wav(&level, 24000, 16384);
    let file = crate::files::import_sample(&dir, &level).unwrap();
    let (engine, server, mut worker) = engine_worker();
    // the code the tracker writes for a sampler; the song is 125 bpm, so a sample recorded at 250 bpm lasts twice as long
    let source = |from: u32| format!("smp = sample(\"{file}\").root(c4)\ninstr hit(freq: hz, begin: 0..1 = 0, rate: -4..4 = 1, stretch: 0.25..4 = 1, stretch_mode: 0..1 = 0, from_bpm: 0..400 = {from}, gate: 0..1 = 0, release: 0ms..2sec = 10ms) {{\n  smp(freq: freq, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode) * (1 - gate + gate * env[1 | release 0])\n}}\n");
    let lasts = |worker: &mut Worker, from: u32| {
        worker.command(Command::Apply { id: "tone".into(), source: source(from), kind: Kind::Instr, dir: Some(dir.to_string_lossy().into_owned()) }).unwrap();
        play_song(worker, one_column(16, &[(0, "C-4")], &[]));
        let audio = run(&engine, worker, 7200 + 16 * 5760);
        worker.command(Command::Stop).unwrap();
        let mean = |from: usize, to: usize| audio[7200 + from..7200 + to].iter().map(|s| s.abs()).sum::<f32>() / (to - from) as f32;
        (mean(30000, 40000), mean(55000, 65000))
    };
    let (at_three_quarters, at_one_and_a_quarter) = lasts(&mut worker, 0);
    assert!(at_three_quarters < 0.01 && at_one_and_a_quarter < 0.01, "unfitted it plays out in half a second: {at_three_quarters}");
    let (at_three_quarters, at_one_and_a_quarter) = lasts(&mut worker, 250);
    assert!(at_three_quarters > 0.4, "fitted it is stretched to a second: {at_three_quarters}");
    assert!(at_one_and_a_quarter < 0.01, "and then ends: {at_one_and_a_quarter}");
    worker.command(Command::Disconnect).unwrap();
    drop(worker);
    server.stop();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn loop_edits_reach_held_pattern_and_midi_notes_without_retriggering() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-live-loops-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("ramp.wav");
    write_ramp(&file, 4800);
    let (engine, server, mut worker) = engine_worker();
    let apply = |worker: &mut Worker, start: f64, end: f64| {
        worker.command(Command::Apply {
            id: "tone".into(), kind: Kind::Instr, dir: None,
            source: format!("s = sample(\"{}\").root(c4).loop(forward)\ninstr live_loop(freq: hz, loop_start: 0..1 = {start}, loop_end: 0..1 = {end}, loop_crossfade: sec = 1ms) {{ s(freq: freq, loop_start: loop_start, loop_end: loop_end, loop_crossfade: loop_crossfade) * env[1 | 1ms 0] }}", file.display()),
        }).unwrap();
    };
    for midi in [false, true] {
        apply(&mut worker, 0.1, 0.3);
        if midi {
            worker.command(Command::MidiOn { key: 60, instrument: "tone".into(), note: "C-4".into(), track: 0, velocity: 127 }).unwrap();
        } else {
            play_song(&mut worker, one_column(32, &[(0, "C-4")], &[]));
        }
        let before = run(&engine, &mut worker, 14000);
        let level = |audio: &[f32]| audio[audio.len()-1000..].iter().sum::<f32>() / 1000.0;
        assert!(level(&before) < 0.35, "initial loop: {}", level(&before));
        apply(&mut worker, 0.7, 0.9);
        let after = run(&engine, &mut worker, 6000);
        assert!(level(&after) > 0.6, "held note must use edited loop, midi={midi}: {}", level(&after));
        worker.command(Command::Stop).unwrap();
    }
    drop(worker); drop(server); drop(engine);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_one_shot_sampler_played_as_a_held_note_plays_once() {
    let base = std::env::temp_dir().join(format!("niminal-tracker-oneshot-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("hit.wav");
    write_wav(&file, 4800, 16384); // a tenth of a second
    let (engine, server, mut worker) = engine_worker();
    build_strips(&mut worker);
    // as the tracker writes a sampler of a short sample: gate 0, so the note's end changes nothing
    let source = format!("s = sample(\"{}\").root(c4)\ninstr hit(freq: hz, begin: 0..1 = 0, rate: -4..4 = 1, stretch: 0.25..4 = 1, stretch_mode: 0..2 = 0, gate: 0..1 = 0, release: 0ms..2sec = 10ms) {{ s(freq: freq, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode) * (1 - gate + gate * env[1 | release 0]) }}", file.display());
    worker.command(Command::Apply { id: "tone".into(), kind: Kind::Instr, dir: None, source }).unwrap();
    for held in [false, true] {
        if held {
            worker.command(Command::MidiOn { key: 2096, instrument: "tone".into(), note: "C-4".into(), track: 0, velocity: 127 }).unwrap();
        } else {
            worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 300, commands: vec![] }).unwrap();
        }
        let audio = run(&engine, &mut worker, 48000);
        let early = audio[1000..4000].iter().map(|s| s.abs()).sum::<f32>() / 3000.0;
        let late = audio[8000..48000].iter().map(|s| s.abs()).sum::<f32>() / 40000.0;
        assert!(early > 0.2, "it sounds, held={held}: {early}");
        assert!(late < 0.001, "and stops with the sample instead of repeating, held={held}: {late}");
        worker.command(Command::Stop).unwrap();
    }
    // a key pressed, repeated by the keyboard (a second note on for the same key) and let go of while the sample is still playing
    worker.command(Command::MidiOn { key: 2096, instrument: "tone".into(), note: "C-4".into(), track: 0, velocity: 127 }).unwrap();
    let first = run(&engine, &mut worker, 1000);
    worker.command(Command::MidiOff { key: Some(2096) }).unwrap();
    let rest = run(&engine, &mut worker, 47000);
    assert!(first[500..].iter().map(|s| s.abs()).sum::<f32>() / 500.0 > 0.2, "it sounds while the key is down");
    let late = rest[8000..].iter().map(|s| s.abs()).sum::<f32>() / 39000.0;
    assert!(late < 0.001, "letting go early does not make it repeat: {late}");
    drop(worker); drop(server); drop(engine);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn an_effect_can_be_applied_before_any_instrument() {
    let sources = library_definitions("std/fx");
    let (_engine, _server, mut worker) = engine_worker();
    worker
        .command(Command::Apply { id: "first".into(), source: sources[0].into(), kind: Kind::Fx, dir: None })
        .unwrap();
    assert!(worker.status.effects.contains_key("first"));
    assert!(!worker.status.effect_parameters["first"].is_empty());
}
