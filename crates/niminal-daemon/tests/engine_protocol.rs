//! The engine server: JSON-RPC over whatever carries the messages.

use niminal_daemon::{ClientId, EngineServer, PROTOCOL_VERSION, Session};
use niminal_lang::Layout;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const SETUP: &str = "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\ntrack lead { instrument = pluck }";

fn call(d: &mut EngineServer, client: ClientId, id: u64, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let replies = d.handle(client, &request);
    assert_eq!(replies.len(), 1, "one reply: {replies:?}");
    serde_json::from_str(&replies[0]).unwrap()
}

fn ready() -> (EngineServer, ClientId) {
    let mut d = EngineServer::new(Session::new(SR, Layout::Mono).without_limiter());
    let c = d.connect();
    let hello = call(
        &mut d,
        c,
        1,
        "hello",
        json!({"protocol": PROTOCOL_VERSION, "client": "test"}),
    );
    assert_eq!(hello["result"]["protocol"], PROTOCOL_VERSION);
    (d, c)
}

/// Define the setup the way a client does.
fn define(d: &mut EngineServer, c: ClientId, source: &str) {
    let reply = call(
        d,
        c,
        2,
        "events.send",
        json!({"events": [{"op": "define", "at": "now", "source": source}]}),
    );
    assert!(reply.get("error").is_none(), "{reply}");
}

