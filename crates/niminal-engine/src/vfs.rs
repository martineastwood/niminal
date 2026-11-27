//! Files held in memory, looked at before the file system. It is empty unless a program puts files in it: a browser has
//! no file system, so its page hands over the files a piece names (samples, `.nml` modules, `.sfz` instruments) and
//! the rest of niminal finds them by the same relative paths it would use on disk.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct Store {
    /// path (relative, `/` separated) -> bytes and the generation it was put at
    files: BTreeMap<String, (Arc<Vec<u8>>, u64)>,
    generation: u64,
}

fn store() -> std::sync::MutexGuard<'static, Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// A path as the store knows it: relative, with `.` and `x/..` folded away. `None` for a path that starts at a root
/// or leaves the project.
fn key(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_str()?.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// Add or replace a file. `path` is relative to the project, such as `kick.wav` or `kits/drums/snare.wav`.
pub fn put(path: &str, bytes: Vec<u8>) {
    let Some(key) = key(Path::new(path)) else {
        return;
    };
    let mut store = store();
    store.generation += 1;
    let generation = store.generation;
    store.files.insert(key, (Arc::new(bytes), generation));
}

/// Take a file out.
pub fn remove(path: &str) {
    if let Some(key) = key(Path::new(path)) {
        store().files.remove(&key);
    }
}

/// Take every file out.
pub fn clear() {
    store().files.clear();
}

/// A file's bytes, and a number that changes whenever the file is replaced (to tell a cache the file is new).
pub fn get(path: &Path) -> Option<(Arc<Vec<u8>>, u64)> {
    let key = key(path)?;
    store().files.get(&key).cloned()
}

pub fn exists(path: &Path) -> bool {
    get(path).is_some()
}

/// A file read as text, if the store has it and it is text.
pub fn read_to_string(path: &Path) -> Option<String> {
    String::from_utf8(get(path)?.0.to_vec()).ok()
}

/// The files directly inside a folder, if the store has any there.
pub fn files_in(dir: &Path) -> Option<Vec<PathBuf>> {
    let prefix = key(dir)?;
    let store = store();
    let found: Vec<PathBuf> = store
        .files
        .keys()
        .filter(|k| {
            let rest = if prefix.is_empty() {
                Some(k.as_str())
            } else {
                k.strip_prefix(prefix.as_str()).and_then(|r| r.strip_prefix('/'))
            };
            rest.is_some_and(|r| !r.contains('/'))
        })
        .map(PathBuf::from)
        .collect();
    (!found.is_empty()).then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_found_however_they_are_written() {
        put("vfs_test/kick.wav", vec![1, 2, 3]);
        for spelling in ["vfs_test/kick.wav", "./vfs_test/kick.wav", "vfs_test/../vfs_test/./kick.wav"] {
            assert!(exists(Path::new(spelling)), "{spelling}");
        }
        assert!(!exists(Path::new("/vfs_test/kick.wav")));
        assert!(!exists(Path::new("../vfs_test/kick.wav")));
        assert_eq!(files_in(Path::new("./vfs_test")).unwrap(), vec![PathBuf::from("vfs_test/kick.wav")]);
        assert!(files_in(Path::new("vfs_test/none")).is_none());
        remove("vfs_test/kick.wav");
        assert!(!exists(Path::new("vfs_test/kick.wav")));
    }

    #[test]
    fn replacing_a_file_changes_its_generation() {
        put("vfs_gen/a.wav", vec![1]);
        let before = get(Path::new("vfs_gen/a.wav")).unwrap().1;
        put("vfs_gen/a.wav", vec![2]);
        let after = get(Path::new("vfs_gen/a.wav")).unwrap();
        assert_ne!(before, after.1);
        assert_eq!(*after.0, vec![2]);
        remove("vfs_gen/a.wav");
    }
}
