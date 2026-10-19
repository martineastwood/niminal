//! The niminal language: lex, parse, unit-check, and lower to engine graphs and
//! score events.

mod analysis;
mod ast;
mod compile;
mod diag;
mod dsp;
pub use dsp::{dsp_source, dsp_value, validate_dsp, LIVE_BPM, LIVE_ORIGIN, LIVE_METER};
mod env_shortcut;
mod funcs;
mod ide;
mod kernel;
mod layout;
mod latency;
mod lexer;
mod lower;
mod modules;
mod opcodes;
mod parser;
mod perform;
mod performance;
mod program;
mod project;
pub use project::{Entry, Located, Origin, Piece, Problem, Project};
mod sample;
mod schedule;
mod tables;
mod unit;

pub use analysis::{
    CommandAction, PlayInfo, Ramp, Statement, StatementData, StatementKind, analyze, scene_entries,
};
pub use ast::{ControlInput, InputSource};
pub use compile::{CompileOptions, compile, compile_with, compile_dsp_with};
pub use diag::{Diagnostic, Span, closest};
pub use ide::{
    OpInfo, ParamInfo, RESERVED_WORDS, Symbol, SymbolKind, SymbolParam, builtin_ops, pattern_ops,
    symbols, waves,
};
pub use layout::Layout;
pub use lexer::{Tok, Token, lex};
pub use modules::{Expanded, expand};
pub use perform::PITCH_PARAM;
pub use performance::EnvLane;
pub use performance::{
    Action, Arrangement, Clip, Keys, Quantize, QuantizeRelation, QuantizeUnit, Scene, Scheduled,
};
pub use program::{
    ArgError, ControlBinding, ControlDef, Instrument, MidiNote, MidiTarget, NotePlan, OscTap,
    Param, Program, QuantizeConfig, TrackInfo,
};
pub use sample::Samples;
pub use schedule::{ControlSet, Found, Schedule, ScheduleError, TrackState};
pub use unit::Unit;

#[cfg(test)]
mod score_prog;
