//! Audio regression checks for every example and a scripted live session.
//! Compare render dimensions exactly and block RMS levels with a tolerance: DSP math can
//! round differently across platforms. Set `BLESS=1` to update dimensions, and `REFERENCE=1`
//! to update audio references after an understood change.

use std::path::{Path, PathBuf};

use niminal_lang::Layout;
use niminal_live::Offline;

const SR: f32 = 48_000.0;
const BAR: u64 = 96_000; // 120bpm in 4/4

fn dimensions(channels: &[Vec<f32>]) -> String {
    format!("{}x{}", channels.len(), channels.first().map_or(0, Vec::len))
}

/// How loud each stretch of a render is: the RMS of every `DIGEST_BLOCK` samples of each channel. It is what
/// "sounds the same" means across a change that moves the rounding of the mix, such as cutting blocks
/// somewhere else.
const DIGEST_BLOCK: usize = 1024;

fn digest(channels: &[Vec<f32>]) -> String {
    channels
        .iter()
        .map(|c| {
            c.chunks(DIGEST_BLOCK)
                .map(|b| format!("{:.5e}", (b.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / b.len() as f64).sqrt()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn close(a: &str, b: &str) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.split_whitespace().collect(), b.split_whitespace().collect());
    a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            if x == y {
                return true;
            }
            match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(x), Ok(y)) => (x - y).abs() <= 1e-4 * x.abs().max(y.abs()) + 1e-6,
                _ => false,
            }
        })
}

/// Compare with a reference digest, which is only rewritten with `REFERENCE=1`, on purpose.
fn check_reference(name: &str, lines: Vec<(String, String)>) {
    let path = baseline_file(name);
    let text: String = lines.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
    if std::env::var_os("REFERENCE").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no reference {}; run with REFERENCE=1", path.display()));
    let (got, want): (Vec<_>, Vec<_>) = (text.lines().collect(), expected.lines().collect());
    assert_eq!(got.len(), want.len(), "{name}: different number of renders");
    for (g, w) in got.iter().zip(&want) {
        let (gn, gv) = g.split_once(": ").unwrap();
        let (wn, wv) = w.split_once(": ").unwrap();
        assert_eq!(gn, wn);
        assert!(close(gv, wv), "{name}: {gn} no longer sounds like its reference");
    }
}

fn baseline_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/baseline").join(name)
}

fn check(name: &str, lines: Vec<(String, String)>) {
    let path = baseline_file(name);
    let text: String = lines.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no baseline {}; run with BLESS=1", path.display()));
    assert_eq!(text, expected, "audio differs from {}", path.display());
}

fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let mut r = hound::WavReader::open(path).unwrap();
    let n = usize::from(r.spec().channels);
    let samples: Vec<f32> = r.samples::<f32>().map(Result::unwrap).collect();
    (0..n).map(|c| samples.iter().skip(c).step_by(n).copied().collect()).collect()
}

