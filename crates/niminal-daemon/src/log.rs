//! The record of everything sent to the engine and when, in samples. Replaying it into a fresh engine
//! gives back the same audio.

use serde::{Deserialize, Serialize};

use crate::events::Op;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogInput {
    /// A batch of events a client sent, with every timestamp resolved to a sample.
    Send { client: u64, ops: Vec<Op> },
    /// A client took back its waiting events.
    CancelEvents {
        client: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        from: u64,
        release: bool,
    },
    Panic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The engine clock when it was applied.
    pub sample: u64,
    #[serde(flatten)]
    pub input: LogInput,
}

/// Write a log as one JSON object per line.
pub fn to_lines(log: &[LogEntry]) -> String {
    log.iter()
        .map(|e| serde_json::to_string(e).expect("log entries serialize"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn from_lines(text: &str) -> Result<Vec<LogEntry>, serde_json::Error> {
    text.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_round_trip_as_lines() {
        let log = vec![
            LogEntry { sample: 0, input: LogInput::Panic },
            LogEntry {
                sample: 96_001,
                input: LogInput::CancelEvents { client: 1, tag: Some("a".into()), from: 5, release: false },
            },
        ];
        let text = to_lines(&log);
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().next().unwrap().contains("\"panic\""));
        assert_eq!(from_lines(&text).unwrap(), log);
        assert!(from_lines("{nope").is_err());
    }
}
