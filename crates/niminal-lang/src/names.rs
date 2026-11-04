//! The words that cannot be the name of anything you define.
//!
//! Some are refused everywhere a name is made: a constant, function, instrument, clip, parameter, local or
//! loop variable. They are control flow, logic, or a value the language provides.
//!
//! Others begin a line or a declaration at the top level of a file (`tempo`, `clip`, `capture`, ...), so
//! they are refused as the name of a top-level definition but are free inside an instrument, function or
//! effect, where no such line exists: `instr voice(sample)` and a local called `at` are fine.
//!
//! A word that only means something after a dot or an `@` (`start`, `stop`, `mute`, `next`, `last`, ...) is
//! in neither list, so it can be a name. Nor are the unit words (`beat`, `hz`, ...): they are only refused
//! as the name of a function, since `x.beat` converts.

/// Refused everywhere. Kept in step with the first table in `docs/.../reference/reserved-words.md`.
pub const EVERYWHERE: &[&str] = &[
    // control flow
    "if", "else", "match", "for", "in", "after", "spawn", "oversample", "reset_on",
    // logic and rates
    "and", "or", "not", "true", "false", "init", "control", "state", "const", "fn",
    // names the language provides
    "out", "it", "note", "clock", "instrument", "env", "grid", "pi", "sample_rate",
];

/// Refused as the name of a top-level definition. Kept in step with the second table in the docs.
pub const TOP_LEVEL: &[&str] = &[
    // declarations
    "instr", "mono", "def", "bus", "buffer", "sample", "kit", "sfz", "ctl", "macro", "node", "fx",
    "clip", "scene", "arrangement", "config", "use", "as", "midi_out", "events",
    // statements that begin a line
    "tempo", "meter", "at", "on", "hush", "panic", "reset", "new", "unsolo", "capture",
];

/// Whether `name` may not be the name of a top-level definition.
pub fn is_reserved(name: &str) -> bool {
    EVERYWHERE.contains(&name) || TOP_LEVEL.contains(&name)
}

/// Whether `name` may not be a parameter, local or loop variable.
pub fn is_reserved_local(name: &str) -> bool {
    EVERYWHERE.contains(&name)
}
