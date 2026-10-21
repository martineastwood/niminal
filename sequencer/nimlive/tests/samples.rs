use std::path::{Path, PathBuf};

use niminal_compose::{Composition as Program, compile_with};
use niminal_lang::{CompileOptions, Samples};
use nimlive::render::{RenderOptions, SAMPLE_RATE, render};

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
    let options = CompileOptions {
        samples: Samples::new(dir),
        ..Default::default()
    };
    compile_with(src, &options).map_err(|errs| {
        errs.iter()
            .map(|d| d.render("test", src))
            .collect::<String>()
    })
}

fn play(dir: &Path, src: &str) -> Vec<Vec<f32>> {
    let program = compile_in(dir, src).unwrap_or_else(|e| panic!("{e}"));
    render(
        &program,
        &[],
        &RenderOptions {
            limiter: false,
            until: Some(2.0),
            ..Default::default()
        },
    )
    .unwrap()
    .channels
}

/// A sample played by a clip `t`, which plays `notes`.
fn node(notes: &str) -> String {
    format!(
        "tempo 120bpm\ns = sample(\"s.wav\")\nclip t {{ instrument: s, notes: {notes} }}\nt.start"
    )
}

#[test]
fn a_sample_at_its_root_plays_as_recorded() {
    let dir = scratch("root");
    let audio = ramp(2000);
    write_wav(&dir.join("s.wav"), std::slice::from_ref(&audio));
    let out = play(&dir, &node("[c4]"));
    assert!(
        (0..2000).all(|i| (out[0][i] - audio[i]).abs() < 1e-3),
        "{:?}",
        &out[0][..8]
    );
    assert!(
        out[0][2000..].iter().all(|v| v.abs() < 1e-6) && out[0].len() < 2200,
        "it plays once and stops"
    );
}

#[test]
fn a_higher_note_plays_faster() {
    let dir = scratch("pitch");
    write_wav(&dir.join("s.wav"), &[block(0.5, 100)]);
    let length = |note: &str| {
        let out = play(&dir, &node(&format!("[{note}]")));
        out[0].iter().rposition(|v| v.abs() > 0.1).unwrap()
    };
    let (root, up) = (length("c4"), length("c5"));
    assert!((root as f64 / up as f64 - 2.0).abs() < 0.01, "{root} {up}");
}

#[test]
fn the_root_option_shifts_which_note_is_the_original() {
    let dir = scratch("rootopt");
    write_wav(&dir.join("s.wav"), &[block(0.5, 100)]);
    let src = node("[a3]").replace("\"s.wav\")", "\"s.wav\").root(a3)");
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
    let src = "tempo 120bpm\ndrums = kit(\"drums\")\nclip d { instrument: drums, notes: [kick ~ snare ~] }\nd.start";
    let out = play(&dir, src);
    let at = |s: f64| {
        out[0]
            .get((s * SR as f64) as usize + 100)
            .copied()
            .unwrap_or(0.0)
    };
    // a bar at 120bpm is 2 seconds: kick on the downbeat, snare on the third beat
    assert!((at(0.0) - 0.5).abs() < 1e-3, "{}", at(0.0));
    assert!(at(0.5).abs() < 1e-6);
    assert!((at(1.0) + 0.25).abs() < 1e-3, "{}", at(1.0));
    assert!(at(1.5).abs() < 1e-6);
}

#[test]
fn a_kit_member_is_an_instrument_on_its_own_node() {
    let dir = scratch("member");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 100)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(0.5, 100)]);
    let src = "tempo 120bpm\ndrums = kit(\"drums\")\nclip snare { instrument: drums.snare, notes: [c4 ~ ~ ~] }\nsnare.start";
    let out = play(&dir, src);
    let at = |s: f64| {
        out[0]
            .get((s * SR as f64) as usize + 100)
            .copied()
            .unwrap_or(0.0)
    };
    assert!((at(0.0) - 0.5).abs() < 1e-3, "{}", at(0.0));
    assert!(at(0.5).abs() < 1e-6);
}

