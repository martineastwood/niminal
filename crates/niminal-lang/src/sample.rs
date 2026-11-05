//! Loading the audio files that `sample`, `kit` and `file()` declarations name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use niminal_engine::Table;

/// A region of an SFZ file with the audio it plays.
pub struct SfzSample {
    pub region: crate::sfz::SfzRegion,
    pub channels: Vec<Arc<Table>>,
    /// What the audio file says about its root note and loop.
    pub info: niminal_engine::WavInfo,
}

/// A kit: each sample's name with its channels as tables.
pub type Kit = Vec<(String, Vec<Arc<Table>>)>;

/// Channels of a loaded file, with the modification time it had then.
type Loaded = (Option<SystemTime>, Vec<Arc<Table>>);

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
        Samples {
            bases: vec![base.into()],
            cache: Arc::default(),
        }
    }

    /// The folders paths are looked for in, the first match winning.
    pub fn bases(&self) -> &[PathBuf] {
        &self.bases
    }

    /// The same cache, with paths looked for in `bases`, first match wins.
    pub fn with_bases(&self, bases: Vec<PathBuf>) -> Self {
        Samples {
            bases,
            cache: self.cache.clone(),
        }
    }

    /// Where `path` is, or where it was first looked for if it isn't anywhere.
    fn resolve(&self, path: &str) -> PathBuf {
        let found = self.bases.iter().map(|b| b.join(path)).find(|p| p.exists());
        found.unwrap_or_else(|| {
            self.bases
                .first()
                .map_or_else(|| PathBuf::from(path), |b| b.join(path))
        })
    }

    /// Public form of [`Samples::resolve`] for `load("…")` and similar.
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        self.resolve(path)
    }

    /// Read the files `source` declares into the cache, ignoring any problems
    /// (compiling reports them). A live server does this before it takes the
    /// lock that the audio thread needs, so reading a big file can't stall it.
    pub fn warm(&self, source: &str) {
        let Ok(items) = crate::parser::parse(source) else {
            return;
        };
        for item in items {
            let crate::ast::Item::Sample(decl) = item else {
                continue;
            };
            let _ = match &decl.source {
                crate::ast::SampleSource::Path(path) if decl.is_kit => {
                    self.load_kit(path).map(drop)
                }
                crate::ast::SampleSource::Path(path) => self.load(path).map(drop),
                crate::ast::SampleSource::Sfz(path) => self.load_sfz(path).map(drop),
                _ => continue,
            };
        }
    }

    /// Load an audio or numeric file as one table per channel.
    pub fn load(&self, path: &str) -> Result<Vec<Arc<Table>>, String> {
        self.load_file(&self.resolve(path))
    }

    /// Load one channel of an audio or numeric file as a table (`channel` counts from 0).
    pub fn load_channel(
        &self,
        path: &str,
        channel: usize,
        start: Option<usize>,
        end: Option<usize>,
    ) -> Result<Arc<Table>, String> {
        let channels = self.load(path)?;
        let table = channels.get(channel).ok_or_else(|| {
            format!(
                "{} has {} channel{}, not channel {channel}",
                path,
                channels.len(),
                if channels.len() == 1 { "" } else { "s" }
            )
        })?;
        let len = table.len();
        let from = start.unwrap_or(0).min(len);
        let to = end.unwrap_or(len).min(len);
        if to <= from {
            return Err(format!("the slice of {path} is empty"));
        }
        if from == 0 && to == len {
            Ok(table.clone())
        } else {
            Ok(Arc::new(table.slice(from, to)))
        }
    }

    fn load_file(&self, path: &Path) -> Result<Vec<Arc<Table>>, String> {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((when, data)) = cache.get(path)
            && *when == modified
        {
            return Ok(data.clone());
        }
        let tables = niminal_engine::read_sample_file(path)?;
        let data = tables.into_iter().map(Arc::new).collect::<Vec<_>>();
        cache.insert(path.to_path_buf(), (modified, data.clone()));
        Ok(data)
    }

    /// An SFZ file's regions, each with its audio loaded. Sample paths are relative to the file.
    pub fn load_sfz(&self, path: &str) -> Result<Vec<SfzSample>, String> {
        let file = self.resolve(path);
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("can't read {}: {e}", file.display()))?;
        let sfz = crate::sfz::parse(&text, &mut |name| {
            let included = dir.join(name.replace('\\', "/"));
            std::fs::read_to_string(&included)
                .map_err(|e| format!("can't read {}: {e}", included.display()))
        })
        .map_err(|e| format!("{}: {e}", file.display()))?;
        sfz.regions
            .into_iter()
            .map(|region| {
                let audio = dir.join(&region.sample);
                let channels = self.load_file(&audio)?;
                let info = niminal_engine::read_wav_info(&audio).unwrap_or_default();
                Ok(SfzSample {
                    region,
                    channels,
                    info,
                })
            })
            .collect()
    }

    /// Every audio file in a folder, by name without the extension, in name order.
    /// Each entry is the file's channels as tables.
    pub fn load_kit(&self, path: &str) -> Result<Kit, String> {
        let dir = self.resolve(path);
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("can't read the folder {}: {e}", dir.display()))?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_audio(p))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(format!("there are no audio files in {}", dir.display()));
        }
        files
            .iter()
            .map(|file| {
                let name = file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                Ok((name, self.load_file(file)?))
            })
            .collect()
    }
}

