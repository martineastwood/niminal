use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use niminal_engine::{Graph, Mixer, TrackDef};
use niminal_score::{Event, Tempo, Value};

use crate::ast::InputSource;
use crate::diag::closest;
use crate::layout::Layout;
use crate::unit::Unit;

/// A compiled instrument: an engine graph plus the typed signature that
/// notes are checked against.
#[derive(Clone)]
pub struct Instrument {
    pub taps: Vec<OscTap>,
    pub name: String,
    pub graph: Arc<Graph>,
    /// In graph parameter order.
    pub params: Vec<Param>,
    pub arrays: BTreeMap<String, Vec<String>>,
    pub resources: BTreeMap<String, Vec<String>>,
    /// For a kit, the names of its samples, in the order its `sample` parameter counts them.
    pub members: Vec<String>,
    /// The choke group of each sample: one entry per kit member, or at most
    /// one for a lone sample.
    pub chokes: Vec<Option<u32>>,
    /// `with(max: 8)`, or `mono`: the most notes of this instrument sounding on a track at once.
    pub max_voices: Option<usize>,
    /// `with(same_note: release)`: a note releases the sounding note of the same pitch.
    pub release_same_note: bool,
    /// `with(steal: quietest)`: when `max` is reached, the quietest note ends, not the oldest.
    pub steal_quietest: bool,
    /// `with(legato: true)` on a `mono` instrument: a note over a held one changes its pitch.
    pub legato: bool,
    /// `with(priority: lowest)`: which held note a legato voice sounds.
    pub priority: niminal_engine::Priority,
    /// `with(glide: 80ms)`: how long a change of pitch takes.
    pub glide: Option<niminal_score::Time>,
    /// `with(pitch: note)`: the parameter a note's pitch sets, `freq` unless renamed.
    pub pitch: String,
    /// `midi_out`: notes are sent to external gear instead of sounding here.
    pub midi: Option<MidiTarget>,
}

/// Where a `midi_out` instrument sends its notes.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiTarget {
    /// Part of the name of a MIDI output port; if no port matches, a virtual one of this name is made.
    pub port: std::sync::Arc<str>,
    /// From 1 to 16.
    pub channel: u8,
}

/// A note ready to send to external gear.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiNote {
    pub port: std::sync::Arc<str>,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
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
        ArgError {
            message,
            help: None,
        }
    }
}

/// A signal sent out over OSC: `x.osc_out("/vj/level", port: 9000)`. The audio thread keeps `value`
/// up to date; whoever sends it reads it `rate` times a second.
#[derive(Clone, Debug)]
pub struct OscTap {
    pub address: String,
    pub host: String,
    pub port: u16,
    /// Messages a second at most.
    pub rate: f64,
    pub value: std::sync::Arc<niminal_engine::ops::Tap>,
}

/// A track as the mixer sees it, plus what the language knows about it.
#[derive(Clone, Default)]
pub struct TrackInfo {
    pub accepts_notes: bool,
    /// Its signals sent over OSC.
    pub taps: Vec<OscTap>,
    /// Empty for the implicit track that plays bare instruments.
    pub name: String,
    /// Index into `Program::instruments`.
    pub instrument: Option<usize>,
    /// Arguments given in `instrument = name(...)`, applied to every note.
    pub defaults: BTreeMap<String, Value>,
    /// `notes = it.thin(10%)`: how every clip played on the track has its notes changed, with `it` the clip's.
    pub notes: Option<crate::ast::Expr>,
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
    pub input: Option<ControlBinding>,
}

/// A control driven from outside: a message value from 0 to 1 maps onto `lo..hi`
/// (engine units; geometric for hz and db, so sweeps are even by ear).
#[derive(Clone, Debug, PartialEq)]
pub struct ControlBinding {
    pub source: InputSource,
    pub lo: f32,
    pub hi: f32,
}

impl ControlDef {
    /// The control's value, in its unit's own suffix (so `convert` accepts it), for a
    /// normalised input `t` from 0 to 1.
    pub fn map_input(&self, t: f64) -> Option<f64> {
        let b = self.input.as_ref()?;
        let t = t.clamp(0.0, 1.0);
        let (lo, hi) = (f64::from(b.lo), f64::from(b.hi));
        let geometric = matches!(self.unit, Unit::Hz | Unit::Db) && lo > 0.0 && hi > 0.0;
        let engine = if geometric {
            lo * (hi / lo).powf(t)
        } else {
            lo + (hi - lo) * t
        };
        Some(if self.unit == Unit::Db {
            20.0 * engine.log10()
        } else {
            engine
        })
    }

