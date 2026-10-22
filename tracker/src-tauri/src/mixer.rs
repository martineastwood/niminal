use crate::{
    commands::EffectParameters,
    pattern::{Track, TrackFx},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn setup() -> Vec<Value> {
    let mut edits = vec![
        json!({"op":"fx", "name":"__tracker_mix", "source":"(level: num = 1, balance: -1..1 = 0) { out = in * [1 - balance.clamp(0..1), 1 + balance.clamp(-1..0)] * level }"}),
    ];
    edits.extend(crate::modulation::declarations());
    for track in 0..16 {
        edits.push(json!({"op":"node", "id":format!("track_{track}"), "fx":"__tracker_mix"}));
        edits.push(json!({"op":"connect", "id":format!("out_{track}"), "from":{"kind":"node", "id":format!("track_{track}")}, "to":{"kind":"master"}}));
    }
    edits
}

fn arguments(slot: &TrackFx, parameters: &EffectParameters) -> BTreeMap<String, Value> {
    parameters
        .get(&slot.definition)
        .into_iter()
        .flatten()
        .map(|parameter| {
            (
                parameter.name.clone(),
                parameter.argument(
                    slot.values
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
    index: usize,
    slot: &TrackFx,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Value, String> {
    let fx = effects
        .get(&slot.definition)
        .ok_or("Apply the effect assigned to this track first")?;
    Ok(
        json!({"op":"replace", "id":format!("fx_{index}"), "fx":fx, "args":arguments(slot, parameters), "fade":0.02}),
    )
}

pub fn replacements(
    tracks: &[Track],
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Vec<Value>, String> {
    let mut edits = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        if let Some(slot) = &track.fx {
            edits.push(replacement(index, slot, effects, parameters)?);
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
        if let Some(slot) = &track.fx {
            let args = arguments(slot, parameters);
            if !args.is_empty() {
                edits.push(
                    json!({"op":"set", "id":format!("fx_{index}"), "args":args, "glide":0.01}),
                );
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
        let slot = tracks.get(index).and_then(|track| track.fx.as_ref());
        let old_slot = old.and_then(|track| track.fx.as_ref());
        if slot.is_none() && old_slot.is_some() {
            topology.push(json!({"op":"free_node", "id":format!("fx_{index}")}));
        }
        if let Some(track) = tracks.get(index) {
            if let Some(slot) = slot {
                let id = format!("fx_{index}");
                match old_slot {
                    None => {
                        let fx = effects.get(&slot.definition).ok_or_else(|| format!("Apply the effect assigned to {} first", track.name))?;
                        topology.push(json!({"op":"node", "id":id, "fx":fx, "args":arguments(slot, parameters)}));
                        topology.push(json!({"op":"connect", "id":format!("fx_in_{index}"), "from":{"kind":"node", "id":format!("track_{index}")}, "to":{"kind":"node", "id":id}}));
                    }
                    Some(old) if old.definition != slot.definition => topology.push(replacement(index, slot, effects, parameters)?),
                    Some(old) if old.values != slot.values => controls.push(json!({"op":"set", "id":id, "args":arguments(slot, parameters), "glide":0.01})),
                    _ => {}
                }
            }
            if slot.is_some() != old_slot.is_some() {
                let from = if slot.is_some() {
                    format!("fx_{index}")
                } else {
                    format!("track_{index}")
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
