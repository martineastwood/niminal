use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use niminal_cli::audio::{NullClock, Shared};
use niminal_cli::server::{Client, Server, ServerConfig};
use niminal_daemon::{Daemon, Session};
use niminal_lang::Layout;
use serde_json::json;

const SETUP: &str = "tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track lead { instrument = pluck }";

const WAIT: Duration = Duration::from_secs(5);

struct Running {
    daemon: Shared,
    server: Server,
    _clock: NullClock,
}

fn start(token: Option<&str>) -> Running {
    let daemon: Shared = Arc::new(Mutex::new(Daemon::new(Session::new(48_000.0, Layout::Mono).without_limiter())));
    // a bar is two seconds of music: at 20x that is a tenth of a second
    let clock = NullClock::start(daemon.clone(), 20.0);
    let config = ServerConfig { port: 0, token: token.map(str::to_string), ..ServerConfig::default() };
    let server = Server::start(daemon.clone(), config).unwrap();
    Running { daemon, server, _clock: clock }
}

fn connect(r: &Running) -> Client {
    Client::connect(r.server.port(), None).unwrap()
}

#[test]
fn a_client_evaluates_code_and_is_told_when_it_lands() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("subscribe", json!({"topics": ["landed"]})).unwrap();
    c.call("eval", json!({"source": SETUP, "quantize": "now"})).unwrap();

    let reply = c.call("eval", json!({"source": "play lead = [c4]"})).unwrap();
    assert!(reply["lands_at"].as_u64().is_some());
    assert_eq!(reply["changes"][0], "play lead = [c4]");

    // it waits for its boundary, and the daemon says so
    let landed = c.wait_for("landed", WAIT).unwrap();
    assert!(landed["changes"].to_string().contains("tempo 120bpm") || landed["changes"].to_string().contains("play lead"));
    let mut saw_play = landed["changes"].to_string().contains("play lead");
    while !saw_play {
        saw_play = c.wait_for("landed", WAIT).unwrap()["changes"].to_string().contains("play lead");
    }
    r.server.stop();
}

#[test]
fn code_that_does_not_compile_is_refused_with_its_problems() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("eval", json!({"source": SETUP, "quantize": "now"})).unwrap();
    let refusal = c.call_raw("eval", json!({"source": "play lead = [c4\n"})).unwrap().unwrap_err();
    assert_eq!(refusal["code"], -32001);
    assert_eq!(refusal["data"]["problems"][0]["message"], "this `[` is never closed");
    assert_eq!(refusal["data"]["problems"][0]["line"], 1);
    // and the session is unharmed
    assert!(c.call("status", json!({})).unwrap()["pending"].as_array().unwrap().is_empty());
}

#[test]
fn the_transport_keeps_time_and_is_pushed_to_subscribers() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("subscribe", json!({"topics": ["transport"]})).unwrap();
    let first = c.wait_for("transport", WAIT).unwrap()["sample"].as_u64().unwrap();
    let mut later = first;
    for _ in 0..5 {
        later = c.wait_for("transport", WAIT).unwrap()["sample"].as_u64().unwrap();
    }
    assert!(later > first, "{first} -> {later}");
}

#[test]
fn clients_share_one_session() {
    let r = start(None);
    let mut a = connect(&r);
    let mut b = connect(&r);
    a.call("eval", json!({"source": SETUP, "quantize": "now"})).unwrap();
    // `b` builds on what `a` defined
    b.call("eval", json!({"source": "play lead = [c4]", "quantize": "now"})).unwrap();
    let started = std::time::Instant::now();
    while b.call("status", json!({})).unwrap()["transport"]["voices"].as_u64().unwrap() == 0 {
        assert!(started.elapsed() < WAIT, "the shared clip never sounded");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(a);
    assert!(b.call("status", json!({})).is_ok(), "a disconnect doesn't disturb the others");
}

#[test]
fn a_token_is_required_when_one_is_set() {
    let r = start(Some("sesame"));
    let refused = Client::connect(r.server.port(), None);
    assert!(refused.is_err());
    assert!(refused.err().unwrap().contains("token"));
    let wrong = Client::connect(r.server.port(), Some("nope"));
    assert!(wrong.is_err());
    let mut ok = Client::connect(r.server.port(), Some("sesame")).unwrap();
    assert!(ok.call("status", json!({})).is_ok());
}

#[test]
fn only_local_connections_are_accepted_until_there_is_tls() {
    let daemon: Shared = Arc::new(Mutex::new(Daemon::new(Session::new(48_000.0, Layout::Mono))));
    let config = ServerConfig { listen: IpAddr::from([0, 0, 0, 0]), port: 0, token: Some("t".into()) };
    let refused = Server::start(daemon, config);
    assert!(refused.err().unwrap().contains("TLS"));
}

#[test]
fn the_audio_the_clock_produces_is_the_sessions() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("eval", json!({"source": SETUP, "quantize": "now"})).unwrap();
    c.call("eval", json!({"source": "lead(freq: a4) for 1beat", "quantize": "now"})).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let peaks = r.daemon.lock().unwrap().session_mut().take_peaks();
    assert!(peaks[0] > 0.5, "the note was mixed: {peaks:?}");
}

#[test]
fn websocket_controls_reach_the_live_renderer_and_appear_in_status() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("eval", json!({
        "source": "ctl level = 0.0.smooth(0ms)\ninstr flat(freq: hz) { level }\ntrack t { instrument = flat }\nt(freq: 440hz) for 100bars",
        "quantize": "now"
    })).unwrap();
    let reply = c.call("control.set", json!({ "name": "level", "value": 0.5, "smooth_ms": 0 })).unwrap();
    assert!(reply["at"].as_u64().is_some());
    let started = std::time::Instant::now();
    loop {
        let peaks = r.daemon.lock().unwrap().session_mut().take_peaks();
        if peaks[0] >= 0.5 { break; }
        assert!(started.elapsed() < WAIT, "the live control never sounded");
        std::thread::sleep(Duration::from_millis(2));
    }
    let status = c.call("status", json!({})).unwrap();
    assert_eq!(status["controls"][0]["name"], "level");
    assert_eq!(status["controls"][0]["value"], 0.5);
}
