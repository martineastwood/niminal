//! A project folder: the files to open as tabs, and the one that plays them.

use std::path::{Path, PathBuf};

/// The module names a source file `use`s, ignoring aliases and comments.
fn used_modules(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let name = line.trim_start().strip_prefix("use")?.strip_prefix(char::is_whitespace)?;
            let name: String = name
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '/')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// The files of the project in `dir`, and the entry that plays it. With a `main.nml` that is the file
/// and every module it reaches through `use` (found next to the file using it, or in `lib`); without
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
    let mut files = Vec::new();
    visit(dir, &main, &mut files)?;
    Ok((Some(main), files))
}

fn visit(dir: &Path, file: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if files.iter().any(|f| f == file) {
        return Ok(());
    }
    files.push(file.to_path_buf());
    let beside = file.parent().unwrap_or(dir);
    for name in used_modules(&std::fs::read_to_string(file)?) {
        let file_name = format!("{name}.nml");
        if let Some(found) = [beside.join(&file_name), dir.join("lib").join(&file_name)]
            .into_iter()
            .find(|p| p.exists())
        {
            visit(dir, &found, files)?;
        }
    }
    Ok(())
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
        dir
    }

    fn names(dir: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|f| f.strip_prefix(dir).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn use_lines_give_module_names() {
        assert_eq!(used_modules("use a\n  use score/b as x\n// use c\nlet user = 1\nuse"), ["a", "score/b"]);
    }

    #[test]
    fn main_leads_and_its_modules_follow_once_each() {
        let dir = folder(&[
            ("main.nml", "use drums\nuse std/filters\nuse bass\nuse pads\n"),
            ("drums.nml", "use bass\n"),
            ("bass.nml", ""),
            ("lib/pads.nml", ""),
            ("scratch.nml", ""),
        ]);
        let (entry, files) = files(&dir).unwrap();
        assert_eq!(entry, Some(dir.join("main.nml")));
        assert_eq!(names(&dir, &files), ["main.nml", "drums.nml", "bass.nml", "lib/pads.nml"]);
    }

    #[test]
    fn without_main_every_file_is_a_tab() {
        let dir = folder(&[("b.nml", ""), ("a.nml", ""), ("notes.txt", "")]);
        let (entry, files) = files(&dir).unwrap();
        assert_eq!(entry, None);
        assert_eq!(names(&dir, &files), ["a.nml", "b.nml"]);
    }
}
