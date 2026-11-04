//! A tempo change that lands on a bar line must not swallow the note on that downbeat.

use std::process::Command;

fn render(name: &str, source: &str) -> Vec<f32> {
    let dir = std::env::temp_dir().join(format!("niminal-ramp-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (file, out) = (dir.join("ramp.nml"), dir.join("ramp.wav"));
    std::fs::write(&file, source).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_niminal"))
        .args(["render"])
        .arg(&file)
        .args(["--bars", "3", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    hound::WavReader::open(&out)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

/// Seconds at which a click starts: the first loud sample after at least 50ms of quiet (stereo, 48k).
fn onsets(samples: &[f32]) -> Vec<f64> {
    let mut found = Vec::new();
    let mut quiet = usize::MAX;
    for (i, frame) in samples.chunks(2).enumerate() {
        if frame[0].abs() > 0.05 {
            if quiet > 2_400 {
                found.push(i as f64 / 48_000.0);
            }
            quiet = 0;
        } else {
            quiet = quiet.saturating_add(1);
        }
    }
    found
}

fn piece(change: &str) -> String {
    format!(
        "config {{ channels: stereo }}
tempo 120bpm
instr click(freq: hz = 1000hz) {{ (osc(sine, freq) * env[1 15ms.exp 0]).pan(angle: 0deg) }}
clip c {{ instrument: click, notes: [c5 d5 e5 g5] }}
c.start
{change}
"
    )
}

#[test]
fn the_note_on_the_downbeat_where_a_short_ramp_starts_still_sounds() {
    let heard = onsets(&render("short", &piece("tempo 60bpm @ next 2bars over 1beat")));
    // bar 1 at 120bpm is four notes half a second apart; the ramp begins as bar 2 does, at 4 seconds
    assert!(
        heard.iter().any(|t| (t - 4.0).abs() < 0.02),
        "no note at the bar line: {heard:?}"
    );
}

#[test]
fn a_ramp_over_a_bar_keeps_the_downbeat_too() {
    let heard = onsets(&render("long", &piece("tempo 60bpm @ next 2bars over 1bar")));
    assert!(heard.iter().any(|t| (t - 4.0).abs() < 0.02), "{heard:?}");
}

#[test]
fn speeding_up_keeps_the_downbeat_too() {
    let heard = onsets(&render("faster", &piece("tempo 140bpm @ next 2bars over 1beat")));
    assert!(heard.iter().any(|t| (t - 4.0).abs() < 0.02), "{heard:?}");
}

#[test]
fn no_note_is_lost_or_doubled_through_a_ramp() {
    // four notes a bar for three bars is twelve, wherever the tempo goes
    for (name, change) in [
        ("a", "tempo 60bpm @ next 2bars over 1beat"),
        ("b", "tempo 140bpm @ next 2bars over 1beat"),
        ("c", "tempo 90bpm @ next bar over 2beats"),
    ] {
        let heard = onsets(&render(name, &piece(change)));
        assert!(heard.len() >= 12, "{change}: {heard:?}");
    }
}
