//! A project's working folder. Imported samples are copied into `samples/` there, so the
//! project keeps working if the original files move, and a saved project can carry them along.
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Project ids name a folder, so keep them to plain identifier characters.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The working folder for a project, created on first use.
pub fn project_dir(base: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid project id".into());
    }
    let dir = base.join("projects").join(id);
    fs::create_dir_all(dir.join("samples")).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Copies an audio file into the project and returns its path relative to the project folder.
pub fn import_sample(dir: &Path, source: &Path) -> Result<String, String> {
    if !niminal_lang::is_audio(source) {
        return Err("Choose a supported audio file".into());
    }
    let extension = source.extension().unwrap().to_string_lossy().to_lowercase();
    let stem: String = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sample")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let samples = dir.join("samples");
    fs::create_dir_all(&samples).map_err(|e| e.to_string())?;
    // a file already imported, byte for byte, is reused instead of copied again
    let size = fs::metadata(source).map_err(|e| e.to_string())?.len();
    let bytes = fs::read(source).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(&samples).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.is_file() && entry.metadata().is_ok_and(|m| m.len() == size) && fs::read(&path).is_ok_and(|existing| existing == bytes) {
            return Ok(format!("samples/{}", entry.file_name().to_string_lossy()));
        }
    }
    let mut name = format!("{stem}.{extension}");
    for n in 2.. {
        if !samples.join(&name).exists() {
            break;
        }
        name = format!("{stem}-{n}.{extension}");
    }
    fs::copy(source, samples.join(&name)).map_err(|e| e.to_string())?;
    Ok(format!("samples/{name}"))
}

/// The audio files under the project's `samples` folder (relative to the project) that nothing in `code` plays: a file
/// is used when its path appears there, or, for a kit, when its folder does.
pub fn unused_samples(dir: &Path, code: &str) -> Vec<String> {
    fn walk(base: &Path, folder: &Path, found: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(folder) else { return };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                walk(base, &path, found);
            } else if let (true, Ok(relative)) = (niminal_lang::is_audio(&path), path.strip_prefix(base)) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut all = Vec::new();
    walk(dir, &dir.join("samples"), &mut all);
    all.sort();
    all.retain(|file| {
        let folder = file.rsplit_once('/').map_or("", |(folder, _)| folder);
        !code.contains(file.as_str()) && !(folder != "samples" && code.contains(&format!("\"{folder}\"")))
    });
    all
}

/// Deletes `files` (relative to the project, under `samples`) and any folder that is left empty.
pub fn delete_samples(dir: &Path, files: &[String]) -> Result<(), String> {
    let samples = dir.join("samples");
    for file in files {
        let path = dir.join(file);
        if !path.starts_with(&samples) || file.contains("..") {
            return Err(format!("{file} is not a sample of the project"));
        }
        fs::remove_file(&path).map_err(|e| e.to_string())?;
        let mut folder = path.parent();
        while let Some(parent) = folder.filter(|f| *f != samples && f.starts_with(&samples)) {
            if fs::remove_dir(parent).is_err() {
                break;
            }
            folder = parent.parent();
        }
    }
    Ok(())
}

/// A name made of letters, digits and underscores, as a sample in a kit needs to be named in a pattern.
fn member_name(stem: &str) -> String {
    let name: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) { format!("s{name}") } else { name }
}

/// Copies audio files into a new kit folder of the project, `samples/<name>`, each named for its sample. Returns
/// the folder, relative to the project, and the sample names in the order the engine counts them (by file name).
pub fn import_kit(dir: &Path, sources: &[PathBuf], name: &str) -> Result<(String, Vec<String>), String> {
    if sources.is_empty() {
        return Err("Choose the samples of the kit".into());
    }
    let base = member_name(name);
    let samples = dir.join("samples");
    let mut folder = base.clone();
    for n in 2.. {
        if !samples.join(&folder).exists() {
            break;
        }
        folder = format!("{base}_{n}");
    }
    let target = samples.join(&folder);
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut members: Vec<String> = Vec::new();
    let mut filenames = Vec::new();
    for source in sources {
        if !niminal_lang::is_audio(source) {
            return Err("Choose a supported audio file".into());
        }
        let stem = member_name(source.file_stem().and_then(|s| s.to_str()).unwrap_or("sample"));
        let mut member = stem.clone();
        for n in 2.. {
            if !members.contains(&member) {
                break;
            }
            member = format!("{stem}_{n}");
        }
        let extension = source.extension().unwrap().to_string_lossy().to_lowercase();
        fs::copy(source, target.join(format!("{member}.{extension}"))).map_err(|e| e.to_string())?;
        filenames.push((format!("{member}.{extension}"), member.clone()));
        members.push(member);
    }
    filenames.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((format!("samples/{folder}"), filenames.into_iter().map(|(_, member)| member).collect()))
}

