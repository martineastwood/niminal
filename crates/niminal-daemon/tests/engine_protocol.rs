//! The engine server: JSON-RPC over whatever carries the messages.

use niminal_daemon::{ClientId, EngineServer, PROTOCOL_VERSION, Session};
use niminal_lang::Layout;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const SETUP: &str = "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\ntrack lead { instrument = pluck }";

fn call(d: &mut EngineServer, client: ClientId, id: u64, method: &str, params: Value) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let replies = d.handle(client, &request);
    assert_eq!(replies.len(), 1, "one reply: {replies:?}");
    serde_json::from_str(&replies[0]).unwrap()
}

fn ready() -> (EngineServer, ClientId) {
    let mut d = EngineServer::new(Session::new(SR, Layout::Mono).without_limiter());
    let c = d.connect();
    let hello = call(&mut d, c, 1, "hello", json!({"protocol": PROTOCOL_VERSION, "client": "test"}));
    assert_eq!(hello["result"]["protocol"], PROTOCOL_VERSION);
    (d, c)
}

/// Define the setup the way a client does.
fn define(d: &mut EngineServer, c: ClientId, source: &str) {
    let reply = call(d, c, 2, "events.send", json!({"events": [{"op": "define", "at": "now", "source": source}]}));
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
    assert_eq!(ok["result"]["topics"], json!(["transport", "meters", "notices"]));
    assert_eq!(call(&mut d, c, 4, "nothing.here", json!({}))["error"]["code"], -32601);
    assert_eq!(call(&mut d, c, 5, "events.send", json!({}))["error"]["code"], -32602);
    // the clients of the sequencer's own methods are not served here
    assert_eq!(call(&mut d, c, 6, "eval", json!({"source": "hush"}))["error"]["code"], -32601);
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
        json!({"events": [{"op": "note", "target": "lead", "at": "0.1sec", "dur": "0.2sec", "args": {"freq": "a4"}}]}),
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
            {"op": "note", "target": "lead", "at": "0sec", "dur": "1sec", "args": {"freq": "a4"}},
            {"op": "note", "target": "nobody", "at": "0sec", "dur": "1sec"},
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
        op.as_object_mut().unwrap().entry("at").or_insert(json!("now"));
        let result = call(&mut d, c, 5, "events.send", json!({"events": [op]}));
        assert!(result.get("error").is_some(), "{result}");
    }
}


#[test]
fn events_cancel_takes_back_waiting_tagged_events() {
    let (mut d, c) = ready();
    define(&mut d, c, &SETUP.replace("\\n", "\n"));
    let note = |tag: &str, at: &str| json!({"op": "note", "target": "lead", "at": at, "dur": "0.1sec", "args": {"freq": "a4"}, "tag": tag});
    let sent = call(&mut d, c, 3, "events.send", json!({"events": [note("a", "1sec"), note("a", "2sec"), note("b", "2sec")]}));
    assert_eq!(sent["result"]["accepted"], 3);
    let cancelled = call(&mut d, c, 4, "events.cancel", json!({"tag": "a", "from": "1.5sec"}));
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
    assert_eq!((result["bpm"].as_f64(), result["beats_per_bar"].as_f64(), result["sample"].as_u64()), (Some(120.0), Some(4.0), Some(0)));
    assert!(result["wall_micros"].is_null() && result["sample_rate"].is_number());

    let sent = call(&mut d, c, 4, "events.send", json!({"events": [
        {"op": "define", "at": "1beat", "source": "instr extra(freq: hz, cut: hz = 1khz) { osc(saw, freq) }"},
        {"op": "transport", "at": "2beat", "bpm": 90},
    ]}));
    assert_eq!(sent["result"]["accepted"], 2);
    let bad = call(&mut d, c, 5, "events.send", json!({"events": [{"op": "define", "at": "now", "source": "lead.play([c4])"}]}));
    assert_eq!(bad["error"]["code"], -32001);
    assert_eq!(bad["error"]["data"]["problems"][0]["event"], 0);

    d.session_mut().process(96_000);
    let status = call(&mut d, c, 6, "status", json!({}));
    let instruments = status["result"]["instruments"].as_array().unwrap();
    let extra = instruments.iter().find(|i| i["name"] == "extra").expect("the defined instrument");
    assert_eq!(extra["params"][1]["name"], "cut");
    assert_eq!(call(&mut d, c, 7, "clock", json!({}))["result"]["bpm"], 90.0);
}
