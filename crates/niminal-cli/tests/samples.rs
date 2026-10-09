use std::path::{Path, PathBuf};

use niminal_cli::render::{RenderOptions, SAMPLE_RATE, render};
use niminal_lang::{CompileOptions, Program, Samples, compile_with};

const SR: usize = SAMPLE_RATE as usize;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("niminal-samples-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a 16-bit WAV at the engine's rate; `channels` is one buffer per channel.
fn write_wav(path: &Path, channels: &[Vec<f32>]) {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..channels[0].len() {
        for c in channels {
            w.write_sample((c[i] * 32767.0) as i16).unwrap();
        }
    }
    w.finalize().unwrap();
}

/// A constant level for `ms` milliseconds, which is easy to find in a mix.
fn block(level: f32, ms: usize) -> Vec<f32> {
    vec![level; SR * ms / 1000]
}

fn ramp(len: usize) -> Vec<f32> {
    (0..len).map(|i| i as f32 / len as f32 * 0.5).collect()
}

fn errors(dir: &Path, src: &str) -> String {
    match compile_in(dir, src) {
        Ok(_) => panic!("expected errors compiling:\n{src}"),
        Err(e) => e,
    }
}

fn compile_in(dir: &Path, src: &str) -> Result<Program, String> {
    let options = CompileOptions { samples: Samples::new(dir), ..Default::default() };
    compile_with(src, &options).map_err(|errs| errs.iter().map(|d| d.render("test", src)).collect::<String>())
}

fn play(dir: &Path, src: &str) -> Vec<Vec<f32>> {
    let program = compile_in(dir, src).unwrap_or_else(|e| panic!("{e}"));
    render(&program, &[], &RenderOptions { limiter: false, until: Some(2.0) }).unwrap().channels
}

const TRACK: &str = "tempo 120bpm\nsample s = \"s.wav\"\ntrack t { instrument = s }\n";

#[test]
fn a_sample_at_its_root_plays_as_recorded() {
    let dir = scratch("root");
    let audio = ramp(2000);
    write_wav(&dir.join("s.wav"), std::slice::from_ref(&audio));
    let out = play(&dir, &format!("{TRACK}play t = [c4]"));
    assert!((0..2000).all(|i| (out[0][i] - audio[i]).abs() < 1e-3), "{:?}", &out[0][..8]);
    assert!(out[0][2000..].iter().all(|v| v.abs() < 1e-6) && out[0].len() < 2200, "it plays once and stops");
}

#[test]
fn a_higher_note_plays_faster() {
    let dir = scratch("pitch");
    write_wav(&dir.join("s.wav"), &[block(0.5, 100)]);
    let length = |note: &str| {
        let out = play(&dir, &format!("{TRACK}play t = [{note}]"));
        out[0].iter().rposition(|v| v.abs() > 0.1).unwrap()
    };
    let (root, up) = (length("c4"), length("c5"));
    assert!((root as f64 / up as f64 - 2.0).abs() < 0.01, "{root} {up}");
}

#[test]
fn the_root_option_shifts_which_note_is_the_original() {
    let dir = scratch("rootopt");
    write_wav(&dir.join("s.wav"), &[block(0.5, 100)]);
    let src = TRACK.replace("\"s.wav\"", "\"s.wav\" with(root: a3)") + "play t = [a3]";
    let out = play(&dir, &src);
    let end = out[0].iter().rposition(|v| v.abs() > 0.1).unwrap();
    assert!((end as f64 - 4800.0).abs() < 4.0, "{end}");
}

#[test]
fn a_kit_plays_the_samples_a_pattern_names() {
    let dir = scratch("kit");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 100)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(-0.25, 100)]);
    let src = "tempo 120bpm\nkit drums = \"drums\"\ntrack d { instrument = drums }\nplay d = [kick ~ snare ~]";
    let out = play(&dir, src);
    let at = |s: f64| out[0].get((s * SR as f64) as usize + 100).copied().unwrap_or(0.0);
    // a bar at 120bpm is 2 seconds: kick on the downbeat, snare on the third beat
    assert!((at(0.0) - 0.5).abs() < 1e-3, "{}", at(0.0));
    assert!(at(0.5).abs() < 1e-6);
    assert!((at(1.0) + 0.25).abs() < 1e-3, "{}", at(1.0));
    assert!(at(1.5).abs() < 1e-6);
}

#[test]
fn a_stereo_sample_keeps_its_sides() {
    let dir = scratch("stereo");
    write_wav(&dir.join("s.wav"), &[block(0.5, 50), block(-0.5, 50)]);
    let src = format!("config {{ channels: stereo }}\n{TRACK}play t = [c4]");
    let out = play(&dir, &src);
    assert_eq!(out.len(), 2);
    assert!((out[0][100] - 0.5).abs() < 1e-3 && (out[1][100] + 0.5).abs() < 1e-3);
}

#[test]
fn a_mono_sample_is_centred_in_a_stereo_mix() {
    let dir = scratch("centre");
    write_wav(&dir.join("s.wav"), &[block(0.5, 50)]);
    let out = play(&dir, &format!("config {{ channels: stereo }}\n{TRACK}play t = [c4]"));
    assert!((out[0][100] - out[1][100]).abs() < 1e-6 && out[0][100] > 0.3);
}

