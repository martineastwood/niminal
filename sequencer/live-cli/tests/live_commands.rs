use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use niminal_ws::Client;

/// The server, `niminal`, built from the engine workspace the first time it is needed.
fn niminal() -> Command {
    static BUILT: std::sync::Once = std::sync::Once::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    BUILT.call_once(|| {
        let status = Command::new("cargo")
            .args(["build", "-q", "-p", "niminal-cli", "--manifest-path"])
            .arg(root.join("Cargo.toml"))
            .status()
            .unwrap();
        assert!(status.success(), "the niminal server would not build");
    });
    Command::new(root.join("target/debug/niminal"))
}

fn live_cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_niminal-live"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("niminal-live-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A free port that no other test in this process was just given (the system may hand a port out again as soon as it is released).
fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    let mut given = GIVEN.lock().unwrap();
    loop {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        if !given.contains(&port) {
            given.push(port);
            return port;
        }
    }
}

/// A server running as a child process, killed when dropped.
struct Server {
    child: Child,
    engine: Child,
    port: u16,
}

impl Server {
    fn start(extra: &[&str]) -> Server {
        // Another test may take the port between choosing it and binding it, so try again.
        for _ in 0..5 {
            let (port, engine_port) = (free_port(), free_port());
            let (engine_args, live_args) = if extra.first() == Some(&"--log") {
                (extra, &[][..])
            } else {
                (&[][..], extra)
            };
            let mut engine = niminal()
                .args(["server", "--no-audio", "--port", &engine_port.to_string()])
                .args(engine_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            BufReader::new(engine.stdout.as_mut().unwrap())
                .read_line(&mut line)
                .unwrap();
            if !line.contains("niminal engine listening") {
                let _ = engine.kill();
                let _ = engine.wait();
                continue;
            }
            let mut child = live_cli()
                .args([
                    "live",
                    "--port",
                    &port.to_string(),
                    "--engine-port",
                    &engine_port.to_string(),
                ])
                .args(live_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut reader = BufReader::new(child.stdout.as_mut().unwrap());
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap() == 0
                    || line.contains("niminal-live listening")
                {
                    break;
                }
            }
            if line.contains("niminal-live listening on ws://127.0.0.1") {
                return Server {
                    child,
                    engine,
                    port,
                };
            }
            let _ = engine.kill();
            let _ = engine.wait();
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("the server would not start");
    }

    fn send(&self, code: &str, quantize: Option<&str>) -> std::process::Output {
        let mut cmd = live_cli();
        cmd.args(["send", code, "--port", &self.port.to_string()]);
        if let Some(q) = quantize {
            cmd.args(["--quantize", q]);
        }
        cmd.output().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.engine.kill();
        let _ = self.engine.wait();
    }
}

const SETUP: &str = "tempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip lead { instrument: pluck }";

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn send_evaluates_code_in_a_running_server() {
    let d = Server::start(&[]);
    let applied = d.send(SETUP, Some("now"));
    assert!(applied.status.success(), "{}", stderr(&applied));
    assert_eq!(stdout(&applied).trim(), "applied");

    std::thread::sleep(Duration::from_millis(100));
    let waiting = d.send("lead.__update([c4])", None);
    assert!(waiting.status.success());
    assert!(
        stdout(&waiting).starts_with("lands at bar 2 beat 1.00"),
        "{}",
        stdout(&waiting)
    );

    let nothing = d.send("// just a comment", None);
    assert_eq!(stdout(&nothing).trim(), "nothing to do");
}

#[test]
fn send_shows_what_is_wrong_with_the_code_sent() {
    let d = Server::start(&[]);
    d.send(SETUP, Some("now"));
    let bad = d.send(
        "riff = [c4]\nclip lead { instrument: pluck\n notes: riif }",
        None,
    );
    assert!(!bad.status.success());
    let text = stderr(&bad);
    assert!(text.contains("error: `riif` is not defined"), "{text}");
    assert!(text.contains("--> <code>:3:9"), "{text}");
    assert!(text.contains("notes: riif }"), "{text}");
    assert!(
        text.contains("help: did you mean `riff`?") || !text.contains("help"),
        "{text}"
    );
}

#[test]
fn send_reads_a_file_or_the_standard_input() {
    let d = Server::start(&[]);
    let dir = scratch("send-file");
    let file = dir.join("setup.nml");
    std::fs::write(&file, SETUP).unwrap();
    let out = live_cli()
        .args([
            "send",
            "--file",
            file.to_str().unwrap(),
            "--port",
            &d.port.to_string(),
            "--quantize",
            "now",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    let mut child = live_cli()
        .args(["send", "--port", &d.port.to_string(), "--quantize", "now"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"lead(freq: a4) for 1beat")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn sending_to_nothing_says_so() {
    let out = live_cli()
        .args(["send", "hush", "--port", &free_port().to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("can't reach the server"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_project_file_is_loaded_and_loaded_again_when_it_is_saved() {
    let dir = scratch("watch");
    let project = dir.join("set.nml");
    std::fs::write(
        &project,
        format!("{SETUP}\nriff = [c4]\nlead.__update(riff)\n"),
    )
    .unwrap();
    let d = Server::start(&[project.to_str().unwrap()]);

    let mut c = Client::connect(d.port, None).unwrap();
    c.call("subscribe", serde_json::json!({"topics": ["landed"]}))
        .unwrap();
    // the file was evaluated at once
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = c.call("status", serde_json::json!({})).unwrap();
        if status["transport"]["voices"].as_u64().unwrap() >= 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the file was never evaluated: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // saving it again evaluates it again; the change waits for a boundary and lands
    std::thread::sleep(Duration::from_millis(1100)); // so the modification time differs
    std::fs::write(
        &project,
        format!("{SETUP}\nriff = [e4]\nlead.__update(riff)\n"),
    )
    .unwrap();
    let landed = c.wait_for("landed", Duration::from_secs(8)).unwrap();
    assert!(landed["changes"].to_string().contains("riff"), "{landed}");

    // a mistake in the file is reported and changes nothing
    std::fs::write(&project, format!("{SETUP}\nlead.__update(nothing_here)\n")).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        c.call("status", serde_json::json!({})).unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn repl_runs_multiline_blocks_and_reports_errors_without_stopping() {
    let server = Server::start(&[]);
    let mut repl = live_cli()
        .args([
            "repl",
            "--port",
            &server.port.to_string(),
            "--quantize",
            "now",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = "tempo 100bpm\ninstr tone(freq: hz) {\n  osc(sine, freq)\n}\nclip t { instrument: tone }\nt.__update([c4 zz])\nt.__update([c4])\n";
    repl.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = repl.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("`zz` isn't a note"), "{stderr}");
    // The tempo, the instrument and node, and the final play all went through.
    assert_eq!(
        stdout.matches("applied").count() + stdout.matches("lands at").count(),
        4,
        "{stdout}"
    );
}

#[test]
fn the_lsp_subcommand_answers_an_editor_over_stdio() {
    use std::io::Read;
    let mut child = live_cli()
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for body in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ] {
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    }
    drop(stdin);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(out.contains("hoverProvider"), "{out}");
}
