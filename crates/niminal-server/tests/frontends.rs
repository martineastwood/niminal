#![allow(clippy::unwrap_used)]
use niminal_lang::Layout;
use niminal_server::{EngineServer, PROTOCOL_VERSION, RealtimeConfig, Session};
use serde_json::{Value, json};
fn call(s: &mut EngineServer, c: u64, method: &str, params: Value) -> Value {
    let replies = s.handle(
        c,
        &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
    );
    let response: Value = serde_json::from_str(&replies[0]).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}
fn setup() -> (EngineServer, u64, u64) {
    let mut s = EngineServer::new(Session::new(48000., Layout::Mono).without_limiter());
    let a = s.connect();
    let b = s.connect();
    for c in [a, b] {
        call(&mut s, c, "hello", json!({"protocol":PROTOCOL_VERSION}));
    }
    (s, a, b)
}
fn send(s: &mut EngineServer, c: u64, events: Value) -> Value {
    call(s, c, "events.send", json!({"events":events}))
}
#[test]
fn definitions_and_controls_are_private() {
    let (mut s, a, b) = setup();
    for (c, value) in [(a, 0.25), (b, 0.5)] {
        send(
            &mut s,
            c,
            json!([{"op":"define","at":"now","source":format!("ctl level = {value}\nbuffer data = zeros(8)\ninstr flat() {{ level }}")},{"op":"note","target":"flat","at":"now","id":1}]),
        );
    }
    assert!(s.session_mut().process(32)[0].iter().all(|x| *x == 0.75));
    let status = call(&mut s, a, "status", json!({}));
    assert_eq!(status["instruments"].as_array().unwrap().len(), 1);
    assert_eq!(status["controls"][0]["name"], "level");
    call(&mut s, a, "panic", json!({}));
    assert!(s.session_mut().process(32)[0].iter().all(|x| *x == 0.5));
    send(&mut s, a, json!([{"op":"forget","at":"now"}]));
    // the other client's definitions are untouched by what this one forgot
    let status = call(&mut s, b, "status", json!({}));
    assert_eq!(status["instruments"].as_array().unwrap().len(), 1);
    assert_eq!(status["controls"][0]["name"], "level");
}
#[test]
fn reports_and_lifecycle_use_renderer_samples_and_note_tokens() {
    let (mut s, a, _) = setup();
    call(
        &mut s,
        a,
        "subscribe",
        json!({"topics":["reports","voices"]}),
    );
    let result = send(
        &mut s,
        a,
        json!([{"op":"define","at":"now","source":"instr flat() { [0.25, 0.5].report(\"values\", on: note.start).sum }"},{"op":"note","target":"flat","at":17,"dur":"1ms"}]),
    );
    let token = result["notes"][0]["token"].clone();
    s.session_mut().process(1000);
    let notifications: Vec<Value> = s
        .notifications()
        .into_iter()
        .filter(|(c, _)| *c == a)
        .map(|(_, v)| serde_json::from_str(&v).unwrap())
        .collect();
    let report = notifications
        .iter()
        .find(|v| v["method"] == "report")
        .expect("DSP report");
    assert_eq!(report["params"]["sample"], 17);
    assert_eq!(report["params"]["token"], token);
    assert_eq!(report["params"]["values"], json!([0.25, 0.5]));
    let voices: Vec<_> = notifications
        .iter()
        .filter(|v| v["method"] == "voice")
        .map(|v| &v["params"])
        .collect();
    assert_eq!(voices.len(), 3, "{voices:?}");
    assert_eq!(voices[0]["sample"], 17);
    assert_eq!(voices[1]["sample"], 81);
    assert_eq!(voices[2]["state"], "ended");
}
#[test]
fn running_voice_binds_new_stereo_buffer_in_realtime() {
    let (mut s, a, _) = setup();
    send(
        &mut s,
        a,
        json!([{"op":"define","at":"now","source":"buffer original = zeros(8, channels: 2)\ninstr reader(source: buffer = original) { source.at(0, channel: 2) }"},{"op":"note","target":"reader","at":"now","id":1}]),
    );
    let mut renderer = s.session_mut().start_realtime(RealtimeConfig::default());
    s.session_mut().prepare_realtime();
    renderer.render(32, &mut |_, _| {});
    send(
        &mut s,
        a,
        json!([{"op":"define","at":"now","source":"buffer fresh = zeros(8, channels: 2)"}]),
    );
    send(
        &mut s,
        a,
        json!([{"op":"buffer_write","name":"fresh","at":49,"values":[0.25,0.5]},{"op":"bind_buffer","id":1,"param":"source","buffer":"fresh","at":49}]),
    );
    s.session_mut().prepare_realtime();
    let mut out = Vec::new();
    renderer.render(64, &mut |b, n| out.extend_from_slice(&b[0][..n]));
    assert!(out[..17].iter().all(|v| *v == 0.), "{out:?}");
    assert!(out[17..].iter().all(|v| *v == 0.5), "{out:?}");
}
#[test]
fn fx_arrays_buffers_and_modulation_share_live_controls() {
    let (mut s, a, _) = setup();
    send(
        &mut s,
        a,
        json!([{"op":"transaction","at":"now","source":"buffer first = zeros(8)\nbuffer second = zeros(8)\nctl level = 0.5.smooth(0ms)","edits":[
            {"op":"fx","name":"fx","source":"(weights: [num] = [0.25, 0.5], source: buffer = first, level: num = 1) { out = weights.sum + source[0] + level }"},
            {"op":"node","id":"fx","fx":"fx"},{"op":"connect","id":"out","from":{"kind":"node","id":"fx"},"to":{"kind":"master"}}
        ]}]),
    );
    assert!(s.session_mut().process(16)[0].iter().all(|v| *v == 1.75));
    send(
        &mut s,
        a,
        json!([{"op":"buffer_write","name":"second","at":32,"values":[0.25]},{"op":"routing","at":32,"edits":[
            {"op":"array","id":"fx","param":"weights","values":[0,0]},
            {"op":"bind_buffer","id":"fx","param":"source","buffer":"second"},
            {"op":"map","id":"fx","param":"level","source":{"kind":"control","name":"level"}}
        ]}]),
    );
    let out = s.session_mut().process(32)[0].clone();
    assert!(out[..16].iter().all(|v| *v == 1.75));
    assert!(out[16..].iter().all(|v| *v == 0.75), "{out:?}");
}
#[test]
fn streamed_player_runs_and_seeks_without_loading_a_memory_buffer() {
    let path = std::env::temp_dir().join(format!("niminal-stream-test-{}.wav", std::process::id()));
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..1000 {
        writer.write_sample(i as f32 / 1000.).unwrap();
    }
    writer.finalize().unwrap();
    let (mut s, a, _) = setup();
    send(
        &mut s,
        a,
        json!([{"op":"routing","at":0,"edits":[{"op":"player","id":"file","path":path},{"op":"connect","id":"out","from":{"kind":"node","id":"file"},"to":{"kind":"master"}}]}]),
    );
    let out = s.session_mut().process(32)[0].clone();
    for (i, v) in out.iter().enumerate() {
        assert!((*v - i as f32 / 1000.).abs() < 1e-6, "{out:?}");
    }
    send(
        &mut s,
        a,
        json!([{"op":"routing","at":32,"edits":[{"op":"seek","id":"file","seconds":0.01}]}]),
    );
    let out = s.session_mut().process(32)[0].clone();
    assert!((out[0] - 0.48).abs() < 1e-5, "{out:?}");
    let _ = std::fs::remove_file(path);
}
#[test]
fn client_configs_and_relative_sample_paths_are_independent() {
    let (mut s, a, b) = setup();
    let root = std::env::temp_dir().join(format!("niminal-private-samples-{}", std::process::id()));
    for (c, name, value) in [(a, "one", 0.25), (b, "two", 0.5)] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut wav = hound::WavWriter::create(
            dir.join("same.wav"),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..4800 {
            wav.write_sample(value).unwrap();
        }
        wav.finalize().unwrap();
        send(
            &mut s,
            c,
            json!([{"op":"define","at":"now","dir":dir,"source":"config { channels: mono, tuning: { a4: 432hz } }\nhit = sample(\"same.wav\")"},{"op":"note","target":"hit","at":"now","dur":"10ms"}]),
        );
    }
    let out = s.session_mut().process(32);
    assert!(out[0].iter().all(|v| (*v - 0.75).abs() < 1e-5), "{out:?}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn panic_lifecycle_uses_the_actual_renderer_boundary() {
    for realtime in [false, true] {
        let (mut s, a, b) = setup();
        call(&mut s, a, "subscribe", json!({"topics":["voices"]}));
        for c in [a, b] {
            send(
                &mut s,
                c,
                json!([{"op":"define","at":"now","source":"instr flat() { 0.25 }"},{"op":"note","target":"flat","at":"now","id":1}]),
            );
        }
        let mut renderer =
            realtime.then(|| s.session_mut().start_realtime(RealtimeConfig::default()));
        if let Some(r) = &mut renderer {
            s.session_mut().prepare_realtime();
            r.render(49, &mut |_, _| {});
        } else {
            s.session_mut().process(49);
        }
        call(&mut s, a, "panic", json!({}));
        if let Some(r) = &mut renderer {
            s.session_mut().prepare_realtime();
            r.render(32, &mut |_, _| {});
        } else {
            s.session_mut().process(32);
        }
        let messages: Vec<Value> = s
            .notifications()
            .into_iter()
            .filter(|(c, _)| *c == a)
            .map(|(_, v)| serde_json::from_str(&v).unwrap())
            .collect();
        let ended = messages
            .iter()
            .find(|v| v["method"] == "voice" && v["params"]["state"] == "ended")
            .expect("panic ended receipt");
        assert_eq!(ended["params"]["sample"], 49);
        assert_eq!(ended["params"]["reason"], "panic");
        call(&mut s, b, "subscribe", json!({"topics":["voices"]}));
        call(&mut s, b, "engine.panic", json!({}));
        if let Some(r) = &mut renderer {
            s.session_mut().prepare_realtime();
            r.render(32, &mut |_, _| {});
        } else {
            s.session_mut().process(32);
        }
        let messages: Vec<Value> = s
            .notifications()
            .into_iter()
            .filter(|(c, _)| *c == b)
            .map(|(_, v)| serde_json::from_str(&v).unwrap())
            .collect();
        let ended = messages
            .iter()
            .find(|v| v["method"] == "voice" && v["params"]["state"] == "ended")
            .expect("global panic ended receipt");
        assert_eq!(ended["params"]["sample"], 81);
    }
}
