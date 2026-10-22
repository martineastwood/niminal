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

/// Copies a WAV into the project and returns its path relative to the project folder.
pub fn import_sample(dir: &Path, source: &Path) -> Result<String, String> {
    let extension = source
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    if extension.as_deref() != Some("wav") {
        return Err("Samples need to be WAV files".into());
    }
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
    let mut name = format!("{stem}.wav");
    for n in 2.. {
        if !samples.join(&name).exists() {
            break;
        }
        name = format!("{stem}-{n}.wav");
    }
    fs::copy(source, samples.join(&name)).map_err(|e| e.to_string())?;
    Ok(format!("samples/{name}"))
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
    fn samples_are_copied_in_with_safe_unique_names() {
        let base = scratch("files");
        let dir = project_dir(&base, "abc-123").unwrap();
        let original = base.join("Kick Drum!.WAV");
        fs::write(&original, b"RIFF").unwrap();
        assert_eq!(
            import_sample(&dir, &original).unwrap(),
            "samples/Kick_Drum_.wav"
        );
        assert_eq!(
            import_sample(&dir, &original).unwrap(),
            "samples/Kick_Drum_-2.wav"
        );
        assert!(dir.join("samples/Kick_Drum_.wav").exists());
        assert!(import_sample(&dir, &base.join("notes.txt")).is_err());
        assert!(project_dir(&base, "../escape").is_err());

        let package = base.join("package");
        copy_samples(&dir.join("samples"), &package.join("samples")).unwrap();
        assert!(package.join("samples/Kick_Drum_-2.wav").exists());
        copy_samples(&base.join("missing"), &package).unwrap();
        let _ = fs::remove_dir_all(&base);
    }
}