    /// A lane's value as the control's engine value, with the unit rules a parameter has.
    pub fn from_value(&self, value: Value, tempo: Tempo) -> Result<f32, niminal_score::UnitError> {
        engine_value(self.unit, value, tempo).map(|v| v as f32)
    }

    pub fn convert(&self, value: f64, suffix: Option<&str>, tempo: Tempo) -> Result<f32, String> {
        let suffix = suffix.or(self.unit.suffix());
        let (unit, converted) =
            crate::lower::literal_sig(value, suffix, tempo, crate::Span::default())
                .map_err(|d| d.message)?;
        if unit != self.unit {
            return Err(format!(
                "control `{}` expects {}",
                self.name,
                self.unit.describe()
            ));
        }
        let converted = converted as f32;
        if !value.is_finite() || !converted.is_finite() || (unit == Unit::Db && converted <= 0.0) {
            return Err("control values must be finite and representable".into());
        }
        Ok(converted)
    }
}

/// A score value in the engine's representation for a quantity of `unit`, with the strict-unit rules.
pub fn engine_value(
    unit: Unit,
    value: Value,
    tempo: Tempo,
) -> Result<f64, niminal_score::UnitError> {
    Ok(match unit {
        Unit::Hz => value.as_hz_in(&tempo.tuning)?,
        Unit::Db => value.as_gain()?,
        Unit::Time => value.as_time()?.to_seconds(tempo),
        Unit::Angle => value.as_angle()?,
        Unit::Semitones => value.as_semitones()?,
        Unit::Num => value.as_number()?,
    })
}

/// `config { quantize: { ... } }`: when each kind of change lands, where the project says.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct QuantizeConfig {
    pub default: Option<crate::performance::Quantize>,
    pub play: Option<crate::performance::Quantize>,
    pub instrument: Option<crate::performance::Quantize>,
    pub track: Option<crate::performance::Quantize>,
    pub control: Option<crate::performance::Quantize>,
    pub tempo: Option<crate::performance::Quantize>,
}

#[derive(Clone)]
pub struct Program {
    pub bus_meters: Vec<(usize, Arc<niminal_engine::Meter>)>,
    /// The text it was compiled from.
    pub source: String,
    /// Where the sample files it names were looked for.
    pub samples: crate::sample::Samples,
    pub quantize: QuantizeConfig,
    /// Every signal the project sends over OSC.
    pub taps: Vec<OscTap>,
    pub controls: Vec<ControlDef>,
    pub tempo: Tempo,
    /// The layout of the output, and of every voice and track.
    pub master: Layout,
    pub instruments: Vec<Instrument>,
    pub buses: Vec<String>,
    pub bus_layouts: Vec<Layout>,
    /// Shared writable tables, by name.
    pub buffers: Vec<(String, std::sync::Arc<niminal_engine::Buffer>)>,
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
    /// How many notes of the instrument may sound at once, if limited.
    pub rule: Option<niminal_engine::VoiceRule>,
    /// Track parameters the note's clip sets as it starts: control key, value, and how long it glides.
    pub controls: Vec<(String, f32, f64)>,
    /// How the note is spread over the master's speakers, if it has a pan of its own.
    pub pan: Option<[f32; niminal_engine::MAX_CHANNELS]>,
    /// The note to send, if its instrument is a `midi_out`.
    pub midi: Option<MidiNote>,
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
        if !track.accepts_notes && track.instrument.is_some() {
            return Err(ArgError::new("destination node is draining".into()));
        }
        return match track.instrument {
            Some(i) => Ok(Target {
                track: t + 1,
                instrument: i,
                defaults: Some(&track.defaults),
            }),
            None => Err(ArgError {
                message: format!("track `{target}` has no instrument to play"),
                help: Some("add `instrument = ...` to the track".into()),
            }),
        };
    }
    if let Some((instrument, destination)) = target.split_once('@') {
        let instrument = instruments
            .iter()
            .position(|i| i.name == instrument)
            .ok_or_else(|| ArgError::new("unknown routed instrument".into()))?;
        let track = tracks
            .iter()
            .position(|t| t.name == destination && t.accepts_notes)
            .ok_or_else(|| ArgError::new("destination node is absent or draining".into()))?;
        return Ok(Target {
            track,
            instrument,
            defaults: None,
        });
    }
    if let Some(i) = instruments.iter().position(|i| i.name == target) {
        return Ok(Target {
            track: 0,
            instrument: i,
            defaults: None,
        });
    }
    let known = instruments
        .iter()
        .map(|i| i.name.as_str())
        .chain(tracks.iter().skip(1).map(|t| t.name.as_str()));
    Err(ArgError {
        message: format!("no instrument or track named `{target}`"),
        help: closest(target, known).map(|c| format!("did you mean `{c}`?")),
    })
}

