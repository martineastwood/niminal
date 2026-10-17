//! The language's view of built-in opcodes: names, argument order, and the unit
//! each argument expects. Argument names match the engine's port names. User
//! functions are compiled where they are called (see `lower` and `kernel`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use niminal_engine::Opcode;
pub use niminal_engine::ops::Wave;
use niminal_engine::ops::{
    Bitcrush, Chorus, Clip, Compressor, Delay, FilterMode, FinishWhen, Fn1, Fold, Func, Gain, Noise,
    Osc, Reverb, Svf, Wavetable,
};

use crate::unit::Unit;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A waveform name such as `saw`.
    Wave,
    /// A signal or value of exactly this unit.
    Signal(Unit),
    /// A gain: decibels, or a plain linear factor.
    Gain,
    /// A channel layout such as `stereo`.
    Layout,
}

pub struct ParamSpec {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
    /// False for settings that shape the opcode when it is built (such as a
    /// delay's `max`) rather than signals wired into it.
    pub wired: bool,
}

fn req(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        kind,
        required: true,
        wired: true,
    }
}

fn opt(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        kind,
        required: false,
        wired: true,
    }
}

fn setting(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        kind,
        required: false,
        wired: false,
    }
}

/// What an oscillator plays: a built-in waveform, one cycle of a table, or a morphing bank.
#[derive(Clone)]
pub enum WaveArg {
    Builtin(Wave),
    Table(Arc<Wavetable>),
    /// Several band-limited tables morphable with `position:`.
    Bank(Arc<[Arc<Wavetable>]>),
}

/// The longest table an oscillator accepts.
const MAX_TABLE: usize = 1 << 16;

impl WaveArg {
    /// One cycle of a wave, as given in a program.
    pub fn table(cycle: &[f64]) -> Result<Self, String> {
        if cycle.is_empty() {
            return Err("a table can't be empty".into());
        }
        if cycle.len() > MAX_TABLE {
            return Err(format!("a table to play can have at most {MAX_TABLE} entries"));
        }
        Ok(WaveArg::Table(cached_wavetable(cycle)))
    }

    pub fn from_table(table: &niminal_engine::Table) -> Result<Self, String> {
        if table.is_empty() {
            return Err("a table can't be empty".into());
        }
        if table.len() > MAX_TABLE {
            return Err(format!("a table to play can have at most {MAX_TABLE} entries"));
        }
        Ok(WaveArg::Table(cached_wavetable(&table.as_f64())))
    }

    pub fn bank(tables: &[Arc<niminal_engine::Table>]) -> Result<Self, String> {
        if tables.len() < 2 {
            return Err("a wavetable bank needs at least two tables".into());
        }
        let waves: Result<Vec<_>, _> = tables
            .iter()
            .map(|t| {
                if t.is_empty() {
                    return Err("a table can't be empty".into());
                }
                if t.len() > MAX_TABLE {
                    return Err(format!("a table to play can have at most {MAX_TABLE} entries"));
                }
                Ok(cached_wavetable(&t.as_f64()))
            })
            .collect();
        Ok(WaveArg::Bank(waves?.into()))
    }
}

/// Tables are shared while any program still plays them, so the same cycle is analysed once.
fn cached_wavetable(cycle: &[f64]) -> Arc<Wavetable> {
    static MADE: LazyLock<Mutex<HashMap<Vec<u64>, Weak<Wavetable>>>> = LazyLock::new(Mutex::default);
    let key: Vec<u64> = cycle.iter().map(|v| v.to_bits()).collect();
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(table) = made.get(&key).and_then(Weak::upgrade) {
        return table;
    }
    made.retain(|_, table| table.strong_count() > 0);
    let table = Arc::new(Wavetable::from_cycle(cycle));
    made.insert(key, Arc::downgrade(&table));
    table
}

/// What is known at compile time when an opcode is built.
#[derive(Default)]
pub struct BuildArgs {
    pub wave: Option<WaveArg>,
    /// Which channel of a multichannel signal this copy of the opcode serves.
    pub channel: usize,
    /// Arguments whose value is a compile-time constant, in engine units.
    pub consts: HashMap<String, f64>,
}

type BuildFn = fn(&BuildArgs) -> Result<Box<dyn Opcode>, String>;

/// The longest delay line a program may ask for.
const MAX_DELAY_SECONDS: f64 = 30.0;

pub enum Build {
    Builtin(BuildFn),
    /// `pan`: one output per speaker of the master layout.
    Pan,
    /// `to_layout`: converts between layouts.
    ToLayout,
}

pub struct OpSpec {
    pub name: String,
    /// What the opcode does, for hover and completion.
    pub doc: String,
    pub params: Vec<ParamSpec>,
    /// How many leading arguments may be given without a name: the leading
    /// required ones. The receiver of a method call counts as the first.
    pub positional: usize,
    pub build: Build,
}

impl OpSpec {
    fn new(name: &str, doc: &str, params: Vec<ParamSpec>, build: Build) -> Self {
        OpSpec {
            name: name.into(),
            doc: doc.into(),
            positional: crate::funcs::positional(params.iter().map(|p| p.required)),
            params,
            build,
        }
    }

