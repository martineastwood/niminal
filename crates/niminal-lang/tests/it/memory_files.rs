//! A program with no file system (a browser page) can put files in memory: samples, kits and modules are found by the
//! same relative paths as on disk, and a replaced file is read again.
#![allow(clippy::unwrap_used)]

use niminal_engine::vfs;
use niminal_lang::{Samples, compile, expand};
use std::path::PathBuf;
use std::sync::Arc;

/// A one-channel 16-bit WAV of `frames` samples that all have the value `level`.
fn wav(frames: usize, level: i16) -> Vec<u8> {
    let mut out = Vec::new();
    let mut writer = hound::WavWriter::new(
        std::io::Cursor::new(&mut out),
        hound::WavSpec { channels: 1, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int },
    )
    .unwrap();
    for _ in 0..frames {
        writer.write_sample(level).unwrap();
    }
    writer.finalize().unwrap();
    out
}

#[test]
fn a_sample_is_read_from_memory_and_read_again_when_replaced() {
    vfs::put("mem_a/kick.wav", wav(100, 1000));
    let samples = Samples::new(".");
    let first = samples.load("mem_a/kick.wav").unwrap();
    assert_eq!(first[0].len(), 100);
    vfs::put("mem_a/kick.wav", wav(250, 1000));
    let second = samples.load("mem_a/kick.wav").unwrap();
    assert_eq!(second[0].len(), 250, "a replaced file must not be served from the cache");
    assert!(!Arc::ptr_eq(&first[0], &second[0]));
    vfs::remove("mem_a/kick.wav");
    assert!(samples.load("mem_a/kick.wav").is_err());
}

#[test]
fn a_kit_is_the_audio_files_of_a_folder_in_memory() {
    vfs::put("mem_b/kit/kick.wav", wav(10, 1));
    vfs::put("mem_b/kit/snare.wav", wav(20, 2));
    vfs::put("mem_b/kit/notes.txt", b"not audio".to_vec());
    let kit = Samples::new(".").load_kit("mem_b/kit").unwrap();
    let names: Vec<&str> = kit.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["kick", "snare"]);
    for name in ["mem_b/kit/kick.wav", "mem_b/kit/snare.wav", "mem_b/kit/notes.txt"] {
        vfs::remove(name);
    }
}

#[test]
fn a_module_and_its_sample_in_memory_compile_into_a_piece() {
    vfs::put("mem_c/hit.wav", wav(64, 3000));
    vfs::put("mem_c/voices.nml", b"hit = sample(\"hit.wav\").root(c3)\n".to_vec());
    let expanded = expand("use mem_c/voices\n", &PathBuf::from(".")).unwrap();
    let program = compile(&expanded.text).unwrap();
    assert!(program.instruments.iter().any(|i| i.name == "hit"));
    for name in ["mem_c/hit.wav", "mem_c/voices.nml"] {
        vfs::remove(name);
    }
}