#[test]
fn a_grid_row_that_names_a_node_goes_through_its_effects() {
    let dir = scratch("route");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 80)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(0.5, 80)]);
    let src = "tempo 120bpm\ndrums = kit(\"drums\")\n               clip beat { instrument: drums, notes: grid {\n  kick:  x... ....\n  snare: .... x...\n} }\n               bus snare_audio\nfx snare_fx {out = snare_audio.gain(-6db)}\ninstr snare_voice(sample, freq: hz = c4) { voice = drums(sample: sample, freq: freq)\nsnare_audio += voice }\nclip snare { instrument: snare_voice }\n               beat.start";
    let out = play(&dir, src);
    let at = |s: f64| {
        out[0]
            .get((s * SR as f64) as usize + 100)
            .copied()
            .unwrap_or(0.0)
    };
    let quiet = 0.5 * 10f32.powf(-6.0 / 20.0);
    assert!(
        (at(0.0) - 0.5).abs() < 1e-3,
        "kick stays on the host: {}",
        at(0.0)
    );
    assert!(
        (at(1.0) - quiet).abs() < 1e-3,
        "snare is quieter through its node: {}",
        at(1.0)
    );
}

#[test]
fn a_grid_row_can_take_params_and_mini_notation() {
    let dir = scratch("rowmini");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/hat.wav"), &[block(0.5, 40)]);
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 40)]);
    let src = "tempo 120bpm\ndrums = kit(\"drums\")\nclip d { instrument: drums, notes: grid {\n  kick(gain: -6db): x...\n  hat: [x*2 ~ ~ ~]\n} }\n               d.start";
    let out = play(&dir, src);
    let at = |s: f64| {
        out[0]
            .get((s * SR as f64) as usize + 50)
            .copied()
            .unwrap_or(0.0)
    };
    let quiet = 0.5 * 10f32.powf(-6.0 / 20.0);
    // kick on the downbeat at -6db; hat*2 puts a second hit halfway through the first step (0.25s)
    assert!(
        (at(0.0) - (quiet + 0.5)).abs() < 1e-2,
        "kick and first hat: {}",
        at(0.0)
    );
    assert!(
        (at(0.25) - 0.5).abs() < 1e-2,
        "second hat from x*2: {}",
        at(0.25)
    );
}

#[test]
fn qualified_sample_names_work_in_patterns() {
    let dir = scratch("qualified");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/a.wav"), &[block(0.5, 50)]);
    write_wav(&dir.join("drums/kick.wav"), &[block(0.25, 50)]);
    // bare `a` is a note; `drums.a` is the sample
    let src = "tempo 120bpm\ndrums = kit(\"drums\")\nclip d { instrument: drums, notes: [drums.a ~ kick ~] }\nd.start";
    let out = play(&dir, src);
    let at = |s: f64| {
        out[0]
            .get((s * SR as f64) as usize + 50)
            .copied()
            .unwrap_or(0.0)
    };
    assert!((at(0.0) - 0.5).abs() < 1e-3, "{}", at(0.0));
    assert!((at(1.0) - 0.25).abs() < 1e-3, "{}", at(1.0));
}

#[test]
fn kit_member_mistakes_are_reported() {
    let dir = scratch("membererr");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 10)]);
    let e = errors(
        &dir,
        "drums = kit(\"drums\")\nclip t { instrument: drums.snare }",
    );
    assert!(
        e.contains("there is no sample `snare` in the kit `drums`"),
        "{e}"
    );
    let e = errors(
        &dir,
        "drums = kit(\"drums\")\nclip t { instrument: drums.kick(sample: kick) }",
    );
    assert!(
        e.contains("already chosen by the name after the kit"),
        "{e}"
    );
}

#[test]
fn a_stereo_sample_keeps_its_sides() {
    let dir = scratch("stereo");
    write_wav(&dir.join("s.wav"), &[block(0.5, 50), block(-0.5, 50)]);
    let src = format!("config {{ channels: stereo }}\n{}", node("[c4]"));
    let out = play(&dir, &src);
    assert_eq!(out.len(), 2);
    assert!((out[0][100] - 0.5).abs() < 1e-3 && (out[1][100] + 0.5).abs() < 1e-3);
}