    /// `Ok(None)` for an opcode that isn't a node of its own (`pan`, `to_layout`).
    pub fn instantiate(&self, args: &BuildArgs) -> Result<Option<Box<dyn Opcode>>, String> {
        match &self.build {
            Build::Builtin(f) => f(args).map(Some),
            Build::Pan | Build::ToLayout => Ok(None),
        }
    }
}

const SIGNAL: Kind = Kind::Signal(Unit::Num);

fn filter_params() -> Vec<ParamSpec> {
    vec![
        req("x", SIGNAL),
        req("cutoff", Kind::Signal(Unit::Hz)),
        opt("res", SIGNAL),
    ]
}

fn filter(name: &str, doc: &str, build: BuildFn) -> OpSpec {
    OpSpec::new(name, doc, filter_params(), Build::Builtin(build))
}

fn math(name: &str, doc: &str, build: BuildFn) -> OpSpec {
    OpSpec::new(name, doc, vec![req("x", SIGNAL)], Build::Builtin(build))
}

pub struct Registry {
    ops: Vec<OpSpec>,
}

impl Registry {
    pub fn builtin() -> Self {
        let ops = vec![
            OpSpec::new(
                "finish_when",
                "Silence this layer permanently when done becomes positive. After release, the voice waits for every layer and downstream tail to finish.",
                vec![req("x", SIGNAL), req("done", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(FinishWhen::default()))),
            ),
            OpSpec::new(
                "osc",
                "An oscillator: `osc(saw, 440hz)`. The waveform is sine, saw, square or tri, a table (`harmonics([...])`), or a `bank([...])` of tables; `phase_mod` bends the phase for FM; `position` morphs a bank.",
                vec![
                    req("wave", Kind::Wave),
                    req("freq", Kind::Signal(Unit::Hz)),
                    opt("phase_mod", SIGNAL),
                    opt("position", SIGNAL),
                ],
                Build::Builtin(|a| {
                    Ok(Box::new(match a.wave.clone().expect("osc has a wave") {
                        WaveArg::Builtin(wave) => Osc::new(wave),
                        WaveArg::Table(table) => Osc::with_table(table),
                        WaveArg::Bank(bank) => Osc::with_bank(bank),
                    }))
                }),
            ),
            filter("lpf", "A low-pass filter: lets frequencies below `cutoff` through. `res` adds resonance.", |_| Ok(Box::new(Svf::new(FilterMode::Low)))),
            filter("hpf", "A high-pass filter: lets frequencies above `cutoff` through. `res` adds resonance.", |_| Ok(Box::new(Svf::new(FilterMode::High)))),
            filter("bpf", "A band-pass filter: lets frequencies around `cutoff` through. `res` narrows the band.", |_| Ok(Box::new(Svf::new(FilterMode::Band)))),
            filter("notch", "A notch filter: removes frequencies around `cutoff`. `res` narrows the notch.", |_| Ok(Box::new(Svf::new(FilterMode::Notch)))),
            OpSpec::new(
                "gain",
                "Scales a signal by a gain in db or a plain factor.",
                vec![req("x", SIGNAL), req("gain", Kind::Gain)],
                Build::Builtin(|_| Ok(Box::new(Gain))),
            ),
            OpSpec::new(
                "delay",
                "A delay line. `time` is how long to wait, `feedback` feeds the output back in. A changing `time` needs `max:` to size the line (at most 30 seconds).",
                vec![
                    req("x", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("feedback", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| {
                    let max = a.consts.get("max").or_else(|| a.consts.get("time")).copied().ok_or_else(|| {
                        "`delay` needs a constant `time`, or a `max:` that bounds a changing one".to_string()
                    })?;
                    if max > MAX_DELAY_SECONDS {
                        return Err(format!("a delay can be at most {MAX_DELAY_SECONDS} seconds long"));
                    }
                    Ok(Box::new(Delay::new(max.max(0.0) as f32)))
                }),
            ),
            OpSpec::new(
                "reverb",
                "A reverb: `room` sets its size and `damp` how quickly the highs die away.",
                vec![req("x", SIGNAL), opt("room", SIGNAL), opt("damp", SIGNAL)],
                Build::Builtin(|a| Ok(Box::new(Reverb::for_channel(a.channel)))),
            ),
            OpSpec::new(
                "noise",
                "White noise in -1..1. No arguments: `noise()`, or `noise() * env[1 5ms 0]` for a click.",
                vec![],
                Build::Builtin(|_| Ok(Box::new(Noise::white()))),
            ),
            OpSpec::new(
                "pinknoise",
                "Pink noise (1/f). Quieter than `noise`, useful for hats and textures.",
                vec![],
                Build::Builtin(|_| Ok(Box::new(Noise::pink()))),
            ),
            OpSpec::new(
                "compress",
                "A compressor. `threshold` and `makeup` take db or a plain factor; `ratio`, `attack` and `release` shape the envelope. Pass `sidechain:` to key off another signal (for ducking).",
                vec![
                    req("x", SIGNAL),
                    opt("threshold", Kind::Gain),
                    opt("ratio", SIGNAL),
                    opt("attack", Kind::Signal(Unit::Time)),
                    opt("release", Kind::Signal(Unit::Time)),
                    opt("makeup", Kind::Gain),
                    opt("sidechain", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Compressor::default()))),
            ),
            OpSpec::new(
                "wavefold",
                "Wavefolds a signal: louder input folds back and adds harmonics. `drive` is how hard it folds (1 at ±1).",
                vec![req("x", SIGNAL), opt("drive", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(Fold))),
            ),
            OpSpec::new(
                "clip",
                "Hard-clips a signal to ±`level` (a plain factor, default 1).",
                vec![req("x", SIGNAL), opt("level", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(Clip))),
            ),
            OpSpec::new(
                "chorus",
                "A chorus (or flanger, with more `depth` and less `time`). `rate` is the LFO in hz; `mix` blends wet with dry.",
                vec![
                    req("x", SIGNAL),
                    opt("rate", Kind::Signal(Unit::Hz)),
                    opt("depth", SIGNAL),
                    opt("time", Kind::Signal(Unit::Time)),
                    opt("mix", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Chorus::default()))),
            ),
            OpSpec::new(
                "bitcrush",
                "Reduces sample rate and bit depth. `rate` is how often a new sample is taken (hz); `bits` is the bit depth.",
                vec![
                    req("x", SIGNAL),
                    opt("rate", Kind::Signal(Unit::Hz)),
                    opt("bits", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Bitcrush::default()))),
            ),
            OpSpec::new(
                "pan",
                "Places a signal in the master layout. `azimuth` is in degrees (0 is straight ahead, positive is right); `spread` widens it.",
                vec![req("x", SIGNAL), opt("azimuth", Kind::Signal(Unit::Angle)), opt("spread", SIGNAL)],
                Build::Pan,
            ),
            OpSpec::new(
                "to_layout",
                "Converts a signal to a channel layout such as `stereo`.",
                vec![req("x", SIGNAL), req("layout", Kind::Layout)],
                Build::ToLayout,
            ),
            math("sin", "Sine of a signal (radians).", |_| Ok(Box::new(Func(Fn1::Sin)))),
            math("cos", "Cosine of a signal (radians).", |_| Ok(Box::new(Func(Fn1::Cos)))),
            math("tanh", "Soft-clips a signal into -1..1; a classic saturator.", |_| Ok(Box::new(Func(Fn1::Tanh)))),
            math("exp", "e raised to a signal.", |_| Ok(Box::new(Func(Fn1::Exp)))),
            math("abs", "Absolute value of a signal.", |_| Ok(Box::new(Func(Fn1::Abs)))),
            math("tan", "Tangent of a signal (radians).", |_| Ok(Box::new(Func(Fn1::Tan)))),
            math("floor", "The whole number at or below a signal.", |_| Ok(Box::new(Func(Fn1::Floor)))),
            math("ceil", "The whole number at or above a signal.", |_| Ok(Box::new(Func(Fn1::Ceil)))),
            math("sqrt", "Square root of a signal (0 for a negative one).", |_| Ok(Box::new(Func(Fn1::Sqrt)))),
            math("log", "Natural logarithm of a signal.", |_| Ok(Box::new(Func(Fn1::Log)))),
            math("sign", "1 for a positive signal, -1 for a negative one, 0 for zero.", |_| Ok(Box::new(Func(Fn1::Sign)))),
            math("fract", "The part of a signal after the point: `x - floor(x)`.", |_| Ok(Box::new(Func(Fn1::Fract)))),
        ];
        Registry { ops }
    }

    pub fn find(&self, name: &str) -> Option<&OpSpec> {
        self.ops.iter().find(|o| o.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.ops.iter().map(|o| o.name.as_str())
    }
}

pub const WAVES: &[(&str, Wave)] = &[
    ("sine", Wave::Sine),
    ("saw", Wave::Saw),
    ("square", Wave::Square),
    ("tri", Wave::Tri),
];

pub fn wave(name: &str) -> Option<Wave> {
    WAVES.iter().find(|(n, _)| *n == name).map(|(_, w)| *w)
}

/// The one-argument math functions.
pub fn math_fn(name: &str) -> Option<Fn1> {
    Fn1::ALL.into_iter().find(|f| f.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_cycle_is_analysed_once_while_it_is_in_use() {
        let cycle = [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5, 0.123];
        let (WaveArg::Table(a), WaveArg::Table(b)) = (WaveArg::table(&cycle).unwrap(), WaveArg::table(&cycle).unwrap()) else {
            panic!("tables")
        };
        assert!(Arc::ptr_eq(&a, &b));
        let WaveArg::Table(other) = WaveArg::table(&cycle[..8]).unwrap() else { panic!("table") };
        assert!(!Arc::ptr_eq(&a, &other));
    }
}
