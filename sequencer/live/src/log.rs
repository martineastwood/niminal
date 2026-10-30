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

/// What was sent between samples `from` and `to` (the end is not included) as a script to read or keep: each
/// evaluation under a comment saying how far into the stretch it was sent, and when it was set to land.
pub fn transcript(entries: &[LiveLogEntry], from: u64, to: u64, sample_rate: f32) -> String {
    let mut out = String::new();
    for entry in entries.iter().filter(|e| (from..to).contains(&e.sample)) {
        let seconds = ((entry.sample - from) as f64 / f64::from(sample_rate)) as u64;
        let when = format!("{}:{:02}", seconds / 60, seconds % 60);
        match &entry.input {
            LiveInput::Eval { source, quantize } => {
                out.push_str(&format!("// {when}"));
                if let Some(quantize) = quantize {
                    out.push_str(&format!(" · {quantize}"));
                }
                out.push_str(&format!("\n{}\n\n", source.trim_end()));
            }
            LiveInput::Panic => out.push_str(&format!("// {when} · panic\n\n")),
            LiveInput::Cancel { .. } => {
                out.push_str(&format!("// {when} · cancelled a change\n\n"))
            }
            LiveInput::Key { .. } | LiveInput::Expression { .. } => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(sample: u64, source: &str, quantize: Option<&str>) -> LiveLogEntry {
        LiveLogEntry {
            sample,
            input: LiveInput::Eval {
                source: source.into(),
                quantize: quantize.map(Into::into),
            },
        }
    }

    #[test]
    fn a_transcript_holds_what_was_sent_in_the_stretch_with_when() {
        let log = vec![
            eval(0, "before.start", None),
            eval(100, "bass.start\n", Some("next bar")),
            LiveLogEntry {
                sample: 100 + 75 * 10,
                input: LiveInput::Panic,
            },
            LiveLogEntry {
                sample: 150,
                input: LiveInput::Key {
                    channel: 0,
                    note: 60,
                    velocity: 90,
                },
            },
            eval(1000, "after.start", None),
        ];
        assert_eq!(
            transcript(&log, 100, 1000, 10.0),
            "// 0:00 · next bar\nbass.start\n\n// 1:15 · panic\n\n"
        );
    }
}
