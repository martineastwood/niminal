//! The engine graph behind the mixer. Every track, return and the master is a *strip*:
//!
//! `in_<s>` → its effect devices → `track_<s>` (level and pan) → `tap_<s>` → `out_<s>` (mute) → next stage
//!
//! Notes and other strips arrive at `in_<s>`. Tracks and returns feed `in_m`, the master's strip, which
//! feeds the engine's master output. Only the strips a song has exist in the engine: adding a track or
//! return builds its nodes and links, and removing one frees them, so an idle graph costs what the song uses. The tap is a pass-through so each strip's finished signal has one
//! steady place for a scope to listen. A return is a strip labelled `r<n>`. A track's or return's tap
//! feeds the master through `out_<s>`, or a return through `outr_<s>_<n>`, whichever its output names;
//! a return can only feed later returns. A send device is a node that feeds a return through
//! `sendout_<s>_<device>`. Post-fader (the default) it listens to the tap; pre-fader it listens at its
//! place in the chain, and may *mute the source*, so nothing after it carries sound on.
use crate::{
    commands::{EffectParameters, fx_node, macro_edits},
    pattern::{Device, Output, Track},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The label of the master strip; tracks are labelled by number.
pub const MASTER: &str = "m";

pub fn setup() -> Vec<Value> {
    let mut edits = vec![
        json!({"op":"fx", "name":"__tracker_mix", "source":"(level: num = 1, balance: -1..1 = 0) { out = in * [1 - balance.clamp(0..1), 1 + balance.clamp(-1..0)] * level }"}),
        json!({"op":"fx", "name":"__tracker_tap", "source":"() { out = in }"}),
    ];
    edits.extend(crate::modulation::declarations());
    edits.extend(strip_nodes(MASTER));
    edits
}

/// The nodes and links of one strip: input, level and pan, tap, and the link on to the master (or, for
/// the master itself, to the engine's output).
fn strip_nodes(label: &str) -> Vec<Value> {
    let to = if label == MASTER { json!({"kind":"master"}) } else { json!({"kind":"node", "id":format!("in_{MASTER}")}) };
    vec![
        json!({"op":"node", "id":format!("in_{label}"), "fx":"__tracker_tap"}),
        json!({"op":"node", "id":format!("track_{label}"), "fx":"__tracker_mix"}),
        json!({"op":"node", "id":format!("tap_{label}"), "fx":"__tracker_tap"}),
        json!({"op":"connect", "id":format!("fadein_{label}"), "from":{"kind":"node", "id":format!("in_{label}")}, "to":{"kind":"node", "id":format!("track_{label}")}}),
        json!({"op":"connect", "id":format!("tapin_{label}"), "from":{"kind":"node", "id":format!("track_{label}")}, "to":{"kind":"node", "id":format!("tap_{label}")}}),
        json!({"op":"connect", "id":format!("out_{label}"), "from":{"kind":"node", "id":format!("tap_{label}")}, "to":to}),
    ]
}

/// The label of every track and return a song has, in order; the master is not among them.
pub fn labels(tracks: &[Track], returns: &[Track]) -> Vec<String> {
    (0..tracks.len()).map(|t| t.to_string()).chain((0..returns.len()).map(|r| format!("r{r}"))).collect()
}

/// The strips of a song, in order: its tracks, its returns, then the master.
pub fn strips<'a>(
    tracks: &'a [Track],
    returns: &'a [Track],
    master: &'a Track,
) -> Vec<(String, &'a Track)> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i.to_string(), t))
        .chain(returns.iter().enumerate().map(|(i, t)| (format!("r{i}"), t)))
        .chain([(MASTER.to_string(), master)])
        .collect()
}

/// Whether a track makes no sound: muted, or another track is soloed. The master only has a level.
pub fn silent(tracks: &[Track], index: usize) -> bool {
    tracks[index].muted || tracks.iter().any(|t| t.solo) && !tracks[index].solo
}

/// A link that carries a strip's signal on: from this node to that one, open at `gain`.
#[derive(Clone, Debug, PartialEq)]
struct Link {
    from: String,
    to: String,
    gain: f64,
}