#[test]
fn file_and_name_mistakes_are_reported_where_they_are() {
    let dir = scratch("errors");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("s.wav"), &[block(0.5, 10)]);

    let missing = errors(&dir, "sample s = \"nope.wav\"");
    assert!(missing.contains("can't read") && missing.contains("nope.wav"), "{missing}");
    assert!(missing.contains("1 | sample s = \"nope.wav\""), "points at the declaration: {missing}");

    let typo = errors(&dir, "kit drums = \"drums\"\ntrack d { instrument = drums }\nplay d = [kick ~ snar ~]");
    assert!(typo.contains("`snar` isn't a note, a value, or a sample in a kit"), "{typo}");
    assert!(typo.contains("did you mean `snare`?"), "{typo}");

    let wrong_kit = "kit a = \"drums\"\nsample s = \"s.wav\"\nkit b = \"drums\"\ntrack t { instrument = s }\nplay t = [kick]";
    let on_sample = {
        let p = compile_in(&dir, wrong_kit);
        // a sample track has no kit to pick from; found when the note is planned
        let program = p.unwrap();
        let event = program.schedule().events(0.0, 1.0).unwrap();
        program.plan(&event[0]).unwrap_err().to_string()
    };
    assert!(on_sample.contains("isn't a kit"), "{on_sample}");

    let option = errors(&dir, "sample s = \"s.wav\" with(rot: c4)");
    assert!(option.contains("`rot` isn't an option"), "{option}");

    let name = errors(&dir, "sample s = \"s.wav\"\ninstr s(freq: hz) { osc(sine, freq) }");
    assert!(name.contains("`s` is already the name of"), "{name}");
}

#[test]
fn a_kit_sample_on_the_wrong_kit_is_an_error() {
    let dir = scratch("twokits");
    for kit in ["a", "b"] {
        std::fs::create_dir_all(dir.join(kit)).unwrap();
    }
    write_wav(&dir.join("a/kick.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("b/hat.wav"), &[block(0.5, 10)]);
    let program = compile_in(
        &dir,
        "kit a = \"a\"\nkit b = \"b\"\ntrack t { instrument = a }\nplay t = [hat]",
    )
    .unwrap();
    let events = program.schedule().events(0.0, 1.0).unwrap();
    let err = program.plan(&events[0]).unwrap_err().to_string();
    assert!(err.contains("`hat` isn't in the kit `a`"), "{err}");
}

#[test]
fn a_choke_group_cuts_off_the_sample_it_replaces() {
    let dir = scratch("choke");
    std::fs::create_dir_all(dir.join("hats")).unwrap();
    write_wav(&dir.join("hats/open.wav"), &[block(0.5, 1500)]);
    write_wav(&dir.join("hats/closed.wav"), &[block(0.125, 20)]);
    let src = |opts: &str| {
        format!("tempo 120bpm\nkit h = \"hats\"{opts}\ntrack t {{ instrument = h }}\nplay t = [open ~ closed ~]")
    };
    let at = |out: &[Vec<f32>], s: f64| out[0].get((s * SR as f64) as usize).copied().unwrap_or(0.0);

    // without a group, the open hat rings on under the closed one
    let free = play(&dir, &src(""));
    assert!((at(&free, 0.5) - 0.5).abs() < 1e-3);
    assert!((at(&free, 1.005) - 0.625).abs() < 1e-3, "{}", at(&free, 1.005));
    assert!((at(&free, 0.95) - 0.5).abs() < 1e-3);

    // in one group, the closed hat ends it: nothing is left of the open hat once the fade is done
    let choked = play(&dir, &src(" with(choke: hats(open, closed))"));
    assert!((at(&choked, 0.99) - 0.5).abs() < 1e-3, "before the closed hat it's still sounding");
    assert!((at(&choked, 1.01) - 0.125).abs() < 1e-3, "{}", at(&choked, 1.01));
    assert!(at(&choked, 1.05).abs() < 1e-6, "and afterwards only silence");
}

#[test]
fn choke_options_are_checked() {
    let dir = scratch("chokeerr");
    std::fs::create_dir_all(dir.join("hats")).unwrap();
    write_wav(&dir.join("hats/open.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("hats/closed.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("s.wav"), &[block(0.5, 10)]);

    let e = errors(&dir, "kit h = \"hats\" with(choke: hats(open, clsed))");
    assert!(e.contains("no sample `clsed` in the kit `h`") || e.contains("there is no sample `clsed`"), "{e}");
    assert!(e.contains("did you mean `closed`?"), "{e}");
    let e = errors(&dir, "kit h = \"hats\" with(choke: hats)");
    assert!(e.contains("a kit names the samples in each group"), "{e}");
    let e = errors(&dir, "sample s = \"s.wav\" with(choke: hats(a))");
    assert!(e.contains("expected the name of a choke group"), "{e}");
    let e = errors(&dir, "sample s = \"s.wav\" with(chok: hats)");
    assert!(e.contains("did you mean `choke`?"), "{e}");
}

#[test]
fn single_samples_can_share_a_choke_group_with_each_other_and_a_kit() {
    let dir = scratch("chokeshared");
    write_wav(&dir.join("a.wav"), &[block(0.5, 1500)]);
    write_wav(&dir.join("b.wav"), &[block(0.25, 20)]);
    let src = "tempo 120bpm\nsample a = \"a.wav\" with(choke: g)\nsample b = \"b.wav\" with(choke: g)\n\
               track ta { instrument = a }\ntrack tb { instrument = b }\nplay ta = [c4 ~ ~ ~]\nplay tb = [~ ~ c4 ~]";
    let out = play(&dir, src);
    // `a` would ring until 1.5s; the render ends when `b` ends at 1.02s because `a` was cut off
    assert!(out[0].len() < (1.1 * SR as f64) as usize, "{}", out[0].len());
}
