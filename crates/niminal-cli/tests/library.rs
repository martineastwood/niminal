//! The installed command can discover presets and supply a playable sample kit without downloads.

use std::process::Command;

fn niminal() -> Command {
    Command::new(env!("CARGO_BIN_EXE_niminal"))
}

#[test]
fn presets_are_discoverable_and_the_printed_patch_is_editable_source() {
    let list = niminal()
        .args(["presets", "--kind", "instrument"])
        .output()
        .unwrap();
    assert!(list.status.success());
    let text = String::from_utf8(list.stdout).unwrap();
    for name in [
        "electric_piano",
        "organ",
        "fm_keys",
        "struck_string",
        "cowbell",
    ] {
        assert!(
            text.lines()
                .any(|line| line.starts_with(&format!("{name}\t"))),
            "missing {name}"
        );
    }
    assert!(!text.lines().any(|line| line.starts_with("echo\t")));
    let patch = niminal().args(["presets", "kick_deep"]).output().unwrap();
    assert!(patch.status.success());
    let source = String::from_utf8(patch.stdout).unwrap();
    assert!(source.contains("fn drum("));
    assert!(niminal_lang::compile(&source).is_ok());
    assert!(
        !niminal()
            .args(["presets", "not_a_sound"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let effects = niminal()
        .args(["presets", "--kind", "fx"])
        .output()
        .unwrap();
    let text = String::from_utf8(effects.stdout).unwrap();
    assert!(text.contains("vowel_filter\t") && text.contains("stutter\t"));
    assert!(!text.contains("electric_piano\t"));
}

#[test]
fn bundled_recordings_export_with_sources_and_play_as_a_kit_without_overwriting_edits() {
    let root = std::env::temp_dir().join(format!("niminal-starter-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let pack = root.join("samples/starter");
    let exported = niminal()
        .args(["samples", "--out"])
        .arg(&pack)
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let mut recordings = 0;
    for entry in std::fs::read_dir(&pack).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "wav") {
            continue;
        }
        recordings += 1;
        let mut wav = hound::WavReader::open(&path).unwrap();
        assert_eq!(wav.spec().sample_rate, 48_000);
        assert_eq!(wav.spec().channels, 1);
        assert_eq!(wav.spec().bits_per_sample, 16);
        let samples: Vec<i16> = wav.samples().map(Result::unwrap).collect();
        let peak = samples.iter().map(|s| i32::from(*s).abs()).max().unwrap();
        assert!(
            (655..=26_215).contains(&peak),
            "{} peak {peak}",
            path.display()
        );
        assert_eq!(
            samples.last(),
            Some(&0),
            "{} needs a finished fade",
            path.display()
        );
        if path.file_stem().unwrap() == "kick_808" {
            assert!(
                peak > 24_000,
                "the exported 808 needs its curated playback level"
            );
        }
        let source = std::fs::read_to_string(path.with_extension("nml")).unwrap();
        niminal_compose::compile(&source).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
    }
    assert_eq!(recordings, 12);
    assert!(
        std::fs::read_to_string(pack.join("LICENSE"))
            .unwrap()
            .contains("MIT License")
    );
    std::fs::write(pack.join("README.md"), "my edits").unwrap();
    assert!(
        !niminal()
            .args(["samples", "--out"])
            .arg(&pack)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(pack.join("README.md")).unwrap(),
        "my edits"
    );

    let source = "use samples/starter/starter as sounds\nconfig { channels: stereo }\ntempo 120bpm\nclip beat { instrument: sounds::drums\n notes: grid { kick: x...\n snare: ..x.\n hat: xxxx } }\nbeat.start\nat +1bar hush\n";
    let piece = root.join("piece.nml");
    let audio = root.join("mix.wav");
    std::fs::write(&piece, source).unwrap();
    let rendered = niminal()
        .arg("render")
        .arg(&piece)
        .arg("--out")
        .arg(&audio)
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let mut wav = hound::WavReader::open(audio).unwrap();
    let samples: Vec<f32> = wav.samples().map(Result::unwrap).collect();
    assert!(
        samples
            .iter()
            .all(|s| s.is_finite() && s.abs() <= niminal_engine::DEFAULT_CEILING + 1e-4)
    );
    assert!(samples.iter().any(|s| s.abs() > 0.05));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn builtin_recordings_play_from_an_isolated_project_without_exporting_samples() {
    let root = std::env::temp_dir().join(format!("niminal-builtin-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = "use std/samples.{drums, electric_piano}\nconfig { channels: stereo }\ntempo 120bpm\nclip beat { instrument: drums\n notes: grid { kick: x...\n snare: ..x.\n hat: xxxx } }\nclip keys { instrument: electric_piano\n notes: [c3 e3 g3 b3] }\nbeat.start\nkeys.start\nat +1bar hush\n";
    let piece = root.join("piece.nml");
    let audio = root.join("mix.wav");
    std::fs::write(&piece, source).unwrap();
    let rendered = niminal()
        .current_dir(&root)
        .arg("render")
        .arg(&piece)
        .arg("--out")
        .arg(&audio)
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let mut wav = hound::WavReader::open(&audio).unwrap();
    let samples: Vec<f32> = wav.samples().map(Result::unwrap).collect();
    assert!(
        samples
            .iter()
            .all(|s| s.is_finite() && s.abs() <= niminal_engine::DEFAULT_CEILING + 1e-4)
    );
    assert!(samples.iter().any(|s| s.abs() > 0.1));
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        2,
        "only the source and requested render may be written"
    );
    std::fs::remove_dir_all(root).unwrap();
}