#[test]
fn a_client_must_say_hello_with_the_right_version() {
    let mut d = EngineServer::new(Session::new(SR, Layout::Mono));
    let c = d.connect();
    let early = call(&mut d, c, 1, "status", json!({}));
    assert_eq!(early["error"]["code"], -32000);
    let wrong = call(&mut d, c, 2, "hello", json!({"protocol": 99}));
    assert_eq!(wrong["error"]["code"], -32000);
    let ok = call(&mut d, c, 3, "hello", json!({"protocol": PROTOCOL_VERSION}));
    assert_eq!(ok["result"]["channels"], 1);
    assert_eq!(ok["result"]["topics"], json!(niminal_daemon::ENGINE_TOPICS));
    assert_eq!(
        call(&mut d, c, 4, "nothing.here", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        call(&mut d, c, 5, "events.send", json!({}))["error"]["code"],
        -32602
    );
    // the clients of the sequencer's own methods are not served here
    assert_eq!(
        call(&mut d, c, 6, "eval", json!({"source": "hush"}))["error"]["code"],
        -32601
    );
}

#[test]
fn event_stream_plays_notes_from_outside() {
    let (mut d, c) = ready();
    define(&mut d, c, &SETUP.replace("\\n", "\n"));
    let ok = call(
        &mut d,
        c,
        3,
        "events.send",
        json!({"events": [{"op": "note", "target": "lead", "at": 4800, "dur": "0.2sec", "args": {"freq": "a4"}}]}),
    );
    assert_eq!(ok["result"]["accepted"], 1);
    assert_eq!(ok["result"]["late"], 0);
    assert_eq!(ok["result"]["first"], 4800);
    let out = d.session_mut().process(12_000).remove(0);
    assert_eq!(out[..4799].iter().fold(0.0f32, |m, s| m.max(s.abs())), 0.0);
    assert!(out[4800..8000].iter().fold(0.0f32, |m, s| m.max(s.abs())) > 0.1);

    let bad = call(
        &mut d,
        c,
        4,
        "events.send",
        json!({"events": [
            {"op": "note", "target": "lead", "at": 0, "dur": "1sec", "args": {"freq": "a4"}},
            {"op": "note", "target": "nobody", "at": 0, "dur": "1sec"},
        ]}),
    );
    assert_eq!(bad["error"]["code"], -32001);
    assert_eq!(bad["error"]["data"]["problems"][0]["event"], 1);
    let malformed = call(
        &mut d,
        c,
        5,
        "events.send",
        json!({"events": [{"op": "note", "target": "lead"}]}),
    );
    assert_eq!(malformed["error"]["code"], -32602);
}

#[test]
fn controls_are_typed_timestamped_and_reported_in_status() {
    let (mut d, c) = ready();
    define(&mut d, c, "ctl cutoff = 1khz\nctl volume = -6db");
    let update = call(
        &mut d,
        c,
        3,
        "events.send",
        json!({"events": [{
            "op": "control", "name": "cutoff", "value": 2.0, "unit": "khz", "at": 37, "glide": "0ms"
        }]}),
    );
    assert_eq!(update["result"]["first"], 37);
    d.session_mut().process(64);
    let status = call(&mut d, c, 4, "status", json!({}));
    assert_eq!(status["result"]["controls"][0]["name"], "cutoff");
    assert_eq!(status["result"]["controls"][0]["value"], 2000.0);
    assert_eq!(status["result"]["controls"][0]["unit"], "hz");
    assert!((status["result"]["controls"][1]["value"].as_f64().unwrap() + 6.0).abs() < 1e-5);
    for params in [
        json!({"name": "cutoff", "value": 2.0, "unit": "db"}),
        json!({"name": "missing", "value": 2.0}),
        json!({"name": "cutoff", "value": "2khz"}),
        json!({"name": "cutoff", "value": 2.0, "at": -1}),
        json!({"name": "cutoff", "value": 2.0, "glide": "-1sec"}),
        json!({"name": "cutoff", "value": 2.0, "unit": 7}),
    ] {
        let mut op = params;
        op["op"] = json!("control");
        op.as_object_mut()
            .unwrap()
            .entry("at")
            .or_insert(json!("now"));
        let result = call(&mut d, c, 5, "events.send", json!({"events": [op]}));
        assert!(result.get("error").is_some(), "{result}");
    }
}

#[test]
fn events_cancel_takes_back_waiting_tagged_events() {
    let (mut d, c) = ready();
    define(&mut d, c, &SETUP.replace("\\n", "\n"));
    let note = |tag: &str, at: u64| json!({"op": "note", "target": "lead", "at": at, "dur": "0.1sec", "args": {"freq": "a4"}, "tag": tag});
    let sent = call(
        &mut d,
        c,
        3,
        "events.send",
        json!({"events": [note("a", 48000), note("a", 96000), note("b", 96000)]}),
    );
    assert_eq!(sent["result"]["accepted"], 3);
    let cancelled = call(
        &mut d,
        c,
        4,
        "events.cancel",
        json!({"tag": "a", "from": 72000}),
    );
    assert_eq!(cancelled["result"], json!({"cancelled": 1, "released": 0}));
    let all = call(&mut d, c, 5, "events.cancel", json!({}));
    assert_eq!(all["result"]["cancelled"], 2);
    let bad = call(&mut d, c, 6, "events.cancel", json!({"tag": 7}));
    assert_eq!(bad["error"]["code"], -32602);
}

#[test]
fn clock_and_status_describe_the_engine_and_define_goes_through_events_send() {
    let (mut d, c) = ready();
    define(&mut d, c, &SETUP.replace("\\n", "\n"));
    let clock = call(&mut d, c, 3, "clock", json!({}));
    let result = &clock["result"];
    assert_eq!(result["sample"], 0);
    for field in ["bpm", "beats_per_bar", "bar", "beat"] {
        assert!(result.get(field).is_none());
    }
    assert!(result["wall_micros"].is_null() && result["sample_rate"].is_number());

    let sent = call(
        &mut d,
        c,
        4,
        "events.send",
        json!({"events": [
            {"op": "define", "at": 24000, "source": "instr extra(freq: hz, cut: hz = 1khz) { osc(saw, freq) }"},

        ]}),
    );
    assert_eq!(sent["result"]["accepted"], 1);
    let bad = call(
        &mut d,
        c,
        5,
        "events.send",
        json!({"events": [{"op": "define", "at": "now", "source": "lead.play([c4])"}]}),
    );
    assert_eq!(bad["error"]["code"], -32001);
    assert_eq!(bad["error"]["data"]["problems"][0]["event"], 0);

    d.session_mut().process(96_000);
    let status = call(&mut d, c, 6, "status", json!({}));
    let instruments = status["result"]["instruments"].as_array().unwrap();
    let extra = instruments
        .iter()
        .find(|i| i["name"] == "extra")
        .expect("the defined instrument");
    assert_eq!(extra["params"][1]["name"], "cut");
    assert_eq!(
        call(&mut d, c, 7, "clock", json!({}))["result"]["sample"],
        96_000
    );
}

#[test]
fn local_and_network_clients_keep_their_own_notifications() {
    let (mut server, network) = ready();
    let local = server.connect_local();
    call(
        &mut server,
        local,
        1,
        "hello",
        json!({"protocol": PROTOCOL_VERSION}),
    );
    for client in [local, network] {
        call(
            &mut server,
            client,
            2,
            "subscribe",
            json!({"topics": ["clock"]}),
        );
    }
    let local_messages = server.notifications_for(local);
    assert_eq!(local_messages.len(), 1);
    assert_eq!(local_messages[0].0, local);
    let network_messages = server.notifications();
    assert_eq!(network_messages.len(), 1);
    assert_eq!(network_messages[0].0, network);

    server.session_mut().process(4801);
    let network_messages = server.notifications();
    assert_eq!(network_messages.len(), 1);
    assert_eq!(network_messages[0].0, network);
    let local_messages = server.notifications_for(local);
    assert_eq!(local_messages.len(), 1);
    assert_eq!(local_messages[0].0, local);
    assert!(server.notifications().is_empty());
    assert!(server.notifications_for(local).is_empty());
}

#[test]
fn musical_time_and_composition_are_rejected_at_the_engine_boundary() {
    let (mut d, c) = ready();
    define(&mut d, c, SETUP);
    for at in [
        json!("1beat"),
        json!("2bar"),
        json!("1sec"),
        json!(-1),
        json!(0.5),
    ] {
        let reply = call(
            &mut d,
            c,
            3,
            "events.send",
            json!({"events": [{"op":"note", "target":"lead", "at":at, "dur":"100ms"}]}),
        );
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
    }
    for source in [
        "tempo 90bpm",
        "meter 3/4",
        "clip riff { notes: [c4] }",
        "scene verse { lead: ~ }",
        "arrangement song { sections: [] }",
        "p = [c4 e4]",
        "def riff() { [c4] }",
        "track lead { instrument = pluck\nnotes = it.reverse }",
        "instr x() { quantize = 1bar\n 0 }",
        "config { quantize: { default: 1bar } }",
        "ctl x = midi.cc(7).range(0..1)",
        "instr x() { osc(sine, 100hz) * env[1 | 1beat 0] }",
        "instr x() { clock.beat }",
    ] {
        let reply = call(
            &mut d,
            c,
            4,
            "events.send",
            json!({"events": [{"op":"define", "source":source, "at":"now"}]}),
        );
        assert_eq!(reply["error"]["code"], -32001, "{source}: {reply}");
    }
    for dur in ["1beat", "2bars"] {
        let reply = call(
            &mut d,
            c,
            5,
            "events.send",
            json!({"events": [{"op":"note", "target":"lead", "at":100, "dur":dur}]}),
        );
        assert_eq!(reply["error"]["code"], -32001, "{reply}");
    }
    let reply = call(
        &mut d,
        c,
        6,
        "events.send",
        json!({"events": [{"op":"transport", "at":0, "bpm":90}]}),
    );
    assert_eq!(reply["error"]["code"], -32601);
    assert_eq!(d.session().waiting_changes(), 0);
}

#[test]
fn dsp_clock_time_is_absolute_and_sample_accurate() {
    let (mut d, c) = ready();
    define(
        &mut d,
        c,
        "instr seconds() { tail = 0ms\n clock.time / 1sec }",
    );
    let sent = call(
        &mut d,
        c,
        2,
        "events.send",
        json!({"events": [{"op":"note", "target":"seconds", "at":2400, "dur":"100ms"}]}),
    );
    assert_eq!(sent["result"]["accepted"], 1);
    let audio = d.session_mut().process(6000).remove(0);
    assert!(audio[..2400].iter().all(|v| *v == 0.0));
    for i in [2400, 2401, 3000, 5999] {
        assert!((audio[i] - i as f32 / SR).abs() < 1e-6);
    }
}

#[test]
fn transaction_notifications_are_owned_and_report_cancellation_and_application() {
    let (mut d, c) = ready();
    let other = d.connect();
    call(
        &mut d,
        other,
        1,
        "hello",
        json!({"protocol":PROTOCOL_VERSION}),
    );
    for client in [c, other] {
        call(
            &mut d,
            client,
            2,
            "subscribe",
            json!({"topics":["transactions"]}),
        );
    }
    let result = call(
        &mut d,
        c,
        3,
        "events.send",
        json!({"events":[{"op":"transaction","at":32,"tag":"edit","source":"instr flat() { 1 }","edits":[{"op":"node","id":"voices"}]}]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    let id = result["result"]["transactions"][0]["id"].as_u64().unwrap();
    let queued = d.notifications();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, c);
    call(&mut d, c, 4, "events.cancel", json!({"tag":"edit"}));
    let cancelled = d.notifications();
    assert_eq!(cancelled.len(), 1);
    let message: Value = serde_json::from_str(&cancelled[0].1).unwrap();
    assert_eq!(message["params"]["id"], id);
    assert_eq!(message["params"]["status"], "cancelled");
    let result = call(
        &mut d,
        c,
        5,
        "events.send",
        json!({"events":[{"op":"transaction","at":32,"edits":[{"op":"node","id":"voices"}]}]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    d.notifications();
    d.session_mut().process(33);
    let applied = d.notifications();
    assert_eq!(applied.len(), 1);
    let message: Value = serde_json::from_str(&applied[0].1).unwrap();
    assert_eq!(message["params"]["status"], "applied");
    assert_eq!(message["params"]["applied_at"], 32);
}
