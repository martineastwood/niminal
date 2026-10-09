use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use niminal_cli::server::Client;

fn niminal() -> Command {
    Command::new(env!("CARGO_BIN_EXE_niminal"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("niminal-live-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A daemon running as a child process, killed when dropped.
struct Daemon {
    child: Child,
    port: u16,
}

impl Daemon {
    fn start(extra: &[&str]) -> Daemon {
        let port = free_port();
        let mut child = niminal()
            .args(["daemon", "--no-audio", "--port", &port.to_string()])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        assert!(line.contains("listening on ws://127.0.0.1"), "{line}");
        Daemon { child, port }
    }

    fn send(&self, code: &str, quantize: Option<&str>) -> std::process::Output {
        let mut cmd = niminal();
        cmd.args(["send", code, "--port", &self.port.to_string()]);
        if let Some(q) = quantize {
            cmd.args(["--quantize", q]);
        }
        cmd.output().unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const SETUP: &str = "tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track lead { instrument = pluck }";

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn send_evaluates_code_in_a_running_daemon() {
    let d = Daemon::start(&[]);
    let applied = d.send(SETUP, Some("now"));
    assert!(applied.status.success(), "{}", stderr(&applied));
    assert_eq!(stdout(&applied).trim(), "applied");

    std::thread::sleep(Duration::from_millis(100));
    let waiting = d.send("play lead = [c4]", None);
    assert!(waiting.status.success());
    assert!(stdout(&waiting).starts_with("lands at bar 2 beat 1.00"), "{}", stdout(&waiting));

    let nothing = d.send("// just a comment", None);
    assert_eq!(stdout(&nothing).trim(), "nothing to do");
}

#[test]
fn send_shows_what_is_wrong_with_the_code_sent() {
    let d = Daemon::start(&[]);
    d.send(SETUP, Some("now"));
    let bad = d.send("riff = [c4]\nplay lead = riif", None);
    assert!(!bad.status.success());
    let text = stderr(&bad);
    assert!(text.contains("error: `riif` is not defined"), "{text}");
    assert!(text.contains("--> <code>:2:13"), "{text}");
    assert!(text.contains("play lead = riif"), "{text}");
    assert!(text.contains("help: did you mean `riff`?") || !text.contains("help"), "{text}");
}

#[test]
fn send_reads_a_file_or_the_standard_input() {
    let d = Daemon::start(&[]);
    let dir = scratch("send-file");
    let file = dir.join("setup.nml");
    std::fs::write(&file, SETUP).unwrap();
    let out = niminal()
        .args(["send", "--file", file.to_str().unwrap(), "--port", &d.port.to_string(), "--quantize", "now"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    let mut child = niminal()
        .args(["send", "--port", &d.port.to_string(), "--quantize", "now"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"lead(freq: a4) for 1beat").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn sending_to_nothing_says_so() {
    let out = niminal().args(["send", "hush", "--port", &free_port().to_string()]).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("can't reach the daemon"), "{}", stderr(&out));
}

#[test]
fn a_recorded_session_replays_to_audio() {
    let dir = scratch("replay");
    let log = dir.join("set.log");
    {
        let d = Daemon::start(&["--log", log.to_str().unwrap()]);
        d.send(SETUP, Some("now"));
        d.send("lead(freq: a4) for 2beats", Some("now"));
        d.send("lead(freq: e5) for 2beats", Some("next beat"));
        // the log is written a few times a second
        std::thread::sleep(Duration::from_millis(700));
    }
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert!(text.lines().next().unwrap().contains("\"eval\""));

    let wav = dir.join("set.wav");
    let out = niminal()
        .args(["render", "--replay", log.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--channels", "2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("replayed 3 inputs"), "{}", stdout(&out));
    let mut reader = hound::WavReader::open(&wav).unwrap();
    assert_eq!(reader.spec().channels, 2);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1 && peak <= 0.9661, "{peak}");
    // two one-second notes, the second starting on the next beat
    let seconds = samples.len() as f32 / 2.0 / 48_000.0;
    assert!((1.0..4.0).contains(&seconds), "{seconds}");

    // an unlimited replay can be asked for, and a bad layout is refused
    let raw = niminal()
        .args(["render", "--replay", log.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--no-limiter"])
        .output()
        .unwrap();
    assert!(raw.status.success());
    let bad = niminal().args(["render", "--replay", log.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--channels", "3"]).output().unwrap();
    assert!(stderr(&bad).contains("no standard layout"), "{}", stderr(&bad));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_project_file_is_loaded_and_loaded_again_when_it_is_saved() {
    let dir = scratch("watch");
    let project = dir.join("set.nml");
    std::fs::write(&project, format!("{SETUP}\nriff = [c4]\nplay lead = riff\n")).unwrap();
    let d = Daemon::start(&[project.to_str().unwrap()]);

    let mut c = Client::connect(d.port, None).unwrap();
    c.call("subscribe", serde_json::json!({"topics": ["landed"]})).unwrap();
    // the file was evaluated at once
    std::thread::sleep(Duration::from_millis(400));
    let status = c.call("status", serde_json::json!({})).unwrap();
    assert!(status["transport"]["voices"].as_u64().unwrap() >= 1);

    // saving it again evaluates it again; the change waits for a boundary and lands
    std::thread::sleep(Duration::from_millis(1100)); // so the modification time differs
    std::fs::write(&project, format!("{SETUP}\nriff = [e4]\nplay lead = riff\n")).unwrap();
    let landed = c.wait_for("landed", Duration::from_secs(8)).unwrap();
    assert!(landed["changes"].to_string().contains("riff"), "{landed}");

    // a mistake in the file is reported and changes nothing
    std::fs::write(&project, format!("{SETUP}\nplay lead = nothing_here\n")).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert!(c.call("status", serde_json::json!({})).unwrap()["pending"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_daemon_refuses_what_it_cannot_do() {
    let out = niminal().args(["daemon", "--no-audio", "--channels", "3"]).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no standard layout"));

    let out = niminal().args(["daemon", "--no-audio", "--listen", "0.0.0.0", "--port", &free_port().to_string()]).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("TLS"), "{}", stderr(&out));
}

#[test]
fn repl_runs_multiline_blocks_and_reports_errors_without_stopping() {
    let daemon = Daemon::start(&[]);
    let mut repl = niminal()
        .args(["repl", "--port", &daemon.port.to_string(), "--quantize", "now"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = "tempo 100bpm\ninstr tone(freq: hz) {\n  osc(sine, freq)\n}\ntrack t { instrument = tone }\nplay t = [c4 zz]\nplay t = [c4]\n";
    repl.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = repl.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("`zz` isn't a note"), "{stderr}");
    // The tempo, the instrument and track, and the final play all went through.
    assert_eq!(stdout.matches("applied").count() + stdout.matches("lands at").count(), 4, "{stdout}");
}
