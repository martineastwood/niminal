use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use niminal_server::{ClientId, EngineServer, PROTOCOL_VERSION, Session};
use niminal_lang::Layout;
use niminal_live::{InProcess, Live, LiveServer};
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

    /// Mix `frames` samples, with the sequencer sending the engine what falls in them first.
    fn process(&mut self, frames: usize) -> Vec<Vec<f32>> {
        let end = self.engine.lock().unwrap().session().clock() + frames as u64;
        self.server.live_mut().advance_to(end);
        let out = self.engine.lock().unwrap().session_mut().process(frames);
        self.server.live_mut().advance();
        out
    }

    fn take_peaks(&mut self) -> Vec<f32> {
        self.engine.lock().unwrap().session_mut().take_peaks()
    }
}
const SETUP: &str = "tempo 120bpm\\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\\npart lead { instrument = pluck }";

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
    let r = call(&mut d, c, 3, "eval", json!({"source": "lead.play([c4])"}));
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
        json!({"source": "riff = [c4]\nlead.play(nope)"}),
    );
    assert_eq!(r["error"]["code"], -32001);
    assert_eq!(r["error"]["message"], "`nope` is not defined");
    let p = &r["error"]["data"]["problems"][0];
    assert_eq!(p["line"], 2);
    assert_eq!(p["column"], 11);
    assert_eq!(p["span"]["start"], 22);
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
    call(&mut d, c, 3, "eval", json!({"source": "lead.play([c4])"}));
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
    call(&mut d, c, 4, "eval", json!({"source": "lead.play([c4])"}));

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
        "lead.play([c4])"
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
        json!({"source": "lead.play([c4*2])", "quantize": "now"}),
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
        json!({"source": "lead.play([c4])", "quantize": "now"}),
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
        "{SETUP}\\npart bass {{ instrument = pluck }}\\nclip riff {{ notes: [c2 ~] }}\\nscene verse {{ lead: [c4 e4], bass: riff }}"
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
    assert_eq!(before["scenes"], json!(["verse"]));
    assert_eq!(before["clips"], json!(["riff"]));
    assert_eq!(
        before["parts"][0],
        json!({"name": "lead", "clip": null, "playing": null, "muted": false, "soloed": false, "edits": []})
    );

    call(
        &mut d,
        c,
        4,
        "eval",
        json!({"source": "verse.start\nbass.mute\nlead.solo", "quantize": "now"}),
    );
    let after = call(&mut d, c, 5, "status", json!({}))["result"].clone();
    assert_eq!(
        after["parts"][0]["clip"], "lead",
        "a bare pattern is named for its node"
    );
    assert_eq!(after["parts"][0]["soloed"], true);
    assert_eq!(after["parts"][1]["name"], "bass");
    assert_eq!(after["parts"][1]["clip"], "riff");
    assert_eq!(after["parts"][1]["muted"], true);
    assert_eq!(after["parts"][1]["soloed"], false);
    assert_eq!(after["parts"][1]["edits"], json!([]));
    assert!(after["parts"][1]["playing"].is_string() || after["parts"][1]["playing"].is_null());

    call(
        &mut d,
        c,
        6,
        "eval",
        json!({"source": "bass.reverse\nbass.thin(25%)", "quantize": "now"}),
    );
    let edited = call(&mut d, c, 7, "status", json!({}))["result"].clone();
    assert_eq!(
        edited["parts"][1]["edits"],
        json!(["reverse", "thin(25%)"])
    );
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
    let bad = json!({"jsonrpc": "2.0", "id": 3, "method": "eval", "params": {"source": "lead.play([zz])"}});
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
        json!({"source": "lead.play([c4])", "quantize": "next 4bars"}),
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
        json!({"source": "lead.play([c4])", "quantize": "now"}),
    );
    let request = json!({"jsonrpc": "2.0", "id": 4, "method": "hush", "params": {}}).to_string();
    let parsed = LiveServer::prepare_eval(&request);
    let Request::Compile(ticket) = d.begin_prepared(c, &request, parsed) else {
        panic!("hush needs background work")
    };
    let reply: Value = serde_json::from_str(&d.finish(ticket.compile())[0]).unwrap();
    assert_eq!(reply["id"], 4);
    assert!(reply["result"]["id"].is_u64());
    assert!(d.live().parts().iter().all(|t| t.clip.is_none()));
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
        json!({ "part": "riff", "change": "reverse", "quantize": "now" }),
    );
    assert!(edited["result"].is_object(), "{edited}");

    let status = call(&mut d, c, 14, "status", json!({}));
    let nodes = status["result"]["parts"].as_array().unwrap();
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
        status["result"]["clips"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str() == Some("riff"))
    );
}