/// Every link that carries a strip's signal on, by connection id: the one to the master, the one to
/// the return it outputs to, and one out of each send device whose return exists. Nothing flows while
/// a strip is silent. A return can only feed later returns, which rules out loops. The master has none.
fn links(tracks: &[Track], returns: &[Track]) -> BTreeMap<String, Link> {
    let mut links = BTreeMap::new();
    let strips = tracks.iter().enumerate().map(|(i, t)| (i.to_string(), t, !silent(tracks, i))).chain(returns.iter().enumerate().map(|(i, t)| (format!("r{i}"), t, !t.muted)));
    for (label, strip, open) in strips {
        let gate = |on: bool| if open && on { 1.0 } else { 0.0 };
        let tap = format!("tap_{label}");
        links.insert(format!("out_{label}"), Link { from: tap.clone(), to: format!("in_{MASTER}"), gain: gate(strip.output == Output::Master) });
        if let Output::Return { index } = strip.output
            && index < returns.len()
        {
            links.insert(format!("outr_{label}_{index}"), Link { from: tap, to: format!("in_r{index}"), gain: gate(true) });
        }
        for device in strip.active() {
            if let Some(target) = device.target.filter(|t| *t < returns.len()) {
                links.insert(format!("sendout_{label}_{}", device.id), Link { from: fx_node(&label, &device.id), to: format!("in_r{target}"), gain: gate(true) });
            }
        }
    }
    links
}

/// Every node a song has in the engine's graph: the strips' own and their active devices.
fn nodes(tracks: &[Track], returns: &[Track], master: &Track) -> BTreeSet<String> {
    let mut nodes = BTreeSet::new();
    for (label, strip) in strips(tracks, returns, master) {
        nodes.extend(["in", "track", "tap"].map(|part| format!("{part}_{label}")));
        nodes.extend(strip.active().map(|device| fx_node(&label, &device.id)));
    }
    nodes
}

fn gain(id: &str, value: f64) -> Value {
    json!({"op":"gain", "id":id, "value":value, "glide":0.01})
}

fn arguments(device: &Device, parameters: &EffectParameters) -> BTreeMap<String, Value> {
    parameters
        .get(&device.definition)
        .into_iter()
        .flatten()
        .map(|parameter| {
            // left at its default, a length in beats stays to them as the tempo changes
            let argument = match device.values.get(&parameter.name) {
                None => parameter.default_argument(),
                Some(value) => parameter.argument(value.clamp(parameter.min, parameter.max)),
            };
            (parameter.name.clone(), argument)
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
    returns: &[Track],
    master: &Track,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
) -> Result<Vec<Value>, String> {
    let mut edits = Vec::new();
    for (label, strip) in strips(tracks, returns, master) {
        for device in strip.active() {
            edits.push(replacement(&label, device, effects, parameters)?);
        }
        for (index, knob) in strip.macros.iter().enumerate() {
            edits.extend(macro_value(&label, strip, index, knob.value, parameters));
        }
    }
    Ok(edits)
}

fn base(label: &str, strip: &Track) -> Value {
    json!({"op":"set", "id":format!("track_{label}"), "args":{"level":10.0_f64.powf(strip.volume_db / 20.0), "balance":strip.pan}, "glide":0.01})
}

pub fn reset(
    tracks: &[Track],
    returns: &[Track],
    master: &Track,
    parameters: &EffectParameters,
) -> Vec<Value> {
    let mut edits = Vec::new();
    for (label, strip) in strips(tracks, returns, master) {
        edits.push(base(&label, strip));
        for device in strip.active() {
            let args = arguments(device, parameters);
            if !args.is_empty() {
                edits.push(
                    json!({"op":"set", "id":fx_node(&label, &device.id), "args":args, "glide":0.01}),
                );
            }
        }
        for (index, knob) in strip.macros.iter().enumerate() {
            edits.extend(macro_value(&label, strip, index, knob.value, parameters));
        }
    }
    edits.extend(links(tracks, returns).iter().map(|(id, link)| gain(id, link.gain)));
    vec![json!({"op":"routing", "at":"now", "edits":edits})]
}

fn macro_value(label: &str, strip: &Track, index: usize, value: f64, parameters: &EffectParameters) -> Vec<Value> {
    let mut edits = macro_edits(label, strip, index, value, parameters);
    for edit in &mut edits { edit["glide"] = json!(0.01); }
    edits
}

fn find<'a>(strips: &[(String, &'a Track)], label: &str) -> Option<&'a Track> {
    strips.iter().find(|(l, _)| l == label).map(|(_, t)| *t)
}

