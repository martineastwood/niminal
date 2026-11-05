//! Bundled recordings work without a project folder, exported files, or a writable cache directory.

use niminal_engine::{Voice, block_size};
use niminal_lang::{Samples, compile, expand};
use std::path::PathBuf;
use std::sync::Arc;

fn missing_project() -> PathBuf {
    std::env::temp_dir()
        .join(format!("niminal-no-project-{}", std::process::id()))
        .join("does-not-exist")
}

#[test]
fn a_selected_recording_plays_without_creating_any_files() {
    let dir = missing_project();
    assert!(!dir.exists());
    let expanded = expand("use std/samples.{electric_piano}\n", &dir).unwrap();
    let joined = expand(&expanded.text, &dir).unwrap();
    let program = compile(&joined.text).unwrap();
    assert_eq!(
        program.instruments.len(),
        1,
        "only the requested recording is imported"
    );
    let instrument = &program.instruments[0];
    assert_eq!(instrument.name, "electric_piano");
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    voice.set_param_by_name("freq", 130.812_79);
    voice.start_note();
    let mut audio = vec![0.0; 24_000];
    for block in audio.chunks_mut(block_size()) {
        voice.process(block);
    }
    assert!(audio.iter().all(|s| s.is_finite()));
    assert!(audio.iter().any(|s| s.abs() > 0.1));
    assert!(
        !dir.exists(),
        "loading a bundled sample must not extract files"
    );
}

#[test]
fn selecting_the_kit_keeps_its_private_members_and_does_not_shadow_synths() {
    let expanded = expand(
        "use std/samples.{drums}\nuse std/instruments.{electric_piano}\n",
        &missing_project(),
    )
    .unwrap();
    let program = compile(&expanded.text).unwrap();
    assert!(program.instruments.iter().any(|i| i.name == "drums"));
    assert_eq!(
        program
            .instruments
            .iter()
            .filter(|i| i.name == "electric_piano")
            .count(),
        1
    );
    assert!(!program.instruments.iter().any(|i| i.name == "fm_keys"));
    assert!(!expanded.text.contains("std://samples/electric_piano.wav"));
    assert!(expanded.text.contains("std://samples/kick_808.wav"));
}

#[test]
fn decoded_audio_is_shared_between_sessions_and_bad_names_are_reported() {
    let a = Samples::new(missing_project())
        .load("std://samples/kick_808.wav")
        .unwrap();
    let b = Samples::default()
        .load("std://samples/kick_808.wav")
        .unwrap();
    assert!(
        Arc::ptr_eq(&a[0], &b[0]),
        "the embedded recording should only be decoded once"
    );
    let error = Samples::default()
        .load("std://samples/missing.wav")
        .unwrap_err();
    assert!(error.contains("no bundled recording"), "{error}");
}

#[test]
fn a_local_module_can_reexport_a_bundled_recording() {
    let dir = std::env::temp_dir().join(format!("niminal-reexport-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("voice.nml"),
        "hit = sample(\"std://samples/kick_808.wav\").root(c3)\n",
    )
    .unwrap();
    let expanded = expand("use voice\n", &dir).unwrap();
    assert!(compile(&expanded.text).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}