#[test]
fn examples_render_as_they_did() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut files: Vec<_> = std::fs::read_dir(&examples).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "nml")).collect();
    files.sort();
    let dir = std::env::temp_dir().join(format!("niminal-baseline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut lines, mut digests) = (Vec::new(), Vec::new());
    for file in files {
        let out = dir.join("out.wav");
        let _ = std::fs::remove_file(&out);
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_nimlive"));
        command.arg("render").arg(&file);
        if file.file_name().is_some_and(|name| name == "song.nml") {
            command.arg("song");
        }
        let run = command.args(["--bars", "4", "--out"]).arg(&out).output().unwrap();
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let (value, sound) = if run.status.success() {
            let audio = read_wav(&out);
            (dimensions(&audio), digest(&audio))
        } else {
            ("does not render".to_string(), "does not render".to_string())
        };
        lines.push((name.clone(), value));
        digests.push((name, sound));
    }
    check("examples.txt", lines);
    check_reference("examples_reference.txt", digests);
}

/// One thing that happens to a live session, and when.
enum Step {
    Eval(&'static str, Option<&'static str>),
    Cancel(Option<u64>),
    Key(u8, u8, u8),
    Panic,
}
use Step::*;

const SETUP: &str = "\ntempo 120bpm\ninstr pluck(freq: hz, cut: hz = 3khz) { osc(saw, freq).lpf(cutoff: cut) * env[1 | 120ms 0] * 0.3 }\ninstr organ(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] * 0.3 }\nbus space\ninstr click(freq: hz) { space += osc(sine, freq) * env[1 0.5ms 0] * 0.3 }\nprocessor hall { out = space.reverb(room: 0.6, damp: 0.5) }\nclip lead { instrument: pluck }\nclip bass { instrument: pluck(cut: 800hz) }\nclip clicks { instrument: click }\nclip pad { instrument: organ\n  keys: midi.keys(channel: 1) }\nctl bright = 3khz\n";

/// Quantized evals, `launch ... then`, live commands, undo, a tempo ramp, cancel, hush, reset, held keys.
fn live_script() -> Vec<(u64, Step)> {
    vec![
        (0, Eval(SETUP, Some("now"))),
        (0, Eval("lead.play([c4 e4 g4 b4])\nbass.play([c2 ~ g2 ~])", Some("next beat"))),
        (BAR / 2, Eval("clicks.play([a5 a5 a5 a5])", Some("next bar"))),
        (BAR, Eval("lead.reverse", Some("next beat"))),
        (BAR + BAR / 3, Eval("lead.undo", Some("next bar"))),
        (2 * BAR, Eval("scene one { lead: [c5 e5], bass: [c2] }\nscene two { lead: [g4 b4], bass: [g2] }\none.start then two after 1bar", Some("next bar"))),
        (2 * BAR + 1234, Eval("bass.mute", Some("next beat"))),
        (3 * BAR, Eval("tempo 141bpm @ next bar over 2bars", Some("now"))),
        (3 * BAR + 5000, Eval("lead.play([c4 d4 e4 f4 g4 a4 b4 c5])", Some("next bar"))),
        (3 * BAR + 5100, Eval("bass.unmute\nbass.play([c3 c3])", Some("next 2bars"))),
        (4 * BAR, Eval("clicks.play([a5 a5])", Some("next 4bars"))),
        (4 * BAR + 100, Cancel(None)),
        (4 * BAR + 20_000, Key(1, 60, 100)),
        (4 * BAR + 40_000, Key(1, 64, 90)),
        (4 * BAR + 60_000, Key(1, 60, 0)),
        (4 * BAR + 80_000, Key(1, 64, 0)),
        (5 * BAR, Eval("pad.start", Some("now"))),
        (5 * BAR + 30_000, Key(1, 67, 110)),
        (5 * BAR + 90_000, Key(1, 67, 0)),
        (6 * BAR, Eval("hush", Some("next beat"))),
        (7 * BAR, Eval("lead.play([c4 e4])", Some("now"))),
        (7 * BAR + 10_000, Eval("reset", Some("now"))),
        (7 * BAR + 40_000, Eval("lead.play([g4 g4])", Some("next bar"))),
        (8 * BAR, Panic),
    ]
}

/// Play `script` into a fresh session and return `frames` of audio, block-size independent.
fn play(script: &[(u64, Step)], frames: u64) -> Vec<Vec<f32>> {
    let mut session = Offline::new(SR, Layout::Stereo).without_limiter();
    let mut out = vec![Vec::new(); 2];
    let mut run_to = |s: &mut Offline, to: u64| {
        let gap = to.saturating_sub(s.clock()) as usize;
        for (o, p) in out.iter_mut().zip(s.process(gap)) {
            o.extend(p);
        }
    };
    for (at, step) in script {
        run_to(&mut session, *at);
        match step {
            Eval(src, q) => {
                if let Err(problems) = session.eval(src, *q) {
                    panic!("rejected at {at}: {}", problems[0].message);
                }
            }
            Cancel(id) => {
                session.cancel(*id);
            }
            Key(c, n, v) => {
                session.key_input(*c, *n, *v);
            }
            Panic => session.panic(),
        }
    }
    run_to(&mut session, frames);
    out
}

#[test]
fn a_scripted_live_session_sounds_as_it_did() {
    let audio = play(&live_script(), 10 * BAR);
    assert!(audio[0].iter().any(|s| s.abs() > 0.01), "the session made sound");
    check("live_session.txt", vec![("live_session".into(), dimensions(&audio))]);
    check_reference("live_reference.txt", vec![("live_session".into(), digest(&audio))]);
}
