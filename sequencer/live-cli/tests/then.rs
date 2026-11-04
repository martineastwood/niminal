//! A clip's `then:` runs when its last pass ends, so a render has to be played through a session to hear it.

use std::process::Command;

const SOURCE: &str = "tempo 120bpm
instr first(freq: hz) { osc(sine, freq) * env[1 | 50ms 0] * 0.5 }
instr second(freq: hz) { osc(sine, freq) * env[1 | 50ms 0] * 0.5 }
clip one { instrument: first, notes: [c4], times: 1, then: two.start }
clip two { instrument: second, notes: [c5], times: 1 }
one.start
";

fn render(args: &[&str]) -> (bool, String, Vec<f32>) {
    let dir = std::env::temp_dir().join(format!("niminal-then-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (file, out) = (dir.join("then.nml"), dir.join("then.wav"));
    std::fs::write(&file, SOURCE).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_niminal"))
        .arg("render")
        .arg(&file)
        .args(args)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    let samples = hound::WavReader::open(&out)
        .map(|mut r| r.samples::<f32>().map(Result::unwrap).collect())
        .unwrap_or_default();
    (
        run.status.success(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
        samples,
    )
}

#[test]
fn the_clip_a_then_starts_is_heard() {
    let (ok, error, samples) = render(&["--bars", "3"]);
    assert!(ok, "{error}");
    // a bar at 120bpm is two seconds: the first clip plays in bar 1 and the second in bar 2
    let bar = 2 * 48_000 * 2;
    let loud = |part: &[f32]| part.iter().any(|v| v.abs() > 0.05);
    assert!(
        samples.len() > bar && loud(&samples[..bar]) && loud(&samples[bar..]),
        "{} samples",
        samples.len()
    );
}

#[test]
fn a_file_with_then_asks_how_long_to_render() {
    let (ok, error, _) = render(&[]);
    assert!(
        !ok && error.contains("`then:`") && error.contains("--bars"),
        "{error}"
    );
}
