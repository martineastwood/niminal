//! The engine graph behind the mixer. Every track, and the master, is a *strip*:
//!
//! `track_<s>` (level and pan) → its effect devices → `tap_<s>` → `out_<s>` (mute) → next stage
//!
//! Tracks feed `track_m`, the master's strip, which feeds the engine's master output. The tap is a
//! pass-through so each channel's finished signal has one steady place for a scope to listen.
use crate::{
    commands::{EffectParameters, fx_node},
    pattern::{Device, Track},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The label of the master strip; tracks are labelled by number.
pub const MASTER: &str = "m";

pub fn setup() -> Vec<Value> {
    let mut edits = vec![
        json!({"op":"fx", "name":"__tracker_mix", "source":"(level: num = 1, balance: -1..1 = 0) { out = in * [1 - balance.clamp(0..1), 1 + balance.clamp(-1..0)] * level }"}),
        json!({"op":"fx", "name":"__tracker_tap", "source":"() { out = in }"}),
    ];
    edits.extend(crate::modulation::declarations());
    let labels = (0..16).map(|t| t.to_string()).chain([MASTER.to_string()]);
    for label in labels {
        edits.push(json!({"op":"node", "id":format!("track_{label}"), "fx":"__tracker_mix"}));
        edits.push(json!({"op":"node", "id":format!("tap_{label}"), "fx":"__tracker_tap"}));
        edits.push(json!({"op":"connect", "id":format!("tapin_{label}"), "from":{"kind":"node", "id":format!("track_{label}")}, "to":{"kind":"node", "id":format!("tap_{label}")}}));
        let to = if label == MASTER {
            json!({"kind":"master"})
        } else {
            json!({"kind":"node", "id":format!("track_{MASTER}")})
        };
        edits.push(json!({"op":"connect", "id":format!("out_{label}"), "from":{"kind":"node", "id":format!("tap_{label}")}, "to":to}));
    }
    edits
}

/// The strips of a song, in order: its tracks, then the master.
pub fn strips<'a>(tracks: &'a [Track], master: &'a Track) -> Vec<(String, &'a Track)> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i.to_string(), t))
        .chain([(MASTER.to_string(), master)])
        .collect()
}

/// Whether a track makes no sound: muted, or another track is soloed. The master only has a level.
fn silent(tracks: &[Track], index: usize) -> bool {
    tracks[index].muted || tracks.iter().any(|t| t.solo) && !tracks[index].solo
}

fn arguments(device: &Device, parameters: &EffectParameters) -> BTreeMap<String, Value> {
    parameters
        .get(&device.definition)
        .into_iter()
        .flatten()
        .map(|parameter| {
            (
                parameter.name.clone(),
                parameter.argument(
                    device
                        .values
                        .get(&parameter.name)
                        .copied()
                        .unwrap_or(parameter.default)
                        .clamp(parameter.min, parameter.max),
                ),
            )
        })
        .collect()
}

