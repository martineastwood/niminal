//! The language's view of opcodes, built-in and user-defined: names, argument
//! order, and the unit each argument expects. Argument names match the engine's
//! port names.

use std::collections::HashMap;
use std::sync::Arc;

use niminal_engine::ops::{Custom, Delay, Fn1, FilterMode, Func, Gain, Kernel, Osc, Reverb, Svf};
pub use niminal_engine::ops::Wave;
use niminal_engine::Opcode;

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
    ParamSpec { name: name.into(), kind, required: true, wired: true }
}

fn opt(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec { name: name.into(), kind, required: false, wired: true }
}

fn setting(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec { name: name.into(), kind, required: false, wired: false }
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
    Custom(Arc<Kernel>),
    /// `pan`: one output per speaker of the master layout.
    Pan,
    /// `to_layout`: converts between layouts.
    ToLayout,
    /// A user-defined opcode whose body had errors. Calls to it are still
    /// checked against its signature, so one mistake isn't reported twice.
    Invalid,
}

pub struct OpSpec {
    pub name: String,
    pub params: Vec<ParamSpec>,
    /// How many leading arguments may be given without a name. The receiver of
    /// a method call counts as the first.
    pub positional: usize,
    pub build: Build,
}

impl OpSpec {
    /// `Ok(None)` for an invalid opcode, which has nothing to run.
    pub fn instantiate(&self, args: &BuildArgs) -> Result<Option<Box<dyn Opcode>>, String> {
        match &self.build {
            Build::Builtin(f) => f(args).map(Some),
            Build::Custom(k) => Ok(Some(Box::new(Custom::new(k.clone())))),
            Build::Invalid | Build::Pan | Build::ToLayout => Ok(None),
        }
    }
}

const SIGNAL: Kind = Kind::Signal(Unit::Num);

fn filter_params() -> Vec<ParamSpec> {
    vec![req("x", SIGNAL), req("cutoff", Kind::Signal(Unit::Hz)), opt("res", SIGNAL)]
}

fn filter(name: &str, build: BuildFn) -> OpSpec {
    OpSpec { name: name.into(), params: filter_params(), positional: 1, build: Build::Builtin(build) }
}

fn math(name: &str, build: BuildFn) -> OpSpec {
    OpSpec { name: name.into(), params: vec![req("x", SIGNAL)], positional: 1, build: Build::Builtin(build) }
}

pub struct Registry {
    ops: Vec<OpSpec>,
}

impl Registry {
    pub fn builtin() -> Self {
        let ops = vec![
            OpSpec {
                name: "osc".into(),
                params: vec![req("wave", Kind::Wave), req("freq", Kind::Signal(Unit::Hz)), opt("phase_mod", SIGNAL)],
                positional: 2,
                build: Build::Builtin(|a| Ok(Box::new(Osc::new(a.wave.expect("osc has a wave"))))),
            },
            filter("lpf", |_| Ok(Box::new(Svf::new(FilterMode::Low)))),
            filter("hpf", |_| Ok(Box::new(Svf::new(FilterMode::High)))),
            filter("bpf", |_| Ok(Box::new(Svf::new(FilterMode::Band)))),
            filter("notch", |_| Ok(Box::new(Svf::new(FilterMode::Notch)))),
            OpSpec {
                name: "gain".into(),
                params: vec![req("x", SIGNAL), req("gain", Kind::Gain)],
                positional: 2,
                build: Build::Builtin(|_| Ok(Box::new(Gain))),
            },
            OpSpec {
                name: "delay".into(),
                params: vec![
                    req("x", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("feedback", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                positional: 1,
                build: Build::Builtin(|a| {
                    let max = a.consts.get("max").or_else(|| a.consts.get("time")).copied().ok_or_else(|| {
                        "`delay` needs a constant `time`, or a `max:` that bounds a changing one".to_string()
                    })?;
                    if max > MAX_DELAY_SECONDS {
                        return Err(format!("a delay can be at most {MAX_DELAY_SECONDS} seconds long"));
                    }
                    Ok(Box::new(Delay::new(max.max(0.0) as f32)))
                }),
            },
            OpSpec {
                name: "reverb".into(),
                params: vec![req("x", SIGNAL), opt("room", SIGNAL), opt("damp", SIGNAL)],
                positional: 1,
                build: Build::Builtin(|a| Ok(Box::new(Reverb::for_channel(a.channel)))),
            },
            OpSpec {
                name: "pan".into(),
                params: vec![req("x", SIGNAL), opt("azimuth", Kind::Signal(Unit::Angle)), opt("spread", SIGNAL)],
                positional: 1,
                build: Build::Pan,
            },
            OpSpec {
                name: "to_layout".into(),
                params: vec![req("x", SIGNAL), req("layout", Kind::Layout)],
                positional: 2,
                build: Build::ToLayout,
            },
            math("sin", |_| Ok(Box::new(Func(Fn1::Sin)))),
            math("cos", |_| Ok(Box::new(Func(Fn1::Cos)))),
            math("tanh", |_| Ok(Box::new(Func(Fn1::Tanh)))),
            math("exp", |_| Ok(Box::new(Func(Fn1::Exp)))),
            math("abs", |_| Ok(Box::new(Func(Fn1::Abs)))),
        ];
        Registry { ops }
    }

    pub fn find(&self, name: &str) -> Option<&OpSpec> {
        self.ops.iter().find(|o| o.name == name)
    }

    pub fn add(&mut self, spec: OpSpec) {
        self.ops.push(spec);
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

/// The math functions that can be used inside an `opcode` body.
pub fn math_fn(name: &str) -> Option<Fn1> {
    [Fn1::Sin, Fn1::Cos, Fn1::Tanh, Fn1::Exp, Fn1::Abs].into_iter().find(|f| f.name() == name)
}
