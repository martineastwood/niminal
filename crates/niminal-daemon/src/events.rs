//! The engine's event API: timestamped things a client asks the sound server to do.

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
    /// A position on the transport, measured from its bar origin: `"32beat"`, `"8.5bar"`, `"12.25sec"`.
    Time(Time),
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
            serde_json::Value::String(s) => s
                .parse::<Time>()
                .map(At::Time)
                .map_err(|e| de::Error::custom(format!("`at` should be a sample, \"now\" or a time such as \"32beat\": {e}"))),
            _ => Err(de::Error::custom("`at` should be a sample, \"now\" or a time such as \"32beat\"")),
        }
    }
}

impl Serialize for At {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            At::Sample(n) => s.serialize_u64(*n),
            At::Now => s.serialize_str("now"),
            At::Time(t) => s.collect_str(t),
        }
    }
}

/// One thing to do, as it comes in an `events.send` batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Play a note on a track or instrument for `dur`.
    Note {
        target: String,
        at: At,
        #[serde(default)]
        args: BTreeMap<String, Value>,
        dur: Time,
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
    },
    /// Set parameters of a track.
    Track {
        name: String,
        at: At,
        args: BTreeMap<String, Value>,
    },
}

/// What came of a batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sent {
    pub accepted: usize,
    /// Events whose time had passed: they sounded at once.
    pub late: usize,
    /// The earliest sample any of them was resolved to.
    pub first: Option<u64>,
}
