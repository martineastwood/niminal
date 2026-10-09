use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::value::{Time, Value};

/// The `.nms` format version this crate reads and writes.
pub const VERSION: u32 = 1;

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

/// A named, finite stretch of events.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub length: Time,
    pub events: Vec<Event>,
}

#[derive(Debug)]
pub enum ScoreError {
    Json(serde_json::Error),
    UnsupportedVersion(u32),
}

impl fmt::Display for ScoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScoreError::Json(e) => write!(f, "invalid score: {e}"),
            ScoreError::UnsupportedVersion(v) => {
                write!(f, "score format version {v} is not supported (this build reads version {VERSION})")
            }
        }
    }
}

impl std::error::Error for ScoreError {}

impl From<serde_json::Error> for ScoreError {
    fn from(e: serde_json::Error) -> Self {
        ScoreError::Json(e)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    section: String,
    length: Time,
    events: Vec<Event>,
}

impl Section {
    pub fn from_json(json: &str) -> Result<Section, ScoreError> {
        let file: File = serde_json::from_str(json)?;
        if file.version != VERSION {
            return Err(ScoreError::UnsupportedVersion(file.version));
        }
        Ok(Section { name: file.section, length: file.length, events: file.events })
    }

    pub fn to_json(&self) -> String {
        let file = File {
            version: VERSION,
            section: self.name.clone(),
            length: self.length,
            events: self.events.clone(),
        };
        serde_json::to_string_pretty(&file).expect("a score always serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHI: &str = r#"{
        "version": 1,
        "section": "phi", "length": "4beats",
        "events": [
          { "at": "0beat", "dur": "1/4beat", "target": "lead", "args": { "freq": "e4", "gain": "-6db" } },
          { "at": "1/4beat", "dur": "250ms", "target": "lead", "args": { "freq": "g4", "bright": 0.5 } },
          { "at": "1beat", "dur": "1beat", "target": "lead" }
        ]
    }"#;

    #[test]
    fn reads_a_score() {
        let s = Section::from_json(PHI).unwrap();
        assert_eq!(s.name, "phi");
        assert_eq!(s.length, Time::Beats(4.0));
        assert_eq!(s.events.len(), 3);
        assert_eq!(s.events[0].args["freq"], Value::Note(64));
        assert_eq!(s.events[1].at, Time::Beats(0.25));
        assert_eq!(s.events[1].dur, Time::Seconds(0.25));
        assert_eq!(s.events[1].args["bright"], Value::Num(0.5));
        assert!(s.events[2].args.is_empty());
    }

    #[test]
    fn round_trips() {
        let s = Section::from_json(PHI).unwrap();
        assert_eq!(Section::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn rejects_other_versions_and_bad_fields() {
        let v2 = PHI.replace("\"version\": 1", "\"version\": 2");
        assert!(matches!(Section::from_json(&v2), Err(ScoreError::UnsupportedVersion(2))));

        let no_version = PHI.replace("\"version\": 1,", "");
        assert!(Section::from_json(&no_version).is_err());

        let typo = PHI.replace("\"dur\": \"1/4beat\"", "\"duration\": \"1/4beat\"");
        assert!(Section::from_json(&typo).is_err(), "unknown event fields are errors");

        let bad_unit = PHI.replace("\"1/4beat\", \"target\"", "\"440hz\", \"target\"");
        let e = Section::from_json(&bad_unit).unwrap_err().to_string();
        assert!(e.contains("expected a time"), "{e}");
    }
}
