//! Shared clip, scene and project definitions: the JSON schema on the wire
//! and in `.nms` v2 files, plus the canonical `.nml` text they compile to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Event, ScoreError, Section, VERSION as VERSION_1};
use crate::value::{Time, Value};

/// The project (`.nms` v2) format version.
pub const VERSION_2: u32 = 2;

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

/// What a scene puts on a track: a named clip or an inline clip.
/// Silence is `null` / `None` in the entries map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SceneEntry {
    /// Play the clip of this name.
    Name(String),
    /// An inline clip definition.
    Clip(ClipDef),
}

/// A named set of clips, one per track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDef {
    pub name: String,
    pub entries: BTreeMap<String, Option<SceneEntry>>,
}

/// One scene in an arrangement, and how long it lasts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrangementSection {
    pub scene: String,
    pub length: Time,
}

/// Scenes in order for an offline render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrangementDef {
    pub name: String,
    pub sections: Vec<ArrangementSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempos: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meters: Option<Vec<String>>,
}

/// A v2 `.nms` project: clips, scenes and an optional arrangement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<SceneDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<ArrangementDef>,
}

/// What `from_json` returns: a v1 section or a v2 project.
#[derive(Debug, Clone, PartialEq)]
pub enum ScoreFile {
    Section(Section),
    Project(Project),
}

impl ScoreFile {
    /// Read a `.nms` file: version 1 is a single section, version 2 a project.
    pub fn from_json(json: &str) -> Result<ScoreFile, ScoreError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let version = value
            .get("version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ScoreError::Message("score needs a `version` field".into()))?;
        match version {
            v if v == VERSION_1 as u64 => {
                let file: SectionFile = serde_json::from_value(value)?;
                Ok(ScoreFile::Section(Section {
                    name: file.section,
                    length: file.length,
                    events: file.events,
                }))
            }
            v if v == VERSION_2 as u64 => {
                let file: ProjectFile = serde_json::from_value(value)?;
                Ok(ScoreFile::Project(file.into_project()))
            }
            v => Err(ScoreError::UnsupportedVersion(v as u32)),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionFile {
    version: u32,
    section: String,
    length: Time,
    events: Vec<Event>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    version: u32,
    #[serde(default)]
    clips: Vec<ClipDef>,
    #[serde(default)]
    scenes: Vec<SceneDef>,
    #[serde(default)]
    arrangement: Option<ArrangementDef>,
}

impl ProjectFile {
    fn into_project(self) -> Project {
        Project {
            clips: self.clips,
            scenes: self.scenes,
            arrangement: self.arrangement,
        }
    }
}

impl Project {
    pub fn from_json(json: &str) -> Result<Project, ScoreError> {
        match ScoreFile::from_json(json)? {
            ScoreFile::Project(p) => Ok(p),
            ScoreFile::Section(_) => Err(ScoreError::Message(
                "expected a version 2 project, got a version 1 section".into(),
            )),
        }
    }

    pub fn to_json(&self) -> String {
        let file = ProjectFile {
            version: VERSION_2,
            clips: self.clips.clone(),
            scenes: self.scenes.clone(),
            arrangement: self.arrangement.clone(),
        };
        serde_json::to_string_pretty(&file).expect("a project always serializes")
    }

    /// Canonical `.nml` for every clip, scene and arrangement in the project.
    pub fn to_source(&self) -> String {
        let mut out = String::new();
        for clip in &self.clips {
            out.push_str(&clip.to_source());
            out.push('\n');
        }
        for scene in &self.scenes {
            out.push_str(&scene.to_source());
            out.push('\n');
        }
        if let Some(arr) = &self.arrangement {
            out.push_str(&arr.to_source());
            out.push('\n');
        }
        out
    }
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
            lines.push("  loop: false".into());
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
        let mut parts = Vec::new();
        // Inline clips first, so the scene can name them.
        let mut preamble = String::new();
        for (track, entry) in &self.entries {
            match entry {
                None => parts.push(format!("{track}: ~")),
                Some(SceneEntry::Name(name)) if name == track => parts.push(track.clone()),
                Some(SceneEntry::Name(name)) => parts.push(format!("{track}: {name}")),
                Some(SceneEntry::Clip(clip)) => {
                    preamble.push_str(&clip.to_source());
                    preamble.push('\n');
                    if clip.name == *track {
                        parts.push(track.clone());
                    } else {
                        parts.push(format!("{track}: {}", clip.name));
                    }
                }
            }
        }
        let body = if parts.is_empty() {
            format!("scene {} {{}}", self.name)
        } else {
            format!("scene {} {{ {} }}", self.name, parts.join(", "))
        };
        if preamble.is_empty() {
            body
        } else {
            format!("{preamble}{body}")
        }
    }
}

impl ArrangementDef {
    /// Canonical `.nml` text for this arrangement.
    pub fn to_source(&self) -> String {
        let sections: Vec<String> = self
            .sections
            .iter()
            .map(|s| format!("{}.over({})", s.scene, s.length))
            .collect();
        let mut lines = vec![format!("  sections: [{}]", sections.join(" "))];
        if let Some(tempos) = &self.tempos {
            lines.push(format!("  tempo: [{}]", tempos.join(" ")));
        }
        if let Some(meters) = &self.meters {
            lines.push(format!("  meter: [{}]", meters.join(" ")));
        }
        format!("arrangement {} {{\n{}\n}}", self.name, lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
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
        assert!(src.contains("0beats c4 for 1/2beat gain: -6db") || src.contains("0beats c4 for 0.5beats gain: -6db"), "{src}");
    }

    #[test]
    fn scene_to_source_and_project_round_trip() {
        let project = Project {
            clips: vec![ClipDef {
                name: "bassline".into(),
                instrument: None,
                length: None,
                r#loop: true,
                lanes: BTreeMap::from([("notes".into(), "c2 ~ eb2".into())]),
                events: Vec::new(),
            }],
            scenes: vec![SceneDef {
                name: "verse".into(),
                entries: BTreeMap::from([
                    ("bass".into(), Some(SceneEntry::Name("bassline".into()))),
                    ("lead".into(), None),
                ]),
            }],
            arrangement: Some(ArrangementDef {
                name: "song".into(),
                sections: vec![ArrangementSection {
                    scene: "verse".into(),
                    length: Time::Bars(8.0),
                }],
                tempos: Some(vec!["110bpm".into()]),
                meters: None,
            }),
        };
        let json = project.to_json();
        let back = Project::from_json(&json).unwrap();
        assert_eq!(back, project);
        let src = project.to_source();
        assert!(src.contains("clip bassline"), "{src}");
        assert!(src.contains("scene verse"), "{src}");
        assert!(src.contains("arrangement song"), "{src}");
        assert!(src.contains("verse.over(8bars)"), "{src}");
    }

    #[test]
    fn score_file_reads_v1_and_v2() {
        let v1 = r#"{
            "version": 1,
            "section": "phi",
            "length": "4beats",
            "events": [{ "at": "0beat", "dur": "1beat", "target": "lead" }]
        }"#;
        match ScoreFile::from_json(v1).unwrap() {
            ScoreFile::Section(s) => assert_eq!(s.name, "phi"),
            other => panic!("expected section, got {other:?}"),
        }

        let v2 = r#"{
            "version": 2,
            "clips": [{ "name": "riff", "lanes": { "notes": "[c4]" } }],
            "scenes": []
        }"#;
        match ScoreFile::from_json(v2).unwrap() {
            ScoreFile::Project(p) => assert_eq!(p.clips[0].name, "riff"),
            other => panic!("expected project, got {other:?}"),
        }
    }
}
