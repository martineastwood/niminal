use crate::{
    commands::{EffectParameters, fx_node},
    pattern::{Device, Track},
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
    index: usize,
    device: &Device,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Value, String> {
    let fx = effects
        .get(&device.definition)
        .ok_or("Apply the effect assigned to this track first")?;
    Ok(
        json!({"op":"replace", "id":fx_node(index, &device.id), "fx":fx, "args":arguments(device, parameters), "fade":0.02}),
    )
}

pub fn replacements(
    tracks: &[Track],
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Vec<Value>, String> {
    let mut edits = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        for device in track.active() {
            edits.push(replacement(index, device, effects, parameters)?);
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
        for device in track.active() {
            let args = arguments(device, parameters);
            if !args.is_empty() {
                edits.push(
                    json!({"op":"set", "id":fx_node(index, &device.id), "args":args, "glide":0.01}),
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
        let old_chain: Vec<&Device> = old.map(|t| t.active().collect()).unwrap_or_default();
        let chain: Vec<&Device> = tracks
            .get(index)
            .map(|t| t.active().collect())
            .unwrap_or_default();
        for device in &old_chain {
            if !chain.iter().any(|d| d.id == device.id) {
                topology.push(json!({"op":"free_node", "id":fx_node(index, &device.id)}));
            }
        }
        if let Some(track) = tracks.get(index) {
            for device in &chain {
                match old_chain.iter().find(|d| d.id == device.id) {
                    None => {
                        let fx = effects.get(&device.definition).ok_or_else(|| format!("Apply the effect assigned to {} first", track.name))?;
                        topology.push(json!({"op":"node", "id":fx_node(index, &device.id), "fx":fx, "args":arguments(device, parameters)}));
                    }
                    Some(old) if old.definition != device.definition => topology.push(replacement(index, device, effects, parameters)?),
                    Some(old) if old.values != device.values => controls.push(json!({"op":"set", "id":fx_node(index, &device.id), "args":arguments(device, parameters), "glide":0.01})),
                    _ => {}
                }
            }
            if !chain
                .iter()
                .map(|d| &d.id)
                .eq(old_chain.iter().map(|d| &d.id))
            {
                // Links are named by position, so reordering or removing a device re-points them in place.
                let mut from = format!("track_{index}");
                for (position, device) in chain.iter().enumerate() {
                    let to = fx_node(index, &device.id);
                    topology.push(json!({"op":"connect", "id":format!("link_{index}_{position}"), "from":{"kind":"node", "id":from}, "to":{"kind":"node", "id":to}}));
                    from = to;
                }
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
