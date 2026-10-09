//! The language's view of opcodes, built-in and user-defined: names, argument
//! order, and the unit each argument expects. Argument names match the engine's
//! port names.

use std::sync::Arc;

use niminal_engine::ops::{Custom, Fn1, FilterMode, Func, Gain, Kernel, Osc, Svf, Wave};
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
}

pub struct ParamSpec {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
}

fn req(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec { name: name.into(), kind, required: true }
}

fn opt(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec { name: name.into(), kind, required: false }
}

pub enum Build {
    Builtin(fn(Option<Wave>) -> Box<dyn Opcode>),
    Custom(Arc<Kernel>),
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
    /// `None` for an invalid opcode, which has nothing to run.
    pub fn instantiate(&self, wave: Option<Wave>) -> Option<Box<dyn Opcode>> {
        match &self.build {
            Build::Builtin(f) => Some(f(wave)),
            Build::Custom(k) => Some(Box::new(Custom::new(k.clone()))),
            Build::Invalid => None,
        }
    }
}

const SIGNAL: Kind = Kind::Signal(Unit::Num);

fn filter_params() -> Vec<ParamSpec> {
    vec![req("x", SIGNAL), req("cutoff", Kind::Signal(Unit::Hz)), opt("res", SIGNAL)]
}

fn filter(name: &str, build: fn(Option<Wave>) -> Box<dyn Opcode>) -> OpSpec {
    OpSpec { name: name.into(), params: filter_params(), positional: 1, build: Build::Builtin(build) }
}

fn math(name: &str, build: fn(Option<Wave>) -> Box<dyn Opcode>) -> OpSpec {
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
                build: Build::Builtin(|w| Box::new(Osc::new(w.expect("osc has a wave")))),
            },
            filter("lpf", |_| Box::new(Svf::new(FilterMode::Low))),
            filter("hpf", |_| Box::new(Svf::new(FilterMode::High))),
            filter("bpf", |_| Box::new(Svf::new(FilterMode::Band))),
            filter("notch", |_| Box::new(Svf::new(FilterMode::Notch))),
            OpSpec {
                name: "gain".into(),
                params: vec![req("x", SIGNAL), req("gain", Kind::Gain)],
                positional: 2,
                build: Build::Builtin(|_| Box::new(Gain)),
            },
            math("sin", |_| Box::new(Func(Fn1::Sin))),
            math("cos", |_| Box::new(Func(Fn1::Cos))),
            math("tanh", |_| Box::new(Func(Fn1::Tanh))),
            math("exp", |_| Box::new(Func(Fn1::Exp))),
            math("abs", |_| Box::new(Func(Fn1::Abs))),
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
