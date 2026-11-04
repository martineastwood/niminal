//! Which sound card the stage plays through, and how, as the window shows and sets it.

use serde::{Deserialize, Serialize};

/// What the player chose. Nothing chosen means the system's default output at the card's own rate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioChoice {
    /// Part of the output device's name (as the engine matches it), or none for the default.
    #[serde(default)]
    pub output: Option<String>,
    /// Frames in each sound card callback: smaller is lower latency but less forgiving.
    #[serde(default)]
    pub buffer_size: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
}

/// The sample rates worth offering, when the device can do them.
const RATES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];

/// A sound card the stage can play through.
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub name: String,
    /// The sample rates it can play stereo float audio at.
    pub rates: Vec<u32>,
}

/// What the window needs to show the audio settings.
#[derive(Debug, Clone, Serialize)]
pub struct AudioState {
    pub devices: Vec<Device>,
    pub choice: AudioChoice,
    /// The sample rate and the output the engine is running with now.
    pub sample_rate: u32,
    pub output: String,
    /// The UDP port OSC is listened for on, if it is.
    pub osc_port: Option<u16>,
}

/// The output devices there are, read from the engine's own listing.
pub fn devices() -> Vec<Device> {
    let listing = niminal_cli::audio::devices();
    let mut seen = std::collections::BTreeSet::new();
    listing["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|device| {
            let name = device["name"].as_str()?.to_string();
            if !seen.insert(name.clone()) {
                return None;
            }
            let configs = device["configs"].as_array().cloned().unwrap_or_default();
            let can = |rate: u32| {
                configs.iter().any(|c| {
                    c["channels"] == 2
                        && c["format"] == "F32"
                        && c["min_sample_rate"].as_u64().is_some_and(|min| u64::from(rate) >= min)
                        && c["max_sample_rate"].as_u64().is_some_and(|max| u64::from(rate) <= max)
                })
            };
            Some(Device { name, rates: RATES.into_iter().filter(|r| can(*r)).collect() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_survives_being_written_and_read_and_missing_fields_mean_default() {
        let choice = AudioChoice { output: Some("Scarlett".into()), buffer_size: Some(128), sample_rate: Some(48_000) };
        let text = serde_json::to_string(&choice).unwrap();
        assert_eq!(serde_json::from_str::<AudioChoice>(&text).unwrap(), choice);
        assert_eq!(serde_json::from_str::<AudioChoice>("{}").unwrap(), AudioChoice::default());
    }
}
