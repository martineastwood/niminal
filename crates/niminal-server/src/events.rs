//! The engine's event API: timestamped things a client asks the audio server to do.

use std::collections::BTreeMap;

use niminal_score::{Time, Value};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

/// When something happens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum At {
    /// An absolute sample.
    Sample(u64),
    /// As soon as possible.
    Now,
}

impl<'de> Deserialize<'de> for At {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        match &value {
            serde_json::Value::Number(n) => n
                .as_u64()
                .map(At::Sample)
                .ok_or_else(|| de::Error::custom("`at` as a number is a whole sample count")),
            serde_json::Value::String(s) if s == "now" => Ok(At::Now),
            _ => Err(de::Error::custom(
                "`at` must be an absolute sample count or \"now\"",
            )),
        }
    }
}

impl Serialize for At {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            At::Sample(n) => s.serialize_u64(*n),
            At::Now => s.serialize_str("now"),
        }
    }
}

/// One thing to do, as it comes in an `events.send` batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Compile definitions and edit routing as one graph change, with no intermediate graph.
    Transaction {
        at: At,
        #[serde(default)]
        source: String,
        #[serde(default)]
        dir: Option<String>,
        #[serde(default)]
        edits: Vec<crate::RoutingEdit>,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Atomically edit this connection's processor instances, buses and audio connections.
    Routing {
        at: At,
        edits: Vec<crate::RoutingEdit>,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Map a parameter to a control or a block-delayed bus; null disconnects it.
    Map {
        id: u64,
        at: At,
        param: String,
        #[serde(default)]
        source: Option<String>,
        #[serde(default = "first_channel")]
        channel: usize,
        #[serde(default = "unity")]
        scale: f32,
        #[serde(default)]
        offset: f32,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Replace a fixed-shape array control on a held note.
    Array {
        id: u64,
        at: At,
        param: String,
        values: Vec<Value>,
        #[serde(default)]
        glide: Option<Time>,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Select a prepared shared buffer for a held note.
    BindBuffer {
        id: u64,
        at: At,
        param: String,
        buffer: String,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Replace a range of a declared shared buffer, before notes at this sample.
    BufferWrite {
        name: String,
        at: At,
        #[serde(default)]
        offset: usize,
        values: Vec<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Play a note on a track or instrument. With `dur` and no `id` it is fire-and-forget. With an `id` and
    /// no `dur` it is held until an `off`. With both it is held for at most `dur`.
    Note {
        target: String,
        /// Optional collection/processor node, scoped to this connection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        at: At,
        #[serde(default)]
        args: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dur: Option<Time>,
        /// Chosen by the client, scoped to its connection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Change parameters of a sounding or waiting note.
    Set {
        id: u64,
        at: At,
        args: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glide: Option<Time>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Compile definitions (instruments, tracks, effects, buses, controls, tables, buffers, functions) and
    /// swap them in at `at`. Performance statements are an error. `dir` is where sample files and
    /// modules named in `source` are looked for.
    Define {
        source: String,
        at: At,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Silence everything at `at`: every voice, and the memory of every effect. Events waiting for later
    /// still play.
    Panic {
        at: At,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Drop every definition. The output layout is kept.
    Forget {
        at: At,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Release a note, so its envelope runs its release stage.
    Off {
        id: u64,
        at: At,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Move a `ctl`.
    Control {
        name: String,
        at: At,
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glide: Option<Time>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Set parameters of a track.
    Track {
        name: String,
        at: At,
        args: BTreeMap<String, Value>,
        /// How long to take getting there; the parameter's own smoothing if not given.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glide: Option<Time>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
}

/// What came of a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    pub notes: Vec<NoteReceipt>,
    pub transactions: Vec<TransactionReceipt>,
    pub accepted: usize,
    /// Events whose time had passed: they sounded at once.
    pub late: usize,
    /// The earliest sample any of them was resolved to.
    pub first: Option<u64>,
}

/// What a cancellation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cancelled {
    /// Waiting events dropped.
    pub cancelled: usize,
    /// Held notes already sounding that were let go of.
    pub released: usize,
}

fn first_channel() -> usize {
    1
}
fn unity() -> f32 {
    1.0
}

/// An accepted graph transaction and its effective scheduled sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionReceipt {
    pub id: u64,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteReceipt {
    pub event: usize,
    pub token: u64,
}