/// Copies every file in `from` into `to`, replacing files of the same name.
pub fn copy_samples(from: &Path, to: &Path) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(());
    };
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            fs::copy(entry.path(), to.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("niminal-tracker-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn compressed_samples_keep_their_format_and_use_engine_decoding() {
        let base = scratch("formats");
        let dir = project_dir(&base, "formats").unwrap();
        let source = base.join("Hit.FLAC");
        let bytes = include_bytes!("../../../crates/niminal-engine/tests/fixtures/stereo.flac");
        fs::write(&source, bytes).unwrap();
        let file = import_sample(&dir, &source).unwrap();
        assert_eq!(file, "samples/Hit.flac");
        assert_eq!(fs::read(dir.join(&file)).unwrap(), bytes);
        let peaks = crate::slicing::peaks(&dir, &file, 2).unwrap();
        assert_eq!((peaks.frames, peaks.rate), (4, 44100.0));
        let other = base.join("Hit_.wav");
        fs::write(&other, b"RIFF").unwrap();
        let (folder, members) = import_kit(&dir, &[other, source.clone(), source], "mixed").unwrap();
        assert_eq!(members, ["Hit", "Hit_", "Hit_2"]);
        assert_eq!(fs::read(dir.join(folder).join("Hit_2.flac")).unwrap(), bytes);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn unused_samples_are_found_and_removed_with_their_empty_folders() {
        let base = scratch("clean");
        let dir = project_dir(&base, "clean").unwrap();
        for file in ["kick.wav", "snare.wav", "kit/a.wav", "kit/b.wav", "old/c.wav"] {
            fs::create_dir_all(dir.join("samples").join(file).parent().unwrap()).unwrap();
            fs::write(dir.join("samples").join(file), b"RIFF").unwrap();
        }
        fs::write(dir.join("samples/notes.txt"), b"x").unwrap();
        let code = r#"k = sample("samples/kick.wav") d = kit("samples/kit")"#;
        let unused = unused_samples(&dir, code);
        assert_eq!(unused, ["samples/old/c.wav", "samples/snare.wav"]);
        assert!(delete_samples(&dir, &["../x".into()]).is_err());
        delete_samples(&dir, &unused).unwrap();
        assert!(!dir.join("samples/old").exists() && !dir.join("samples/snare.wav").exists());
        assert!(dir.join("samples/kit/a.wav").exists() && dir.join("samples/notes.txt").exists() && dir.join("samples").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn samples_are_copied_in_with_safe_unique_names() {
        let base = scratch("files");
        let dir = project_dir(&base, "abc-123").unwrap();
        let original = base.join("Kick Drum!.WAV");
        fs::write(&original, b"RIFF").unwrap();
        assert_eq!(
            import_sample(&dir, &original).unwrap(),
            "samples/Kick_Drum_.wav"
        );
        // the same bytes again reuse the file; a different file of the same name gets a new one
        assert_eq!(import_sample(&dir, &original).unwrap(), "samples/Kick_Drum_.wav");
        let other = base.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("Kick Drum!.WAV"), b"RIFF2").unwrap();
        assert_eq!(import_sample(&dir, &other.join("Kick Drum!.WAV")).unwrap(), "samples/Kick_Drum_-2.wav");
        assert!(dir.join("samples/Kick_Drum_.wav").exists());
        assert!(import_sample(&dir, &base.join("notes.txt")).is_err());
        assert!(project_dir(&base, "../escape").is_err());

        fs::write(base.join("snare.WAV"), b"RIFF").unwrap();
        let (folder, members) = import_kit(&dir, &[original.clone(), base.join("snare.WAV"), original.clone()], "My kit!").unwrap();
        assert_eq!(folder, "samples/My_kit_");
        assert_eq!(members, ["Kick_Drum_", "Kick_Drum__2", "snare"]);
        assert!(dir.join("samples/My_kit_/snare.wav").exists());
        assert_eq!(import_kit(&dir, std::slice::from_ref(&original), "My kit!").unwrap().0, "samples/My_kit__2");
        assert!(import_kit(&dir, &[base.join("notes.txt")], "x").is_err() && import_kit(&dir, &[], "x").is_err());
        assert_eq!(member_name("808 kick"), "s808_kick");

        let package = base.join("package");
        copy_samples(&dir.join("samples"), &package.join("samples")).unwrap();
        assert!(package.join("samples/Kick_Drum_-2.wav").exists());
        copy_samples(&base.join("missing"), &package).unwrap();
        let _ = fs::remove_dir_all(&base);
    }
}
