//! The niminal composition language: clips, scenes, patterns, commands and notes. A composition holds
//! the sound-server definitions that `niminal-lang` compiles and everything else, which becomes the timed
//! events a client sends to a server.

mod analysis;
mod ast;
mod compile;
mod dsp;
mod ide;
mod lexer;
mod literal;
mod modules;
mod parser;
mod perform;
mod performance;
mod rhythm_gate;
mod schedule;
mod score_file;
#[cfg(test)]
mod score_prog;
#[cfg(test)]
mod tests;

pub use analysis::{
    CommandAction, PlayInfo, Ramp, Statement, StatementData, StatementKind, analyze, scene_clips,
};
pub use ast::{ControlInput, InputSource};
pub use compile::{
    Composition, ControlBinding, MacroDef, MacroTarget, QuantizeConfig, Trigger, compile, compile_with, interpolate,
};
pub use dsp::{LIVE_BPM, LIVE_METER, LIVE_ORIGIN, dsp_source, resolve_musical_time, warnings};
pub use ide::{RESERVED_WORDS, Symbol, SymbolKind, SymbolParam, pattern_ops, symbols};
pub use lexer::{Tok, Token, lex};
pub use modules::expand;
pub use perform::PITCH_PARAM;
pub use performance::{
    Action, Arrangement, Clip, EnvLane, Keys, LaneEvent, LanePreview, Quantize, QuantizeRelation,
    QuantizeUnit, Scene, Scheduled,
};
pub use schedule::{ControlSet, Finish, Found, PartState, Schedule, ScheduleError};
pub use score_file::{
    ArrangementDef, ArrangementSection, ClipDef, ClipEvent, Project, SceneDef, ScoreError,
    ScoreFile, Section, VERSION, VERSION_2,
};
