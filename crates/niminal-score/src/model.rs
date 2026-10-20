use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::value::{Time, Value};

/// One note: an instrument to play, when, for how long, and its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub target: String,
    pub at: Time,
    pub dur: Time,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, Value>,
}
