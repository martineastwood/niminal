use crate::pattern::{Device, Track};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub type EffectParameters = BTreeMap<String, Vec<Parameter>>;

#[derive(Clone, Debug, Serialize)]
pub struct Parameter {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: Option<String>,
}

impl Parameter {
    pub fn argument(&self, value: f64) -> Value {
        self.unit
            .as_ref()
            .map_or_else(|| json!(value), |unit| json!(format!("{value}{unit}")))
    }
    pub fn engine_value(&self, value: f64) -> f64 {
        if self.unit.as_deref() == Some("db") {
            10.0_f64.powf(value / 20.0)
        } else {
            value
        }
    }
}

/// The engine node that runs one device of a track's chain.
pub fn fx_node(track: usize, device: &str) -> String {
    format!("fx_{track}_{device}")
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FxTarget {
    Volume,
    Pan,
    Effect { device: String, parameter: String },
}

impl FxTarget {
    pub fn device<'a>(&self, track: &'a Track) -> Option<&'a Device> {
        match self {
            Self::Effect { device, .. } => track.device(device),
            _ => None,
        }
    }

    pub fn parameter<'a>(
        &self,
        track: &Track,
        parameters: &'a EffectParameters,
    ) -> Option<&'a Parameter> {
        let Self::Effect { parameter, .. } = self else {
            return None;
        };
        parameters
            .get(&self.device(track)?.definition)?
            .iter()
            .find(|p| p.name == *parameter)
    }

    /// Whether the lane can drive anything right now: a bypassed device has no engine node.
    pub fn available(&self, track: &Track, parameters: &EffectParameters) -> bool {
        match self {
            Self::Effect { .. } => {
                self.device(track).is_some_and(|device| !device.bypass)
                    && self.parameter(track, parameters).is_some()
            }
            _ => true,
        }
    }

    /// Detaches the parameter's LFO so a written value takes effect.
    pub fn unmap(&self, index: usize, track: &Track) -> Option<Value> {
        let Self::Effect { device, parameter } = self else {
            return None;
        };
        self.device(track)?
            .modulations
            .get(parameter)
            .filter(|lfo| lfo.enabled)
            .map(|_| json!({"op":"map", "id":fx_node(index, device), "param":parameter, "source":null}))
    }
}

pub fn edit(
    target: &FxTarget,
    value: &str,
    index: usize,
    track: &Track,
    parameters: &EffectParameters,
) -> Result<Value, String> {
    let value: f64 = value
        .parse()
        .map_err(|_| "Parameter lane needs a numeric value")?;
    if !value.is_finite() {
        return Err("Parameter lane needs a finite value".into());
    }
    let bounded = |min, max| {
        if (min..=max).contains(&value) {
            Ok(())
        } else {
            Err(format!(
                "{}: parameter value {value} is outside {min}..{max}",
                track.name
            ))
        }
    };
    match target {
        FxTarget::Volume => {
            bounded(-60.0, 6.0)?;
            Ok(
                json!({"op":"set", "id":format!("track_{index}"), "args":{"level":10.0_f64.powf(value/20.0)}, "glide":0.0}),
            )
        }
        FxTarget::Pan => {
            bounded(-1.0, 1.0)?;
            Ok(
                json!({"op":"set", "id":format!("track_{index}"), "args":{"balance":value}, "glide":0.0}),
            )
        }
        FxTarget::Effect { device, parameter } => {
            target
                .device(track)
                .ok_or_else(|| format!("{}: lane addresses a missing effect", track.name))?;
            let info = target.parameter(track, parameters).ok_or_else(|| {
                format!(
                    "{}: lane addresses missing parameter {parameter}",
                    track.name
                )
            })?;
            bounded(info.min, info.max)?;
            Ok(
                json!({"op":"set", "id":fx_node(index, device), "args":{parameter:info.argument(value)}, "glide":0.0}),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_controls_use_exact_physical_values_and_validate_bounds() {
        let mut track = Track::default();
        assert_eq!(
            edit(&FxTarget::Volume, "0", 2, &track, &BTreeMap::new()).unwrap()["args"]["level"],
            1.0
        );
        assert_eq!(
            edit(&FxTarget::Pan, "0", 2, &track, &BTreeMap::new()).unwrap()["args"]["balance"],
            0.0
        );
        assert!(edit(&FxTarget::Pan, "2", 0, &track, &BTreeMap::new()).is_err());
        assert!(edit(&FxTarget::Volume, "NaN", 0, &track, &BTreeMap::new()).is_err());
        track.fx = vec![crate::pattern::Device {
            id: "d0".into(),
            definition: "filter".into(),
            ..Default::default()
        }];
        let parameters = [(
            "filter".into(),
            vec![Parameter {
                name: "cutoff".into(),
                min: 20.0,
                max: 20000.0,
                default: 2400.0,
                unit: Some("hz".into()),
            }],
        )]
        .into();
        let target = FxTarget::Effect {
            device: "d0".into(),
            parameter: "cutoff".into(),
        };
        assert_eq!(
            edit(&target, "2345.6789", 0, &track, &parameters).unwrap()["args"]["cutoff"],
            "2345.6789hz"
        );
        assert!(edit(&target, "20001", 0, &track, &parameters).is_err());
    }
}
