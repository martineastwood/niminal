//! The record of everything sent to a session and when, in samples. Replaying
//! it into a fresh session gives back the same audio.

use niminal_score::Event;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogInput {
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
    Control {
        name: String,
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        at: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        smooth_ms: Option<f64>,
    },
    Events {
        events: Vec<Event>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The session clock when it was applied.
    pub sample: u64,
    #[serde(flatten)]
    pub input: LogInput,
}

/// Write a log as one JSON object per line.
pub fn to_lines(log: &[LogEntry]) -> String {
    log.iter().map(|e| serde_json::to_string(e).expect("log entries serialize")).collect::<Vec<_>>().join("\n")
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
            LogEntry { sample: 0, input: LogInput::Eval { source: "hush".into(), quantize: Some("now".into()) } },
            LogEntry { sample: 96_000, input: LogInput::Panic },
            LogEntry { sample: 96_001, input: LogInput::Cancel { id: None } },
        ];
        let text = to_lines(&log);
        assert_eq!(text.lines().count(), 3);
        assert!(text.lines().next().unwrap().contains("\"eval\""));
        assert_eq!(from_lines(&text).unwrap(), log);
        assert!(from_lines("{nope").is_err());
    }
}
