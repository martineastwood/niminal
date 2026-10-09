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
    render(&program, &[], &RenderOptions { limiter: false, until: Some(2.0), ..Default::default() }).unwrap().channels
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

/// Four 100ms pieces at distinct levels, as one file: a stand-in for a break.
fn write_break(dir: &Path) {
    let levels = [0.1, 0.2, 0.3, 0.4];
    write_wav(&dir.join("amen.wav"), &[levels.iter().flat_map(|&l| block(l, 100)).collect()]);
}

#[test]
fn slices_of_a_sample_play_in_any_order() {
    let dir = scratch("slices");
    write_break(&dir);
    let src = "tempo 120bpm\nsample amen = \"amen.wav\"\nkit chops = amen.slices(4)\n\
               track t { instrument = chops }\nplay t = [s3 s0 s2 s1]";
    let out = play(&dir, src);
    // four steps of half a second; each plays its quarter of the file (100ms) from its start
    let at = |s: f64| out[0].get((s * SR as f64) as usize).copied().unwrap_or(0.0);
    assert!((at(0.05) - 0.4).abs() < 1e-3, "{}", at(0.05));
    assert!((at(0.55) - 0.1).abs() < 1e-3, "{}", at(0.55));
    assert!((at(1.05) - 0.3).abs() < 1e-3, "{}", at(1.05));
    assert!((at(1.55) - 0.2).abs() < 1e-3, "{}", at(1.55));
    assert!(at(0.2).abs() < 1e-6, "a slice ends where the next piece of the file would start");
    assert!(at(0.0).abs() < 0.01, "and fades in so it doesn't click");
}

#[test]
fn slices_take_choke_groups_by_name() {
    let dir = scratch("slicekit");
    write_break(&dir);
    let program = compile_in(
        &dir,
        "sample amen = \"amen.wav\"\nkit chops = amen.slices(4) with(choke: g(s0, s1))",
    )
    .unwrap();
    assert_eq!(program.instrument("chops").unwrap().chokes, [Some(0), Some(0), None, None]);
}

#[test]
fn slicing_mistakes_are_reported() {
    let dir = scratch("sliceerr");
    write_break(&dir);
    let e = errors(&dir, "kit c = amen.slices(4)");
    assert!(e.contains("no sample named `amen` to slice"), "{e}");
    let e = errors(&dir, "sample amen = \"amen.wav\"\nkit c = amen.slces(4)");
    assert!(e.contains("a sample has no `slces`"), "{e}");
    let e = errors(&dir, "sample amen = \"amen.wav\"\nkit c = amen.slices(2.5)");
    assert!(e.contains("can be cut into 1 to"), "{e}");
    let e = errors(&dir, "sample amen = \"amen.wav\"\nkit c = amen.slices(4)\ntrack t { instrument = c }\nplay t = [s9]");
    assert!(e.contains("`s9` isn't a note, a value, or a sample in a kit"), "{e}");
    let e = errors(&dir, "sample amen = \"amen.wav\"\nsample c = amen.slices(4)");
    assert!(e.contains("a file path in quotes"), "{e}");
}

#[test]
fn the_breakbeat_example_plays_the_break_back_unchanged_when_the_slices_are_in_order() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    // the break was recorded at 100bpm, so at that tempo it plays back unchanged
    let source = std::fs::read_to_string(dir.join("breakbeat.nml")).unwrap().replace("tempo 130bpm", "tempo 100bpm");
    let program = compile_in(&dir, &source).unwrap_or_else(|e| panic!("{e}"));
    let out = render(&program, &[], &RenderOptions { limiter: false, until: Some(4.8), ..Default::default() }).unwrap().channels;

    // a bar at 100bpm is 2.4s
    let original = hound::WavReader::open(dir.join("break.wav")).unwrap().samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect::<Vec<_>>();
    let rate = |t: f64, n: usize| (t * n as f64) as usize;
    // compare a window in the middle of the bar, away from the joins' tiny fades
    let (mut ours, mut theirs) = (0.0f64, 0.0f64);
    for i in 0..2000 {
        let t = 0.32 + i as f64 / 48_000.0;
        let a = f64::from(out[0][rate(t, SR)]);
        // the master is mono here only if configured; the example is stereo, so left is -3dB of the mono file
        let b = f64::from(original[rate(t, 24_000)]) * std::f64::consts::FRAC_1_SQRT_2;
        ours += a * a;
        theirs += b * b;
    }
    assert!((ours / theirs - 1.0).abs() < 0.1, "{ours} {theirs}");
}

