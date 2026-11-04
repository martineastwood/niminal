//! Clip, scene and project definitions and the `.nms` files they live in: the JSON schema on the wire
//! and in `.nms` v2 files, plus the canonical `.nml` text they compile to.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use niminal_score::{Event, Time, Value};

/// The `.nms` format version this crate reads and writes.
pub const VERSION: u32 = 1;

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
    Message(String),
}

impl fmt::Display for ScoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScoreError::Json(e) => write!(f, "invalid score: {e}"),
            ScoreError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "score format version {v} is not supported (this build reads version {VERSION})"
                )
            }
            ScoreError::Message(m) => f.write_str(m),
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
        // the version first, so a file of another format is told so rather than for its first unknown field
        let value: serde_json::Value = serde_json::from_str(json)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ScoreError::Message("score needs a `version` field".into()))?;
        if version != u64::from(VERSION) {
            return Err(ScoreError::UnsupportedVersion(version as u32));
        }
        let file: File = serde_json::from_value(value)?;
        Ok(Section {
            name: file.section,
            length: file.length,
            events: file.events,
        })
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
        assert!(matches!(
            Section::from_json(&v2),
            Err(ScoreError::UnsupportedVersion(2))
        ));

        let no_version = PHI.replace("\"version\": 1,", "");
        assert!(Section::from_json(&no_version).is_err());

        let typo = PHI.replace("\"dur\": \"1/4beat\"", "\"duration\": \"1/4beat\"");
        assert!(
            Section::from_json(&typo).is_err(),
            "unknown event fields are errors"
        );

        let bad_unit = PHI.replace("\"1/4beat\", \"target\"", "\"440hz\", \"target\"");
        let e = Section::from_json(&bad_unit).unwrap_err().to_string();
        assert!(e.contains("expected a time"), "{e}");
    }
}

/// One note inside a clip: a value (pitch, sample name, …) at a time, for a length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipEvent {
    pub at: Time,
    pub dur: Time,
    /// The note, sample name, or other step value (`"c4"`, `"kick"`, `0.5`).
    pub value: Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, Value>,
}

/// A clip as data: lanes of mini-notation, and/or a finite list of events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    /// Length in bars (or other time). Omitted means one bar, endless when `loop` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Time>,
    /// When true (the default) the clip repeats. A finite event list with
    /// `loop: false` plays once.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub r#loop: bool,
    /// Mini-notation text per lane (`"notes": "[c4 e4 g4]"`, `"gain": "[0db -6db]"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lanes: BTreeMap<String, String>,
    /// Finite timed events on the clip's timeline (tracker-style).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ClipEvent>,
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

/// A named list of clips, started and stopped together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDef {
    pub name: String,
    pub clips: Vec<String>,
}

impl ClipDef {
    /// Canonical `.nml` text for this clip.
    pub fn to_source(&self) -> String {
        let mut lines = Vec::new();
        if let Some(instr) = &self.instrument {
            lines.push(format!("  instrument: {instr}"));
        }
        if let Some(length) = &self.length {
            lines.push(format!("  length: {length}"));
        }
        if !self.r#loop {
            lines.push("  times: 1".into());
        }
        for (lane, text) in &self.lanes {
            let body = if text.trim_start().starts_with('[')
                || text.trim_start().starts_with("grid")
                || text.trim_start().starts_with("env")
            {
                text.clone()
            } else {
                format!("[{text}]")
            };
            lines.push(format!("  {lane}: {body}"));
        }
        if !self.events.is_empty() {
            lines.push("  events {".into());
            for e in &self.events {
                lines.push(format!("    {}", event_line(e)));
            }
            lines.push("  }".into());
        }
        if lines.is_empty() {
            format!("clip {} {{}}", self.name)
        } else {
            format!("clip {} {{\n{}\n}}", self.name, lines.join("\n"))
        }
    }
}

fn event_line(e: &ClipEvent) -> String {
    let mut s = format!("{} {} for {}", e.at, e.value, e.dur);
    for (k, v) in &e.args {
        s.push_str(&format!(" {k}: {v}"));
    }
    s
}

impl SceneDef {
    /// Canonical `.nml` text for this scene.
    pub fn to_source(&self) -> String {
        if self.clips.is_empty() {
            format!("scene {} {{}}", self.name)
        } else {
            format!("scene {} {{ {} }}", self.name, self.clips.join(", "))
        }
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;

    #[test]
    fn clip_to_source_writes_lanes_and_events() {
        let mut lanes = BTreeMap::new();
        lanes.insert("notes".into(), "c4 e4 g4".into());
        lanes.insert("gain".into(), "[0db -6db]".into());
        let clip = ClipDef {
            name: "riff".into(),
            instrument: Some("pluck(bright: 0.5)".into()),
            length: Some(Time::Bars(2.0)),
            r#loop: true,
            lanes,
            events: vec![ClipEvent {
                at: Time::Beats(0.0),
                dur: Time::Beats(0.5),
                value: Value::Note(60),
                args: BTreeMap::from([("gain".into(), Value::Db(-6.0))]),
            }],
        };
        let src = clip.to_source();
        assert!(src.contains("clip riff {"), "{src}");
        assert!(src.contains("instrument: pluck(bright: 0.5)"), "{src}");
        assert!(src.contains("length: 2bars"), "{src}");
        assert!(src.contains("notes: [c4 e4 g4]"), "{src}");
        assert!(src.contains("gain: [0db -6db]"), "{src}");
        assert!(src.contains("events {"), "{src}");
        assert!(
            src.contains("0beats c4 for 1/2beat gain: -6db")
                || src.contains("0beats c4 for 0.5beats gain: -6db"),
            "{src}"
        );
    }

    #[test]
    fn a_scene_is_written_as_code() {
        let scene = SceneDef {
            name: "verse".into(),
            clips: vec!["bassline".into(), "drums".into()],
        };
        assert!(scene.to_source().contains("scene verse { bassline, drums }"), "{}", scene.to_source());
    }

    #[test]
    fn only_version_1_sections_are_read() {
        let v1 = r#"{
            "version": 1,
            "section": "phi",
            "length": "4beats",
            "events": [{ "at": "0beat", "dur": "1beat", "target": "lead" }]
        }"#;
        assert_eq!(Section::from_json(v1).unwrap().name, "phi");
        let v2 = r#"{ "version": 2, "clips": [{ "name": "riff", "lanes": { "notes": "[c4]" } }], "scenes": [] }"#;
        let e = Section::from_json(v2).unwrap_err().to_string();
        assert!(e.contains("version 2"), "{e}");
    }
}
