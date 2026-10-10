//! The niminal language: lex, parse, unit-check, and lower to engine graphs and
//! score events.

mod analysis;
mod ast;
mod compile;
mod diag;
mod ide;
mod kernel;
mod layout;
mod lexer;
mod lower;
mod opcodes;
mod parser;
mod perform;
mod performance;
mod program;
mod sample;
mod schedule;
mod unit;

pub use analysis::{PlayInfo, Statement, StatementKind, analyze};
pub use compile::{CompileOptions, compile, compile_with};
pub use diag::{Diagnostic, Span, closest};
pub use ide::{OpInfo, ParamInfo, Symbol, SymbolKind, SymbolParam, builtin_ops, symbols, waves};
pub use performance::{Action, Arrangement, Clip, Quantize, QuantizeRelation, QuantizeUnit, Scene, Scheduled};
pub use schedule::{Found, Schedule, ScheduleError, TrackState};
pub use ast::{ControlInput, InputSource};
pub use program::{ControlBinding, ControlDef, ArgError, Instrument, NotePlan, Param, Program, TrackInfo};
pub use layout::Layout;
pub use sample::Samples;
pub use unit::Unit;
