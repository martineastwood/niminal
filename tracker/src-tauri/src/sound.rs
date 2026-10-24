use crate::commands::Parameter;
use niminal_lang::{SymbolKind, symbols};
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Instr,
    Fx,
}

/// A control as the tracker shows it: bounds, default and unit as a person would read them.
fn parameter(
    name: &str,
    unit: niminal_lang::Unit,
    range: Option<(f64, f64)>,
    initial: f64,
) -> Parameter {
    let to_db = |gain: f64| 20.0 * gain.max(0.000001).log10();
    let default = if unit == niminal_lang::Unit::Db { to_db(initial) } else { initial };
    // a declared range is in the engine's units, so a gain in decibels is a plain factor there
    let range = range.map(|(low, high)| if unit == niminal_lang::Unit::Db { (to_db(low), to_db(high)) } else { (low, high) });
    let (min, max) = range.unwrap_or(match unit {
        niminal_lang::Unit::Hz => (20.0, 20000.0),
        niminal_lang::Unit::Db => (-60.0, 6.0),
        niminal_lang::Unit::Time => (0.0, 10.0),
        niminal_lang::Unit::Angle => (-180.0, 180.0),
        niminal_lang::Unit::Semitones => (-48.0, 48.0),
        _ => (0.0, 1.0),
    });
    // Without declared bounds, make room for a default outside the usual range.
    let (min, max) = if range.is_some() {
        (min, max)
    } else {
        (min.min(default), max.max(default))
    };
    Parameter {
        name: name.to_string(),
        min,
        max,
        default,
        unit: unit.suffix().map(str::to_string),
    }
}

#[derive(Clone)]
pub struct Sound {
    pub engine_source: String,
    pub target: String,
    pub fx_source: Option<String>,
    pub parameters: Vec<Parameter>,
    /// For a sliced sample, how many pieces it has: the instrument has a `sample` parameter and the
    /// source slices one sample into a kit. Notes then pick the piece, from C-4 up.
    pub slices: Option<usize>,
}

impl Sound {
    /// `dir` is the project folder, where the files a sampler names are found.
    pub fn parse(kind: Kind, source: String, dir: Option<&str>) -> Result<Self, String> {
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
        let mut kits = definitions.iter().filter(|s| s.kind == SymbolKind::Kit && !s.params.is_empty());
        let slices = match (kind, kits.next(), kits.next()) {
            (Kind::Instr, Some(kit), None) if definition.params.iter().any(|p| p.name == "sample") => Some(kit.params.len()),
            _ => None,
        };
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
                parameters.push(parameter(
                    &param.name,
                    control.unit,
                    control.range,
                    f64::from(control.initial),
                ));
            }
        }
        let options = niminal_lang::CompileOptions {
            samples: dir.map_or_else(niminal_lang::Samples::default, niminal_lang::Samples::new),
            ..Default::default()
        };
        if kind == Kind::Instr
            // The compiler can't always load an instrument on its own (a sampler needs its
            // folder), and the engine reports real errors on Apply, so no controls will do.
            && let Ok(program) = niminal_lang::compile_with(&source, &options)
            && let Some(instrument) = program.instruments.iter().find(|i| i.name == definition.name)
        {
            for param in &instrument.params {
                // Pitch, level and position come from the note itself, and so does the piece of a sliced sample.
                if matches!(param.name.as_str(), "freq" | "gain" | "pan")
                    || slices.is_some() && param.name == "sample"
                    || parameters.len() == 15
                {
                    continue;
                }
                parameters.push(parameter(
                    &param.name,
                    param.unit,
                    param.range,
                    f64::from(param.default.unwrap_or(0.0)),
                ));
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
            slices,
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
            "fn soften(x) { x * 0.5 }\nfx low { out = soften(in) }\n".into(),
            None,
        )
        .unwrap();
        assert_eq!(sound.target, "low");
        assert_eq!(sound.fx_source.as_deref(), Some("(){ out = soften(in) }"));
        assert!(sound.engine_source.contains("fn soften"));
        assert!(!sound.engine_source.contains("fx low"));
    }
}

#[cfg(test)]
mod parameter_tests {
    use super::*;
    #[test]
    fn exposes_typed_fx_parameters_in_signature_order() {
        let sound = Sound::parse(Kind::Fx, "fx filter(cutoff: hz = 2400hz, resonance: 0..1 = 0.2) { out = in.lpf(cutoff: cutoff, res: resonance) }".into(), None).unwrap();
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

    #[test]
    fn a_gain_in_decibels_has_its_bounds_in_decibels_too() {
        let sound = Sound::parse(Kind::Fx, "fx squeeze(threshold: -60db..0db = -18db, makeup: 0db..24db = 3db) { out = in.compress(threshold: threshold, makeup: makeup) }".into(), None).unwrap();
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let (threshold, makeup) = (&sound.parameters[0], &sound.parameters[1]);
        assert!(near(threshold.min, -60.0) && near(threshold.max, 0.0) && near(threshold.default, -18.0), "{threshold:?}");
        assert!(near(makeup.min, 0.0) && near(makeup.max, 24.0) && near(makeup.default, 3.0), "{makeup:?}");
        // so a value in decibels is not squeezed into the bounds of a plain factor
        assert!(near(threshold.argument(-40.0).as_str().map_or(f64::NAN, |a| a.trim_end_matches("db").parse().unwrap()), -40.0));
    }
}
