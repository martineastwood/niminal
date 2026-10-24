//! The niminal DSP language: lex, parse, unit-check and compile instruments, effects, nodes and buses
//! to engine graphs. Sequencing (clips, scenes, patterns, tempo) is not part of it: a client does that
//! and sends timestamped events.

mod analysis;
mod ast;
mod call;
mod compile;
mod diag;
mod dsp;
pub use dsp::{dsp_value, validate_dsp};
mod env_shortcut;
mod explain;
mod funcs;
mod ide;
mod kernel;
mod latency;
mod layout;
mod lexer;
mod lower;
mod modules;
mod namespace;
mod opcodes;
mod parser;
mod prelude;
mod program;
mod project;
pub use project::{Entry, Located, Origin, Piece, Problem, Project};
mod sample;
mod tables;
mod unit;

pub use analysis::{Statement, analyze};

pub use compile::{CompileOptions, compile, compile_with};
pub use diag::{Diagnostic, Span, closest};
pub use ide::{
    OpInfo, ParamInfo, RESERVED_WORDS, Symbol, SymbolKind, SymbolParam, builtin_ops, symbols, waves,
};
pub use layout::Layout;
pub use lexer::{Tok, Token, lex};
pub use lower::{GAIN_PARAM, unknown_unit};
pub use modules::{
    Expanded, Import, Module, ModuleRules, STD_DIR, check_private_words,
    embedded as embedded_module, expand, expand_with_rules, packages_dir,
};
pub use program::{
    ArgError, ControlDef, Instrument, MidiNote, MidiTarget, NodeInfo, NotePlan, OscTap,
    PITCH_PARAM, Param, Program, RateHint, engine_value, resolve_target,
};
pub use sample::Samples;
pub use unit::Unit;

pub use niminal_engine::ops::detect_transients;
