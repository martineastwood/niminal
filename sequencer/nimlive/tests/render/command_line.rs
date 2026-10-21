use std::process::Command;

fn niminal() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nimlive"))
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("niminal-cli-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const OPEN_ENDED: &str = "\ninstr p(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip t { instrument: p }\nt.play([c4 e4])\n";

#[test]
fn bars_bound_a_piece_that_plays_until_stopped() {
    let dir = scratch("bars");
    let src = dir.join("open.nml");
    std::fs::write(&src, OPEN_ENDED).unwrap();
    let wav = dir.join("open.wav");

    let refused = niminal()
        .args([
            "render",
            src.to_str().unwrap(),
            "--out",
            wav.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--bars"));

    let ok = niminal()
        .args([
            "render",
            src.to_str().unwrap(),
            "--out",
            wav.to_str().unwrap(),
            "--bars",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let said = String::from_utf8_lossy(&ok.stdout);
    assert!(said.contains("1 channel"), "{said}");
    let reader = hound::WavReader::open(&wav).unwrap();
    // 2bars at the default 120bpm is 4 seconds, plus the last note's release
    let seconds = reader.duration() as f64 / 48_000.0;
    assert!((3.9..5.0).contains(&seconds), "{seconds}");

    let seconds_flag = niminal()
        .args([
            "render",
            src.to_str().unwrap(),
            "--out",
            wav.to_str().unwrap(),
            "--seconds",
            "1",
        ])
        .output()
        .unwrap();
    assert!(seconds_flag.status.success());
    assert!(hound::WavReader::open(&wav).unwrap().duration() < 48_000 * 2);

    let both = niminal()
        .args([
            "render",
            src.to_str().unwrap(),
            "--out",
            wav.to_str().unwrap(),
            "--bars",
            "1",
            "--seconds",
            "1",
        ])
        .output()
        .unwrap();
    assert!(!both.status.success(), "the two lengths conflict");
    std::fs::remove_dir_all(dir).ok();
}