#[test]
fn a_mono_sample_is_centred_in_a_stereo_mix() {
    let dir = scratch("centre");
    write_wav(&dir.join("s.wav"), &[block(0.5, 50)]);
    let out = play(
        &dir,
        &format!("config {{ channels: stereo }}\n{}", node("[c4]")),
    );
    assert!((out[0][100] - out[1][100]).abs() < 1e-6 && out[0][100] > 0.3);
}

#[test]
fn file_and_name_mistakes_are_reported_where_they_are() {
    let dir = scratch("errors");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/kick.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("s.wav"), &[block(0.5, 10)]);

    let missing = errors(&dir, "s = sample(\"nope.wav\")");
    assert!(
        missing.contains("can't read") && missing.contains("nope.wav"),
        "{missing}"
    );
    assert!(
        missing.contains("1 | s = sample(\"nope.wav\")"),
        "points at the declaration: {missing}"
    );

    let typo = errors(
        &dir,
        "drums = kit(\"drums\")\nclip d { instrument: drums, notes: [kick ~ snar ~] }\nd.start",
    );
    assert!(
        typo.contains("`snar` isn't a note, a value, or a sample in a kit"),
        "{typo}"
    );
    assert!(typo.contains("did you mean `snare`?"), "{typo}");

    let wrong_kit = "a = kit(\"drums\")\ns = sample(\"s.wav\")\nb = kit(\"drums\")\nclip t { instrument: s, notes: [kick] }\nt.start";
    let on_sample = {
        let p = compile_in(&dir, wrong_kit);
        // a sample node has no kit to pick from; found when the note is planned
        let program = p.unwrap();
        let event = program.schedule().events(0.0, 1.0).unwrap();
        program.plan(&event[0]).unwrap_err().to_string()
    };
    assert!(on_sample.contains("isn't a kit"), "{on_sample}");

    let option = errors(&dir, "s = sample(\"s.wav\").rot(c4)");
    assert!(option.contains("`rot` isn't an option"), "{option}");

    let name = errors(
        &dir,
        "s = sample(\"s.wav\")\ninstr s(freq: hz) { osc(sine, freq) }",
    );
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
        "a = kit(\"a\")\nb = kit(\"b\")\nclip t { instrument: a, notes: [hat] }\nt.start",
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
        format!(
            "tempo 120bpm\nh = kit(\"hats\"){opts}\nclip t {{ instrument: h, notes: [open ~ closed ~] }}\nt.start"
        )
    };
    let at =
        |out: &[Vec<f32>], s: f64| out[0].get((s * SR as f64) as usize).copied().unwrap_or(0.0);

    // without a group, the open hat rings on under the closed one
    let free = play(&dir, &src(""));
    assert!((at(&free, 0.5) - 0.5).abs() < 1e-3);
    assert!(
        (at(&free, 1.005) - 0.625).abs() < 1e-3,
        "{}",
        at(&free, 1.005)
    );
    assert!((at(&free, 0.95) - 0.5).abs() < 1e-3);

    // in one group, the closed hat ends it: nothing is left of the open hat once the fade is done
    let choked = play(&dir, &src(".choke(hats(open, closed))"));
    assert!(
        (at(&choked, 0.99) - 0.5).abs() < 1e-3,
        "before the closed hat it's still sounding"
    );
    assert!(
        (at(&choked, 1.01) - 0.125).abs() < 1e-3,
        "{}",
        at(&choked, 1.01)
    );
    assert!(
        at(&choked, 1.05).abs() < 1e-6,
        "and afterwards only silence"
    );
}

