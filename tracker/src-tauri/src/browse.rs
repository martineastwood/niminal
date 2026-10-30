//! The sample library: the audio files under a folder chosen on disk, and their bytes for the window to audition.

use std::path::Path;

/// How far down a folder tree to look, and how many files to report, so a stray choice of `/` ends.
const DEPTH: usize = 8;
const LIMIT: usize = 5000;
/// The biggest file the window will be sent to audition, in bytes.
const BIGGEST: u64 = 96 * 1024 * 1024;

/// The full paths of the audio files under `dir`, sorted. Hidden folders and build output are skipped.
pub fn list(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    walk(dir, 0, &mut found);
    found.sort_by_key(|f| f.to_lowercase());
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if found.len() >= LIMIT {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < DEPTH && !name.starts_with('.') && name != "node_modules" && name != "target" {
                walk(&path, depth + 1, found);
            }
        } else if niminal_lang::is_audio(&path) && !name.starts_with('.') {
            found.push(path.to_string_lossy().into_owned());
        }
    }
}

/// The bytes of an audio file as a WAV, for the window to decode. Formats the window may not play (FLAC,
/// Ogg, AIFF…) are decoded by the same code that loads them into a piece, so what is heard is what loads.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    if !niminal_lang::is_audio(path) {
        return Err("that is not an audio file".into());
    }
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > BIGGEST {
        return Err(format!("{} is too big to audition", path.display()));
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
        return std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()));
    }
    let tables = niminal_lang::Samples::new(path.parent().unwrap_or(Path::new(".")))
        .load(&path.to_string_lossy())?;
    let frames = tables.iter().map(|t| t.len()).max().unwrap_or(0);
    let rate = tables.first().map_or(48_000.0, |t| t.sample_rate_or(48_000.0));
    let spec = hound::WavSpec {
        channels: tables.len() as u16,
        sample_rate: rate.round() as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut out, spec).map_err(|e| e.to_string())?;
    for frame in 0..frames {
        for table in &tables {
            writer.write_sample(table.get(frame)).map_err(|e| e.to_string())?;
        }
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_audio_below_a_folder_and_skips_hidden_and_other_files() {
        let root = std::env::temp_dir().join(format!("niminal-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("drums")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        for file in ["kick.wav", "drums/snare.flac", "notes.txt", ".hidden/x.wav", ".secret.wav"] {
            std::fs::write(root.join(file), b"").unwrap();
        }
        let names: Vec<_> = list(&root).iter().map(|p| Path::new(p).strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["drums/snare.flac", "kick.wav"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
