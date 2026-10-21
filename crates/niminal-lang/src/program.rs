use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use niminal_engine::{Graph, Mixer, NodeDef};
use niminal_score::{Event, Tempo, Value};

use crate::diag::closest;
use crate::layout::Layout;
use crate::unit::Unit;

/// The parameter that a note's pitch sets, unless an instrument names another with `pitch:`.
pub const PITCH_PARAM: &str = "freq";

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
    pub resource_keys: BTreeMap<String, u64>,
    /// For a kit, the names of its samples, in the order its `sample` parameter counts them.
    pub members: Vec<String>,
    /// The choke group of each sample: one entry per kit member, or at most
    /// one for a lone sample.
    pub chokes: Vec<Option<u64>>,
    /// `max = 8`, or `mono`: the most notes of this instrument sounding on a node at once.
    pub max_voices: Option<usize>,
    /// `same_note = release`: a note releases the sounding note of the same pitch.
    pub release_same_note: bool,
    /// `steal = quietest`: when `max` is reached, the quietest note ends, not the oldest.
    pub steal_quietest: bool,
    /// `legato = true` on a `mono` instrument: a note over a held one changes its pitch.
    pub legato: bool,
    /// `priority = lowest`: which held note a legato voice sounds.
    pub priority: niminal_engine::Priority,
    /// `glide = 80ms`: how long a change of pitch takes.
    pub glide: Option<niminal_score::Time>,
    /// `pitch = note`: the parameter a note's pitch sets, `freq` unless renamed.
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

/// A node as the mixer sees it, plus what the language knows about it.
#[derive(Clone, Default)]
pub struct NodeInfo {
    pub accepts_notes: bool,
    /// Its signals sent over OSC.
    pub taps: Vec<OscTap>,
    /// Empty for the implicit node that plays bare instruments.
    pub name: String,
    /// Index into `Program::instruments`.
    pub instrument: Option<usize>,
    /// Arguments given in `instrument = name(...)`, applied to every note.
    pub defaults: BTreeMap<String, Value>,
    pub def: NodeDef,
}

/// A named scalar control. Values in graphs use engine units (db is gain).
#[derive(Clone, Debug, PartialEq)]
pub struct ControlDef {
    pub range: Option<(f64, f64)>,
    pub name: String,
    pub key: String,
    pub unit: Unit,
    pub initial: f32,
    pub smooth_seconds: f64,
}

impl ControlDef {
    /// A lane's value as the control's engine value, with the unit rules a parameter has.
    pub fn from_value(&self, value: Value, tempo: Tempo) -> Result<f32, niminal_score::UnitError> {
        engine_value(self.unit, value, tempo)
            .map(|v| self.range.map_or(v, |(lo, hi)| v.clamp(lo, hi)) as f32)
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
        Ok(self
            .range
            .map_or(converted, |(lo, hi)| converted.clamp(lo as f32, hi as f32)))
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

#[derive(Clone)]
pub struct Program {
    pub resource_controls: BTreeMap<String, u64>,
    pub array_controls: BTreeMap<String, Vec<String>>,
    pub namespaces: Vec<(usize, usize, String)>,
    pub namespace_samples: std::collections::HashMap<String, crate::Samples>,
    pub bus_meters: Vec<(usize, Arc<niminal_engine::Meter>)>,
    /// What each node costs to render, shared by every mixer built from this program.
    pub profile: Arc<niminal_engine::Profile>,
    /// The text it was compiled from.
    pub source: String,
    /// Where the sample files it names were looked for.
    pub samples: crate::sample::Samples,
    /// Every signal the project sends over OSC.
    pub taps: Vec<OscTap>,
    pub controls: Vec<ControlDef>,
    pub tempo: Tempo,
    /// The layout of the output, and of every voice and node.
    pub master: Layout,
    pub instruments: Vec<Instrument>,
    pub buses: Vec<String>,
    pub bus_layouts: Vec<Layout>,
    /// Shared writable tables, by name.
    pub buffers: Vec<(String, std::sync::Arc<niminal_engine::Buffer>)>,
    /// Node 0 is the implicit one that plays instruments named directly.
    pub nodes: Vec<NodeInfo>,
    /// Every sample name used by a kit, which is what a `Value::Sample` counts.
    pub sample_names: Vec<String>,
    /// The names of the choke groups, which an instrument's `chokes` count.
    pub choke_groups: Vec<String>,
}

/// A note ready to play: which node, which instrument, and its parameters.
#[derive(Debug, PartialEq)]
pub struct NotePlan {
    pub node: usize,
    pub instrument: usize,
    pub params: Vec<(usize, f32)>,
    /// The choke group of the sample this note plays: it ends other notes in the group.
    pub choke: Option<u64>,
    /// How many notes of the instrument may sound at once, if limited.
    pub rule: Option<niminal_engine::VoiceRule>,
    /// Node parameters the note's clip sets as it starts: control key, value, and how long it glides.
    pub controls: Vec<(String, f32, f64)>,
    /// How the note is spread over the master's speakers, if it has a pan of its own.
    pub pan: Option<[f32; niminal_engine::MAX_CHANNELS]>,
    /// The note to send, if its instrument is a `midi_out`.
    pub midi: Option<MidiNote>,
}

/// What a note's target resolves to.
pub struct Target<'a> {
    pub node: usize,
    pub instrument: usize,
    /// Arguments a node applies to all its notes.
    pub defaults: Option<&'a BTreeMap<String, Value>>,
}

/// Find what a note's target plays: an instrument by name (on the implicit
/// node), or a node (with its instrument and default arguments).
pub fn resolve_target<'a>(
    instruments: &[Instrument],
    nodes: &'a [NodeInfo],
    target: &str,
) -> Result<Target<'a>, ArgError> {
    if let Some(t) = nodes.iter().skip(1).position(|t| t.name == target) {
        let node = &nodes[t + 1];
        if !node.accepts_notes && node.instrument.is_some() {
            return Err(ArgError::new("destination node is draining".into()));
        }
        return match node.instrument {
            Some(i) => Ok(Target {
                node: t + 1,
                instrument: i,
                defaults: Some(&node.defaults),
            }),
            None => Err(ArgError {
                message: format!("node `{target}` has no instrument to play"),
                help: Some("add `instrument = ...` to the node".into()),
            }),
        };
    }
    if let Some((instrument, destination)) = target.split_once('@') {
        let instrument = instruments
            .iter()
            .position(|i| i.name == instrument)
            .ok_or_else(|| ArgError::new("unknown routed instrument".into()))?;
        let node = nodes
            .iter()
            .position(|t| t.name == destination && t.accepts_notes)
            .ok_or_else(|| ArgError::new("destination node is absent or draining".into()))?;
        return Ok(Target {
            node,
            instrument,
            defaults: None,
        });
    }
    if let Some(i) = instruments.iter().position(|i| i.name == target) {
        return Ok(Target {
            node: 0,
            instrument: i,
            defaults: None,
        });
    }
    let known = instruments
        .iter()
        .map(|i| i.name.as_str())
        .chain(nodes.iter().skip(1).map(|t| t.name.as_str()));
    Err(ArgError {
        message: format!("no instrument or node named `{target}`"),
        help: closest(target, known).map(|c| format!("did you mean `{c}`?")),
    })
}

