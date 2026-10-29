//! Configure niminal-native oscillator nodes, modulation buses and parameter maps.
//! No DSP or modulation signal generation runs in the tracker.
use crate::{
    commands::{EffectParameters, Parameter, fx_node},
    pattern::{Lfo, Track},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn declarations() -> Vec<Value> {
    let lfos = [("sine", "sine"), ("triangle", "tri"), ("saw", "saw"), ("square", "square")].into_iter().map(|(name, wave)|
        json!({"op":"fx", "name":format!("__tracker_lfo_{name}"), "output_channels":1, "source":format!("(rate: hz = 1hz) {{ out = ((osc({wave}, rate) + 1) * 0.5).to_layout(mono) }}")}));
    lfos.collect()
}

struct Target<'a> {
    /// The strip it belongs to: a track number, `r<n>` for a return, or the master's label.
    track: &'a str,
    device: &'a str,
    parameter: &'a Parameter,
    lfo: Lfo,
    definition: &'a str,
}
impl Target<'_> {
    /// Names the target; an LFO's node and bus carry the same name.
    fn key(&self) -> String {
        format!("mod_{}_{}_{}", self.track, self.device, self.parameter.name)
    }
    fn node(&self, bpm: f64, rows_per_beat: u32) -> Value {
        let hz = bpm * f64::from(rows_per_beat) / (60.0 * self.lfo.period);
        json!({"op":"node", "id":self.key(), "fx":format!("__tracker_lfo_{}", self.lfo.shape), "args":{"rate":format!("{hz}hz")}})
    }
    fn map(&self, enabled: bool) -> Value {
        let (low, high) = (self.lfo.min, self.lfo.max);
        let range = self.parameter.max - self.parameter.min;
        let min = self.parameter.engine_value(self.parameter.min + low * range);
        let max = self.parameter.engine_value(self.parameter.min + high * range);
        json!({"op":"map", "id":fx_node(self.track, self.device), "param":self.parameter.name,
            "source":if enabled {json!({"kind":"bus", "id":self.key()})} else {Value::Null}, "scale":max-min, "offset":min})
    }
    /// The edits that make the target's LFO.
    fn create(&self, bpm: f64, rows_per_beat: u32) -> Vec<Value> {
        let id = self.key();
        vec![json!({"op":"bus", "id":id, "channels":1}), self.node(bpm, rows_per_beat),
            json!({"op":"connect", "id":format!("{id}_send"), "from":{"kind":"node", "id":id}, "to":{"kind":"bus", "id":id}})]
    }
    fn destroy(&self) -> Vec<Value> {
        let id = self.key();
        vec![json!({"op":"free_node", "id":id}), json!({"op":"free_bus", "id":id})]
    }
}

/// The parameters something drives, by target key. A macro's link wins over the parameter's own LFO.
fn targets<'a>(
    labels: &'a [(String, &'a Track)],
    parameters: &'a EffectParameters,
) -> BTreeMap<String, Target<'a>> {
    let mut targets = BTreeMap::new();
    for (track, t) in labels {
        for device in t.active() {
            let Some(params) = parameters.get(&device.definition) else { continue };
            for parameter in params {
                if t.macro_of(&device.id, &parameter.name).is_some() { continue; }
                let Some(lfo) = device.modulations.get(&parameter.name).filter(|lfo| lfo.enabled) else { continue };
                let target = Target { track, device: &device.id, parameter, lfo: lfo.clone(), definition: &device.definition };
                targets.insert(target.key(), target);
            }
        }
    }
    targets
}

#[allow(clippy::too_many_arguments)]
pub fn edits(
    tracks: &[Track],
    returns: &[Track],
    master: &Track,
    previous: &[Track],
    previous_returns: &[Track],
    previous_master: &Track,
    parameters: &EffectParameters,
    bpm: f64,
    rows_per_beat: u32,
    timing_changed: bool,
) -> (Vec<Value>, Vec<Value>) {
    let before = crate::mixer::strips(previous, previous_returns, previous_master);
    let after = crate::mixer::strips(tracks, returns, master);
    let old = targets(&before, parameters);
    let next = targets(&after, parameters);
    let mut remove = Vec::new();
    let mut add = Vec::new();
    for (id, target) in &old {
        if next.get(id).is_none_or(|next| next.definition != target.definition) {
            remove.push(target.map(false));
            remove.extend(target.destroy());
        }
    }
    for (id, target) in &next {
        let existing = old.get(id).filter(|old| old.definition == target.definition);
        match existing {
            None => {
                add.extend(target.create(bpm, rows_per_beat));
                add.push(target.map(true));
            }
            Some(old) if old.lfo != target.lfo => {
                add.push(target.node(bpm, rows_per_beat));
                add.push(target.map(true));
            }
            Some(_) if timing_changed => {
                add.push(target.node(bpm, rows_per_beat));
                add.push(target.map(true));
            }
            Some(_) => {}
        }
    }
    (remove, add)
}

/// A native, sample-timed reset. Oscillator phase and signal generation stay in niminal.
pub fn reset(
    tracks: &[Track],
    returns: &[Track],
    master: &Track,
    parameters: &EffectParameters,
    at: u64,
) -> Vec<Value> {
    let strips = crate::mixer::strips(tracks, returns, master);
    let edits: Vec<_> = targets(&strips, parameters)
        .values()
        .flat_map(|target| {
            [json!({"op":"reset", "id":target.key()}), target.map(true)]
        })
        .collect();
    if edits.is_empty() {
        vec![]
    } else {
        vec![json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits})]
    }
}

/// Keep native oscillator periods aligned with scheduled rows without resetting their phase or maps.
pub fn timing(song: &crate::pattern::Song, parameters: &EffectParameters, row_seconds: f64, at: u64) -> Vec<Value> {
    let strips = crate::mixer::strips(&song.tracks, &song.returns, &song.master);
    let edits: Vec<_> = targets(&strips, parameters).values().map(|target| {
        let hz = 1.0 / (row_seconds * target.lfo.period);
        json!({"op":"set", "id":target.key(), "args":{"rate":format!("{hz}hz")}})
    }).collect();
    if edits.is_empty() { vec![] } else { vec![json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits})] }
}