impl Program {
    /// The index of the track called `name`.
    pub fn track_index(&self, name: &str) -> Option<usize> {
        self.tracks
            .iter()
            .skip(1)
            .position(|t| t.name == name)
            .map(|i| i + 1)
    }

    /// Where the kit sample `id` sits in `instrument`.
    fn kit_index(&self, instrument: &Instrument, id: u32, target: &str) -> Result<usize, ArgError> {
        let name = self
            .sample_names
            .get(id as usize)
            .map_or("?", String::as_str);
        if instrument.members.is_empty() {
            return Err(ArgError {
                message: format!(
                    "`{name}` is a kit sample, but `{target}` plays `{}`, which isn't a kit",
                    instrument.name
                ),
                help: None,
            });
        }
        instrument
            .members
            .iter()
            .position(|m| m == name)
            .ok_or_else(|| ArgError {
                message: format!("`{name}` isn't in the kit `{}`", instrument.name),
                help: closest(name, instrument.members.iter().map(String::as_str))
                    .map(|c| format!("did you mean `{c}`?")),
            })
    }

    pub fn instrument(&self, name: &str) -> Option<&Instrument> {
        self.instruments.iter().find(|i| i.name == name)
    }

    /// Check an event against its target and resolve it to a note to play.
    pub fn plan(&self, event: &Event) -> Result<NotePlan, ArgError> {
        let Target {
            track,
            instrument,
            defaults,
        } = resolve_target(&self.instruments, &self.tracks, &event.target)?;
        let mut args = defaults.cloned().unwrap_or_default();
        args.extend(event.args.iter().map(|(k, v)| (k.clone(), *v)));
        // a note's pitch is `freq` to the score, and whatever `with(pitch: ...)` named it to the instrument
        let pitch_name = &self.instruments[instrument].pitch;
        if pitch_name != crate::perform::PITCH_PARAM
            && let Some(pitch) = args.remove(crate::perform::PITCH_PARAM)
        {
            args.insert(pitch_name.clone(), pitch);
        }
        // a lane that is a parameter of the track, not of the instrument, sets that control
        let mut controls = Vec::new();
        if track != 0 {
            let owner = &self.tracks[track].name;
            let found: Vec<String> = args
                .keys()
                .filter(|k| {
                    self.instruments[instrument].param(k).is_err()
                        && self
                            .controls
                            .iter()
                            .any(|c| c.name == format!("{owner}::{k}"))
                })
                .cloned()
                .collect();
            for lane in found {
                let value = args.remove(&lane).expect("just listed");
                let def = self
                    .controls
                    .iter()
                    .find(|c| c.name == format!("{owner}::{lane}"))
                    .expect("just found");
                let v = def
                    .from_value(value, self.tempo)
                    .map_err(|e| ArgError::new(format!("argument `{lane}`: {e}")))?;
                controls.push((def.key.clone(), v, def.smooth_seconds));
            }
        }
        // a pan belongs to the note, not to its instrument
        let pan = match args.remove("pan") {
            Some(value) => {
                let azimuth = value
                    .as_angle()
                    .map_err(|e| ArgError::new(format!("argument `pan`: {e}")))?;
                Some(niminal_engine::ops::pan_gains(
                    &self.master.speaker_angles(),
                    azimuth as f32,
                    0.0,
                ))
            }
            None => None,
        };
        for value in args.values_mut() {
            if let Value::Sample(id) = *value {
                *value =
                    Value::Num(
                        self.kit_index(&self.instruments[instrument], id, &event.target)? as f64,
                    );
            }
        }
        let params = self.instruments[instrument].bind_args(&args, self.tempo)?;
        let picked = args
            .get("sample")
            .and_then(|v| v.as_number().ok())
            .unwrap_or(0.0) as usize;
        let choke = self.instruments[instrument]
            .chokes
            .get(picked)
            .copied()
            .flatten();
        let rule = {
            let this = &self.instruments[instrument];
            let pitch_param = this
                .params
                .iter()
                .position(|p| p.name == this.pitch);
            (this.max_voices.is_some() || this.release_same_note).then(|| {
                niminal_engine::VoiceRule {
                    instrument: instrument as u32,
                    max: this.max_voices.unwrap_or(usize::MAX),
                    steal_quietest: this.steal_quietest,
                    release_same_pitch: this.release_same_note,
                    pitch: pitch_param
                        .and_then(|i| {
                            params
                                .iter()
                                .find(|(index, _)| *index == i)
                                .map(|&(_, v)| v)
                                .or(this.params[i].default)
                        })
                        .unwrap_or(0.0),
                    pitch_param,
                    legato: this.legato,
                    priority: this.priority,
                    glide: this.glide.map_or(0.0, |t| t.to_seconds(self.tempo) as f32),
                }
            })
        };
        let midi = self.instruments[instrument].midi.as_ref().map(|target| {
            let this = &self.instruments[instrument];
            let value = |name: &str| {
                let i = this.params.iter().position(|p| p.name == name)?;
                params
                    .iter()
                    .find(|(index, _)| *index == i)
                    .map(|&(_, v)| v)
                    .or(this.params[i].default)
            };
            let hz = f64::from(value(&this.pitch).unwrap_or(440.0)).max(1.0);
            let db = 20.0
                * f64::from(value(crate::lower::GAIN_PARAM).unwrap_or(1.0))
                    .max(1e-6)
                    .log10();
            MidiNote {
                port: target.port.clone(),
                channel: target.channel,
                note: (69.0 + 12.0 * (hz / 440.0).log2())
                    .round()
                    .clamp(0.0, 127.0) as u8,
                // 0db is the loudest, -30db the softest: the same scale a keyboard's velocity is read on
                velocity: ((db + 30.0) / 30.0 * 126.0 + 1.0).round().clamp(1.0, 127.0) as u8,
            }
        });
        Ok(NotePlan {
            track,
            instrument,
            params,
            choke,
            rule,
            controls,
            pan,
            midi,
        })
    }

