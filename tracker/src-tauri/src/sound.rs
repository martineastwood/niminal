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
        musical: None,
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
    /// `dir` is the project folder, where the files a sampler names are found. `bpm` and `beats_per_bar` (in
    /// quarter-note beats: 3 in 6/8) say how long the lengths in beats and bars are, which the language works
    /// out and the engine, taking only seconds, never sees. They are set when the slot is applied, so they
    /// follow a tempo change the next time it is.
    pub fn parse(kind: Kind, source: String, dir: Option<&str>, bpm: f64, beats_per_bar: f64) -> Result<Self, String> {
        // before a song has been sent there is no tempo or meter yet
        let (bpm, beats_per_bar) = (if bpm > 0.0 { bpm } else { 120.0 }, if beats_per_bar > 0.0 { beats_per_bar } else { 4.0 });
        // pasted code often carries non-breaking spaces, which the language's own lexer takes as spaces
        let written = source.replace('\u{a0}', " ");
        let musical = niminal_compose::beat_controls(&written);
        let source = niminal_compose::resolve_musical_time(&written, bpm, beats_per_bar).map_err(|e| e.message)?;
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
        let options = niminal_lang::CompileOptions {
            samples: dir.map_or_else(niminal_lang::Samples::default, niminal_lang::Samples::new),
            ..Default::default()
        };
        // The compiler can't always load an instrument on its own (a sampler needs its
        // folder), and the engine reports real errors on Apply, so no controls will do.
        let compiled = if kind == Kind::Instr { niminal_lang::compile_with(&source, &options).ok() } else { None };
        // A sample cut into pieces, or a kit of samples, played by key: the instrument has a `sample` parameter
        // and the code makes exactly one kit. How many members it has is known from the code or, for a folder, the files.
        let mut kits = definitions.iter().filter(|s| s.kind == SymbolKind::Kit);
        let slices = match (kind, kits.next(), kits.next()) {
            (Kind::Instr, Some(kit), None) if definition.params.iter().any(|p| p.name == "sample") => compiled
                .as_ref()
                .and_then(|program| program.instruments.iter().find(|i| i.name == kit.name))
                .map(|kit| kit.members.len())
                .filter(|&count| count > 0)
                .or(Some(kit.params.len()).filter(|&count| count > 0)),
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
            // every parameter is listed for sliders and automation; a command column reaches the first 15
            for param in &definition.params {
                let key = format!("{}::{}", definition.name, param.name);
                let Some(control) = program.controls.iter().find(|control| control.name == key)
                else {
                    continue;
                };
                let mut made = parameter(&param.name, control.unit, control.range, f64::from(control.initial));
                made.musical = musical
                    .iter()
                    .find(|m| m.effect.as_deref() == Some(definition.name.as_str()) && m.name == param.name)
                    .map(|m| (m.value, m.unit.to_string()));
                parameters.push(made);
            }
        }
        if kind == Kind::Instr
            && let Some(program) = &compiled
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
    use serde_json::json;
    #[test]
    fn a_library_effect_with_tempo_synced_defaults_follows_the_tempo() {
        let source = "fx stutter(length: 1ms..1sec = 1/4beat, bar: 100ms..4sec = 1bar) { out = in }";
        let at = |bpm, per_bar| Sound::parse(Kind::Fx, source.into(), None, bpm, per_bar).unwrap().parameters.iter().map(|p| p.default).collect::<Vec<_>>();
        assert_eq!(at(120.0, 4.0), vec![0.125, 2.0]);
        assert_eq!(at(60.0, 4.0), vec![0.25, 4.0]);
        // a bar is as long as the meter says: 3 beats in 3/4 or 6/8, 3.5 in 7/8
        assert_eq!(at(120.0, 3.0), vec![0.125, 1.5]);
        assert_eq!(at(120.0, 3.5), vec![0.125, 1.75]);
    }

    #[test]
    fn a_length_in_beats_is_sent_to_the_engine_in_beats_until_it_is_set() {
        let source = "fx stutter(length: 1ms..1sec = 1/4beat, bar: 100ms..4sec = 1bar, other: 1ms..1sec = 20ms) { out = in }";
        let parameters = Sound::parse(Kind::Fx, source.into(), None, 120.0, 4.0).unwrap().parameters;
        let sent: Vec<_> = parameters.iter().map(|p| p.default_argument()).collect();
        assert_eq!(sent[..2], [json!("0.25beats"), json!("1bars")]);
        assert!(sent[2].as_str().is_some_and(|s| s.ends_with("sec")), "an ordinary default is in seconds: {}", sent[2]);
        // set by hand, it is the number of seconds that was set
        assert_eq!(parameters[0].argument(0.3), json!("0.3sec"));
    }

    #[test]
    fn separates_reusable_fx_from_its_helper_definitions() {
        let sound = Sound::parse(
            Kind::Fx,
            "fn soften(x) { x * 0.5 }\nfx low { out = soften(in) }\n".into(),
            None,
            120.0,
            4.0,
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
    use serde_json::json;
    #[test]
    fn exposes_typed_fx_parameters_in_signature_order() {
        let sound = Sound::parse(Kind::Fx, "fx filter(cutoff: hz = 2400hz, resonance: 0..1 = 0.2) { out = in.lpf(cutoff: cutoff, res: resonance) }".into(), None, 120.0, 4.0).unwrap();
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
        let sound = Sound::parse(Kind::Fx, "fx squeeze(threshold: -60db..0db = -18db, makeup: 0db..24db = 3db) { out = in.compress(threshold: threshold, makeup: makeup) }".into(), None, 120.0, 4.0).unwrap();
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let (threshold, makeup) = (&sound.parameters[0], &sound.parameters[1]);
        assert!(near(threshold.min, -60.0) && near(threshold.max, 0.0) && near(threshold.default, -18.0), "{threshold:?}");
        assert!(near(makeup.min, 0.0) && near(makeup.max, 24.0) && near(makeup.default, 3.0), "{makeup:?}");
        // so a value in decibels is not squeezed into the bounds of a plain factor
        assert!(near(threshold.argument(-40.0).as_str().map_or(f64::NAN, |a| a.trim_end_matches("db").parse().unwrap()), -40.0));
    }
}
