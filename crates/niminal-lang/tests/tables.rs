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
a(freq: 440hz) for 50ms
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
x(freq: 220hz) for 20ms
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
a() for 10ms
"#);
}

#[test]
fn sample_data_and_table_sample() {
    ok(r#"
const organ = harmonics([1, 0, 1/3], size: 64)
sample pad = organ with(root: 440hz, loop: forward)
instr a(freq: hz) { osc(pad.data, freq) * env[1 5ms 0] }
a(freq: 440hz) for 20ms
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
    let path = dir.join("curve{n:2}.csv").to_string_lossy().replace('\\', "/");
    let src = format!("const n = 3 + 4\nconst t = file(\"{path}\", channel: 1)\ninstr a(freq: hz) {{ osc(t, freq) * env[1 10ms 0] }}\na(freq: 220hz) for 20ms");
    ok(&src);
    let missing = src.replace("{n:2}", "{m}");
    let errors = compile(&missing).err().expect("`m` is not defined");
    assert!(errors[0].message.contains("`m`"), "{:?}", errors[0].message);
    std::fs::remove_dir_all(&dir).ok();
}
