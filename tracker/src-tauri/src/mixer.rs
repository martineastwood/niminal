//! The engine graph behind the mixer. Every track, return and the master is a *strip*:
//!
//! `in_<s>` → its effect devices → `track_<s>` (level and pan) → `tap_<s>` → `out_<s>` (mute) → next stage
//!
//! Notes and other strips arrive at `in_<s>`. Tracks and returns feed `in_m`, the master's strip, which
//! feeds the engine's master output. The tap is a pass-through so each strip's finished signal has one
//! steady place for a scope to listen. A return is a strip labelled `r<n>`. A track's or return's tap
//! feeds the master through `out_<s>`, or a return through `outr_<s>_<n>`, whichever its output names;
//! a return can only feed later returns. A send device is a node that feeds a return through
//! `sendout_<s>_<device>`. Post-fader (the default) it listens to the tap; pre-fader it listens at its
//! place in the chain, and may *mute the source*, so nothing after it carries sound on.
use crate::{
    commands::{EffectParameters, fx_node},
    pattern::{Device, MAX_RETURNS, Output, Track},
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
    for label in labels() {
        edits.push(json!({"op":"node", "id":format!("in_{label}"), "fx":"__tracker_tap"}));
        edits.push(json!({"op":"node", "id":format!("track_{label}"), "fx":"__tracker_mix"}));
        edits.push(json!({"op":"node", "id":format!("tap_{label}"), "fx":"__tracker_tap"}));
        edits.push(json!({"op":"connect", "id":format!("fadein_{label}"), "from":{"kind":"node", "id":format!("in_{label}")}, "to":{"kind":"node", "id":format!("track_{label}")}}));
        edits.push(json!({"op":"connect", "id":format!("tapin_{label}"), "from":{"kind":"node", "id":format!("track_{label}")}, "to":{"kind":"node", "id":format!("tap_{label}")}}));
        let to = if label == MASTER {
            json!({"kind":"master"})
        } else {
            json!({"kind":"node", "id":format!("in_{MASTER}")})
        };
        edits.push(json!({"op":"connect", "id":format!("out_{label}"), "from":{"kind":"node", "id":format!("tap_{label}")}, "to":to}));
    }
    let sources = (0..16).map(|t| (t.to_string(), 0)).chain((0..MAX_RETURNS).map(|r| (format!("r{r}"), r + 1)));
    for (label, first) in sources {
        for ret in first..MAX_RETURNS {
            edits.push(json!({"op":"connect", "id":format!("outr_{label}_{ret}"), "from":{"kind":"node", "id":format!("tap_{label}")}, "to":{"kind":"node", "id":format!("in_r{ret}")}, "gain":0.0}));
        }
    }
    edits
}

/// Every strip the engine graph holds, in order: 16 tracks, the returns, then the master.
pub fn labels() -> impl Iterator<Item = String> {
    (0..16)
        .map(|t| t.to_string())
        .chain((0..MAX_RETURNS).map(|r| format!("r{r}")))
        .chain([MASTER.to_string()])
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
fn silent(tracks: &[Track], index: usize) -> bool {
    tracks[index].muted || tracks.iter().any(|t| t.solo) && !tracks[index].solo
}

/// The gain of every connection that carries a strip's signal on, by connection id: to the master or a
/// return, and out of each send device. A return can only feed later returns, which rules out loops.
/// Nothing flows while the strip is silent or its return is missing. The master has none.
fn gains(label: &str, tracks: &[Track], returns: &[Track]) -> Option<BTreeMap<String, f64>> {
    let (strip, open, first) = if let Some(ret) = label.strip_prefix('r') {
        let index: usize = ret.parse().ok()?;
        let strip = returns.get(index)?;
        (strip, !strip.muted, index + 1)
    } else {
        let index: usize = label.parse().ok()?;
        (tracks.get(index)?, !silent(tracks, index), 0)
    };
    let gate = |on: bool| if open && on { 1.0 } else { 0.0 };
    let mut gains = BTreeMap::from([(format!("out_{label}"), gate(strip.output == Output::Master))]);
    for ret in first..MAX_RETURNS {
        gains.insert(format!("outr_{label}_{ret}"), gate(ret < returns.len() && strip.output == Output::Return { index: ret }));
    }
    for device in strip.active() {
        if let Some(target) = device.target {
            gains.insert(format!("sendout_{label}_{}", device.id), gate(target < returns.len()));
        }
    }
    Some(gains)
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
        edits.extend(gains(&label, tracks, returns).into_iter().flatten().map(|(id, value)| gain(&id, value)));
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

/// The connection from a send device to its return, if it is one.
fn send_connection(tracks: &[Track], returns: &[Track], label: &str, device: &Device) -> Option<Value> {
    let target = device.target?;
    let id = format!("sendout_{label}_{}", device.id);
    let open = gains(label, tracks, returns).and_then(|gains| gains.get(&id).copied()).unwrap_or(0.0);
    Some(json!({"op":"connect", "id":id, "from":{"kind":"node", "id":fx_node(label, &device.id)}, "to":{"kind":"node", "id":format!("in_r{target}")}, "gain":open}))
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
    for label in labels() {
        let current = find(&new, &label);
        let before = find(&old, &label);
        let is_master = label == MASTER;
        let old_chain: Vec<&Device> = before.map(|t| t.active().collect()).unwrap_or_default();
        let chain: Vec<&Device> = current.map(|t| t.active().collect()).unwrap_or_default();
        for device in &old_chain {
            if !chain.iter().any(|d| d.id == device.id) {
                topology.push(json!({"op":"free_node", "id":fx_node(&label, &device.id)}));
            }
        }
        let Some(strip) = current else {
            // A removed track or return goes quiet; its nodes stay for when it comes back.
            controls.extend(gains(&label, previous, previous_returns).into_iter().flatten().map(|(id, _)| gain(&id, 0.0)));
            continue;
        };
        for device in &chain {
            match old_chain.iter().find(|d| d.id == device.id) {
                None => {
                    let fx = effects.get(&device.definition).ok_or_else(|| format!("Apply the effect assigned to {} first", strip.name))?;
                    topology.push(json!({"op":"node", "id":fx_node(&label, &device.id), "fx":fx, "args":arguments(device, parameters)}));
                    topology.extend(send_connection(tracks, returns, &label, device));
                }
                Some(old) if old.definition != device.definition => topology.push(replacement(&label, device, effects, parameters)?),
                Some(old) if old.values != device.values => controls.push(json!({"op":"set", "id":fx_node(&label, &device.id), "args":arguments(device, parameters), "glide":0.01})),
                _ => {}
            }
            if old_chain.iter().any(|d| d.id == device.id && d.target != device.target) {
                topology.extend(send_connection(tracks, returns, &label, device));
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
        if is_master {
            continue;
        }
        let was = gains(&label, previous, previous_returns).unwrap_or_default();
        for (id, value) in gains(&label, tracks, returns).into_iter().flatten() {
            if was.get(&id) != Some(&value) {
                controls.push(gain(&id, value));
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