#[test]
fn choke_options_are_checked() {
    let dir = scratch("chokeerr");
    std::fs::create_dir_all(dir.join("hats")).unwrap();
    write_wav(&dir.join("hats/open.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("hats/closed.wav"), &[block(0.5, 10)]);
    write_wav(&dir.join("s.wav"), &[block(0.5, 10)]);

    let e = errors(&dir, "h = kit(\"hats\").choke(hats(open, clsed))");
    assert!(
        e.contains("no sample `clsed` in the kit `h`") || e.contains("there is no sample `clsed`"),
        "{e}"
    );
    assert!(e.contains("did you mean `closed`?"), "{e}");
    let e = errors(&dir, "h = kit(\"hats\").choke(hats)");
    assert!(e.contains("a kit names the samples in each group"), "{e}");
    let e = errors(&dir, "s = sample(\"s.wav\").choke(hats(a))");
    assert!(e.contains("expected the name of a choke group"), "{e}");
    let e = errors(&dir, "s = sample(\"s.wav\").chok(hats)");
    assert!(e.contains("did you mean `choke`?"), "{e}");
}

#[test]
fn single_samples_can_share_a_choke_group_with_each_other_and_a_kit() {
    let dir = scratch("chokeshared");
    write_wav(&dir.join("a.wav"), &[block(0.5, 1500)]);
    write_wav(&dir.join("b.wav"), &[block(0.25, 20)]);
    let src = "tempo 120bpm\na = sample(\"a.wav\").choke(g)\nb = sample(\"b.wav\").choke(g)\n               clip ta { instrument: a, notes: [c4 ~ ~ ~] }\nclip tb { instrument: b, notes: [~ ~ c4 ~] }\nta.start\ntb.start";
    let out = play(&dir, src);
    // `a` would ring until 1.5s; the render ends when `b` ends at 1.02s because `a` was cut off
    assert!(
        out[0].len() < (1.1 * SR as f64) as usize,
        "{}",
        out[0].len()
    );
}

/// Four 100ms pieces at distinct levels, as one file: a stand-in for a break.
fn write_break(dir: &Path) {
    let levels = [0.1, 0.2, 0.3, 0.4];
    write_wav(
        &dir.join("amen.wav"),
        &[levels.iter().flat_map(|&l| block(l, 100)).collect()],
    );
}

#[test]
fn slices_of_a_sample_play_in_any_order() {
    let dir = scratch("slices");
    write_break(&dir);
    let src = "tempo 120bpm\namen = sample(\"amen.wav\")\nchops = amen.slices(4)\n               clip t { instrument: chops, notes: [s3 s0 s2 s1] }\nt.start";
    let out = play(&dir, src);
    // four steps of half a second; each plays its quarter of the file (100ms) from its start
    let at = |s: f64| out[0].get((s * SR as f64) as usize).copied().unwrap_or(0.0);
    assert!((at(0.05) - 0.4).abs() < 1e-3, "{}", at(0.05));
    assert!((at(0.55) - 0.1).abs() < 1e-3, "{}", at(0.55));
    assert!((at(1.05) - 0.3).abs() < 1e-3, "{}", at(1.05));
    assert!((at(1.55) - 0.2).abs() < 1e-3, "{}", at(1.55));
    assert!(
        at(0.2).abs() < 1e-6,
        "a slice ends where the next piece of the file would start"
    );
    assert!(at(0.0).abs() < 0.01, "and fades in so it doesn't click");
}

#[test]
fn a_grid_row_per_slice_plays_slices_together() {
    let dir = scratch("slicegrid");
    write_break(&dir);
    let src = "tempo 120bpm\namen = sample(\"amen.wav\")\nchops = amen.slices(4)\n               clip t { instrument: chops, notes: grid {\n  s3: x... ....\n  s0: .... x...\n  s1: x... ....\n} }\nt.start";
    let out = play(&dir, src);
    let at = |s: f64| out[0].get((s * SR as f64) as usize).copied().unwrap_or(0.0);
    // the first step plays s3 (0.4) and s1 (0.2) at once, the second step s0 (0.1)
    assert!((at(0.05) - 0.6).abs() < 1e-3, "{}", at(0.05));
    assert!((at(1.05) - 0.1).abs() < 1e-3, "{}", at(1.05));
}

#[test]
fn grid_accents_and_ghosts_change_the_gain() {
    let dir = scratch("gridghost");
    write_break(&dir);
    let src = "tempo 120bpm\namen = sample(\"amen.wav\")\nchops = amen.slices(4)\n               clip t { instrument: chops, notes: grid {\n  s3: x.o. X...\n} }\nt.start";
    let out = play(&dir, src);
    let at = |s: f64| out[0].get((s * SR as f64) as usize).copied().unwrap_or(0.0);
    let db = |d: f32| 10f32.powf(d / 20.0);
    assert!((at(0.05) - 0.4).abs() < 1e-3, "{}", at(0.05));
    assert!((at(0.55) - 0.4 * db(-12.0)).abs() < 1e-3, "{}", at(0.55));
    assert!((at(1.05) - 0.4 * db(3.0)).abs() < 1e-3, "{}", at(1.05));
}

#[test]
fn a_grid_follows_the_kits_choke_groups() {
    let dir = scratch("gridchoke");
    write_break(&dir);
    // 32 steps in a 2s bar: s1 starts 62.5ms in, while s0 (100ms long) is still sounding
    let play_with = |with: &str| {
        let src = format!(
            "tempo 120bpm\namen = sample(\"amen.wav\")\nchops = amen.slices(4){with}\n             clip t {{ instrument: chops, notes: grid {{\n  s0: x...............................\n  s1: .x..............................\n}} }}\nt.start"
        );
        play(&dir, &src)
    };
    let at = |out: &[Vec<f32>], s: f64| out[0][(s * SR as f64) as usize];
    assert!(
        (at(&play_with(""), 0.07) - 0.3).abs() < 1e-3,
        "without a choke both sound"
    );
    assert!(
        (at(&play_with(".choke(g(s0, s1))"), 0.07) - 0.2).abs() < 1e-3,
        "s1 chokes s0"
    );
}

#[test]
fn grid_rows_must_name_samples() {
    let dir = scratch("gridrows");
    write_break(&dir);
    let head = |grid: &str| {
        format!(
            "amen = sample(\"amen.wav\")\nc = amen.slices(4)\nclip t {{ instrument: c, notes: {grid} }}\nt.start"
        )
    };
    let e = errors(&dir, &head("grid {\n  s0: x...\n  s7: x...\n}"));
    assert!(e.contains("`s7` isn't a sample in a kit"), "{e}");
    let e = errors(&dir, &head("grid {\n  s0 x...\n}"));
    assert!(e.contains("a grid row is `sample: x...`"), "{e}");
    let e = errors(&dir, &head("grid {\n  s0: x.?.\n}"));
    assert!(e.contains("isn't a grid step"), "{e}");
}

#[test]
fn slices_take_choke_groups_by_name() {
    let dir = scratch("slicekit");
    write_break(&dir);
    let program = compile_in(
        &dir,
        "amen = sample(\"amen.wav\")\nchops = amen.slices(4).choke(g(s0, s1))",
    )
    .unwrap();
    let chokes = &program.instrument("chops").unwrap().chokes;
    assert!(chokes[0].is_some());
    assert_eq!(chokes[0], chokes[1]);
    assert_eq!(&chokes[2..], &[None, None]);
}

#[test]
fn slicing_mistakes_are_reported() {
    let dir = scratch("sliceerr");
    write_break(&dir);
    let e = errors(&dir, "c = amen.slices(4)");
    assert!(e.contains("no sample named `amen` to slice"), "{e}");
    let e = errors(&dir, "amen = sample(\"amen.wav\")\nc = amen.slices(2.5)");
    assert!(e.contains("can be cut into 1 to"), "{e}");
    let e = errors(
        &dir,
        "amen = sample(\"amen.wav\")\nc = amen.slices(4)\nclip t { instrument: c, notes: [s9] }\nt.start",
    );
    assert!(
        e.contains("`s9` isn't a note, a value, or a sample in a kit"),
        "{e}"
    );
}

#[test]
fn the_breakbeat_example_plays_the_break_back_unchanged_when_the_slices_are_in_order() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    // the break was recorded at 100bpm, so at that tempo it plays back unchanged
    let source = std::fs::read_to_string(dir.join("breakbeat.nml"))
        .unwrap()
        .replace("tempo 130bpm", "tempo 100bpm");
    let program = compile_in(&dir, &source).unwrap_or_else(|e| panic!("{e}"));
    let out = render(
        &program,
        &[],
        &RenderOptions {
            limiter: false,
            until: Some(4.8),
            ..Default::default()
        },
    )
    .unwrap()
    .channels;

    // a bar at 100bpm is 2.4s
    let original = hound::WavReader::open(dir.join("break.wav"))
        .unwrap()
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect::<Vec<_>>();
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
            "tempo {tempo}bpm\nl = sample(\"loop.wav\"){opts}\nclip t {{ instrument: l, notes: [c4] }}\nt.start"
        );
        end_of(&play(&dir, &src))
    };
    assert!((at(100, ".beats(4)") - 2.4).abs() < 0.01);
    assert!(
        (at(120, ".beats(4)") - 2.0).abs() < 0.01,
        "four beats at 120bpm"
    );
    assert!((at(60, ".beats(4)") - 4.0).abs() < 0.01, "and at 60bpm");
    assert!(
        (at(120, "") - 2.4).abs() < 0.01,
        "without beats it plays as recorded"
    );
}

