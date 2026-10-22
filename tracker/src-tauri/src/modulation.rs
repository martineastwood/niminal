//! Configure niminal-native oscillator nodes, modulation buses and parameter maps.
//! No DSP or modulation signal generation runs in the tracker.
use crate::{
    commands::{EffectParameters, Parameter, fx_node},
    pattern::{Lfo, Track},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn declarations() -> Vec<Value> {
    [("sine", "sine"), ("triangle", "tri"), ("saw", "saw"), ("square", "square")].into_iter().map(|(name, wave)|
        json!({"op":"fx", "name":format!("__tracker_lfo_{name}"), "output_channels":1, "source":format!("(rate: hz = 1hz) {{ out = ((osc({wave}, rate) + 1) * 0.5).to_layout(mono) }}")})
    ).collect()
}

struct Target<'a> {
    /// The strip it belongs to: a track number, or the master's label.
    track: &'a str,
    device: &'a str,
    parameter: &'a Parameter,
    lfo: &'a Lfo,
    definition: &'a str,
}
impl Target<'_> {
    fn source(&self) -> String {
        format!("mod_{}_{}_{}", self.track, self.device, self.parameter.name)
    }
    fn node(&self, bpm: f64, rows_per_beat: u32) -> Value {
        let hz = bpm * f64::from(rows_per_beat) / (60.0 * self.lfo.period);
        json!({"op":"node", "id":self.source(), "fx":format!("__tracker_lfo_{}", self.lfo.shape), "args":{"rate":format!("{hz}hz")}})
    }
    fn map(&self, enabled: bool) -> Value {
        let min = self.parameter.engine_value(
            self.parameter.min + self.lfo.min * (self.parameter.max - self.parameter.min),
        );
        let max = self.parameter.engine_value(
            self.parameter.min + self.lfo.max * (self.parameter.max - self.parameter.min),
        );
        json!({"op":"map", "id":fx_node(self.track, self.device), "param":self.parameter.name,
            "source":if enabled {json!({"kind":"bus", "id":self.source()})} else {Value::Null}, "scale":max-min, "offset":min})
    }
}

fn targets<'a>(
    labels: &'a [(String, &'a Track)],
    parameters: &'a EffectParameters,
) -> BTreeMap<String, Target<'a>> {
    let mut targets = BTreeMap::new();
    for (track, t) in labels {
        for device in t.active() {
            for (name, lfo) in &device.modulations {
                if !lfo.enabled {
                    continue;
                }
                if let Some(parameter) = parameters
                    .get(&device.definition)
                    .and_then(|params| params.iter().find(|p| p.name == *name))
                {
                    let target = Target {
                        track,
                        device: &device.id,
                        parameter,
                        lfo,
                        definition: &device.definition,
                    };
                    targets.insert(target.source(), target);
                }
            }
        }
    }
    targets
}

#[allow(clippy::too_many_arguments)]
pub fn edits(
    tracks: &[Track],
    master: &Track,
    previous: &[Track],
    previous_master: &Track,
    parameters: &EffectParameters,
    bpm: f64,
    rows_per_beat: u32,
    timing_changed: bool,
) -> (Vec<Value>, Vec<Value>) {
    let before = crate::mixer::strips(previous, previous_master);
    let after = crate::mixer::strips(tracks, master);
    let old = targets(&before, parameters);
    let next = targets(&after, parameters);
    let mut remove = Vec::new();
    let mut add = Vec::new();
    for (id, target) in &old {
        if next
            .get(id)
            .is_none_or(|next| next.definition != target.definition)
        {
            remove.extend([
                target.map(false),
                json!({"op":"free_node", "id":id}),
                json!({"op":"free_bus", "id":id}),
            ]);
        }
    }
    for (id, target) in &next {
        let existing = old
            .get(id)
            .filter(|old| old.definition == target.definition);
        if existing.is_none() {
            add.extend([json!({"op":"bus", "id":id, "channels":1}), target.node(bpm, rows_per_beat),
                json!({"op":"connect", "id":format!("{id}_send"), "from":{"kind":"node", "id":id}, "to":{"kind":"bus", "id":id}}), target.map(true)]);
        } else if timing_changed || existing.is_some_and(|old| old.lfo != target.lfo) {
            add.extend([target.node(bpm, rows_per_beat), target.map(true)]);
        }
    }
    (remove, add)
}

/// A native, sample-timed reset. Oscillator phase and signal generation stay in niminal.
pub fn reset(
    tracks: &[Track],
    master: &Track,
    parameters: &EffectParameters,
    at: u64,
) -> Vec<Value> {
    let strips = crate::mixer::strips(tracks, master);
    let edits: Vec<_> = targets(&strips, parameters)
        .values()
        .flat_map(|target| {
            [
                json!({"op":"reset", "id":target.source()}),
                target.map(true),
            ]
        })
        .collect();
    if edits.is_empty() {
        vec![]
    } else {
        vec![json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits})]
    }
}
