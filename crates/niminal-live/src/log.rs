//! What was sent to the sequencer and when, in samples of the engine's clock. The engine keeps its own
//! log of the events it was sent, which replays to the same audio; this one says what the user did.

use serde::{Deserialize, Serialize};

/// What a keyboard's expression controller is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expression {
    Bend,
    Pressure,
    /// MPE's third dimension (CC 74), 0 to 1.
    Slide,
    /// Down for a value of 0.5 or more.
    Sustain,
}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveInput {
    Eval {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quantize: Option<String>,
    },
    Cancel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
    },
    Panic,
    Key {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    Expression {
        channel: u8,
        what: Expression,
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveLogEntry {
    pub sample: u64,
    #[serde(flatten)]
    pub input: LiveInput,
}
