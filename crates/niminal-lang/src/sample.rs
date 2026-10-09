//! Loading the audio files that `sample` and `kit` declarations name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use niminal_engine::ops::SampleData;

/// A file as read, with the modification time it had then.
type Loaded = (Option<SystemTime>, Arc<SampleData>);

/// Where sample paths are found from, and the files already read. Clones
/// share one cache, so a live session reads each file once until it changes.
///
/// A relative path is looked for in each base folder in turn, so a project
/// built from snippets sent from several folders still finds all its files.
#[derive(Clone)]
pub struct Samples {
    bases: Vec<PathBuf>,
    cache: Arc<Mutex<HashMap<PathBuf, Loaded>>>,
}

impl Default for Samples {
    fn default() -> Self {
        Samples::new(".")
    }
}

impl Samples {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Samples { bases: vec![base.into()], cache: Arc::default() }
    }

    /// The same cache, with paths looked for in `bases`, first match wins.
    pub fn with_bases(&self, bases: Vec<PathBuf>) -> Self {
        Samples { bases, cache: self.cache.clone() }
    }

    /// Where `path` is, or where it was first looked for if it isn't anywhere.
    fn resolve(&self, path: &str) -> PathBuf {
        let found = self.bases.iter().map(|b| b.join(path)).find(|p| p.exists());
        found.unwrap_or_else(|| self.bases.first().map_or_else(|| PathBuf::from(path), |b| b.join(path)))
    }

    /// Read the files `source` declares into the cache, ignoring any problems
    /// (compiling reports them). A live daemon does this before it takes the
    /// lock that the audio thread needs, so reading a big file can't stall it.
    pub fn warm(&self, source: &str) {
        let Ok(items) = crate::parser::parse(source) else { return };
        for item in items {
            if let crate::ast::Item::Sample(decl) = item
                && let crate::ast::SampleSource::Path(path) = &decl.source
            {
                let _ = if decl.is_kit { self.load_kit(path).map(drop) } else { self.load(path).map(drop) };
            }
        }
    }

    pub fn load(&self, path: &str) -> Result<Arc<SampleData>, String> {
        self.load_file(&self.resolve(path))
    }

    fn load_file(&self, path: &Path) -> Result<Arc<SampleData>, String> {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((when, data)) = cache.get(path)
            && *when == modified
        {
            return Ok(data.clone());
        }
        let data = Arc::new(read_wav(path)?);
        cache.insert(path.to_path_buf(), (modified, data.clone()));
        Ok(data)
    }

    /// Every WAV file in a folder, by name without the extension, in name order.
    pub fn load_kit(&self, path: &str) -> Result<Vec<(String, Arc<SampleData>)>, String> {
        let dir = self.resolve(path);
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("can't read the folder {}: {e}", dir.display()))?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(format!("there are no .wav files in {}", dir.display()));
        }
        files
            .iter()
            .map(|file| {
                let name = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
                Ok((name, self.load_file(file)?))
            })
            .collect()
    }
}

fn read_wav(path: &Path) -> Result<SampleData, String> {
    let fail = |e: &dyn std::fmt::Display| format!("can't read {}: {e}", path.display());
    let mut reader = hound::WavReader::open(path).map_err(|e| fail(&e))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>().map_err(|e| fail(&e))?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| fail(&e))?
        }
    };
    if channels == 0 || interleaved.is_empty() {
        return Err(format!("{} has no audio in it", path.display()));
    }
    let mut buffers = vec![Vec::with_capacity(interleaved.len() / channels); channels];
    for frame in interleaved.chunks_exact(channels) {
        for (buffer, &x) in buffers.iter_mut().zip(frame) {
            buffer.push(x);
        }
    }
    Ok(SampleData { channels: buffers, sample_rate: spec.sample_rate as f32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warming_reads_each_declared_file_once_and_ignores_problems() {
        let dir = std::env::temp_dir().join(format!("niminal-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(dir.join("a.wav"), spec).unwrap();
        w.write_sample(1000_i16).unwrap();
        w.finalize().unwrap();

        let samples = Samples::new(&dir);
        samples.warm("sample a = \"a.wav\"\nsample gone = \"missing.wav\"\nthis is not valid");
        samples.warm("sample a = \"a.wav\"\nkit k = \"nofolder\"");
        let first = samples.load("a.wav").unwrap();
        assert!(Arc::ptr_eq(&first, &samples.load("a.wav").unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
