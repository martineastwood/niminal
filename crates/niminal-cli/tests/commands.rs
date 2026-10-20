use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use niminal_ws::Client;
use serde_json::json;

fn niminal() -> Command {
    Command::new(env!("CARGO_BIN_EXE_niminal"))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// A server running as a child process, killed when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(extra: &[&str]) -> Server {
        // Another process may take the port between choosing it and binding it, so try again.
        for _ in 0..5 {
            let port = free_port();
            let mut child = niminal()
                .args(["server", "--no-audio", "--port", &port.to_string()])
                .args(extra)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
            if line.contains("niminal engine listening") {
                return Server { child, port };
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("the server would not start");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_recorded_session_replays_to_audio() {
    let dir = std::env::temp_dir().join(format!("niminal-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("set.log");
    {
        let d = Server::start(&["--log", log.to_str().unwrap()]);
        let mut c = Client::connect(d.port, None).unwrap();
        let clock = c.call("clock", json!({})).unwrap();
        let start = clock["sample"].as_u64().unwrap() + 4800;
        c.call(
            "events.send",
            json!({"events": [
                {"op": "define", "at": "now", "source": "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }"},
                {"op": "note", "target": "pluck", "at": start, "dur": "1sec", "args": {"freq": "a4"}},
                {"op": "note", "target": "pluck", "at": start + 24000, "dur": "1sec", "args": {"freq": "e5"}},
            ]}),
        )
        .unwrap();
        // the log is written a few times a second
        std::thread::sleep(Duration::from_millis(700));
    }
    let text = std::fs::read_to_string(&log).unwrap();
    // the log is of what the engine was sent: the definition and the notes
    assert!(text.lines().next().unwrap().contains("\"send\""), "{text}");
    assert!(text.contains("\"define\"") && text.contains("\"note\""), "{text}");

    let wav = dir.join("set.wav");
    let out = niminal()
        .args(["replay", log.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--channels", "2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("replayed 1 input"), "{}", stdout(&out));
    let mut reader = hound::WavReader::open(&wav).unwrap();
    assert_eq!(reader.spec().channels, 2);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1 && peak <= 0.9661, "{peak}");
    let seconds = samples.len() as f32 / 2.0 / 48_000.0;
    assert!((1.0..4.0).contains(&seconds), "{seconds}");

    // an unlimited replay can be asked for, and a bad layout is refused
    let raw = niminal()
        .args(["replay", log.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--no-limiter"])
        .output()
        .unwrap();
    assert!(raw.status.success());
    let bad = niminal()
        .args(["replay", log.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--channels", "3"])
        .output()
        .unwrap();
    assert!(stderr(&bad).contains("no standard layout"), "{}", stderr(&bad));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_server_refuses_what_it_cannot_do() {
    let out = niminal().args(["server", "--no-audio", "--channels", "3"]).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no standard layout"));

    let out = niminal()
        .args(["server", "--no-audio", "--listen", "0.0.0.0", "--port", &free_port().to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("TLS"), "{}", stderr(&out));
}