/// What changing the mixer from `previous` to `tracks`, `returns` and `master` takes.
#[allow(clippy::too_many_arguments)]
pub fn events(
    tracks: &[Track],
    returns: &[Track],
    master: &Track,
    previous: &[Track],
    previous_returns: &[Track],
    previous_master: &Track,
    effects: &BTreeMap<String, String>,
    parameters: &EffectParameters,
    bpm: f64,
    rows_per_beat: u32,
    timing_changed: bool,
) -> Result<Vec<Value>, String> {
    let (mut topology, modulators) = crate::modulation::edits(
        tracks,
        returns,
        master,
        previous,
        previous_returns,
        previous_master,
        parameters,
        bpm,
        rows_per_beat,
        timing_changed,
    );
    let mut controls = Vec::new();
    let new = strips(tracks, returns, master);
    let old = strips(previous, previous_returns, previous_master);
    // Tracks and returns may be added or removed; the master is always last in both lists.
    let mut every: Vec<String> = new.iter().map(|(l, _)| l.clone()).collect();
    every.extend(old.iter().map(|(l, _)| l.clone()).filter(|l| !new.iter().any(|(n, _)| n == l)));
    for label in every {
        let current = find(&new, &label);
        let before = find(&old, &label);
        let old_chain: Vec<&Device> = before.map(|t| t.active().collect()).unwrap_or_default();
        let chain: Vec<&Device> = current.map(|t| t.active().collect()).unwrap_or_default();
        for device in &old_chain {
            if !chain.iter().any(|d| d.id == device.id) {
                topology.push(json!({"op":"free_node", "id":fx_node(&label, &device.id)}));
            }
        }
        let Some(strip) = current else {
            // A removed track or return takes its nodes, and the links on them, with it.
            topology.extend(["in", "track", "tap"].map(|part| json!({"op":"free_node", "id":format!("{part}_{label}")})));
            continue;
        };
        if before.is_none() {
            topology.extend(strip_nodes(&label));
        }
        let devices_changed = chain.len() != old_chain.len() || chain.iter().zip(&old_chain).any(|(a, b)| a.id != b.id || a.definition != b.definition);
        for device in &chain {
            match old_chain.iter().find(|d| d.id == device.id) {
                None => {
                    let fx = effects.get(&device.definition).ok_or_else(|| format!("Apply the effect assigned to {} first", strip.name))?;
                    topology.push(json!({"op":"node", "id":fx_node(&label, &device.id), "fx":fx, "args":arguments(device, parameters)}));
                }
                Some(old) if old.definition != device.definition => topology.push(replacement(&label, device, effects, parameters)?),
                Some(old) if old.values != device.values => {
                    let mut args = arguments(device, parameters);
                    args.retain(|parameter, _| strip.macro_of(&device.id, parameter).is_none());
                    if !args.is_empty() {
                        controls.push(json!({"op":"set", "id":fx_node(&label, &device.id), "args":args, "glide":0.01}));
                    }
                }
                _ => {}
            }
        }
        // A detached link returns to its device's base setting; other automated parameters stay put.
        for device in &chain {
            let args = arguments(device, parameters);
            let restored: serde_json::Map<String, Value> = args.into_iter().filter(|(parameter, _)| {
                before.is_some_and(|old| old.macro_of(&device.id, parameter).is_some())
                    && strip.macro_of(&device.id, parameter).is_none()
            }).collect();
            if !restored.is_empty() {
                controls.push(json!({"op":"set", "id":fx_node(&label, &device.id), "args":restored, "glide":0.01}));
            }
        }
        let shape = |chain: &[&Device]| chain.iter().map(|d| (d.id.clone(), d.target.is_some(), d.pre, d.mute_source)).collect::<Vec<_>>();
        if before.is_none() || shape(&chain) != shape(&old_chain) {
            // Links are named by their device, so reordering re-points them in place and removing a device takes its link too.
            let mut from = format!("in_{label}");
            let mut cut = false;
            let gate = |cut: bool| if cut { 0.0 } else { 1.0 };
            for device in &chain {
                let to = fx_node(&label, &device.id);
                let link = format!("link_{label}_{}", device.id);
                if device.target.is_some() && !device.pre {
                    topology.push(json!({"op":"connect", "id":link, "from":{"kind":"node", "id":format!("tap_{label}")}, "to":{"kind":"node", "id":to}}));
                    continue;
                }
                topology.push(json!({"op":"connect", "id":link, "from":{"kind":"node", "id":from}, "to":{"kind":"node", "id":to}, "gain":gate(cut)}));
                if device.target.is_none() {
                    from = to;
                } else if device.mute_source {
                    cut = true;
                }
            }
            topology.push(json!({"op":"connect", "id":format!("fadein_{label}"), "from":{"kind":"node", "id":from}, "to":{"kind":"node", "id":format!("track_{label}")}, "gain":gate(cut)}));
        }
        if before.is_none_or(|o| o.volume_db != strip.volume_db || o.pan != strip.pan) {
            controls.push(base(&label, strip));
        }
        // Re-evaluate links after their devices and any former LFO maps have been updated.
        for (index, m) in strip.macros.iter().enumerate() {
            if devices_changed || before.and_then(|o| o.macros.get(index)) != Some(m) {
                controls.extend(macro_value(&label, strip, index, m.value, parameters));
            }
        }
    }
    // links come after the nodes they join; one that is new or points somewhere else is connected, one that
    // is gone is cut unless freeing a node already took it, and one that stays opens or closes
    let (wanted, had) = (links(tracks, returns), links(previous, previous_returns));
    let exists = nodes(tracks, returns, master);
    for (id, link) in &wanted {
        match had.get(id) {
            Some(was) if was.from == link.from && was.to == link.to => {
                if was.gain != link.gain {
                    controls.push(gain(id, link.gain));
                }
            }
            _ => topology.push(json!({"op":"connect", "id":id, "from":{"kind":"node", "id":link.from}, "to":{"kind":"node", "id":link.to}, "gain":link.gain})),
        }
    }
    for (id, link) in &had {
        if !wanted.contains_key(id) && exists.contains(&link.from) && exists.contains(&link.to) {
            topology.push(json!({"op":"disconnect", "id":id}));
        }
    }
    topology.extend(modulators);
    Ok([topology, controls]
        .into_iter()
        .filter(|edits| !edits.is_empty())
        .map(|edits| json!({"op":"routing", "at":"now", "edits":edits}))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{commands::Parameter, pattern::{Lfo, Macro, MacroLink}};

    #[test]
    fn macros_handover_lfos_and_survive_unrelated_parameter_edits() {
        let parameters: EffectParameters = [("level".into(), ["amount", "other"].map(|name| Parameter {
            name: name.into(), min: 0.0, max: 1.0, default: 1.0, unit: None, ..Default::default()
        }).into())].into();
        let effects = [("level".into(), "level".into())].into();
        let mut strip = Track { fx: vec![Device {
            id: "d0".into(), definition: "level".into(),
            modulations: [("amount".into(), Lfo { enabled: true, shape: "sine".into(), period: 4.0, min: 0.0, max: 1.0 })].into(),
            ..Default::default()
        }], ..Default::default() };
        let edits = |next: &Track, old: &Track| {
            events(std::slice::from_ref(next), &[], &Track::default(), std::slice::from_ref(old), &[], &Track::default(), &effects, &parameters, 120.0, 4, false)
                .unwrap().into_iter().flat_map(|event| event["edits"].as_array().unwrap().clone()).collect::<Vec<_>>()
        };
        let before = strip.clone();
        strip.macros = vec![Macro { name: "Gain".into(), value: 0.5, links: vec![MacroLink { device: "d0".into(), parameter: "amount".into(), min: 0.0, max: 1.0 }] }];
        let attached = edits(&strip, &before);
        assert!(attached.iter().any(|edit| edit["op"] == "map" && edit["source"].is_null()));
        assert!(attached.iter().any(|edit| edit["op"] == "free_node"));
        assert_eq!(attached.last().unwrap()["args"]["amount"], 0.5);
        let before = strip.clone();
        strip.fx[0].values.insert("other".into(), 0.25);
        let unrelated = edits(&strip, &before);
        assert!(unrelated.iter().all(|edit| edit["args"].get("amount").is_none()), "an unrelated edit must not reset macro automation: {unrelated:?}");
        let before = strip.clone();
        strip.macros.clear();
        let detached = edits(&strip, &before);
        assert!(detached.iter().any(|edit| edit["op"] == "node" && edit["fx"] == "__tracker_lfo_sine"));
        assert!(detached.iter().any(|edit| edit["op"] == "map" && edit["source"]["kind"] == "bus"));
    }
}
