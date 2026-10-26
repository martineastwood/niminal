use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use niminal_lang::Layout;
use niminal_live::{InProcess, Live, LiveServer};
use niminal_server::{ClientId, EngineServer, PROTOCOL_VERSION, Session};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

/// The sequencer's server, with an engine in the same process whose clock only moves when asked to.
struct Server {
    server: LiveServer,
    engine: Arc<Mutex<EngineServer>>,
}

impl Deref for Server {
    type Target = LiveServer;
    fn deref(&self) -> &LiveServer {
        &self.server
    }
}

impl DerefMut for Server {
    fn deref_mut(&mut self) -> &mut LiveServer {
        &mut self.server
    }
}

impl Server {
    fn new(session: Session) -> Server {
        let engine = Arc::new(Mutex::new(EngineServer::new(session)));
        let live = Live::new(Box::new(InProcess::new(engine.clone())), SR, Layout::Mono);
        Server {
            server: LiveServer::new(live),
            engine,
        }
    }

    /// One that seats a chair of its own for each name a client asks for.
    fn with_chairs(session: Session) -> Server {
        let mut d = Server::new(session);
        let engine = d.engine.clone();
        d.server = std::mem::replace(
            &mut d.server,
            LiveServer::new(Live::new(
                Box::new(InProcess::new(engine.clone())),
                SR,
                Layout::Mono,
            )),
        )
        .with_chairs(move || {
            Ok(Live::new(
                Box::new(InProcess::new(engine.clone())),
                SR,
                Layout::Mono,
            ))
        });
        d
    }

    /// Mix `frames` samples, with the sequencer sending the engine what falls in them first.
    fn process(&mut self, frames: usize) -> Vec<Vec<f32>> {
        let end = self.engine.lock().unwrap().session().clock() + frames as u64;
        self.server.advance_to(end);
        let out = self.engine.lock().unwrap().session_mut().process(frames);
        self.server.advance();
        out
    }

    fn take_peaks(&mut self) -> Vec<f32> {
        self.engine.lock().unwrap().session_mut().take_peaks()
    }
}
const SETUP: &str = "tempo 120bpm\\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\\nclip lead { instrument: pluck }";

fn server() -> Server {
    Server::new(Session::new(SR, Layout::Mono).without_limiter())
}

fn call(d: &mut Server, client: ClientId, id: u64, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let replies = d.handle(client, &request);
    assert_eq!(replies.len(), 1, "one reply: {replies:?}");
    serde_json::from_str(&replies[0]).unwrap()
}

