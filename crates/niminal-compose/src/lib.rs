//! The niminal composition language: clips, scenes, patterns, commands and notes. A composition holds
//! the sound-server definitions that `niminal-lang` compiles and everything else, which becomes the timed
//! events a client sends to a server.

mod analysis;
mod compile;
pub mod drums;
mod dsp;
mod ide;
mod literal;
mod modules;
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
pub use compile::{
    Composition, ControlBinding, MacroDef, MacroTarget, QuantizeConfig, Trigger, compile,
    compile_with, interpolate,
};
pub use dsp::{
    BeatControl, beat_controls, division_controls, dsp_source, resolve_musical_time, warnings,
};
pub use ide::{RESERVED_WORDS, Symbol, SymbolKind, SymbolParam, pattern_ops, symbols};
pub use modules::expand;
pub use niminal_lang::ast::{ControlInput, InputSource};
pub use niminal_lang::{Tok, Token, lex};
pub use perform::PITCH_PARAM;
pub use performance::{
    Action, Arrangement, Clip, EnvLane, Keys, LaneEvent, LanePreview, Quantize, QuantizeRelation,
    QuantizeUnit, Scene, Scheduled, SectionMark,
};
pub use schedule::{ControlSet, Finish, Found, PartState, Schedule, ScheduleError};
pub use score_file::{
    ClipDef, ClipEvent, SceneDef, ScoreError, Section, VERSION,
};
