//! The niminal language: lex, parse, unit-check, and lower to engine graphs and
//! score events.

mod analysis;
mod ast;
mod compile;
mod diag;
mod env_shortcut;
mod ide;
mod kernel;
mod layout;
mod lexer;
mod lower;
mod modules;
mod opcodes;
mod parser;
mod perform;
mod performance;
mod program;
mod sample;
mod schedule;
mod unit;

pub use analysis::{PlayInfo, Ramp, Statement, StatementKind, analyze, scene_entries};
pub use compile::{CompileOptions, compile, compile_with};
pub use diag::{Diagnostic, Span, closest};
pub use lexer::{Tok, Token, lex};
pub use ide::{OpInfo, ParamInfo, Symbol, SymbolKind, SymbolParam, builtin_ops, pattern_ops, symbols, waves};
pub use performance::{Action, Arrangement, Clip, Keys, Quantize, QuantizeRelation, QuantizeUnit, Scene, Scheduled};
pub use schedule::{ControlSet, Found, Schedule, ScheduleError, TrackState};
pub use ast::{ControlInput, InputSource};
pub use performance::EnvLane;
pub use program::{ControlBinding, ControlDef, ArgError, Instrument, NotePlan, Param, Program, TrackInfo};
pub use layout::Layout;
pub use modules::{Expanded, expand};
pub use sample::Samples;
pub use unit::Unit;
