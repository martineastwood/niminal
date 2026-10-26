//! Configure niminal-native oscillator nodes, modulation buses and parameter maps.
//! No DSP or modulation signal generation runs in the tracker.
use crate::{
    commands::{EffectParameters, Parameter, fx_node, macro_node},
    pattern::{Lfo, Track},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn declarations() -> Vec<Value> {
    let lfos = [("sine", "sine"), ("triangle", "tri"), ("saw", "saw"), ("square", "square")].into_iter().map(|(name, wave)|
        json!({"op":"fx", "name":format!("__tracker_lfo_{name}"), "output_channels":1, "source":format!("(rate: hz = 1hz) {{ out = ((osc({wave}, rate) + 1) * 0.5).to_layout(mono) }}")}));
    let knob = json!({"op":"fx", "name":"__tracker_macro", "output_channels":1, "source":"(value: num = 0) { out = value.to_layout(mono) }"});
    lfos.chain([knob]).collect()
}

/// What moves a parameter: an LFO of its own, or one of the strip's macro knobs.
#[derive(Clone, PartialEq)]
enum Drive {
    Lfo(Lfo),
    Macro { index: usize, min: f64, max: f64 },
}

struct Target<'a> {
    /// The strip it belongs to: a track number, `r<n>` for a return, or the master's label.
    track: &'a str,
    device: &'a str,
    parameter: &'a Parameter,
    drive: Drive,
    definition: &'a str,
}
impl Target<'_> {
    /// Names the target; an LFO's node and bus carry the same name.
    fn key(&self) -> String {
        format!("mod_{}_{}_{}", self.track, self.device, self.parameter.name)
    }
    /// The bus the parameter reads.
    fn source(&self) -> String {
        match self.drive {
            Drive::Lfo(_) => self.key(),
            Drive::Macro { index, .. } => macro_node(self.track, index),
        }
    }
    fn lfo(&self) -> Option<&Lfo> {
        match &self.drive {
            Drive::Lfo(lfo) => Some(lfo),
            Drive::Macro { .. } => None,
        }
    }
    fn node(&self, lfo: &Lfo, bpm: f64, rows_per_beat: u32) -> Value {
        let hz = bpm * f64::from(rows_per_beat) / (60.0 * lfo.period);
        json!({"op":"node", "id":self.source(), "fx":format!("__tracker_lfo_{}", lfo.shape), "args":{"rate":format!("{hz}hz")}})
    }
    fn map(&self, enabled: bool) -> Value {
        let (low, high) = match &self.drive {
            Drive::Lfo(lfo) => (lfo.min, lfo.max),
            Drive::Macro { min, max, .. } => (*min, *max),
        };
        let range = self.parameter.max - self.parameter.min;
        let min = self.parameter.engine_value(self.parameter.min + low * range);
        let max = self.parameter.engine_value(self.parameter.min + high * range);
        json!({"op":"map", "id":fx_node(self.track, self.device), "param":self.parameter.name,
            "source":if enabled {json!({"kind":"bus", "id":self.source()})} else {Value::Null}, "scale":max-min, "offset":min})
    }
    /// The edits that make the target's own LFO, which a macro's link doesn't need.
    fn create(&self, bpm: f64, rows_per_beat: u32) -> Vec<Value> {
        let Some(lfo) = self.lfo() else { return vec![] };
        let id = self.source();
        vec![json!({"op":"bus", "id":id, "channels":1}), self.node(lfo, bpm, rows_per_beat),
            json!({"op":"connect", "id":format!("{id}_send"), "from":{"kind":"node", "id":id}, "to":{"kind":"bus", "id":id}})]
    }
    fn destroy(&self) -> Vec<Value> {
        if self.lfo().is_none() {
            return vec![];
        }
        let id = self.source();
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
                let drive = match (t.macro_of(&device.id, &parameter.name), device.modulations.get(&parameter.name)) {
                    (Some((index, link)), _) => Drive::Macro { index, min: link.min, max: link.max },
                    (None, Some(lfo)) if lfo.enabled => Drive::Lfo(lfo.clone()),
                    _ => continue,
                };
                let target = Target { track, device: &device.id, parameter, drive, definition: &device.definition };
                targets.insert(target.key(), target);
            }
        }
    }
    targets
}

/// Every macro knob of the strips: its node and bus name, and its value.
fn macros(labels: &[(String, &Track)]) -> BTreeMap<String, f64> {
    labels.iter().flat_map(|(label, t)| t.macros.iter().enumerate().map(move |(index, m)| (macro_node(label, index), m.value))).collect()
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
    let (old_macros, next_macros) = (macros(&before), macros(&after));
    let mut remove = Vec::new();
    let mut add = Vec::new();
    for (id, target) in &old {
        if next.get(id).is_none_or(|next| next.definition != target.definition || next.lfo().is_some() != target.lfo().is_some()) {
            remove.push(target.map(false));
            remove.extend(target.destroy());
        }
    }
    // a macro's node can go once no map reads its bus
    for id in old_macros.keys().filter(|id| !next_macros.contains_key(*id)) {
        remove.extend([json!({"op":"free_node", "id":id}), json!({"op":"free_bus", "id":id})]);
    }
    for (id, value) in next_macros.iter().filter(|(id, _)| !old_macros.contains_key(*id)) {
        add.extend([json!({"op":"bus", "id":id, "channels":1}), json!({"op":"node", "id":id, "fx":"__tracker_macro", "args":{"value":value}}),
            json!({"op":"connect", "id":format!("{id}_send"), "from":{"kind":"node", "id":id}, "to":{"kind":"bus", "id":id}})]);
    }
    for (id, target) in &next {
        let existing = old.get(id).filter(|old| old.definition == target.definition && old.lfo().is_some() == target.lfo().is_some());
        match existing {
            None => {
                add.extend(target.create(bpm, rows_per_beat));
                add.push(target.map(true));
            }
            Some(old) if old.drive != target.drive => {
                add.extend(target.lfo().map(|lfo| target.node(lfo, bpm, rows_per_beat)));
                add.push(target.map(true));
            }
            Some(_) if timing_changed => {
                add.extend(target.lfo().map(|lfo| target.node(lfo, bpm, rows_per_beat)));
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
            let reset = target.lfo().map(|_| json!({"op":"reset", "id":target.source()}));
            reset.into_iter().chain([target.map(true)])
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
    let edits: Vec<_> = targets(&strips, parameters).values().filter_map(|target| {
        let hz = 1.0 / (row_seconds * target.lfo()?.period);
        Some(json!({"op":"set", "id":target.source(), "args":{"rate":format!("{hz}hz")}}))
    }).collect();
    if edits.is_empty() { vec![] } else { vec![json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits})] }
}