#[test]
fn slices_of_a_fitted_loop_follow_the_tempo_too() {
    let dir = scratch("fitslices");
    write_wav(&dir.join("loop.wav"), &[block(0.5, 2400)]);
    let src = "tempo 120bpm\nl = sample(\"loop.wav\").beats(4)\nk = l.slices(4)\nclip t { instrument: k, notes: [s3] }\nt.start";
    // the last quarter of the loop is a step long (0.5s at 120bpm) once fitted, and starts on the downbeat
    let out = play(&dir, src);
    assert!((end_of(&out) - 0.5).abs() < 0.01, "{}", end_of(&out));
}

#[test]
fn beats_mistakes_are_reported() {
    let dir = scratch("fiterr");
    write_wav(&dir.join("loop.wav"), &[block(0.5, 100)]);
    let e = errors(&dir, "l = sample(\"loop.wav\").fit(tempo)");
    assert!(e.contains("isn't an option of a sample"), "{e}");
    let e = errors(&dir, "l = sample(\"loop.wav\").beats(-1)");
    assert!(e.contains("`beats` is how many beats"), "{e}");
}

#[test]
fn the_song_example_renders_its_arrangement() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let source = std::fs::read_to_string(dir.join("song.nml")).unwrap();
    let program = compile_in(&dir, &source).unwrap_or_else(|e| panic!("{e}"));
    let options = RenderOptions {
        arrangement: Some("song".into()),
        ..Default::default()
    };
    let out = render(&program, &[], &options).unwrap();
    // 16bars at 110bpm is 34.9s, plus the reverb's tail
    let secs = out.len() as f64 / SR as f64;
    assert!((34.9..45.0).contains(&secs), "{secs}");
    assert!(out.peak() > 0.1 && out.peak() <= 1.0);
}

