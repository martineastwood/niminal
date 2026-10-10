use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use niminal_engine::{Graph, Mixer, TrackDef};
use niminal_score::{Event, Tempo, Value};

use crate::diag::closest;
use crate::layout::Layout;
use crate::unit::Unit;

/// A compiled instrument: an engine graph plus the typed signature that
/// notes are checked against.
pub struct Instrument {
    pub name: String,
    pub graph: Arc<Graph>,
    /// In graph parameter order.
    pub params: Vec<Param>,
    /// For a kit, the names of its samples, in the order its `sample` parameter counts them.
    pub members: Vec<String>,
    /// The choke group of each sample: one entry per kit member, or at most
    /// one for a lone sample.
    pub chokes: Vec<Option<u32>>,
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

/// A track as the mixer sees it, plus what the language knows about it.
pub struct TrackInfo {
    /// Empty for the implicit track that plays bare instruments.
    pub name: String,
    /// Index into `Program::instruments`.
    pub instrument: Option<usize>,
    /// Arguments given in `instrument = name(...)`, applied to every note.
    pub defaults: BTreeMap<String, Value>,
    pub def: TrackDef,
}

/// A named scalar control. Values in graphs use engine units (db is gain).
#[derive(Clone, Debug, PartialEq)]
pub struct ControlDef {
    pub name: String,
    pub key: String,
    pub unit: Unit,
    pub initial: f32,
    pub smooth_seconds: f64,
}

impl ControlDef {
    pub fn convert(&self, value: f64, suffix: Option<&str>, tempo: Tempo) -> Result<f32, String> {
        let suffix = suffix.or(self.unit.suffix());
        let (unit, converted) = crate::lower::literal_sig(value, suffix, tempo, crate::Span::default())
            .map_err(|d| d.message)?;
        if unit != self.unit { return Err(format!("control `{}` expects {}", self.name, self.unit.describe())); }
        let converted = converted as f32;
        if !value.is_finite() || !converted.is_finite() || (unit == Unit::Db && converted <= 0.0) {
            return Err("control values must be finite and representable".into());
        }
        Ok(converted)
    }
}

pub struct Program {
    pub controls: Vec<ControlDef>,
    pub tempo: Tempo,
    /// The layout of the output, and of every voice and track.
    pub master: Layout,
    pub instruments: Vec<Instrument>,
    pub buses: Vec<String>,
    pub bus_layouts: Vec<Layout>,
    /// Track 0 is the implicit one that plays instruments named directly.
    pub tracks: Vec<TrackInfo>,
    /// Every sample name used by a kit, which is what a `Value::Sample` counts.
    pub sample_names: Vec<String>,
    /// The names of the choke groups, which an instrument's `chokes` count.
    pub choke_groups: Vec<String>,
    /// Notes written directly in the source.
    pub notes: Vec<Event>,
    /// Commands that play clips and scenes, in source order.
    pub performance: Vec<crate::performance::Scheduled>,
    pub arrangements: Vec<crate::performance::Arrangement>,
}

/// A note ready to play: which track, which instrument, and its parameters.
#[derive(Debug, PartialEq)]
pub struct NotePlan {
    pub track: usize,
    pub instrument: usize,
    pub params: Vec<(usize, f32)>,
    /// The choke group of the sample this note plays: it ends other notes in the group.
    pub choke: Option<u32>,
}

/// What a note's target resolves to.
pub(crate) struct Target<'a> {
    pub track: usize,
    pub instrument: usize,
    /// Arguments a track applies to all its notes.
    pub defaults: Option<&'a BTreeMap<String, Value>>,
}

/// Find what a note's target plays: an instrument by name (on the implicit
/// track), or a track (with its instrument and default arguments).
pub(crate) fn resolve_target<'a>(
    instruments: &[Instrument],
    tracks: &'a [TrackInfo],
    target: &str,
) -> Result<Target<'a>, ArgError> {
    if let Some(t) = tracks.iter().skip(1).position(|t| t.name == target) {
        let track = &tracks[t + 1];
        return match track.instrument {
            Some(i) => Ok(Target { track: t + 1, instrument: i, defaults: Some(&track.defaults) }),
            None => Err(ArgError {
                message: format!("track `{target}` has no instrument to play"),
                help: Some("add `instrument = ...` to the track".into()),
            }),
        };
    }
    if let Some(i) = instruments.iter().position(|i| i.name == target) {
        return Ok(Target { track: 0, instrument: i, defaults: None });
    }
    let known = instruments.iter().map(|i| i.name.as_str()).chain(tracks.iter().skip(1).map(|t| t.name.as_str()));
    Err(ArgError {
        message: format!("no instrument or track named `{target}`"),
        help: closest(target, known).map(|c| format!("did you mean `{c}`?")),
    })
}

impl Program {
    /// The index of the track called `name`.
    pub fn track_index(&self, name: &str) -> Option<usize> {
        self.tracks.iter().skip(1).position(|t| t.name == name).map(|i| i + 1)
    }

    /// Where the kit sample `id` sits in `instrument`.
    fn kit_index(&self, instrument: &Instrument, id: u32, target: &str) -> Result<usize, ArgError> {
        let name = self.sample_names.get(id as usize).map_or("?", String::as_str);
        if instrument.members.is_empty() {
            return Err(ArgError {
                message: format!("`{name}` is a kit sample, but `{target}` plays `{}`, which isn't a kit", instrument.name),
                help: None,
            });
        }
        instrument.members.iter().position(|m| m == name).ok_or_else(|| ArgError {
            message: format!("`{name}` isn't in the kit `{}`", instrument.name),
            help: closest(name, instrument.members.iter().map(String::as_str)).map(|c| format!("did you mean `{c}`?")),
        })
    }

    pub fn instrument(&self, name: &str) -> Option<&Instrument> {
        self.instruments.iter().find(|i| i.name == name)
    }

    /// Check an event against its target and resolve it to a note to play.
    pub fn plan(&self, event: &Event) -> Result<NotePlan, ArgError> {
        let Target { track, instrument, defaults } =
            resolve_target(&self.instruments, &self.tracks, &event.target)?;
        let mut args = defaults.cloned().unwrap_or_default();
        args.extend(event.args.iter().map(|(k, v)| (k.clone(), *v)));
        for value in args.values_mut() {
            if let Value::Sample(id) = *value {
                *value = Value::Num(self.kit_index(&self.instruments[instrument], id, &event.target)? as f64);
            }
        }
        let params = self.instruments[instrument].bind_args(&args, self.tempo)?;
        let picked = args.get("sample").and_then(|v| v.as_number().ok()).unwrap_or(0.0) as usize;
        let choke = self.instruments[instrument].chokes.get(picked).copied().flatten();
        Ok(NotePlan { track, instrument, params, choke })
    }

    /// A mixer for this program's tracks and buses.
    pub fn mixer(&self, sample_rate: f32) -> Mixer {
        let defs = self.tracks.iter().map(|t| t.def.clone()).collect();
        let bus_channels = self.bus_layouts.iter().map(|l| l.channels()).collect();
        Mixer::new(defs, bus_channels, self.master.channels(), sample_rate)
            .expect("feedback is rejected when compiling")
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
            Unit::Angle => value.as_angle().map_err(unit_err)?,
            Unit::Semitones => value.as_semitones().map_err(unit_err)?,
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