fn ready() -> (Server, ClientId) {
    let mut d = server();
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

fn methods(messages: &[(ClientId, String)]) -> Vec<(ClientId, String)> {
    messages
        .iter()
        .map(|(c, m)| {
            (
                *c,
                serde_json::from_str::<Value>(m).unwrap()["method"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect()
}

#[test]
fn a_client_must_say_hello_with_the_right_version() {
    let mut d = server();
    let c = d.connect();
    let early = call(&mut d, c, 1, "status", json!({}));
    assert_eq!(early["error"]["code"], -32000);
    assert!(
        early["error"]["message"]
            .as_str()
            .unwrap()
            .contains("hello")
    );

    let wrong = call(&mut d, c, 2, "hello", json!({"protocol": 99}));
    assert_eq!(wrong["error"]["code"], -32000);
    assert!(
        wrong["error"]["message"]
            .as_str()
            .unwrap()
            .contains("protocol 5")
    );
    assert_eq!(
        call(&mut d, c, 3, "status", json!({}))["error"]["code"],
        -32000,
        "still not greeted"
    );

    let ok = call(&mut d, c, 4, "hello", json!({"protocol": 5}));
    assert_eq!(ok["result"]["sample_rate"], 48_000.0);
    assert_eq!(ok["result"]["channels"], 1);
    assert_eq!(ok["result"]["topics"].as_array().unwrap().len(), 6);
    assert!(call(&mut d, c, 5, "status", json!({}))["result"]["transport"].is_object());
}

#[test]
fn malformed_requests_get_the_standard_errors() {
    let (mut d, c) = ready();
    let parse: Value = serde_json::from_str(&d.handle(c, "{nope")[0]).unwrap();
    assert_eq!(parse["error"]["code"], -32700);
    assert_eq!(parse["id"], Value::Null);

    let no_version: Value =
        serde_json::from_str(&d.handle(c, r#"{"id": 7, "method": "status"}"#)[0]).unwrap();
    assert_eq!(no_version["error"]["code"], -32600);
    assert_eq!(no_version["id"], 7);

    let no_method: Value =
        serde_json::from_str(&d.handle(c, r#"{"jsonrpc": "2.0", "id": 8}"#)[0]).unwrap();
    assert_eq!(no_method["error"]["code"], -32600);

    assert_eq!(
        call(&mut d, c, 9, "wobble", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        call(&mut d, c, 10, "eval", json!({}))["error"]["code"],
        -32602
    );
    assert_eq!(
        call(&mut d, c, 11, "cancel", json!({"id": "x"}))["error"]["code"],
        -32602
    );

    // a notification (no id) is acted on and gets no reply
    let none = d.handle(c, &json!({"jsonrpc": "2.0", "method": "panic"}).to_string());
    assert!(none.is_empty());
}

#[test]
fn eval_replies_with_where_the_change_lands() {
    let (mut d, c) = ready();
    let r = call(
        &mut d,
        c,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    assert_eq!(r["result"]["lands_at"], 0);
    assert_eq!(r["result"]["position"], json!({"bar": 1, "beat": 1.0}));
    assert_eq!(r["result"]["changes"].as_array().unwrap().len(), 3);

    d.process(10_000);
    let r = call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": "lead.__update([c4])"}),
    );
    assert_eq!(r["result"]["lands_at"], 96_000);
    assert_eq!(r["result"]["position"], json!({"bar": 2, "beat": 1.0}));
    let secs = r["result"]["in_seconds"].as_f64().unwrap();
    assert!((secs - 86_000.0 / 48_000.0).abs() < 1e-9);
    assert_eq!(r["result"]["id"], 2);
}

#[test]
fn a_rejected_eval_carries_located_problems() {
    let (mut d, c) = ready();
    call(
        &mut d,
        c,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    let r = call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": "riff = [c4]\nlead.__update(nope)"}),
    );
    assert_eq!(r["error"]["code"], -32001);
    assert_eq!(r["error"]["message"], "`nope` is not defined");
    let p = &r["error"]["data"]["problems"][0];
    assert_eq!(p["line"], 2);
    assert_eq!(p["column"], 15);
    assert_eq!(p["span"]["start"], 26);
    assert_eq!(p["message"], "`nope` is not defined");

    let bad_quantum = call(
        &mut d,
        c,
        4,
        "eval",
        json!({"source": "hush", "quantize": "whenever"}),
    );
    assert_eq!(bad_quantum["error"]["code"], -32001);
    assert!(
        bad_quantum["error"]["message"]
            .as_str()
            .unwrap()
            .contains("isn't a quantum")
    );
}

#[test]
fn cancel_hush_and_panic() {
    let (mut d, c) = ready();
    call(
        &mut d,
        c,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    d.process(1000);
    call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": "lead.__update([c4])"}),
    );
    assert_eq!(
        call(&mut d, c, 4, "status", json!({}))["result"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call(&mut d, c, 5, "cancel", json!({}))["result"]["cancelled"],
        1
    );
    assert_eq!(
        call(&mut d, c, 6, "cancel", json!({"id": 5}))["result"]["cancelled"],
        0
    );
    assert!(call(&mut d, c, 7, "hush", json!({}))["result"]["lands_at"].is_number());
    assert_eq!(call(&mut d, c, 8, "panic", json!({}))["result"], json!({}));
}

#[test]
fn subscribers_hear_about_changes_landing() {
    let (mut d, c) = ready();
    let other = d.connect();
    call(&mut d, other, 1, "hello", json!({"protocol": 5}));
    let ok = call(
        &mut d,
        c,
        2,
        "subscribe",
        json!({"topics": ["landed", "pending"]}),
    );
    assert_eq!(ok["result"]["topics"], json!(["landed", "pending"]));
    // `other` subscribes to nothing
    d.notifications();

    call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    d.process(1000);
    call(
        &mut d,
        c,
        4,
        "eval",
        json!({"source": "lead.__update([c4])"}),
    );

    // the evaluation that is waiting is announced to subscribers, not to others
    let messages = d.notifications();
    assert!(messages.iter().all(|(client, _)| *client == c));
    let kinds = methods(&messages);
    assert!(kinds.contains(&(c, "pending".to_string())), "{kinds:?}");
    let pending: Value = serde_json::from_str(
        &messages
            .iter()
            .find(|(_, m)| m.contains("\"pending\""))
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(
        pending["params"]["changes"][0]["position"],
        json!({"bar": 2, "beat": 1.0})
    );

    // when it lands, they are told, and the waiting list empties
    d.process(100_000);
    let after = d.notifications();
    let landed: Vec<Value> = after
        .iter()
        .map(|(_, m)| serde_json::from_str::<Value>(m).unwrap())
        .filter(|m| m["method"] == "landed")
        .collect();
    assert!(!landed.is_empty());
    assert_eq!(
        landed.last().unwrap()["params"]["changes"][0],
        "lead.__update([c4])"
    );
    assert_eq!(landed.last().unwrap()["params"]["at"], 96_000);
    let empties: Vec<Value> = after
        .iter()
        .map(|(_, m)| serde_json::from_str::<Value>(m).unwrap())
        .filter(|m| m["method"] == "pending")
        .collect();
    assert_eq!(empties.last().unwrap()["params"]["changes"], json!([]));
}

#[test]
fn the_transport_and_meters_are_pushed_regularly() {
    let (mut d, c) = ready();
    call(
        &mut d,
        c,
        2,
        "subscribe",
        json!({"topics": ["transport", "meters"]}),
    );
    let first = d.notifications();
    assert_eq!(methods(&first).len(), 2, "an immediate snapshot of each");

    assert!(
        d.notifications().is_empty(),
        "nothing new while the clock stands still"
    );
    d.process(2000);
    assert!(
        d.notifications().is_empty(),
        "not until a tenth of a second has passed"
    );
    d.process(3000);
    let later = d.notifications();
    let t: Value = serde_json::from_str(
        &later
            .iter()
            .find(|(_, m)| m.contains("\"transport\""))
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(t["params"]["sample"], 5000);
    assert_eq!(t["params"]["bar"], 1);
    assert!(later.iter().any(|(_, m)| m.contains("\"peaks\"")));
}

#[test]
fn the_meters_are_left_alone_unless_someone_is_listening() {
    let (mut d, c) = ready();
    call(
        &mut d,
        c,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": "lead(freq: a4) for 1beat", "quantize": "now"}),
    );
    d.process(10_000);
    d.notifications(); // someone polling must not eat the readings
    d.process(10_000);
    d.notifications();
    assert!(d.take_peaks()[0] > 0.5);
}

#[test]
fn notices_tell_subscribers_what_went_wrong_while_playing() {
    let (mut d, c) = ready();
    call(&mut d, c, 2, "subscribe", json!({"topics": ["notices"]}));
    call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    call(
        &mut d,
        c,
        4,
        "eval",
        json!({"source": "lead.__update([c4*2])", "quantize": "now"}),
    );
    call(
        &mut d,
        c,
        5,
        "eval",
        json!({"source": "instr pluck(pitch: hz) { osc(sine, pitch) }", "quantize": "now"}),
    );
    d.process(96_000);
    let notes: Vec<Value> = d
        .notifications()
        .iter()
        .map(|(_, m)| serde_json::from_str::<Value>(m).unwrap())
        .filter(|m| m["method"] == "notice")
        .collect();
    assert!(!notes.is_empty());
    assert!(
        notes[0]["params"]["message"]
            .as_str()
            .unwrap()
            .contains("could not play")
    );
}

#[test]
fn subscription_requests_are_checked() {
    let (mut d, c) = ready();
    assert_eq!(
        call(&mut d, c, 2, "subscribe", json!({"topics": ["weather"]}))["error"]["code"],
        -32602
    );
    assert_eq!(
        call(&mut d, c, 3, "subscribe", json!({}))["error"]["code"],
        -32602
    );
    call(
        &mut d,
        c,
        4,
        "subscribe",
        json!({"topics": ["landed", "meters"]}),
    );
    let after = call(&mut d, c, 5, "unsubscribe", json!({"topics": ["meters"]}));
    assert_eq!(after["result"]["topics"], json!(["landed"]));
}

#[test]
fn clients_share_one_session_and_a_disconnect_does_not_disturb_it() {
    let (mut d, a) = ready();
    let b = d.connect();
    call(&mut d, b, 1, "hello", json!({"protocol": 5}));
    call(
        &mut d,
        a,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    // the other client sees what the first defined, and builds on it
    let r = call(
        &mut d,
        b,
        2,
        "eval",
        json!({"source": "lead.__update([c4])", "quantize": "now"}),
    );
    assert!(r["result"].is_object());
    d.disconnect(a);
    assert!(d.process(10_000).remove(0).iter().any(|s| s.abs() > 0.1));
    assert!(
        call(&mut d, b, 3, "status", json!({}))["result"]["transport"]["voices"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

#[test]
fn status_lists_scenes_clips_and_what_each_node_is_doing() {
    let (mut d, c) = ready();
    let code = format!(
        "{SETUP}\\nclip lead {{ instrument: pluck\\n notes: [c4 e4] }}\\nclip bass {{ instrument: pluck\\n notes: [c2 ~] }}\\nclip riff {{ notes: [c2 ~] }}\\nscene verse {{ lead, bass }}"
    );
    // where the notes are written, which the editor lights as they play
    let (c4, e4) = (
        code.replace("\\n", "\n").find("c4").unwrap(),
        code.replace("\\n", "\n").find("e4").unwrap(),
    );
    assert!(
        call(
            &mut d,
            c,
            2,
            "eval",
            json!({"source": code.replace("\\n", "\n"), "quantize": "now"})
        )["result"]
            .is_object()
    );

    let before = call(&mut d, c, 3, "status", json!({}))["result"].clone();
    assert_eq!(
        before["scenes"],
        json!([{"name": "verse", "clips": ["lead", "bass"], "running": 0, "active": false}])
    );
    assert_eq!(before["defined_clips"], json!(["lead", "bass", "riff"]));
    assert_eq!(
        before["clips"][0],
        json!({
            "name": "lead",
            "instrument": "pluck",
            "running": false,
            "muted": false,
            "soloed": false,
            "edits": [],
            "length": 1.0,
            "looped": true,
            "plays": null,
            "endless": true,
            "cycles": null,
            "lanes": [{
                "name": "notes",
                "truncated": false,
                "events": [
                    {"start": 0.0, "dur": 0.5, "value": "c4", "gain": null, "pan": null, "src": [c4, c4 + 2], "eval": 0},
                    {"start": 0.5, "dur": 0.5, "value": "e4", "gain": null, "pan": null, "src": [e4, e4 + 2], "eval": 0}
                ]
            }]
        })
    );

    call(
        &mut d,
        c,
        4,
        "eval",
        json!({"source": "verse.start\nbass.mute\nlead.solo", "quantize": "now"}),
    );
    let after = call(&mut d, c, 5, "status", json!({}))["result"].clone();
    assert_eq!(after["clips"][0]["running"], true);
    assert_eq!(after["clips"][0]["soloed"], true);
    assert_eq!(after["clips"][1]["name"], "bass");
    assert_eq!(after["clips"][1]["running"], true);
    assert_eq!(after["clips"][1]["muted"], true);
    assert_eq!(after["clips"][1]["soloed"], false);
    assert_eq!(after["clips"][1]["edits"], json!([]));
    assert_eq!(
        after["scenes"][0],
        json!({"name": "verse", "clips": ["lead", "bass"], "running": 2, "active": true})
    );

    call(
        &mut d,
        c,
        6,
        "eval",
        json!({"source": "bass.reverse\nbass.drop(25%)", "quantize": "now"}),
    );
    let edited = call(&mut d, c, 7, "status", json!({}))["result"].clone();
    assert_eq!(edited["clips"][1]["edits"], json!(["reverse", "drop(25%)"]));
}

#[test]
fn begin_and_finish_answer_an_eval_like_handle_does() {
    use niminal_live::Request;
    let (mut d, c) = ready();
    let request = json!({"jsonrpc": "2.0", "id": 2, "method": "eval", "params": {"source": SETUP.replace("\\n", "\n"), "quantize": "now"}});
    let Request::Compile(ticket) = d.begin(c, &request.to_string()) else {
        panic!("an eval needs compiling")
    };
    let replies = d.finish(ticket.compile());
    let reply: Value = serde_json::from_str(&replies[0]).unwrap();
    assert_eq!(reply["id"], 2);
    assert!(reply["result"]["id"].is_u64());

    // a mistake is an error reply, still with the request's id
    let bad = json!({"jsonrpc": "2.0", "id": 3, "method": "eval", "params": {"source": "lead.__update([zz])"}});
    let Request::Compile(ticket) = d.begin(c, &bad.to_string()) else {
        panic!()
    };
    let reply: Value = serde_json::from_str(&d.finish(ticket.compile())[0]).unwrap();
    assert_eq!(reply["id"], 3);
    assert_eq!(reply["error"]["code"], -32001);

    // other methods are answered straight away, and an eval before hello is refused
    let Request::Replies(r) = d.begin(
        c,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "status"}).to_string(),
    ) else {
        panic!()
    };
    assert_eq!(serde_json::from_str::<Value>(&r[0]).unwrap()["id"], 4);
    let stranger = d.connect();
    let Request::Replies(r) = d.begin(stranger, &request.to_string()) else {
        panic!("not greeted")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&r[0]).unwrap()["error"]["code"],
        -32000
    );
}

#[test]
fn cancellation_compiles_outside_the_session_and_retries_if_it_became_stale() {
    use niminal_live::Request;
    let (mut d, c) = ready();
    call(
        &mut d,
        c,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    let first = call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": "lead.__update([c4])", "quantize": "next 4bars"}),
    );
    let request = json!({"jsonrpc": "2.0", "id": 4, "method": "cancel", "params": {"id": first["result"]["id"]}}).to_string();
    let Request::Compile(ticket) = d.begin(c, &request) else {
        panic!("cancel is prepared off the session")
    };
    let done = ticket.compile();
    call(
        &mut d,
        c,
        5,
        "eval",
        json!({"source": "lead.mute", "quantize": "next 4bars"}),
    );
    let retry = d.retry(&done).expect("the pending queue changed");
    let done = retry.compile();
    assert!(d.retry(&done).is_none());
    let reply: Value = serde_json::from_str(&d.finish(done)[0]).unwrap();
    assert_eq!(reply["id"], 4);
    assert_eq!(reply["result"]["cancelled"], 1);
    assert_eq!(d.live().pending().len(), 1);
    assert_eq!(d.live().pending()[0].changes, ["lead.mute"]);
}

#[test]
fn hush_uses_the_background_compilation_path() {
    use niminal_live::Request;
    let (mut d, c) = ready();
    call(
        &mut d,
        c,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    call(
        &mut d,
        c,
        3,
        "eval",
        json!({"source": "lead.__update([c4])", "quantize": "now"}),
    );
    let request = json!({"jsonrpc": "2.0", "id": 4, "method": "hush", "params": {}}).to_string();
    let parsed = LiveServer::prepare_eval(&request);
    let Request::Compile(ticket) = d.begin_prepared(c, &request, parsed) else {
        panic!("hush needs background work")
    };
    let reply: Value = serde_json::from_str(&d.finish(ticket.compile())[0]).unwrap();
    assert_eq!(reply["id"], 4);
    assert!(reply["result"]["id"].is_u64());
    assert!(d.live().clips().iter().all(|t| !t.running));
}

#[test]
fn typed_clip_define_launch_and_edit() {
    let (mut d, c) = ready();
    assert!(
        call(
            &mut d,
            c,
            10,
            "eval",
            json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"})
        )["result"]
            .is_object()
    );

    let defined = call(
        &mut d,
        c,
        11,
        "clip.define",
        json!({
            "name": "riff",
            "instrument": "pluck",
            "lanes": { "notes": "c4 e4 g4 b4" },
            "quantize": "now"
        }),
    );
    assert!(defined["result"].is_object(), "{defined}");

    let launched = call(
        &mut d,
        c,
        12,
        "launch",
        json!({ "target": "riff", "quantize": "now" }),
    );
    assert!(launched["result"].is_object(), "{launched}");

    let edited = call(
        &mut d,
        c,
        13,
        "edit",
        json!({ "clip": "riff", "change": "reverse", "quantize": "now" }),
    );
    assert!(edited["result"].is_object(), "{edited}");

    let status = call(&mut d, c, 14, "status", json!({}));
    let nodes = status["result"]["clips"].as_array().unwrap();
    let riff = nodes.iter().find(|t| t["name"] == "riff").unwrap();
    assert!(
        riff["edits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str() == Some("reverse")),
        "{riff}"
    );
    assert!(
        status["result"]["defined_clips"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str() == Some("riff"))
    );
}

fn seated(d: &mut Server, chair: Option<&str>) -> ClientId {
    let c = d.connect();
    let mut params = json!({"protocol": PROTOCOL_VERSION, "client": "test"});
    if let Some(chair) = chair {
        params["chair"] = json!(chair);
    }
    let hello = call(d, c, 1, "hello", params);
    assert!(hello["error"].is_null(), "{hello}");
    c
}

fn clip_state(d: &mut Server, c: ClientId, name: &str) -> Option<(bool, bool)> {
    let status = call(d, c, 9, "status", json!({}));
    status["result"]["clips"]
        .as_array()?
        .iter()
        .find(|clip| clip["name"] == name)
        .map(|clip| {
            (
                clip["running"].as_bool().unwrap(),
                clip["muted"].as_bool().unwrap(),
            )
        })
}

#[test]
fn a_chair_has_its_own_clips_and_its_own_mute() {
    let mut d = Server::with_chairs(Session::new(SR, Layout::Mono).without_limiter());
    let (a, b) = (seated(&mut d, None), seated(&mut d, Some("bass")));
    // the same name in two chairs is two clips
    for c in [a, b] {
        let eval = call(
            &mut d,
            c,
            2,
            "eval",
            json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
        );
        assert!(eval["error"].is_null(), "{eval}");
    }
    call(&mut d, a, 3, "launch", json!({"target": "lead"}));
    d.process(4_800);
    assert_eq!(clip_state(&mut d, a, "lead"), Some((true, false)));
    assert_eq!(
        clip_state(&mut d, b, "lead"),
        Some((false, false)),
        "b's lead is not playing"
    );

    call(&mut d, b, 4, "launch", json!({"target": "lead"}));
    call(&mut d, a, 5, "mute", json!({"target": "lead"}));
    d.process(4_800);
    assert_eq!(clip_state(&mut d, a, "lead"), Some((true, true)));
    assert_eq!(
        clip_state(&mut d, b, "lead"),
        Some((true, false)),
        "muting a does not mute b"
    );
}

#[test]
fn clients_that_name_one_chair_share_it() {
    let mut d = Server::with_chairs(Session::new(SR, Layout::Mono).without_limiter());
    let (b, c) = (seated(&mut d, Some("bass")), seated(&mut d, Some("bass")));
    call(
        &mut d,
        b,
        2,
        "eval",
        json!({"source": SETUP.replace("\\n", "\n"), "quantize": "now"}),
    );
    assert!(
        clip_state(&mut d, c, "lead").is_some(),
        "c sees what b defined"
    );
    let main = seated(&mut d, None);
    assert!(clip_state(&mut d, main, "lead").is_none(), "main does not");
}

#[test]
fn a_new_chair_starts_at_the_tempo_of_main() {
    let mut d = Server::with_chairs(Session::new(SR, Layout::Mono).without_limiter());
    let a = seated(&mut d, None);
    call(
        &mut d,
        a,
        2,
        "eval",
        json!({"source": "tempo 90bpm\nmeter 3/4", "quantize": "now"}),
    );
    let b = seated(&mut d, Some("bass"));
    let status = call(&mut d, b, 3, "status", json!({}));
    assert_eq!(status["result"]["transport"]["bpm"], 90.0);
}

#[test]
fn notifications_go_to_the_chair_they_are_about() {
    let mut d = Server::with_chairs(Session::new(SR, Layout::Mono).without_limiter());
    let (a, b) = (seated(&mut d, None), seated(&mut d, Some("bass")));
    for c in [a, b] {
        call(&mut d, c, 2, "subscribe", json!({"topics": ["landed"]}));
    }
    d.notifications();
    call(
        &mut d,
        b,
        3,
        "eval",
        json!({"source": "tempo 100bpm", "quantize": "now"}),
    );
    d.process(4_800);
    let heard = methods(&d.notifications());
    assert!(heard.contains(&(b, "landed".into())), "{heard:?}");
    assert!(
        !heard.iter().any(|(c, _)| *c == a),
        "a heard of b's change: {heard:?}"
    );
}

#[test]
fn a_chair_needs_a_server_that_seats_them() {
    let mut d = server();
    let c = d.connect();
    let hello = call(
        &mut d,
        c,
        1,
        "hello",
        json!({"protocol": PROTOCOL_VERSION, "chair": "bass"}),
    );
    assert_eq!(hello["error"]["code"], -32602);
    assert!(
        hello["error"]["message"]
            .as_str()
            .unwrap()
            .contains("one chair")
    );
}

/// Every note of the beat's lane points at an `x` in the text the stage sent, however the clip has
/// been changed since: reversed, undone, or compiled again because an earlier definition changed.
#[test]
fn lane_sources_stay_on_the_notes_as_the_project_changes() {
    let mut d = server();
    let c = d.connect();
    call(
        &mut d,
        c,
        1,
        "hello",
        json!({"protocol": PROTOCOL_VERSION, "client": "test"}),
    );
    let source = include_str!("../../../examples/synth_drums.nml")
        .replace("at +4bars hush", "")
        .replace("config { channels: stereo }", "")
        .replace("bus glue: stereo", "bus glue: mono");
    let mut id = 1;
    let mut eval = |d: &mut Server, src: &str, origin: u64| {
        id += 1;
        let r = call(
            d,
            c,
            id,
            "eval",
            json!({"source": src, "quantize": "now", "origin": origin}),
        );
        assert!(r["result"].is_object(), "{r}");
    };
    // `eval` number to the text that evaluation sent
    let sent = |origin: u64| match origin {
        5 => source.clone(),
        other => panic!("no evaluation {other}"),
    };
    let check = |d: &mut Server, what: &str, reversed: Option<bool>| {
        let status = call(d, c, 99, "status", json!({}))["result"].clone();
        let beat = status["clips"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "beat")
            .unwrap()
            .clone();
        let events = beat["lanes"][0]["events"].as_array().unwrap();
        assert!(!events.is_empty(), "{what}");
        for e in events {
            let (src, origin) = (&e["src"], e["eval"].as_u64().unwrap());
            let (a, b) = (
                src[0].as_u64().unwrap() as usize,
                src[1].as_u64().unwrap() as usize,
            );
            assert_eq!(&sent(origin)[a..b], "x", "{what}: {e}");
        }
        if let Some(reversed) = reversed {
            // the first hat of the bar is the last `x` of its row when the clip is reversed
            let first = events[0]["src"][0].as_u64().unwrap();
            let last_x =
                source.find("xxxx xxxx xxxx xxxx").unwrap() + "xxxx xxxx xxxx xxxx".len() - 1;
            assert_eq!(first as usize == last_x, reversed, "{what}: {}", events[0]);
        }
    };
    eval(&mut d, &source, 5);
    check(&mut d, "started", Some(false));
    // an earlier definition changes length, so the project is laid out differently now
    eval(
        &mut d,
        "instr sd(freq: hz = 180hz) {\n  // a longer comment that moves everything after it\n  tone = osc(tri, freq) * perc(decay: 120ms) * 0.4\n  out = tone\n  glue += out\n}",
        9,
    );
    check(&mut d, "after redefining an instrument", None);
    eval(&mut d, "beat.reverse", 6);
    check(&mut d, "reversed", Some(true));
    eval(&mut d, "beat.undo", 7);
    check(&mut d, "undone", Some(false));
}

/// The stage reads these straight from the session, so a bar and beat must be an object, as in the
/// protocol, not a pair.
#[test]
fn positions_are_sent_as_a_bar_and_a_beat() {
    use niminal_live::{Accepted, PendingInfo};
    let pending = PendingInfo {
        id: 3,
        lands_at: 96_000,
        in_samples: 48_000,
        position: (4, 1.5),
        changes: vec!["beat.reverse".into()],
    };
    assert_eq!(
        serde_json::to_value(&pending).unwrap()["position"],
        json!({"bar": 4, "beat": 1.5})
    );
    let accepted = Accepted {
        id: Some(3),
        lands_at: 96_000,
        position: (4, 1.5),
        changes: vec![],
    };
    assert_eq!(
        serde_json::to_value(&accepted).unwrap()["position"],
        json!({"bar": 4, "beat": 1.5})
    );
}

/// The groove example in a session, with a clip that belongs to no scene, and a way to ask what is running.
struct Groove {
    d: Server,
    c: ClientId,
    id: u64,
}

impl Groove {
    fn new() -> Groove {
        let mut d = server();
        let c = d.connect();
        call(
            &mut d,
            c,
            1,
            "hello",
            json!({"protocol": PROTOCOL_VERSION, "client": "test"}),
        );
        let source = include_str!("../../../examples/groove.nml")
            .replace("song.start", "")
            .replace("config { channels: stereo }", "")
            .replace("bus space: stereo", "bus space: mono")
            + "\nclip solo {\n  instrument: pluck\n  notes: [c3]\n}\n";
        let mut groove = Groove { d, c, id: 1 };
        groove.run(&source);
        groove
    }

    fn run(&mut self, source: &str) -> Value {
        self.id += 1;
        let r = call(
            &mut self.d,
            self.c,
            self.id,
            "eval",
            json!({"source": source, "quantize": "now"}),
        );
        assert!(r["result"].is_object(), "{r}");
        r
    }

    fn running(&mut self) -> Vec<String> {
        self.id += 1;
        let status = call(&mut self.d, self.c, self.id, "status", json!({}))["result"].clone();
        let mut names: Vec<String> = status["clips"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["running"] == true)
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }
}

#[test]
fn starting_a_scene_switches_to_it() {
    let mut g = Groove::new();
    g.run("verse.start");
    assert_eq!(g.running(), ["bass", "lead"]);
    g.run("chorus.start");
    // verse's clips stopped as chorus's started
    assert_eq!(g.running(), ["bass_chorus", "lead_chorus"]);
    g.run("chorus.stop");
    assert_eq!(g.running(), Vec::<String>::new());
}

#[test]
fn a_clip_started_by_hand_follows_the_scene_that_holds_it() {
    let mut g = Groove::new();
    // `lead_outro` is part of `outro`, though nothing started it as a scene
    g.run("lead_outro.start");
    g.run("verse.start");
    assert_eq!(g.running(), ["bass", "lead"]);
}

#[test]
fn a_clip_in_no_scene_is_left_alone() {
    let mut g = Groove::new();
    g.run("solo.start");
    g.run("verse.start");
    g.run("chorus.start");
    assert_eq!(g.running(), ["bass_chorus", "lead_chorus", "solo"]);
}

#[test]
fn defining_a_playing_scene_again_changes_what_plays() {
    let mut g = Groove::new();
    g.run("verse.start");
    g.run("scene verse { bass, lead, lead_outro }");
    assert_eq!(g.running(), ["bass", "lead", "lead_outro"]);
    g.run("scene verse { lead }");
    assert_eq!(g.running(), ["lead"]);
}

#[test]
fn defining_a_scene_that_is_not_playing_starts_nothing() {
    let mut g = Groove::new();
    g.run("verse.start");
    g.run("scene chorus { bass_chorus, lead_chorus, lead_outro }");
    assert_eq!(g.running(), ["bass", "lead"]);
}

#[test]
fn a_switch_is_one_change_that_lands_together() {
    let mut g = Groove::new();
    g.run("verse.start");
    // it is accepted as a single change, whose stops and start land at the same moment
    g.id += 1;
    let r = call(
        &mut g.d,
        g.c,
        g.id,
        "eval",
        json!({"source": "chorus.start"}),
    );
    let changes: Vec<&str> = r["result"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(changes, ["bass.stop", "lead.stop", "chorus.start"]);
    assert!(r["result"]["id"].is_u64(), "{r}");
}
