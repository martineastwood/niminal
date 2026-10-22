use crate::{
    commands::EffectParameters,
    pattern::{EffectSlot, Track},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn setup() -> Vec<Value> {
    let mut edits = vec![
        json!({"op":"fx", "name":"__tracker_mix", "source":"(level: num = 1, balance: -1..1 = 0) { out = it * [1 - balance.clamp(0..1), 1 + balance.clamp(-1..0)] * level }"}),
    ];
    edits.extend(crate::modulation::declarations());
    for track in 0..16 {
        edits.push(json!({"op":"node", "id":format!("track_{track}"), "fx":"__tracker_mix"}));
        edits.push(json!({"op":"connect", "id":format!("out_{track}"), "from":{"kind":"node", "id":format!("track_{track}")}, "to":{"kind":"master"}}));
    }
    edits
}

fn arguments(slot: &EffectSlot, parameters: &EffectParameters) -> BTreeMap<String, Value> {
    parameters
        .get(&slot.definition)
        .into_iter()
        .flatten()
        .map(|parameter| {
            (
                parameter.name.clone(),
                parameter.argument(
                    *slot
                        .values
                        .get(&parameter.name)
                        .unwrap_or(&parameter.default),
                ),
            )
        })
        .collect()
}

fn replacement(
    index: usize,
    device: usize,
    slot: &EffectSlot,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Value, String> {
    let fx = effects
        .get(&slot.definition)
        .ok_or("Apply the effect assigned to this track first")?;
    Ok(
        json!({"op":"replace", "id":format!("fx_{index}_{device}"), "fx":fx, "args":arguments(slot, parameters), "fade":0.02}),
    )
}

pub fn replacements(
    tracks: &[Track],
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Vec<Value>, String> {
    let mut edits = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        for (device, slot) in track.effects.iter().enumerate() {
            edits.push(replacement(index, device, slot, effects, parameters)?);
        }
    }
    Ok(edits)
}

fn base(index: usize, track: &Track) -> Value {
    json!({"op":"set", "id":format!("track_{index}"), "args":{"level":10.0_f64.powf(track.volume_db / 20.0), "balance":track.pan}, "glide":0.01})
}

fn output(index: usize, track: &Track) -> Value {
    json!({"op":"gain", "id":format!("out_{index}"), "value":if track.muted {0.0} else {1.0}, "glide":0.01})
}

pub fn reset(tracks: &[Track], parameters: &EffectParameters) -> Vec<Value> {
    let mut edits = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        edits.extend([base(index, track), output(index, track)]);
        for (device, slot) in track.effects.iter().enumerate() {
            let args = arguments(slot, parameters);
            if !args.is_empty() {
                edits.push(json!({"op":"set", "id":format!("fx_{index}_{device}"), "args":args, "glide":0.01}));
            }
        }
    }
    if edits.is_empty() {
        vec![]
    } else {
        vec![json!({"op":"routing", "at":"now", "edits":edits})]
    }
}

pub fn events(
    tracks: &[Track],
    previous: &[Track],
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
    bpm: f64,
    rows_per_beat: u32,
    timing_changed: bool,
) -> Result<Vec<Value>, String> {
    let (mut topology, modulators) = crate::modulation::edits(
        tracks,
        previous,
        parameters,
        bpm,
        rows_per_beat,
        timing_changed,
    );
    let mut controls = Vec::new();
    for index in 0..tracks.len().max(previous.len()) {
        let old = previous.get(index);
        let slots = tracks
            .get(index)
            .map_or(&[][..], |track| track.effects.as_slice());
        let old_slots = old.map_or(&[][..], |track| track.effects.as_slice());
        for device in slots.len()..old_slots.len() {
            topology.push(json!({"op":"free_node", "id":format!("fx_{index}_{device}")}));
        }
        if let Some(track) = tracks.get(index) {
            for (device, slot) in slots.iter().enumerate() {
                let id = format!("fx_{index}_{device}");
                match old_slots.get(device) {
                    None => {
                        let fx = effects.get(&slot.definition).ok_or_else(|| format!("Apply the effect assigned to {} first", track.name))?;
                        topology.push(json!({"op":"node", "id":id, "fx":fx, "args":arguments(slot, parameters)}));
                    }
                    Some(old) if old.definition != slot.definition => topology.push(replacement(index, device, slot, effects, parameters)?),
                    Some(old) if old.values != slot.values => controls.push(json!({"op":"set", "id":id, "args":arguments(slot, parameters), "glide":0.01})),
                    _ => {}
                }
                if slots.len() != old_slots.len() {
                    let from = if device == 0 {
                        format!("track_{index}")
                    } else {
                        format!("fx_{index}_{}", device - 1)
                    };
                    topology.push(json!({"op":"connect", "id":format!("chain_{index}_{device}"), "from":{"kind":"node", "id":from}, "to":{"kind":"node", "id":id}}));
                }
            }
            if slots.len() != old_slots.len() {
                let from = if slots.is_empty() {
                    format!("track_{index}")
                } else {
                    format!("fx_{index}_{}", slots.len() - 1)
                };
                topology.push(json!({"op":"connect", "id":format!("out_{index}"), "from":{"kind":"node", "id":from}, "to":{"kind":"master"}, "gain":if track.muted {0.0} else {1.0}}));
            }
            if old.is_none_or(|old| old.volume_db != track.volume_db || old.pan != track.pan) {
                controls.push(base(index, track));
            }
            if old.is_none_or(|old| old.muted != track.muted) {
                controls.push(output(index, track));
            }
        } else {
            topology.push(json!({"op":"connect", "id":format!("out_{index}"), "from":{"kind":"node", "id":format!("track_{index}")}, "to":{"kind":"master"}, "gain":0.0}));
        }
    }
    topology.extend(modulators);
    Ok([topology, controls]
        .into_iter()
        .filter(|edits| !edits.is_empty())
        .map(|edits| json!({"op":"routing", "at":"now", "edits":edits}))
        .collect())
}