#[test]
fn begin_starts_a_note_part_way_into_the_sample() {
    let dir = scratch("begin");
    let audio = ramp(4000);
    write_wav(&dir.join("s.wav"), std::slice::from_ref(&audio));
    let out = play(&dir, &node("[c4].begin(50%)"));
    // the ramp climbs to 0.5, so half way in it is at 0.25, and the second half is what plays
    assert!((out[0][0] - 0.25).abs() < 1e-3, "{}", out[0][0]);
    assert!((out[0][1000] - audio[3000]).abs() < 1e-3);
    assert!(
        out[0][2000..].iter().all(|v| v.abs() < 1e-6),
        "it stops at the end of the sample"
    );
}

#[test]
fn chop_plays_the_next_part_of_the_sample_for_each_piece() {
    let dir = scratch("chop");
    // four equal blocks at different levels
    let audio: Vec<f32> = [0.1, 0.2, 0.3, 0.4]
        .iter()
        .flat_map(|&l| vec![l; 4800])
        .collect();
    write_wav(&dir.join("s.wav"), std::slice::from_ref(&audio));
    let out = play(&dir, &node("[c4].chop(4)"));
    // a bar is two seconds, so a piece starts every half second
    for (i, level) in [0.1, 0.2, 0.3, 0.4].iter().enumerate() {
        let at = i * SR / 2 + 100;
        assert!(
            (out[0][at] - level).abs() < 1e-3,
            "piece {i}: {} vs {level}",
            out[0][at]
        );
    }
}

