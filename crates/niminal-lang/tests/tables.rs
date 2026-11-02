//! Table constructors and readers through the compiler.

use niminal_lang::compile;

fn ok(source: &str) {
    compile(source).unwrap_or_else(|e| panic!("{e:?}"));
}

#[test]
fn harmonics_and_osc() {
    ok(r#"
const wave = harmonics([1, 0.5], size: 64)
instr a(freq: hz) { osc(wave, freq) * env[1 10ms 0] }
"#);
}

#[test]
fn curve_cheby_window_bank() {
    ok(r#"
const c = curve([[0, 0], [1, 1]], size: 32, shape: linear)
const t = cheby([1, 0.2], size: 32)
const w = window(hann, size: 32)
const a = harmonics([1], size: 32)
const b = harmonics([1, 0.5], size: 32)
const morph = bank([a, b])
instr x(freq: hz) { osc(morph, freq, position: 0.5).waveshape(t) * c.phase(0.25) }
"#);
}

#[test]
fn buffer_write_and_read() {
    ok(r#"
buffer scratch = zeros(64)
instr a() {
  scratch[0] = 1
  scratch.at(0)
}
"#);
}

#[test]
fn sample_data_and_table_sample() {
    ok(r#"
const organ = harmonics([1, 0, 1/3], size: 64)
pad = sample(organ).root(440hz).loop(forward)
instr a(freq: hz) { osc(pad.data, freq) * env[1 5ms 0] }
"#);
}

#[test]
fn random_requires_seed() {
    assert!(compile("const n = random(16, dist: uniform)\n").is_err());
    ok("const n = random(16, dist: uniform, seed: 1)\n");
}

#[test]
fn a_path_can_hold_constants_and_data_files_are_tables() {
    let dir = std::env::temp_dir().join(format!("niminal-paths-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("curve07.csv"), "x, y\n0, 0\n1, 1\n").unwrap();
    let path = dir
        .join("curve{n:2}.csv")
        .to_string_lossy()
        .replace('\\', "/");
    let src = format!(
        "const n = 3 + 4\nconst t = file(\"{path}\", channel: 1)\ninstr a(freq: hz) {{ osc(t, freq) * env[1 10ms 0] }}"
    );
    ok(&src);
    let missing = src.replace("{n:2}", "{m}");
    let errors = compile(&missing).err().expect("`m` is not defined");
    assert!(errors[0].message.contains("`m`"), "{:?}", errors[0].message);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sampler_interpolation_options() {
    ok(r#"
const wave = harmonics([1], size: 64)
pad = sample(wave).interp(sinc).loop(forward)
chops = pad.slices(4).interp(sinc)
plain = sample(wave).interp(cubic)
rough = sample(wave).interp(linear)
instr a(freq: hz) { pad(freq: freq) }
"#);
    let errors = compile("const wave = zeros(64)\npad = sample(wave).interp(nearest)\n")
        .err()
        .unwrap();
    assert!(errors.iter().any(|e| {
        e.message
            .contains("`interp` is `linear`, `cubic` or `sinc`")
    }));
}

#[test]
fn a_sample_can_be_cut_at_places_in_it() {
    let program = compile("const wave = harmonics([1], size: 1000)\npad = sample(wave)\nchops = pad.slices([0, 0.25, 0.6])\ninstr a(freq: hz) { chops(sample: s1) }").unwrap();
    let chops = program
        .instruments
        .iter()
        .find(|i| i.name == "chops")
        .unwrap();
    assert_eq!(chops.members, ["s0", "s1", "s2"]);
    for bad in [
        "[0.5, 0.2]",
        "[0, 1]",
        "[0, 0, 0.5]",
        "[-0.1]",
        "[0.001, 0.0011]",
    ] {
        let source = format!(
            "const wave = harmonics([1], size: 1000)\npad = sample(wave)\nchops = pad.slices({bad})\n"
        );
        assert!(compile(&source).is_err(), "{bad}");
    }
    assert!(
        compile("const wave = zeros(64)\npad = sample(wave)\nchops = pad.slices([\"a\"])\n")
            .is_err()
    );
}

/// A one second 16-bit mono file with a short decaying noise burst starting at each of `starts` (in seconds).
fn write_hits(dir: &std::path::Path, starts: &[f32]) {
    let rate = 48_000usize;
    let mut audio = vec![0i16; rate];
    let mut noise = 12345u32;
    for &start in starts {
        for n in 0..rate / 5 {
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let hiss = (noise >> 16) as f32 / 32768.0 - 1.0;
            let at = (start * rate as f32) as usize + n;
            audio[at] += (hiss * (-(n as f32) / rate as f32 * 25.0).exp() * 16000.0) as i16;
        }
    }
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36u32 + audio.len() as u32 * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]);
    wav.extend((rate as u32).to_le_bytes());
    wav.extend((rate as u32 * 2).to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend((audio.len() as u32 * 2).to_le_bytes());
    wav.extend(audio.iter().flat_map(|v| v.to_le_bytes()));
    std::fs::write(dir.join("hits.wav"), wav).unwrap();
}

#[test]
fn a_sample_can_be_cut_at_the_hits_found_in_it() {
    let dir = std::env::temp_dir().join(format!("niminal-detect-slices-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_hits(&dir, &[0.0, 0.3, 0.6]);
    let members = |args: &str| {
        let options = niminal_lang::CompileOptions {
            samples: niminal_lang::Samples::new(&dir),
            ..Default::default()
        };
        let source = format!("hits = sample(\"hits.wav\")\nchops = hits.slices({args})\n");
        let program = niminal_lang::compile_with(&source, &options).unwrap();
        program
            .instruments
            .iter()
            .find(|i| i.name == "chops")
            .unwrap()
            .members
            .clone()
    };
    // three hits are found out of up to sixteen, and the names run from s0 for as many as there are
    assert_eq!(members("detect: 16"), ["s0", "s1", "s2"]);
    assert_eq!(members("detect: 16, sensitivity: 0.7"), ["s0", "s1", "s2"]);
    // never more than asked for
    assert_eq!(members("detect: 2"), ["s0", "s1"]);
    for bad in [
        "detect: 16, sensitivity: 2",
        "detect: x",
        "detect: 4, 8",
        "detect: 4, other: 1",
    ] {
        let source = format!("hits = sample(\"hits.wav\")\nchops = hits.slices({bad})\n");
        let options = niminal_lang::CompileOptions {
            samples: niminal_lang::Samples::new(&dir),
            ..Default::default()
        };
        assert!(
            niminal_lang::compile_with(&source, &options).is_err(),
            "{bad}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tables_can_be_combined_with_arithmetic() {
    ok(r#"
const a = harmonics([1, 0.5], size: 64)
const w = window(hann, size: 64)
const shaped = a * w
const louder = shaped * 2 + 0.1
const flipped = -a
const folded = a.abs
const mixed = 1 - w
instr x(freq: hz) { osc(shaped, freq) + osc(folded, freq) * env[1 10ms 0] }
"#);
    let errors =
        compile("const a = ramp(0, 1, size: 4)\nconst b = ramp(0, 1, size: 8)\nconst c = a + b\n")
            .err()
            .expect("sizes differ");
    assert!(errors[0].message.contains("4"), "{:?}", errors[0].message);
}

#[test]
fn tables_can_be_cut_and_chopped_into_a_bank() {
    ok(r#"
const a = harmonics([1], size: 64)
const b = harmonics([1, 1], size: 64)
const both = a.concat(b)
const first = both.slice(0, 64)
const second = both.slice(64)
const frames = both.frames(64)
const morph = bank(frames)
instr x(freq: hz, p: num) { osc(morph, freq, position: p) + osc(first.concat(second).resample(64), freq) }
"#);
    for bad in [
        "both.slice(5, 5)",
        "both.slice(0, 500)",
        "both.frames(500)",
        "both.frames(0)",
    ] {
        let source = format!(
            "const a = harmonics([1], size: 64)\nconst both = a.concat(a)\nconst x = {bad}\n"
        );
        assert!(compile(&source).is_err(), "{bad}");
    }
}

#[test]
fn a_table_can_be_measured_and_rebuilt_from_its_harmonics() {
    ok(r#"
const organ = harmonics([1, 0, 1/3, 0, 1/5], size: 512, normalize: false)
const amps = organ.spectrum(8)
const phases = organ.phases(8)
const rebuilt = harmonics(amps.map(it * 0.5), size: 512, phases: phases, normalize: false)
const level = organ.rms + organ.mean
const cleaned = organ.remove_dc.soften(3).fade(16)
instr x(freq: hz) { osc(rebuilt, freq) * level }
"#);
    assert!(compile("const a = harmonics([1])\nconst b = a.spectrum\n").is_err());
}

#[test]
fn a_curve_can_bend_and_an_exp_curve_cannot_start_at_zero() {
    ok("const a = curve([[0, 0], [1, 1]], size: 64, bend: -3)\n");
    ok("const a = curve([[0, 1], [1, 0.001]], size: 64, shape: exp)\n");
    let errors = compile("const a = curve([[0, 0], [1, 1]], size: 64, shape: exp)\n")
        .err()
        .expect("an exp curve through zero");
    assert!(
        errors[0].message.contains("bend"),
        "{:?}",
        errors[0].message
    );
    assert!(compile("const a = curve([[0, 0], [1, 1]], bend: 99)\n").is_err());
}

#[test]
fn random_tables_can_be_gaussian_and_steps_must_rise() {
    ok("const n = random(64, dist: gauss, seed: 3)\n");
    assert!(compile("const s = steps([4, 0], [1, 2], size: 8)\n").is_err());
}

#[test]
fn a_scala_file_is_a_table_of_degrees() {
    let dir = std::env::temp_dir().join(format!("niminal-scl-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("just.scl"), "just\n 2\n 5/4\n 2/1\n").unwrap();
    let path = dir.join("just.scl").to_string_lossy().replace('\\', "/");
    ok(&format!(
        "const degrees = file(\"{path}\")\nconst n = degrees.len\ninstr a(degree: num) {{ osc(sine, 440hz * pow(2, degrees[degree] / 12)) }}"
    ));
    std::fs::remove_dir_all(&dir).ok();
}
