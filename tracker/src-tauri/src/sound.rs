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
        if definition.name == "__tracker_bypass" {
            return Err("This name is reserved for the track bypass".into());
        }
        if kind == Kind::Instr && !definition.params.iter().any(|param| param.name == "freq") {
            return Err("Tracker instruments currently need a freq: hz parameter".into());
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
