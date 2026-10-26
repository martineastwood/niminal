//! Modules through the whole path: joined by the sequencer, sent to the engine, and played.

use std::path::Path;

fn project(files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "niminal-modules-{}-{}",
        std::process::id(),
        files.iter().map(|(n, _)| n.len()).sum::<usize>()
    ));
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

/// The peak of what the file sounds, played the way a render plays it.
fn peak(source: &str, dir: &Path) -> f32 {
    let expanded = niminal_compose::expand(source, dir).unwrap_or_else(|d| panic!("{}", d.message));
    let program = niminal_compose::compile_with(&expanded.text, &Default::default())
        .unwrap_or_else(|e| {
            panic!(
                "{:?}",
                e.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
            )
        });
    let mut session = niminal_live::Offline::new(48_000.0, niminal_lang::Layout::Mono);
    let schedule = program.schedule();
    session.play_program(program, schedule);
    let out = session.process(24_000);
    let notices = session.take_notices();
    assert!(notices.is_empty(), "{notices:?}");
    out[0].iter().fold(0.0, |m, v| m.max(v.abs()))
}

const SONG: &str = "tempo 100bpm\ninstr pad(freq: hz) { osc(saw, freq).twice() * env[0 20ms 1 | 400ms 0] }\npad(freq: a3) for 2beats\n";

#[test]
fn a_module_with_private_names_plays() {
    let dir = project(&[(
        "lib/rig.nml",
        "const _gain = 0.5\nfn _half(x) { x * _gain }\nfn twice(x) { _half(x) * 2 }\n",
    )]);
    assert!(peak(&format!("use rig\n{SONG}"), &dir) > 0.5);
    // under an alias too
    let source = format!("use rig as r\n{}", SONG.replace("twice()", "r::twice()"));
    assert!(peak(&source, &dir) > 0.5);
}

#[test]
fn the_library_that_comes_with_niminal_plays() {
    let source = "use std/filters\ntempo 100bpm\ninstr pad(freq: hz) { osc(saw, freq).biquad_lp(2khz) * env[0 20ms 1 | 400ms 0] }\npad(freq: a3) for 2beats\n";
    assert!(peak(source, Path::new(".")) > 0.1);
}