pub use niminal_engine::{AUDIO_EXTENSIONS, is_audio};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aiff_file_loads_like_a_wav() {
        // 16-bit mono AIFF at 44100 Hz, four frames
        let frames: [i16; 4] = [0, 16384, -16384, 32767];
        let mut comm = Vec::new();
        comm.extend(1_u16.to_be_bytes());
        comm.extend(4_u32.to_be_bytes());
        comm.extend(16_u16.to_be_bytes());
        comm.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        let mut ssnd = vec![0_u8; 8];
        for f in frames {
            ssnd.extend(f.to_be_bytes());
        }
        let mut body = b"AIFF".to_vec();
        for (id, data) in [(b"COMM", comm), (b"SSND", ssnd)] {
            body.extend(id);
            body.extend((data.len() as u32).to_be_bytes());
            body.extend(data);
        }
        let mut file = b"FORM".to_vec();
        file.extend((body.len() as u32).to_be_bytes());
        file.extend(body);
        let dir = std::env::temp_dir().join(format!("niminal-aiff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.aiff"), file).unwrap();
        let table = Samples::new(&dir)
            .load_channel("a.aiff", 0, None, None)
            .unwrap();
        assert_eq!(table.len(), 4);
        assert!((table.get(1) - 0.5).abs() < 1e-3 && (table.get(2) + 0.5).abs() < 1e-3);
        assert!(is_audio(Path::new("x/Kick.FLAC")) && !is_audio(Path::new("x/notes.txt")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_csv_is_a_table_for_each_column() {
        let dir = std::env::temp_dir().join(format!("niminal-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("d.csv"),
            "time, level\n0, 0.5\n# a comment\n1, -0.25\n2, 1\n",
        )
        .unwrap();
        std::fs::write(dir.join("bad.csv"), "1 2\n3\n").unwrap();
        let samples = Samples::new(&dir);
        let level = samples.load_channel("d.csv", 1, None, None).unwrap();
        assert_eq!(level.len(), 3);
        assert_eq!(
            (level.get(0), level.get(1), level.get(2)),
            (0.5, -0.25, 1.0)
        );
        assert!(
            samples
                .load_channel("d.csv", 2, None, None)
                .unwrap_err()
                .contains("2 channels")
        );
        assert!(
            samples
                .load("bad.csv")
                .unwrap_err()
                .contains("line 2 has 1 values")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warming_reads_each_declared_file_once_and_ignores_problems() {
        let dir = std::env::temp_dir().join(format!("niminal-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(dir.join("a.wav"), spec).unwrap();
        w.write_sample(1000_i16).unwrap();
        w.finalize().unwrap();

        let samples = Samples::new(&dir);
        samples.warm("a = sample(\"a.wav\")\ngone = sample(\"missing.wav\")\nthis is not valid");
        samples.warm("a = sample(\"a.wav\")\nk = kit(\"nofolder\")");
        let first = samples.load("a.wav").unwrap();
        assert!(Arc::ptr_eq(&first[0], &samples.load("a.wav").unwrap()[0]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