    /// A mixer for this program's tracks and buses.
    pub fn mixer(&self, sample_rate: f32) -> Mixer {
        let defs = self.tracks.iter().map(|t| t.def.clone()).collect();
        let bus_channels = self.bus_layouts.iter().map(|l| l.channels()).collect();
        let mut mixer = Mixer::new(defs, bus_channels, self.master.channels(), sample_rate)
            .expect("feedback is rejected when compiling");
        mixer.set_bus_meters(self.bus_meters.clone());
        mixer
    }
}

impl Instrument {
    /// A silent-by-default voice instrument; callers set kit members, MIDI, or voice options.
    pub(crate) fn new(name: String, graph: Arc<Graph>, params: Vec<Param>) -> Self {
        Self {
            taps: Vec::new(),
            name,
            graph,
            params,
            arrays: BTreeMap::new(),
            resources: BTreeMap::new(),
            members: Vec::new(),
            chokes: Vec::new(),
            max_voices: None,
            release_same_note: false,
            steal_quietest: false,
            legato: false,
            priority: Default::default(),
            glide: None,
            pitch: crate::perform::PITCH_PARAM.into(),
            midi: None,
        }
    }

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
            ArgError {
                message: format!("argument `{name}`: {e}"),
                help,
            }
        };

        let v = engine_value(param.unit, value, tempo).map_err(unit_err)?;
        if let Some((lo, hi)) = param.range
            && !(lo..=hi).contains(&v)
        {
            return Err(ArgError::new(format!(
                "argument `{name}` must be between {lo} and {hi}, got {v}"
            )));
        }
        Ok(v as f32)
    }

    /// Check a note's arguments and turn them into `(parameter index, value)`
    /// pairs ready for a voice. Every parameter without a default must be given.
    pub fn bind_args(
        &self,
        args: &BTreeMap<String, Value>,
        tempo: Tempo,
    ) -> Result<Vec<(usize, f32)>, ArgError> {
        let mut bound = Vec::with_capacity(args.len());
        for (name, value) in args {
            let (index, param) = self.param(name)?;
            bound.push((index, self.param_value(param, *value, tempo)?));
        }
        for param in self.params.iter().filter(|p| p.default.is_none()) {
            if !args.contains_key(&param.name) {
                return Err(ArgError::new(format!(
                    "`{}` needs an argument `{}`",
                    self.name, param.name
                )));
            }
        }
        Ok(bound)
    }
}
