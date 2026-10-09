//! The niminal language: lex, parse, unit-check, and lower to engine graphs and
//! score events.

mod analysis;
mod ast;
mod compile;
mod diag;
mod kernel;
mod layout;
mod lexer;
mod lower;
mod opcodes;
mod parser;
mod perform;
mod performance;
mod program;
mod schedule;
mod unit;

pub use analysis::{PlayInfo, Statement, StatementKind, analyze};
pub use compile::{CompileOptions, compile, compile_with};
pub use diag::{Diagnostic, Span};
pub use performance::{Action, Clip, Quantize, QuantizeRelation, QuantizeUnit, Scene, Scheduled};
pub use schedule::{Schedule, ScheduleError};
pub use program::{ArgError, Instrument, NotePlan, Param, Program, TrackInfo};
pub use layout::Layout;
pub use unit::Unit;