#[test]
fn a_larger_slice_kit_does_not_build_a_larger_audio_graph() {
    let dir = scratch("kit-graph-size");
    write_wav(&dir.join("s.wav"), &[ramp(4096)]);
    let nodes = |slices| {
        let program = compile_in(
            &dir,
            &format!("s = sample(\"s.wav\")\nk = s.slices({slices})\nclip t {{ instrument: k }}"),
        )
        .unwrap();
        program
            .instruments
            .iter()
            .find(|i| i.name == "k")
            .unwrap()
            .graph
            .node_count()
    };
    assert_eq!(nodes(4), nodes(64));
}

#[test]
fn a_kit_with_mixed_channel_layouts_matches_individual_sample_playback() {
    let dir = scratch("kit-mixed-layouts");
    std::fs::create_dir_all(dir.join("kit")).unwrap();
    write_wav(&dir.join("kit/mono.wav"), &[ramp(2000)]);
    write_wav(
        &dir.join("kit/stereo.wav"),
        &[block(0.3, 40), block(-0.2, 40)],
    );
    let kit = play(
        &dir,
        "config { channels: stereo }\nk = kit(\"kit\").root(c4)\nclip t { instrument: k, notes: [mono stereo] }\nt.start",
    );
    let separate = play(
        &dir,
        "config { channels: stereo }\na = sample(\"kit/mono.wav\").root(c4)\nb = sample(\"kit/stereo.wav\").root(c4)\nclip x { instrument: a, notes: [c4 ~] }\nclip y { instrument: b, notes: [~ c4] }\nx.start\ny.start",
    );
    for (a, b) in kit.iter().zip(&separate) {
        let difference = a
            .iter()
            .zip(b)
            .enumerate()
            .find(|(_, (a, b))| (*a - *b).abs() > 1e-6);
        assert!(
            difference.is_none(),
            "lengths {} {}, first difference {:?}",
            a.len(),
            b.len(),
            difference
        );
        assert_eq!(a.len(), b.len());
    }
}

#[test]
fn two_clips_share_a_kit_with_independent_gain_defaults() {
    let dir = scratch("clip-defaults");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/hat.wav"), &[block(0.5, 80)]);
    let out = play(
        &dir,
        "tempo 120bpm\ndrums = kit(\"drums\")\nclip beat { instrument: drums\nnotes: [hat ~] }\nclip hats { instrument: drums(gain: -8db)\nnotes: [~ hat] }\nscene verse { beat, hats }\nverse.start",
    );
    assert!((out[0][100] - 0.5).abs() < 1e-3);
    assert!((out[0][SR + 100] - 0.5 * 10f32.powf(-8.0 / 20.0)).abs() < 1e-3);
}

#[test]
fn samplers_called_inside_instruments_match_direct_playback() {
    let dir = scratch("nested-samplers");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("s.wav"), &[ramp(9600)]);
    write_wav(&dir.join("drums/kick.wav"), &[block(0.2, 100)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(0.4, 100)]);
    for (definitions, call, selected, notes) in [
        ("s = sample(\"s.wav\")", "s(freq: freq)", "s", "[c4]"),
        (
            "s = kit(\"drums\")",
            "s.snare(freq: freq)",
            "s.snare",
            "[c4]",
        ),
        (
            "s = sample(\"s.wav\")\nk = s.slices(4)",
            "k(sample: s2, freq: freq)",
            "k(sample: s2)",
            "[c4]",
        ),
    ] {
        let direct = play(
            &dir,
            &format!(
                "tempo 120bpm\n{definitions}\nclip c {{ instrument: {selected}, notes: {notes} }}\nc.start"
            ),
        );
        let nested = play(
            &dir,
            &format!(
                "tempo 120bpm\n{definitions}\ninstr voice(freq: hz) {{ {call} }}\nclip c {{ instrument: voice, notes: {notes} }}\nc.start"
            ),
        );
        assert!(
            direct == nested,
            "{call}: direct {}, nested {}",
            direct[0].len(),
            nested[0].len()
        );
        assert!(nested[0].iter().any(|v| v.abs() > 0.1));
    }
}

