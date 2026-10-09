//! The language's view of the built-in opcodes: names, argument order, and the
//! unit each argument expects. Argument names match the engine's port names.

use niminal_engine::Opcode;
use niminal_engine::ops::{FilterMode, Gain, Osc, Svf, Wave};

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
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
}

const fn req(name: &'static str, kind: Kind) -> ParamSpec {
    ParamSpec { name, kind, required: true }
}

const fn opt(name: &'static str, kind: Kind) -> ParamSpec {
    ParamSpec { name, kind, required: false }
}

pub struct OpSpec {
    pub name: &'static str,
    pub params: &'static [ParamSpec],
    /// How many leading arguments may be given without a name. The receiver of
    /// a method call counts as the first.
    pub positional: usize,
    pub build: fn(Option<Wave>) -> Box<dyn Opcode>,
}

const SIGNAL: Kind = Kind::Signal(Unit::Num);

const FILTER_PARAMS: &[ParamSpec] = &[req("x", SIGNAL), req("cutoff", Kind::Signal(Unit::Hz)), opt("res", SIGNAL)];

pub const OPCODES: &[OpSpec] = &[
    OpSpec {
        name: "osc",
        params: &[req("wave", Kind::Wave), req("freq", Kind::Signal(Unit::Hz)), opt("phase_mod", SIGNAL)],
        positional: 2,
        build: |w| Box::new(Osc::new(w.expect("osc has a wave"))),
    },
    OpSpec { name: "lpf", params: FILTER_PARAMS, positional: 1, build: |_| Box::new(Svf::new(FilterMode::Low)) },
    OpSpec { name: "hpf", params: FILTER_PARAMS, positional: 1, build: |_| Box::new(Svf::new(FilterMode::High)) },
    OpSpec { name: "bpf", params: FILTER_PARAMS, positional: 1, build: |_| Box::new(Svf::new(FilterMode::Band)) },
    OpSpec { name: "notch", params: FILTER_PARAMS, positional: 1, build: |_| Box::new(Svf::new(FilterMode::Notch)) },
    OpSpec {
        name: "gain",
        params: &[req("x", SIGNAL), req("gain", Kind::Gain)],
        positional: 2,
        build: |_| Box::new(Gain),
    },
];

pub fn find(name: &str) -> Option<&'static OpSpec> {
    OPCODES.iter().find(|o| o.name == name)
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
