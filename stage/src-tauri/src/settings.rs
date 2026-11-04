//! What the stage remembers between runs: the sound card it plays through, and the port other tools reach it on.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::audio::AudioChoice;

/// The port the VS Code extension reaches the stage on; the engine's own socket is the one above it.
pub const DEFAULT_TOOLS_PORT: u16 = 7400;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub audio: AudioChoice,
    #[serde(default = "default_port")]
    pub tools_port: u16,
    /// The UDP port to listen on for OSC (`osc_in` in the language), if any.
    #[serde(default)]
    pub osc_port: Option<u16>,
}

fn default_port() -> u16 {
    DEFAULT_TOOLS_PORT
}

impl Default for Settings {
    fn default() -> Self {
        Settings { audio: AudioChoice::default(), tools_port: DEFAULT_TOOLS_PORT, osc_port: None }
    }
}

fn file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".niminal").join("stage.json"))
}

/// What was kept from last time; the defaults if there is nothing or it can't be read.
pub fn load() -> Settings {
    file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> Result<(), String> {
    let path = file().ok_or("There is no home folder to keep the settings in.")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_being_written_and_read_and_missing_fields_mean_the_defaults() {
        let settings = Settings {
            audio: AudioChoice { output: Some("Scarlett".into()), buffer_size: Some(128), sample_rate: Some(48_000) },
            tools_port: 7500,
            osc_port: Some(9000),
        };
        let text = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), Settings::default());
    }
}
