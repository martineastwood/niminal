//! The native code generator must not change what anyone hears: every example renders to the same
//! bytes with it off (`NIMINAL_JIT=0`, the interpreter) and on. The setting is read once per process,
//! so each render is its own process.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn render(file: &Path, jit: &str, out: &Path) -> Option<Vec<u8>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_niminal"));
    command.arg("render").arg(file);
    if file
        .file_name()
        .is_some_and(|name| name == "song.nml" || name == "section_sweep.nml")
    {
        command.arg("song");
    }
    let run = command
        .args(["--bars", "2", "--out"])
        .arg(out)
        .env("NIMINAL_JIT", jit)
        .output()
        .unwrap();
    run.status.success().then(|| std::fs::read(out).unwrap())
}

#[test]
fn every_example_renders_the_same_with_the_jit_off_and_on() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut files: Vec<_> = std::fs::read_dir(&examples)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "nml"))
        // renders its whole 28 bars whatever `--bars` says; the baseline test skips it too
        .filter(|p| p.file_name().is_some_and(|n| n != "reel_keys.nml"))
        .collect();
    files.sort();
    let dir = std::env::temp_dir().join(format!("niminal-jit-identical-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let next = AtomicUsize::new(0);
    let problems = Mutex::new(Vec::new());
    let rendered = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else { break };
                    let off = render(file, "0", &dir.join(format!("off{i}.wav")));
                    let on = render(file, "1", &dir.join(format!("on{i}.wav")));
                    let name = file.file_name().unwrap().to_string_lossy().into_owned();
                    match (off, on) {
                        (Some(a), Some(b)) if a == b => {
                            rendered.fetch_add(1, Ordering::Relaxed);
                        }
                        (Some(_), Some(_)) => problems
                            .lock()
                            .unwrap()
                            .push(format!("{name}: the audio differs with the JIT on")),
                        (None, None) => {} // does not render either way
                        _ => problems
                            .lock()
                            .unwrap()
                            .push(format!("{name}: renders with the JIT only on or only off")),
                    }
                }
            });
        }
    });
    let problems = problems.into_inner().unwrap();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(
        rendered.load(Ordering::Relaxed) >= 50,
        "too few examples rendered to mean anything"
    );
}
