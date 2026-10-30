use std::path::{Path, PathBuf};

use niminal_engine::Voice;
use niminal_lang::{CompileOptions, Program, Samples, compile_with};

/// A folder with an SFZ file and the mono WAVs it names, each a constant level so the output says
/// which region played.
fn instrument(name: &str, sfz: &str, files: &[(&str, f32, usize)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("niminal-sfz-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    for (file, level, frames) in files {
        let mut w = hound::WavWriter::create(dir.join(file), spec).unwrap();
        (0..*frames).for_each(|_| w.write_sample(*level).unwrap());
        w.finalize().unwrap();
    }
    std::fs::write(dir.join("kit.sfz"), sfz).unwrap();
    dir
}

fn program(dir: &Path, source: &str) -> Program {
    let options = CompileOptions {
        samples: Samples::new(dir),
        ..CompileOptions::default()
    };
    compile_with(source, &options).unwrap_or_else(|e| panic!("{e:?}"))
}

fn voice(program: &Program, name: &str, params: &[(&str, f32)]) -> Voice {
    let instrument = program.instruments.iter().find(|i| i.name == name).unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    for (param, value) in params {
        assert!(voice.set_param_by_name(param, *value), "{param}");
    }
    voice
}

fn blocks(voice: &mut Voice, count: usize) -> Vec<f32> {
    let mut out = vec![0.0; 64 * count];
    for chunk in out.chunks_mut(64) {
        voice.process(chunk);
    }
    out
}

const LAYERS: &str = "\
<control> default_path=
<group> lokey=0 hikey=127 pitch_keycenter=60
<region> sample=soft.wav lovel=1 hivel=63
<region> sample=loud.wav lovel=64 hivel=127
";

#[test]
fn a_note_plays_the_layer_its_velocity_selects() {
    let dir = instrument(
        "layers",
        LAYERS,
        &[("soft.wav", 0.2, 48_000), ("loud.wav", 0.5, 48_000)],
    );
    let p = program(&dir, "piano = sfz(\"kit.sfz\")");
    let c4 = 261.625_56;
    let loud = blocks(&mut voice(&p, "piano", &[("freq", c4)]), 4);
    assert!((loud[200] - 0.5).abs() < 1e-3, "{}", loud[200]);
    // `vel` picks the layer, and the gain stays a plain level
    let soft = blocks(&mut voice(&p, "piano", &[("freq", c4), ("vel", 0.2)]), 4);
    assert!((soft[200] - 0.2).abs() < 1e-3, "{}", soft[200]);
    let quiet = blocks(&mut voice(&p, "piano", &[("freq", c4), ("gain", 0.5)]), 4);
    assert!((quiet[200] - 0.25).abs() < 1e-3, "{}", quiet[200]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_sfz_instrument_plays_inside_an_instr_and_takes_options() {
    let dir = instrument(
        "inside",
        LAYERS,
        &[("soft.wav", 0.2, 4800), ("loud.wav", 0.5, 4800)],
    );
    let p = program(
        &dir,
        "piano = sfz(\"kit.sfz\").interp(sinc)\ninstr lead(freq: hz) { piano(freq: freq) * 0.5 }",
    );
    let out = blocks(&mut voice(&p, "lead", &[("freq", 261.625_56)]), 4);
    assert!((out[200] - 0.25).abs() < 1e-3, "{}", out[200]);
    std::fs::remove_dir_all(dir).ok();
}

fn error_of(dir: &Path, source: &str) -> String {
    let options = CompileOptions {
        samples: Samples::new(dir),
        ..CompileOptions::default()
    };
    match compile_with(source, &options) {
        Ok(_) => panic!("{source} should not compile"),
        Err(errors) => errors[0].message.clone(),
    }
}

#[test]
fn a_mistake_in_the_file_is_reported_where_it_was_declared() {
    let dir = instrument("bad", "<region> sample=gone.wav\n", &[]);
    assert!(error_of(&dir, "piano = sfz(\"kit.sfz\")").contains("gone.wav"));
    assert!(error_of(&dir, "piano = sfz(\"nope.sfz\")").contains("nope.sfz"));
    assert!(error_of(&dir, "piano = sfz(\"kit.sfz\").root(c4)").contains("isn't an option"));
    std::fs::remove_dir_all(dir).ok();
}
