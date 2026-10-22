use crate::pattern::Track;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn setup() -> Vec<Value> {
    let mut edits = vec![json!({"op":"fx", "name":"__tracker_bypass", "source":"() { out = it }"})];
    for track in 0..16 {
        edits.push(json!({"op":"node", "id":format!("track_{track}"), "fx":"__tracker_bypass"}));
        edits.push(json!({"op":"connect", "id":format!("out_{track}"), "from":{"kind":"node", "id":format!("track_{track}")}, "to":{"kind":"master"}}));
    }
    edits
}

pub fn replacements(
    tracks: &[Track],
    effects: &BTreeMap<String, String>,
) -> Result<Vec<Value>, String> {
    tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.effect.is_some())
        .map(|(index, track)| replacement(index, track, effects))
        .collect()
}

fn replacement(
    index: usize,
    track: &Track,
    effects: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let fx = match &track.effect {
        Some(id) => effects
            .get(id)
            .map(String::as_str)
            .ok_or_else(|| format!("Apply the effect assigned to {} first", track.name))?,
        None => "__tracker_bypass",
    };
    Ok(json!({"op":"replace", "id":format!("track_{index}"), "fx":fx, "fade":0.02}))
}

pub fn events(
    tracks: &[Track],
    previous: &[Track],
    effects: &BTreeMap<String, String>,
) -> Result<Vec<Value>, String> {
    let mut topology = Vec::new();
    let mut controls = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let old = previous.get(index);
        if old.map(|t| &t.effect) != Some(&track.effect) {
            topology.push(replacement(index, track, effects)?);
        }
        if old.is_none_or(|t| t.muted != track.muted || t.volume_db != track.volume_db) {
            let gain = if track.muted {
                0.0
            } else {
                10.0_f64.powf(track.volume_db / 20.0)
            };
            controls.push(
                json!({"op":"gain", "id":format!("out_{index}"), "value":gain, "glide":0.01}),
            );
        }
    }
    // Keep gain moves on the engine's control path, including when an effect
    // changes in this batch. Graph replacement preserves existing live controls.
    Ok([topology, controls]
        .into_iter()
        .filter(|edits| !edits.is_empty())
        .map(|edits| json!({"op":"routing", "at":"now", "edits":edits}))
        .collect())
}