fn replacement(
    slot: &str,
    device: &Device,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Value, String> {
    let fx = effects
        .get(&device.definition)
        .ok_or("Apply the effect assigned to this track first")?;
    Ok(
        json!({"op":"replace", "id":fx_node(slot, &device.id), "fx":fx, "args":arguments(device, parameters), "fade":0.02}),
    )
}

pub fn replacements(
    tracks: &[Track],
    master: &Track,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Vec<Value>, String> {
    let mut edits = Vec::new();
    for (label, strip) in strips(tracks, master) {
        for device in strip.active() {
            edits.push(replacement(&label, device, effects, parameters)?);
        }
    }
    Ok(edits)
}

fn base(label: &str, strip: &Track) -> Value {
    json!({"op":"set", "id":format!("track_{label}"), "args":{"level":10.0_f64.powf(strip.volume_db / 20.0), "balance":strip.pan}, "glide":0.01})
}

fn output(label: &str, silent: bool) -> Value {
    json!({"op":"gain", "id":format!("out_{label}"), "value":if silent {0.0} else {1.0}, "glide":0.01})
}

pub fn reset(tracks: &[Track], master: &Track, parameters: &EffectParameters) -> Vec<Value> {
    let mut edits = Vec::new();
    for (index, (label, strip)) in strips(tracks, master).into_iter().enumerate() {
        edits.push(base(&label, strip));
        if label != MASTER {
            edits.push(output(&label, silent(tracks, index)));
        }
        for device in strip.active() {
            let args = arguments(device, parameters);
            if !args.is_empty() {
                edits.push(
                    json!({"op":"set", "id":fx_node(&label, &device.id), "args":args, "glide":0.01}),
                );
            }
        }
    }
    vec![json!({"op":"routing", "at":"now", "edits":edits})]
}

/// What changing the mixer from `previous` to `tracks` and `master` takes.
#[allow(clippy::too_many_arguments)]
pub fn events(
    tracks: &[Track],
    master: &Track,
    previous: &[Track],
    previous_master: &Track,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
    bpm: f64,
    rows_per_beat: u32,
    timing_changed: bool,
) -> Result<Vec<Value>, String> {
    let (mut topology, modulators) = crate::modulation::edits(
        tracks,
        master,
        previous,
        previous_master,
        parameters,
        bpm,
        rows_per_beat,
        timing_changed,
    );
    let mut controls = Vec::new();
    let new = strips(tracks, master);
    let old = strips(previous, previous_master);
    // Tracks may be added or removed; the master is always last in both lists.
    let count = tracks.len().max(previous.len());
    for index in 0..=count {
        let is_master = index == count;
        let label = if is_master {
            MASTER.to_string()
        } else {
            index.to_string()
        };
        let current = if is_master {
            new.last()
        } else {
            new.get(index).filter(|_| index < tracks.len())
        };
        let before = if is_master {
            old.last()
        } else {
            old.get(index).filter(|_| index < previous.len())
        };
        let old_chain: Vec<&Device> = before
            .map(|(_, t)| t.active().collect())
            .unwrap_or_default();
        let chain: Vec<&Device> = current
            .map(|(_, t)| t.active().collect())
            .unwrap_or_default();
        for device in &old_chain {
            if !chain.iter().any(|d| d.id == device.id) {
                topology.push(json!({"op":"free_node", "id":fx_node(&label, &device.id)}));
            }
        }
        let Some((_, strip)) = current else {
            // A removed track goes quiet; its nodes stay for when it comes back.
            topology.push(json!({"op":"gain", "id":format!("out_{label}"), "value":0.0}));
            continue;
        };
        for device in &chain {
            match old_chain.iter().find(|d| d.id == device.id) {
                None => {
                    let fx = effects.get(&device.definition).ok_or_else(|| format!("Apply the effect assigned to {} first", strip.name))?;
                    topology.push(json!({"op":"node", "id":fx_node(&label, &device.id), "fx":fx, "args":arguments(device, parameters)}));
                }
                Some(old) if old.definition != device.definition => topology.push(replacement(&label, device, effects, parameters)?),
                Some(old) if old.values != device.values => controls.push(json!({"op":"set", "id":fx_node(&label, &device.id), "args":arguments(device, parameters), "glide":0.01})),
                _ => {}
            }
        }
        if !chain
            .iter()
            .map(|d| &d.id)
            .eq(old_chain.iter().map(|d| &d.id))
        {
            // Links are named by position, so reordering or removing a device re-points them in place.
            let mut from = format!("track_{label}");
            for (position, device) in chain.iter().enumerate() {
                let to = fx_node(&label, &device.id);
                topology.push(json!({"op":"connect", "id":format!("link_{label}_{position}"), "from":{"kind":"node", "id":from}, "to":{"kind":"node", "id":to}}));
                from = to;
            }
            topology.push(json!({"op":"connect", "id":format!("tapin_{label}"), "from":{"kind":"node", "id":from}, "to":{"kind":"node", "id":format!("tap_{label}")}}));
        }
        let was = before.map(|(_, t)| *t);
        if was.is_none_or(|o| o.volume_db != strip.volume_db || o.pan != strip.pan) {
            controls.push(base(&label, strip));
        }
        if !is_master {
            let now = silent(tracks, index);
            if before.is_none() || index >= previous.len() || silent(previous, index) != now {
                controls.push(output(&label, now));
            }
        }
    }
    topology.extend(modulators);
    Ok([topology, controls]
        .into_iter()
        .filter(|edits| !edits.is_empty())
        .map(|edits| json!({"op":"routing", "at":"now", "edits":edits}))
        .collect())
}
