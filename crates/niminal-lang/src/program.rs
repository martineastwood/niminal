use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use niminal_engine::Graph;
use niminal_score::{Event, Tempo, Value};

use crate::diag::closest;
use crate::unit::Unit;

/// A compiled instrument: an engine graph plus the typed signature that
/// notes are checked against.
pub struct Instrument {
    pub name: String,
    pub graph: Arc<Graph>,
    /// In graph parameter order.
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub unit: Unit,
    /// Bounds for a `0..1` style parameter.
    pub range: Option<(f64, f64)>,
    /// In the engine's representation. `None` means the note must give it.
    pub default: Option<f32>,
}

/// A bad argument, with an optional hint ("did you mean `800hz`?").
#[derive(Debug, Clone, PartialEq)]
pub struct ArgError {
    pub message: String,
    pub help: Option<String>,
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(h) = &self.help {
            write!(f, " ({h})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ArgError {}

impl ArgError {
    fn new(message: String) -> Self {
        ArgError { message, help: None }
    }
}

pub struct Program {
    pub tempo: Tempo,
    pub instruments: Vec<Instrument>,
    /// Notes written directly in the source.
    pub notes: Vec<Event>,
}

impl Program {
    pub fn instrument(&self, name: &str) -> Option<&Instrument> {
        self.instruments.iter().find(|i| i.name == name)
    }
}

impl Instrument {
    pub fn param(&self, name: &str) -> Result<(usize, &Param), ArgError> {
        match self.params.iter().enumerate().find(|(_, p)| p.name == name) {
            Some(found) => Ok(found),
            None => Err(ArgError {
                message: format!("`{}` has no parameter `{name}`", self.name),
                help: closest(name, self.params.iter().map(|p| p.name.as_str()))
                    .map(|c| format!("did you mean `{c}`?")),
            }),
        }
    }

    /// Convert a score value to the engine's representation for `param`,
    /// applying the strict-unit rules and range checks.
    pub fn param_value(&self, param: &Param, value: Value, tempo: Tempo) -> Result<f32, ArgError> {
        let name = &param.name;
        let unit_err = |e: niminal_score::UnitError| {
            let help = match (e.got, param.unit.suffix()) {
                (Value::Num(n), Some(suffix)) => Some(format!("did you mean `{n}{suffix}`?")),
                _ => None,
            };
            ArgError { message: format!("argument `{name}`: {e}"), help }
        };

        let v = match param.unit {
            Unit::Hz => value.as_hz().map_err(unit_err)?,
            Unit::Db => value.as_gain().map_err(unit_err)?,
            Unit::Time => value.as_time().map_err(unit_err)?.to_seconds(tempo),
            Unit::Num => {
                let n = value.as_number().map_err(unit_err)?;
                if let Some((lo, hi)) = param.range
                    && !(lo..=hi).contains(&n)
                {
                    return Err(ArgError::new(format!("argument `{name}` must be between {lo} and {hi}, got {n}")));
                }
                n
            }
        };
        Ok(v as f32)
    }

    /// Check a note's arguments and turn them into `(parameter index, value)`
    /// pairs ready for a voice. Every parameter without a default must be given.
    pub fn bind_args(&self, args: &BTreeMap<String, Value>, tempo: Tempo) -> Result<Vec<(usize, f32)>, ArgError> {
        let mut bound = Vec::with_capacity(args.len());
        for (name, value) in args {
            let (index, param) = self.param(name)?;
            bound.push((index, self.param_value(param, *value, tempo)?));
        }
        for param in self.params.iter().filter(|p| p.default.is_none()) {
            if !args.contains_key(&param.name) {
                return Err(ArgError::new(format!("`{}` needs an argument `{}`", self.name, param.name)));
            }
        }
        Ok(bound)
    }
}
