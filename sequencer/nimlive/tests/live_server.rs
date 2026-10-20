use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use niminal_cli::audio::{NullClock, Shared};
use nimlive::input::LiveShared;
use nimlive::link::WsLink;
use niminal_ws::{Client, Server, ServerConfig, lock};
use niminal_ws::worker::Worker;
use niminal_daemon::{EngineServer, Session};
use niminal_lang::Layout;
use niminal_live::{Live, LiveServer};
use serde_json::json;

const SETUP: &str = "tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track lead { instrument = pluck }";

const WAIT: Duration = Duration::from_secs(5);

/// An engine, a sequencer playing through it over a WebSocket, and the servers for both.
struct Running {
    engine: Shared,
    live: LiveShared,
    server: Server,
    engine_server: Server,
    _clock: NullClock,
    _ticker: Worker,
}

fn start(token: Option<&str>) -> Running {
    start_at(token, 4.0, 0.4)
}

/// The clock runs `speed` times as fast as the music, and the sequencer works `lookahead` seconds of music ahead.
fn start_at(token: Option<&str>, speed: f64, lookahead: f64) -> Running {
    let engine: Shared = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48_000.0, Layout::Mono).without_limiter(),
    )));
    // a bar is two seconds of music: at 4x that is half a second
    let clock = NullClock::start(engine.clone(), speed);
    let engine_config = ServerConfig {
        port: 0,
        token: token.map(str::to_string),
        ..ServerConfig::default()
    };
    let engine_server = Server::start(engine.clone(), engine_config).unwrap();
    let link = WsLink::connect(engine_server.port(), token).unwrap();
    let live: LiveShared = Arc::new(Mutex::new(LiveServer::new(
        Live::new(Box::new(link), 48_000.0, Layout::Mono).with_lookahead(lookahead),
    )));
    let config = ServerConfig {
        port: 0,
        token: token.map(str::to_string),
        ..ServerConfig::default()
    };
    let server = Server::start(live.clone(), config).unwrap();
    let ticking = live.clone();
    let ticker = Worker::spawn(move |stop| {
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            lock(&ticking).live_mut().advance();
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    Running {
        engine,
        live,
        server,
        engine_server,
        _clock: clock,
        _ticker: ticker,
    }
}

fn connect(r: &Running) -> Client {
    Client::connect(r.server.port(), None).unwrap()
}

#[test]
fn a_client_evaluates_code_and_is_told_when_it_lands() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("subscribe", json!({"topics": ["landed"]})).unwrap();
    c.call("eval", json!({"source": SETUP, "quantize": "now"}))
        .unwrap();

    let reply = c
        .call("eval", json!({"source": "lead.play([c4])"}))
        .unwrap();
    assert!(reply["lands_at"].as_u64().is_some());
    assert_eq!(reply["changes"][0], "lead.play([c4])");

    // it waits for its boundary, and the daemon says so
    let landed = c.wait_for("landed", WAIT).unwrap();
    assert!(
        landed["changes"].to_string().contains("tempo 120bpm")
            || landed["changes"].to_string().contains("lead.play")
    );
    let mut saw_play = landed["changes"].to_string().contains("lead.play");
    while !saw_play {
        saw_play = c.wait_for("landed", WAIT).unwrap()["changes"]
            .to_string()
            .contains("lead.play");
    }
    r.server.stop();
}

#[test]
fn code_that_does_not_compile_is_refused_with_its_problems() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("eval", json!({"source": SETUP, "quantize": "now"}))
        .unwrap();
    let refusal = c
        .call_raw("eval", json!({"source": "lead.play([c4)\n"}))
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal["code"], -32001);
    assert_eq!(
        refusal["data"]["problems"][0]["message"],
        "this `[` is never closed"
    );
    assert_eq!(refusal["data"]["problems"][0]["line"], 1);
    // and the session is unharmed
    assert!(
        c.call("status", json!({})).unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_transport_keeps_time_and_is_pushed_to_subscribers() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("subscribe", json!({"topics": ["transport"]}))
        .unwrap();
    let first = c.wait_for("transport", WAIT).unwrap()["sample"]
        .as_u64()
        .unwrap();
    let mut later = first;
    for _ in 0..5 {
        later = c.wait_for("transport", WAIT).unwrap()["sample"]
            .as_u64()
            .unwrap();
    }
    assert!(later > first, "{first} -> {later}");
}