#[test]
fn one_instrument_wraps_a_whole_kit_and_forwards_grid_selections() {
    let dir = scratch("whole-kit-wrapper");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    // A different kit adds a global sample name before this kit's local indices.
    std::fs::create_dir_all(dir.join("other")).unwrap();
    write_wav(&dir.join("other/aaa.wav"), &[block(0.1, 100)]);
    write_wav(&dir.join("drums/kick.wav"), &[block(0.2, 100)]);
    write_wav(&dir.join("drums/snare.wav"), &[block(0.4, 100)]);
    let definitions = "other = kit(\"other\")\ndrums = kit(\"drums\")";
    let notes = "grid { kick(gain: -6db): x...\n snare: ..x. }";
    let direct = play(
        &dir,
        &format!(
            "tempo 120bpm\n{definitions}\nclip c {{ instrument: drums, notes: {notes} }}\nc.start"
        ),
    );
    let wrapped = play(
        &dir,
        &format!(
            "tempo 120bpm\n{definitions}\nbus audio\ninstr drum_voice(sample, gain: db = 0db) {{ audio += drums(sample: sample, gain: gain) }}\nfx room {{ out = audio }}\nclip beat {{ instrument: drum_voice, notes: {notes} }}\nbeat.start"
        ),
    );
    assert!(
        direct == wrapped,
        "direct {}, wrapped {}",
        direct[0].len(),
        wrapped[0].len()
    );
    assert!((wrapped[0][100] - 0.2 * 10f32.powf(-6.0 / 20.0)).abs() < 1e-3);
    assert!((wrapped[0][SR + 100] - 0.4).abs() < 1e-3);
}

#[test]
fn a_whole_kit_wrapper_keeps_choke_groups() {
    let dir = scratch("wrapped-chokes");
    std::fs::create_dir_all(dir.join("drums")).unwrap();
    write_wav(&dir.join("drums/open.wav"), &[block(0.25, 1500)]);
    write_wav(&dir.join("drums/closed.wav"), &[block(0.1, 80)]);
    let definitions = "drums = kit(\"drums\").choke(hats(open, closed))";
    let notes = "[open closed ~ ~]";
    let direct = play(
        &dir,
        &format!(
            "tempo 120bpm\n{definitions}\nclip c {{ instrument: drums, notes: {notes} }}\nc.start"
        ),
    );
    let wrapped = play(
        &dir,
        &format!(
            "tempo 120bpm\n{definitions}\ninstr drum_voice(sample) {{ drums(sample: sample) }}\nclip c {{ instrument: drum_voice, notes: {notes} }}\nc.start"
        ),
    );
    assert!(direct == wrapped, "wrapping a kit preserves choking");
    assert!(wrapped[0][(0.4 * SR as f64) as usize] > 0.2);
    assert!(wrapped[0].get(SR).copied().unwrap_or(0.0).abs() < 1e-6);
}

#[test]
fn nested_sampler_arguments_are_checked() {
    let dir = scratch("nested-errors");
    write_wav(&dir.join("s.wav"), &[block(0.25, 100)]);
    for (body, expected) in [
        ("s(freq: 1sec)", "expects hz"),
        ("s(freq: 440hz, freq: 220hz)", "given twice"),
        ("s(max: 2)", "no parameter `max`"),
        ("s(440hz)", "name the sampler arguments"),
    ] {
        let message = errors(
            &dir,
            &format!("s = sample(\"s.wav\")\ninstr voice() {{ {body} }}"),
        );
        assert!(message.contains(expected), "{body}: {message}");
    }
    let message = errors(&dir, "s = sample(\"s.wav\")\nfx room { out = s() }");
    assert!(
        message.contains("only be played inside an instrument"),
        "{message}"
    );
}

#[test]
fn a_sample_is_played_by_a_clip_not_by_its_own_name() {
    let dir = scratch("by-name");
    write_wav(&dir.join("s.wav"), &[block(0.25, 100)]);
    let e = errors(&dir, "tempo 120bpm\ns = sample(\"s.wav\")\ns.start");
    assert!(e.contains("`s` is an instrument, not a clip"), "{e}");
}