fn end_of(out: &[Vec<f32>]) -> f64 {
    out[0].iter().rposition(|v| v.abs() > 0.1).unwrap() as f64 / SR as f64
}

#[test]
fn a_loop_fitted_to_the_tempo_lasts_its_beats() {
    let dir = scratch("fit");
    // 2.4s is four beats at 100bpm
    write_wav(&dir.join("loop.wav"), &[block(0.5, 2400)]);
    let at = |tempo: u32, opts: &str| {
        let src = format!(
            "tempo {tempo}bpm\nsample l = \"loop.wav\" with({opts})\ntrack t {{ instrument = l }}\nplay t = [c4]"
        );
        end_of(&play(&dir, &src))
    };
    assert!((at(100, "beats: 4, fit: rate") - 2.4).abs() < 0.01);
    assert!((at(120, "beats: 4, fit: rate") - 2.0).abs() < 0.01, "four beats at 120bpm");
    assert!((at(60, "beats: 4, fit: rate") - 4.0).abs() < 0.01, "and at 60bpm");
    assert!((at(120, "beats: 4") - 2.4).abs() < 0.01, "without fit it plays as recorded");
}

#[test]
fn slices_of_a_fitted_loop_follow_the_tempo_too() {
    let dir = scratch("fitslices");
    write_wav(&dir.join("loop.wav"), &[block(0.5, 2400)]);
    let src = "tempo 120bpm\nsample l = \"loop.wav\" with(beats: 4, fit: rate)\nkit k = l.slices(4)\ntrack t { instrument = k }\nplay t = [s3]";
    // the last quarter of the loop is a step long (0.5s at 120bpm) once fitted, and starts on the downbeat
    let out = play(&dir, src);
    assert!((end_of(&out) - 0.5).abs() < 0.01, "{}", end_of(&out));
}

#[test]
fn fit_mistakes_are_reported() {
    let dir = scratch("fiterr");
    write_wav(&dir.join("loop.wav"), &[block(0.5, 100)]);
    let e = errors(&dir, "sample l = \"loop.wav\" with(fit: rate)");
    assert!(e.contains("needs to know the loop's length"), "{e}");
    let e = errors(&dir, "sample l = \"loop.wav\" with(beats: 4, fit: stretch)");
    assert!(e.contains("`fit: stretch` isn't supported yet"), "{e}");
    let e = errors(&dir, "sample l = \"loop.wav\" with(beats: 4, fit: fast)");
    assert!(e.contains("`fit` should be `rate`"), "{e}");
    let e = errors(&dir, "sample l = \"loop.wav\" with(beats: -1)");
    assert!(e.contains("`beats` is how many beats"), "{e}");
}

#[test]
fn the_song_example_renders_its_arrangement() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let source = std::fs::read_to_string(dir.join("song.nml")).unwrap();
    let program = compile_in(&dir, &source).unwrap_or_else(|e| panic!("{e}"));
    let options = RenderOptions { arrangement: Some("song".into()), ..Default::default() };
    let out = render(&program, &[], &options).unwrap();
    // 16 bars at 110bpm is 34.9s, plus the reverb's tail
    let secs = out.len() as f64 / SR as f64;
    assert!((34.9..45.0).contains(&secs), "{secs}");
    assert!(out.peak() > 0.1 && out.peak() <= 1.0);
}