impl Program {
    /// Translate client/compiler values to physical engine arguments and local kit indices.
    pub fn dsp_args(&self, event: &Event) -> Result<BTreeMap<String, Value>, ArgError> {
        let plan = self.plan(event)?;
        let instrument = &self.instruments[plan.instrument];
        event
            .args
            .iter()
            .map(|(name, value)| {
                let value = if let Value::Sample(id) = *value {
                    Value::Num(self.kit_index(instrument, id, &event.target)? as f64)
                } else {
                    crate::dsp_value(*value, self.tempo)
                };
                Ok((name.clone(), value))
            })
            .collect()
    }
    /// The index of the node called `name`.
    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.nodes
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
            node,
            instrument,
            defaults,
        } = resolve_target(&self.instruments, &self.nodes, &event.target)?;
        let mut args = defaults.cloned().unwrap_or_default();
        args.extend(event.args.iter().map(|(k, v)| (k.clone(), *v)));
        // a note's pitch is `freq` to the score, and whatever `pitch = ...` named it to the instrument
        let pitch_name = &self.instruments[instrument].pitch;
        if pitch_name != PITCH_PARAM
            && let Some(pitch) = args.remove(PITCH_PARAM)
        {
            args.insert(pitch_name.clone(), pitch);
        }
        // a lane that is a parameter of the node, not of the instrument, sets that control
        let mut controls = Vec::new();
        if node != 0 {
            let owner = &self.nodes[node].name;
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
        // An explicit pan parameter is wired by the instrument; otherwise pan the note output.
        let note_pan = if self.instruments[instrument].params.iter().any(|p| p.name == "pan") {
            None
        } else {
            args.remove("pan")
        };
        let pan = match note_pan {
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
            let pitch_param = this.params.iter().position(|p| p.name == this.pitch);
            (this.max_voices.is_some() || this.release_same_note).then(|| {
                niminal_engine::VoiceRule {
                    instrument: niminal_engine::instance_id(&this.name),
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
            node,
            instrument,
            params,
            choke,
            rule,
            controls,
            pan,
            midi,
        })
    }

    /// A mixer for this program's nodes and buses.
    pub fn mixer(&self, sample_rate: f32) -> Mixer {
        let defs = self.nodes.iter().map(|t| t.def.clone()).collect();
        let bus_channels = self.bus_layouts.iter().map(|l| l.channels()).collect();
        let mut mixer = Mixer::new(defs, bus_channels, self.master.channels(), sample_rate)
            .expect("feedback is rejected when compiling");
        mixer.set_bus_meters(self.bus_meters.clone());
        mixer.set_profile(self.profile.clone());
        for (i, node) in self.nodes.iter().enumerate() {
            mixer.set_node_instance(i, niminal_engine::instance_id(&node.name));
        }
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
            resource_keys: BTreeMap::new(),
            members: Vec::new(),
            chokes: Vec::new(),
            max_voices: None,
            release_same_note: false,
            steal_quietest: false,
            legato: false,
            priority: Default::default(),
            glide: None,
            pitch: PITCH_PARAM.into(),
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
