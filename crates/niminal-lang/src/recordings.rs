//! The original recordings shared by Stage, the CLI, and every other language host.
//! Decoding is lazy, in memory, and shared across sessions. No project or cache files are written.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use niminal_engine::Table;

macro_rules! starter_files {
    ($($name:literal),* $(,)?) => {
        &[$((concat!($name, ".wav"), include_bytes!(concat!("../assets/starter/", $name, ".wav")).as_slice()),
            (concat!($name, ".nml"), include_bytes!(concat!("../assets/starter/", $name, ".nml")).as_slice()),)*
            ("starter.nml", include_bytes!("../assets/starter/starter.nml").as_slice()),
            ("README.md", include_bytes!("../assets/starter/README.md").as_slice()),
            ("LICENSE", include_bytes!("../assets/starter/LICENSE").as_slice())]
    };
}

/// The recordings, standalone source patches, editable kit, README and license, for exporting a copy.
pub const FILES: &[(&str, &[u8])] = starter_files![
    "kick_808",
    "snare_tri",
    "clap",
    "hat_closed",
    "hat_open",
    "tom",
    "cowbell",
    "crash",
    "electric_piano",
    "metal_bar",
    "struck_string",
    "fm_keys",
];

pub(crate) const PREFIX: &str = "std://samples/";

type Channels = Vec<Arc<Table>>;

pub(crate) fn load(name: &str) -> Result<Channels, String> {
    let (_, bytes) = FILES
        .iter()
        .find(|(file, _)| *file == name && file.ends_with(".wav"))
        .ok_or_else(|| format!("no bundled recording `{name}`"))?;
    static DECODED: OnceLock<Mutex<HashMap<String, Channels>>> = OnceLock::new();
    let mut decoded = DECODED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(channels) = decoded.get(name) {
        return Ok(channels.clone());
    }
    let channels: Channels = niminal_engine::read_wav_bytes(bytes)?
        .into_iter()
        .map(Arc::new)
        .collect();
    decoded.insert(name.into(), channels.clone());
    Ok(channels)
}