#[test]
fn clients_share_one_session() {
    let r = start(None);
    let mut a = connect(&r);
    let mut b = connect(&r);
    a.call("eval", json!({"source": SETUP, "quantize": "now"}))
        .unwrap();
    // `b` builds on what `a` defined
    b.call(
        "eval",
        json!({"source": "lead.play([c4])", "quantize": "now"}),
    )
    .unwrap();
    let started = std::time::Instant::now();
    while b.call("status", json!({})).unwrap()["transport"]["voices"]
        .as_u64()
        .unwrap()
        == 0
    {
        assert!(started.elapsed() < WAIT, "the shared clip never sounded");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(a);
    assert!(
        b.call("status", json!({})).is_ok(),
        "a disconnect doesn't disturb the others"
    );
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
    let engine: Shared = Arc::new(Mutex::new(EngineServer::new(Session::new(
        48_000.0,
        Layout::Mono,
    ))));
    let config = ServerConfig {
        listen: IpAddr::from([0, 0, 0, 0]),
        port: 0,
        token: Some("t".into()),
    };
    let refused = Server::start(engine, config);
    assert!(refused.err().unwrap().contains("TLS"));
}

#[test]
fn the_audio_the_clock_produces_is_the_sessions() {
    let r = start(None);
    let mut c = connect(&r);
    c.call("eval", json!({"source": SETUP, "quantize": "now"}))
        .unwrap();
    c.call(
        "eval",
        json!({"source": "lead(freq: a4) for 1beat", "quantize": "now"}),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let peaks = r.engine.lock().unwrap().session_mut().take_peaks();
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
    // A second connection owns its definitions and controls.
    let mut engine = Client::connect(r.engine_server.port(), None).unwrap();
    engine.call("events.send", json!({"events":[{"op":"define","at":"now","source":"ctl level = 0.0.smooth(0ms)\ninstr flat() { level }\ntrack own { instrument = flat }"},{"op":"note","target":"own","at":"now","dur":"100sec"}]})).unwrap();
    let reply = engine.call("events.send", json!({ "events": [{ "op": "control", "name": "level", "value": 0.5, "glide": "0ms", "at": "now" }] })).unwrap();
    assert_eq!(reply["accepted"], 1);
    let started = std::time::Instant::now();
    loop {
        let peaks = r.engine.lock().unwrap().session_mut().take_peaks();
        if peaks[0] >= 0.5 {
            break;
        }
        assert!(started.elapsed() < WAIT, "the live control never sounded");
        std::thread::sleep(Duration::from_millis(2));
    }
    let status = c.call("status", json!({})).unwrap();
    assert_eq!(status["controls"][0]["name"], "level");
    assert_eq!(status["controls"][0]["value"], 0.0);
}

#[test]
fn osc_over_udp_moves_a_bound_control_and_is_logged() {
    use nimlive::input::OscInput;
    let r = start(None);
    let mut c = connect(&r);
    c.call("eval", json!({
        "source": "ctl level = osc_in(\"/level\").range(0..1).smooth(0ms)\ninstr flat(freq: hz) { level }\ntrack t { instrument = flat }\nt(freq: 440hz) for 100bars",
        "quantize": "now"
    })).unwrap();
    let osc = OscInput::start(r.live.clone(), 0).unwrap();
    let mut packet = b"/level\0\0,f\0\0".to_vec();
    packet.extend(0.5f32.to_be_bytes());
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.send_to(&packet, ("127.0.0.1", osc.port())).unwrap();
    let started = std::time::Instant::now();
    loop {
        let peaks = r.engine.lock().unwrap().session_mut().take_peaks();
        if peaks[0] >= 0.5 {
            break;
        }
        assert!(started.elapsed() < WAIT, "the OSC control never sounded");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        niminal_daemon::to_lines(r.engine.lock().unwrap().session().log()).contains("\"control\"")
    );
}

/// How long an edit that lands `now` takes to be heard, in real time with the sequencer's default lookahead.
#[test]
fn an_edit_landing_now_is_heard_within_the_lookahead_and_a_window() {
    let r = start_at(None, 1.0, 0.1);
    let mut c = connect(&r);
    c.call("eval", json!({"source": SETUP, "quantize": "now"}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut worst = 0.0f64;
    for _ in 0..5 {
        r.engine.lock().unwrap().session_mut().take_peaks();
        let sent = r.engine.lock().unwrap().session_mut().clock_info().sample;
        c.call(
            "eval",
            json!({"source": "lead(freq: a4) for 1/4beat", "quantize": "now"}),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let heard = loop {
            if r.engine.lock().unwrap().session_mut().take_peaks()[0] > 0.01 {
                break r.engine.lock().unwrap().session_mut().clock_info().sample;
            }
            assert!(started.elapsed() < WAIT, "the note never sounded");
            std::thread::sleep(Duration::from_millis(1));
        };
        let seconds = (heard - sent) as f64 / 48_000.0;
        eprintln!(
            "an edit landing now was heard {:.1}ms later",
            seconds * 1000.0
        );
        worst = worst.max(seconds);
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(worst < 0.3, "{worst}");
}

/// Dense streams: a thousand notes in a batch, over the WebSocket. Printed for the record.
#[test]
fn a_thousand_notes_in_a_batch_cross_the_websocket_quickly() {
    let r = start_at(None, 1.0, 0.1);
    let mut engine = Client::connect(r.engine_server.port(), None).unwrap();
    engine.call("events.send", json!({"events": [{"op": "define", "at": "now", "source": SETUP.replace("tempo 120bpm\n", "")}]})).unwrap();
    let notes: Vec<_> = (0..1000)
        .map(|i| json!({"op": "note", "target": "lead", "at": 48_000 * 30 + i * 100, "dur": "50ms", "args": {"freq": "a4"}}))
        .collect();
    let started = std::time::Instant::now();
    let reply = engine
        .call("events.send", json!({ "events": notes }))
        .unwrap();
    let took = started.elapsed();
    eprintln!("1000 notes accepted in {took:?}");
    assert_eq!(reply["accepted"], 1000);
    assert!(took < Duration::from_millis(250), "{took:?}");
}
