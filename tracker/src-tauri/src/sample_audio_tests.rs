use super::*;
use niminal_lang::Layout;
use niminal_server::{EngineServer, Session};
use niminal_ws::{Server, ServerConfig};

#[test]
fn sample_previews_use_the_limiter_respect_regions_and_cancel_repeats() {
    let dir = std::env::temp_dir().join(format!("niminal-preview-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loud.wav");
    let mut writer = hound::WavWriter::create(&path, hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float }).unwrap();
    for i in 0..48000 { writer.write_sample((i as f32 * std::f32::consts::TAU * 220.0 / 48000.0).sin() * 10.0).unwrap(); } writer.finalize().unwrap();
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo))));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default(); worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::PreviewSample { path: path.to_string_lossy().into(), track: None, gain: 1.0, start: 0.1, end: 0.2, repeat: false }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(32000);
    assert!(audio.iter().flatten().all(|v| v.is_finite() && v.abs() <= 0.9661));
    assert!(audio[0][1024..4096].iter().any(|v| v.abs() > 0.5));
    // Leave time for the protected output's DC filter to settle after the loud release.
    assert!(audio[0][28000..].iter().all(|v| v.abs() < 1e-6));
    worker.command(Command::PreviewSample { path: path.to_string_lossy().into(), track: None, gain: 0.02, start: 0.1, end: 0.2, repeat: true }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(24000);
    assert!(audio[0][20000..].iter().any(|v| v.abs() > 0.05));
    worker.command(Command::AuditionStop).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(16000);
    assert!(audio[0][12000..].iter().all(|v| v.abs() < 1e-4));
    worker.command(Command::Apply { id: "half".into(), source: "fx half { out = in * 0.5 }".into(), kind: Kind::Fx, dir: None }).unwrap();
    let mut song = tests::test_song(None);
    for column in song.patterns.iter_mut().flat_map(|p| p.notes.iter_mut().flatten()) { column.inst.clear(); }
    song.tracks[0].fx.push(crate::pattern::Device { id: "half_device".into(), definition: "half".into(), ..Default::default() });
    worker.command(Command::Update { song }).unwrap();
    worker.command(Command::PreviewSample { path: path.to_string_lossy().into(), track: Some(0), gain: 0.02, start: 0.1, end: 0.2, repeat: false }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(16000);
    let peak = audio[0][2048..4096].iter().map(|v| v.abs()).fold(0.0, f32::max);
    let expected = 10.0 * 0.02 * 0.5 / 2.0_f32.sqrt();
    assert!((peak - expected).abs() < 0.003, "preview goes through track effects: {peak}, expected {expected}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn generated_slice_controls_compile_keep_chokes_and_map_all_note_paths() {
    let dir = std::env::temp_dir().join(format!("niminal-pad-audio-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("samples")).unwrap();
    let mut writer = hound::WavWriter::create(dir.join("samples/hits.wav"), hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float }).unwrap();
    // One second per slice: two distinct pitches make remapping and tuning measurable.
    for i in 0..96000 { let frequency = if i < 48000 { 220.0 } else { 440.0 }; writer.write_sample((i as f32 * std::f32::consts::TAU * frequency / 48000.0).sin() * 0.2).unwrap(); } writer.finalize().unwrap();
    let source = include_str!("../../src/test-fixtures/pads.nml");
    let parsed = Sound::parse(Kind::Instr, source.into(), dir.to_str(), 120.0, 4.0).unwrap();
    assert_eq!(parsed.slices, Some(2)); assert_eq!(parsed.pad_map, Some(vec![1, 0]));
    assert!(!parsed.parameters.iter().any(|p| p.name == "pad_key"));
    let compiled = niminal_lang::compile_with(source, &niminal_lang::CompileOptions { samples: niminal_lang::Samples::new(&dir), ..Default::default() }).unwrap();
    let instrument = compiled.instruments.iter().find(|i| i.name == "chops").unwrap();
    assert_eq!(instrument.chokes.len(), 2); assert!(instrument.chokes.iter().all(Option::is_some));
    let engine = Arc::new(Mutex::new(EngineServer::new(Session::new(48000.0, Layout::Stereo))));
    let server = Server::start(engine.clone(), ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let mut worker = Worker::default(); worker.command(Command::Connect { port: server.port(), token: None }).unwrap();
    worker.command(Command::Apply { id: "tone".into(), source: source.into(), kind: Kind::Instr, dir: dir.to_str().map(str::to_owned) }).unwrap();
    tests::build_strips(&mut worker);
    worker.command(Command::Audition { instrument: "tone".into(), note: "C-4".into(), track: 0, ms: 200, commands: vec![] }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(16000);
    let window = &audio[0][2048..6144];
    let crossings = window.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert!((73..=77).contains(&crossings), "mapped second slice, tuned up an octave: {crossings}");
    let peak = window.iter().copied().map(f32::abs).fold(0.0, f32::max);
    let expected = 0.2 * 10.0_f32.powf(-6.0 / 20.0) / 2.0_f32.sqrt();
    assert!((peak - expected).abs() < 0.001, "-6 dB pad gain with mono-to-stereo panning: {peak}, expected {expected}");
    worker.command(Command::MidiOn { key: 60, instrument: "tone".into(), note: "C-4".into(), track: 0, velocity: 127 }).unwrap();
    let audio = engine.lock().unwrap().session_mut().process(16000);
    let crossings = audio[0][2048..6144].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert!((73..=77).contains(&crossings), "MIDI uses the same key map: {crossings}");
    worker.command(Command::MidiOff { key: None }).unwrap();
    let song = tests::test_song(Some("C-4"));
    let mut playback = Playback::new(0, song.clone(), Mode::Pattern, song.patterns[0].id.clone(), 0).unwrap();
    playback.instrument_slices = worker.status.instrument_slices.clone();
    playback.instrument_pad_maps = worker.status.instrument_pad_maps.clone();
    let (events, _) = playback.events_until(0, 12000, 48000.0, &worker.status.instruments).unwrap();
    let note = events.iter().find(|event| event["op"] == "note").unwrap();
    assert_eq!(note["args"]["sample"], 1); assert_eq!(note["args"]["pad_key"], 0);
    let _ = std::fs::remove_dir_all(dir);
}
