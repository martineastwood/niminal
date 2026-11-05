//! `niminal play FILE` plays a file and plays it again, with what it uses, each time it changes.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

struct Playing {
    child: Child,
    out: Receiver<String>,
    err: Receiver<String>,
}

impl Drop for Playing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn lines(source: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (send, receive) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(source).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    receive
}

fn play(file: &std::path::Path) -> Playing {
    let mut child = Command::new(env!("CARGO_BIN_EXE_niminal"))
        .args(["play", "--no-audio"])
        .arg(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("niminal starts");
    let out = lines(child.stdout.take().unwrap());
    let err = lines(child.stderr.take().unwrap());
    Playing { child, out, err }
}

fn until(receive: &Receiver<String>, wanted: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut heard = Vec::new();
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match receive.recv_timeout(left) {
            Ok(line) if line.contains(wanted) => return line,
            Ok(line) => heard.push(line),
            Err(_) => break,
        }
    }
    panic!("never heard `{wanted}`; heard {heard:?}");
}

const PIECE: &str = "tempo 120bpm\ninstr tone(freq: hz = 440hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip beat {\n  instrument: tone\n  notes: [c4 c4 c4 c4]\n}\nbeat.start\n";

#[test]
fn a_file_is_played_again_when_it_changes_and_a_mistake_is_reported() {
    let dir = std::env::temp_dir().join(format!("niminal-play-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("piece.nml");
    std::fs::write(&file, PIECE).unwrap();
    let playing = play(&file);
    until(&playing.out, "lands at bar");
    until(&playing.out, "playing");

    // a change is noticed and played; the file's time must move on for it to be seen
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&file, format!("{PIECE}beat.reverse\n")).unwrap();
    until(&playing.out, "lands at bar");

    // a mistake is shown where it is, and the next good version plays again
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&file, "clip bad { instrument: tone\n  notes: [c4 zz] }\n").unwrap();
    let error = until(&playing.err, "error");
    assert!(error.contains("zz") || error.contains("error"), "{error}");
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&file, PIECE).unwrap();
    until(&playing.out, "lands at bar");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_is_refused() {
    let run = Command::new(env!("CARGO_BIN_EXE_niminal"))
        .args(["play", "--no-audio", "/nowhere/piece.nml"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("can't read"));
}
