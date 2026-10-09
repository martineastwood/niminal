//! The niminal language: lex, parse, unit-check, and lower to engine graphs and
//! score events.

mod ast;
mod compile;
mod diag;
mod kernel;
mod lexer;
mod lower;
mod opcodes;
mod parser;
mod program;
mod unit;

pub use compile::compile;
pub use diag::{Diagnostic, Span};
pub use program::{ArgError, Instrument, Param, Program};
pub use unit::Unit;
