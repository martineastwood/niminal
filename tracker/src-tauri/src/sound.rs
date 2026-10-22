use crate::commands::Parameter;
use niminal_lang::{SymbolKind, symbols};
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Instr,
    Fx,
}

#[derive(Clone)]
pub struct Sound {
    pub engine_source: String,
    pub target: String,
    pub fx_source: Option<String>,
    pub parameters: Vec<Parameter>,
}

impl Sound {
    pub fn parse(kind: Kind, source: String) -> Result<Self, String> {
        let definitions = symbols(&source).map_err(|e| e.message)?;
        let wanted = if kind == Kind::Instr {
            SymbolKind::Instrument
        } else {
            SymbolKind::Fx
        };
        let sounds: Vec<_> = definitions
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    SymbolKind::Instrument | SymbolKind::Fx | SymbolKind::Node
                )
            })
            .collect();
        if sounds.len() != 1 || sounds[0].kind != wanted {
            return Err("A slot must contain exactly one instrument or effect of its selected kind, plus optional helpers".into());
        }
        let definition = sounds[0];
        if definition.name.starts_with("__tracker_") {
            return Err("This name is reserved for the track bypass".into());
        }
        if kind == Kind::Instr && !definition.params.iter().any(|param| param.name == "freq") {
            return Err("Tracker instruments currently need a freq: hz parameter".into());
        }
        let mut parameters = Vec::new();
        if kind == Kind::Fx {
            let program = niminal_lang::compile(&source).map_err(|errors| {
                errors
                    .iter()
                    .map(|e| e.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
            for param in &definition.params {
                if parameters.len() == 15 {
                    break;
                }
                let key = format!("{}::{}", definition.name, param.name);
                let Some(control) = program.controls.iter().find(|control| control.name == key)
                else {
                    continue;
                };
                let (min, max) = control.range.unwrap_or(match control.unit {
                    niminal_lang::Unit::Hz => (20.0, 20000.0),
                    niminal_lang::Unit::Db => (-60.0, 6.0),
                    niminal_lang::Unit::Time => (0.0, 10.0),
                    niminal_lang::Unit::Angle => (-180.0, 180.0),
                    niminal_lang::Unit::Semitones => (-48.0, 48.0),
                    _ => (0.0, 1.0),
                });
                let default = if control.unit == niminal_lang::Unit::Db {
                    20.0 * f64::from(control.initial).max(0.000001).log10()
                } else {
                    f64::from(control.initial)
                };
                let (min, max) = if control.range.is_some() {
                    (min, max)
                } else {
                    (min.min(default), max.max(default))
                };
                parameters.push(Parameter {
                    name: param.name.clone(),
                    min,
                    max,
                    default,
                    unit: control.unit.suffix().map(str::to_string),
                });
            }
        }
        let mut engine_source = source.clone();
        let fx_source = if kind == Kind::Fx {
            // Reusable routing fx take a signature and body, without `fx name`.
            // Leave helpers in the definition transaction, and avoid creating an
            // implicit global effect instance from the editor's named definition.
            let body = source[definition.name_span.end..definition.span.end].trim();
            let body = if body.starts_with('(') {
                body.to_string()
            } else {
                format!("(){body}")
            };
            engine_source.replace_range(definition.span.start..definition.span.end, "");
            Some(body)
        } else {
            None
        };
        Ok(Self {
            target: definition.name.clone(),
            engine_source,
            fx_source,
            parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_reusable_fx_from_its_helper_definitions() {
        let sound = Sound::parse(
            Kind::Fx,
            "fn soften(x) { x * 0.5 }\nfx low { out = soften(it) }\n".into(),
        )
        .unwrap();
        assert_eq!(sound.target, "low");
        assert_eq!(sound.fx_source.as_deref(), Some("(){ out = soften(it) }"));
        assert!(sound.engine_source.contains("fn soften"));
        assert!(!sound.engine_source.contains("fx low"));
    }
}

#[cfg(test)]
mod parameter_tests {
    use super::*;
    #[test]
    fn exposes_typed_fx_parameters_in_signature_order() {
        let sound = Sound::parse(Kind::Fx, "fx filter(cutoff: hz = 2400hz, resonance: 0..1 = 0.2) { out = it.lpf(cutoff: cutoff, res: resonance) }".into()).unwrap();
        assert_eq!(
            sound
                .parameters
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["cutoff", "resonance"]
        );
        assert_eq!(sound.parameters[0].unit.as_deref(), Some("hz"));
        assert_eq!(sound.parameters[0].default, 2400.0);
        assert_eq!(
            (sound.parameters[1].min, sound.parameters[1].max),
            (0.0, 1.0)
        );
    }
}
