//! A project folder: the files to open as tabs, and the one that plays them.

use std::path::{Path, PathBuf};

/// The files of the project in `dir`, and the entry that plays it. With a `main.nml` that is the file
/// and every module the language resolves through `use`; broken imports still open main. Without
/// one, every `.nml` in the folder, by name, and no entry.
pub fn files(dir: &Path) -> std::io::Result<(Option<PathBuf>, Vec<PathBuf>)> {
    let main = dir.join("main.nml");
    if !main.exists() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "nml"))
            .collect();
        files.sort();
        return Ok((None, files));
    }
    let source = std::fs::read_to_string(&main)?;
    let mut files = vec![main.clone()];
    // A broken import must not prevent opening main to fix it.
    if let Ok(expanded) = niminal_compose::expand(&source, dir) {
        files.extend(expanded.files().map(Path::to_path_buf));
    }
    Ok((Some(main), files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "niminal-project-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir.canonicalize().unwrap()
    }

    fn names(dir: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|f| f.strip_prefix(dir).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn broken_imports_still_open_main() {
        let dir = folder(&[("main.nml", "use missing\n")]);
        let (entry, files) = files(&dir).unwrap();
        assert_eq!(entry, Some(dir.join("main.nml")));
        assert_eq!(names(&dir, &files), ["main.nml"]);
    }

    #[test]
    fn main_leads_and_its_modules_follow_once_each() {
        let dir = folder(&[
            ("main.nml", "use drums\nuse bass\nuse pads\n"),
            ("drums.nml", "use bass\n"),
            ("bass.nml", ""),
            ("lib/pads.nml", ""),
            ("scratch.nml", ""),
        ]);
        let (entry, files) = files(&dir).unwrap();
        assert_eq!(entry, Some(dir.join("main.nml")));
        assert_eq!(names(&dir, &files), ["main.nml", "bass.nml", "drums.nml", "lib/pads.nml"]);
    }

    #[test]
    fn without_main_every_file_is_a_tab() {
        let dir = folder(&[("b.nml", ""), ("a.nml", ""), ("notes.txt", "")]);
        let (entry, files) = files(&dir).unwrap();
        assert_eq!(entry, None);
        assert_eq!(names(&dir, &files), ["a.nml", "b.nml"]);
    }
}
