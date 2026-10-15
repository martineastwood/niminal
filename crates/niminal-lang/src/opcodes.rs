//! The language's view of built-in opcodes: names, argument order, and the unit
//! each argument expects. Argument names match the engine's port names. User
//! functions are compiled where they are called (see `lower` and `kernel`).

use std::collections::HashMap;

use niminal_engine::Opcode;
pub use niminal_engine::ops::Wave;
use niminal_engine::ops::{FinishWhen, Delay, FilterMode, Fn1, Func, Gain, Osc, Reverb, Svf};

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

/// What is known at compile time when an opcode is built.
#[derive(Default)]
pub struct BuildArgs {
    pub wave: Option<Wave>,
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
                "An oscillator: `osc(saw, 440hz)`. The waveform is sine, saw, square or tri; `phase_mod` bends the phase for FM.",
                vec![req("wave", Kind::Wave), req("freq", Kind::Signal(Unit::Hz)), opt("phase_mod", SIGNAL)],
                Build::Builtin(|a| Ok(Box::new(Osc::new(a.wave.expect("osc has a wave"))))),
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
