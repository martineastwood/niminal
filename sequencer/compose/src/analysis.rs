//! Splitting source into its top-level statements, for a live session that
//! applies a snippet to a running project.

use crate::ast::*;
use niminal_lang::Diagnostic;
use crate::parser::parse_spanned;
use crate::performance::Quantize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    /// Defines something: an instrument, node, clip, binding, setting...
    Definition,
    /// Does something once: `play`, `mute`, `launch`...
    Command,
    /// A note written out: `lead(freq: c4) for 1beat`.
    Note,
}

/// `tempo 124bpm @ next bar over 8bars`: the tempo to reach and how long to take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    pub bpm: f64,
    pub over: niminal_score::Time,
}

/// A command that starts a clip on a node, and the source of what it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayInfo {
    pub node: String,
    pub what: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub text: String,
    pub offset: usize,
    /// The `@ ...` written after the statement, if any.
    pub quantize: Option<Quantize>,
    /// Pending writes to this destination supersede older pending writes.
    /// Explicitly timed score commands and notes are never superseded.
    pub write_key: Option<String>,
    pub data: StatementData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementData {
    Definition {
        /// What it defines, as `instr:name`, `clip:name`, `config`...
        key: String,
        /// The name a pattern, clip or scene definition binds.
        binding: Option<String>,
        /// The `quantize` setting of a definition, which stands in for its kind's usual quantum.
        definition_quantize: Option<Quantize>,
        /// A tempo that is reached gradually rather than at once.
        ramp: Option<Ramp>,
    },
    Command {
        /// Whether the statement says when it happens (`at bar 5 ...`).
        /// At a fixed time on the session's clock. `at +2bars` is not: it is after the code lands.
        has_at: bool,
        /// `new`: a live session forgets what it was sent before this statement.
        clears: bool,
        /// Before join_edit: Play / Edit{plays, range} / Plain; after: may become Launch.
        action: CommandAction,
    },
    Note {
        has_at: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandAction {
    Plain,
    Play(PlayInfo),
    Edit {
        plays: PlayInfo,
        range: std::ops::Range<usize>,
    },
    Launch(String),
}

impl Statement {
    pub fn kind(&self) -> StatementKind {
        match self.data {
            StatementData::Definition { .. } => StatementKind::Definition,
            StatementData::Command { .. } => StatementKind::Command,
            StatementData::Note { .. } => StatementKind::Note,
        }
    }

    pub fn has_at(&self) -> bool {
        match self.data {
            StatementData::Command { has_at, .. } | StatementData::Note { has_at } => has_at,
            StatementData::Definition { .. } => false,
        }
    }

    pub fn clears(&self) -> bool {
        matches!(self.data, StatementData::Command { clears: true, .. })
    }

    pub fn key(&self) -> Option<&str> {
        match &self.data {
            StatementData::Definition { key, .. } => Some(key),
            _ => None,
        }
    }

    pub fn binding(&self) -> Option<&str> {
        match &self.data {
            StatementData::Definition { binding, .. } => binding.as_deref(),
            _ => None,
        }
    }

    pub fn definition_quantize(&self) -> Option<Quantize> {
        match self.data {
            StatementData::Definition {
                definition_quantize,
                ..
            } => definition_quantize,
            _ => None,
        }
    }

    pub fn plays(&self) -> Option<&PlayInfo> {
        match &self.data {
            StatementData::Command {
                action: CommandAction::Play(p) | CommandAction::Edit { plays: p, .. },
                ..
            } => Some(p),
            _ => None,
        }
    }

    pub fn launch(&self) -> Option<&str> {
        match &self.data {
            StatementData::Command {
                action: CommandAction::Launch(name),
                ..
            } => Some(name),
            _ => None,
        }
    }

    pub fn ramp(&self) -> Option<&Ramp> {
        match &self.data {
            StatementData::Definition { ramp, .. } => ramp.as_ref(),
            _ => None,
        }
    }

    /// Take the tempo ramp off a definition, if it has one.
    pub fn take_ramp(&mut self) -> Option<Ramp> {
        match &mut self.data {
            StatementData::Definition { ramp, .. } => ramp.take(),
            _ => None,
        }
    }

    /// Replace the command action, keeping `has_at` / `clears`.
    pub fn set_action(&mut self, action: CommandAction) {
        if let StatementData::Command { action: slot, .. } = &mut self.data {
            *slot = action;
        }
    }
}

#[derive(Default)]
struct DefParts {
    binding: Option<String>,
    definition_quantize: Option<Quantize>,
    ramp: Option<Ramp>,
    quantize: Option<Quantize>,
}

fn definition(text: String, offset: usize, key: String, parts: DefParts) -> Statement {
    Statement {
        text,
        offset,
        quantize: parts.quantize,
        write_key: Some(key.clone()),
        data: StatementData::Definition {
            key,
            binding: parts.binding,
            definition_quantize: parts.definition_quantize,
            ramp: parts.ramp,
        },
    }
}

fn simple_def(text: String, offset: usize, key: String) -> Statement {
    definition(text, offset, key, DefParts::default())
}

fn tempo_ramp_of(t: &TempoStmt) -> Option<Ramp> {
    let (
        Some(over),
        ExprKind::Num {
            value: bpm,
            unit: Some(u),
        },
    ) = (&t.over, &t.value.kind)
    else {
        return None;
    };
    if u != "bpm" {
        return None;
    }
    let ExprKind::Num {
        value,
        unit: Some(unit),
    } = &over.kind
    else {
        return None;
    };
    let length = match unit.as_str() {
        "bar" | "bars" => Some(niminal_score::Time::Bars(*value)),
        "beat" | "beats" => Some(niminal_score::Time::Beats(*value)),
        "sec" => Some(niminal_score::Time::Seconds(*value)),
        _ => None,
    };
    length
        .filter(|l| l.to_seconds(niminal_score::Tempo::new(60.0)) > 0.0)
        .map(|over| Ramp { bpm: *bpm, over })
}

fn command_write_key(c: &CommandStmt) -> Option<String> {
    if c.at.is_some() {
        return None;
    }
    match &c.command {
        Command::Play { node, .. } | Command::Stop(node) => Some(format!("play:{}", node.name)),
        Command::Mute(node) | Command::Unmute(node) => Some(format!("mute:{}", node.name)),
        Command::Solo(_) | Command::Unsolo(_) => Some("solo".into()),
        Command::Hush
        | Command::Panic
        | Command::Reset
        | Command::New
        | Command::Capture { .. }
        | Command::Edit { .. }
        | Command::Start { .. } => None,
    }
}

fn command_action(c: &CommandStmt, source: &str, span_start: usize) -> CommandAction {
    match &c.command {
        Command::Start { name, verb, .. } => CommandAction::Edit {
            plays: PlayInfo {
                node: name.name.clone(),
                what: "start".into(),
            },
            range: name.span.start - span_start..verb.end - span_start,
        },
        Command::Edit { node, change } => CommandAction::Edit {
            plays: PlayInfo {
                node: node.name.clone(),
                what: source[change.start..change.end].to_string(),
            },
            range: node.span.start - span_start..change.end - span_start,
        },
        Command::Play { node, what } => CommandAction::Play(PlayInfo {
            node: node.name.clone(),
            what: source[what.span.start..what.span.end].to_string(),
        }),
        _ => CommandAction::Plain,
    }
}

/// What each part plays in the scene defined by `source`, as written: `None` for `~`.
pub fn scene_entries(source: &str) -> Vec<(String, Option<String>)> {
    let Ok(items) = parse_spanned(source) else {
        return Vec::new();
    };
    items
        .into_iter()
        .find_map(|(item, _)| match item {
            Item::Scene(block) => Some(
                block
                    .entries
                    .iter()
                    .map(|(node, e)| {
                        let what = (!matches!(e.kind, ExprKind::Rest))
                            .then(|| source[e.span.start..e.span.end].to_string());
                        (node.name.clone(), what)
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// Split `source` into statements.
pub fn analyze(source: &str) -> Result<Vec<Statement>, Diagnostic> {
    let mut out = Vec::new();
    for (item, span) in parse_spanned(source)? {
        let text = source[span.start..span.end].to_string();
        let offset = span.start;
        let s = match &item {
            Item::Control { name, .. } => simple_def(text, offset, format!("ctl:{}", name.name)),
            Item::Instr(d) => {
                let dq = d
                    .options
                    .iter()
                    .find(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize"))
                    .and_then(|a| crate::perform::quantum_of(&a.value).ok());
                definition(
                    text,
                    offset,
                    format!("instr:{}", d.name.name),
                    DefParts {
                        definition_quantize: dq,
                        ..Default::default()
                    },
                )
            }
            Item::Fn(d) => simple_def(text, offset, format!("fn:{}", d.name.name)),
            Item::Def(d) => simple_def(text, offset, format!("def:{}", d.name.name)),
            Item::For { .. } | Item::If { .. } => Statement {
                text,
                offset,
                quantize: None,
                write_key: None,
                data: StatementData::Note { has_at: false },
            },
            Item::Const { name, .. } => simple_def(text, offset, format!("const:{}", name.name)),
            Item::MidiOut(d) => simple_def(text, offset, format!("instr:{}", d.name.name)),
            Item::Sample(d) => simple_def(
                text,
                offset,
                format!(
                    "{}:{}",
                    if d.is_kit { "kit" } else { "sample" },
                    d.name.name
                ),
            ),
            Item::Arrangement(d) => {
                simple_def(text, offset, format!("arrangement:{}", d.name.name))
            }
            Item::Bus(d) => simple_def(text, offset, format!("bus:{}", d.name.name)),
            Item::Buffer(d) => simple_def(text, offset, format!("buffer:{}", d.name.name)),
            Item::Processor(d) => {
                let dq = crate::perform::definition_quantum(&d.options)
                    .ok()
                    .flatten();
                definition(
                    text,
                    offset,
                    format!("processor:{}", d.name.name),
                    DefParts {
                        definition_quantize: dq,
                        ..Default::default()
                    },
                )
            }
            Item::Config(_) => simple_def(text, offset, "config".into()),
            Item::Meter(_) => simple_def(text, offset, "meter".into()),
            Item::Tempo(t) => definition(
                text,
                offset,
                "tempo".into(),
                DefParts {
                    ramp: tempo_ramp_of(t),
                    quantize: t.quantize,
                    ..Default::default()
                },
            ),
            Item::Bind { name, .. } => definition(
                text,
                offset,
                format!("bind:{}", name.name),
                DefParts {
                    binding: Some(name.name.clone()),
                    ..Default::default()
                },
            ),
            Item::Clip(b) => {
                let dq = crate::perform::definition_quantum(&b.options)
                    .ok()
                    .flatten();
                definition(
                    text,
                    offset,
                    format!("clip:{}", b.name.name),
                    DefParts {
                        binding: Some(b.name.name.clone()),
                        definition_quantize: dq,
                        ..Default::default()
                    },
                )
            }
            Item::Scene(b) => {
                let dq = crate::perform::definition_quantum(&b.options)
                    .ok()
                    .flatten();
                definition(
                    text,
                    offset,
                    format!("scene:{}", b.name.name),
                    DefParts {
                        binding: Some(b.name.name.clone()),
                        definition_quantize: dq,
                        ..Default::default()
                    },
                )
            }
            Item::Command(c) => Statement {
                text,
                offset,
                quantize: c.quantize,
                write_key: command_write_key(c),
                data: StatementData::Command {
                    has_at: matches!(c.at, Some(AtPos::Time(_) | AtPos::Bar { .. })),
                    clears: matches!(c.command, Command::New),
                    action: command_action(c, source, span.start),
                },
            },
            Item::Note(n) => Statement {
                text,
                offset,
                quantize: None,
                write_key: None,
                data: StatementData::Note {
                    has_at: matches!(n.at, Some(AtPos::Time(_) | AtPos::Bar { .. })),
                },
            },
        };
        out.push(s);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_classified_with_their_source() {
        let src = "\ntempo 120bpm @ next bar\ninstr a(f: hz) { osc(sine, f) }\nriff = [c4 e4]\nclip c { notes: [c4] }\nlead.play(riff.fast(2)) @ next 4bars\nlead.mute\na(f: 440hz) for 1beat\n";
        let s = analyze(src).unwrap();
        assert_eq!(s.len(), 7);
        assert_eq!(s[0].key(), Some("tempo"));
        assert_eq!(s[0].quantize.unwrap().count, 1.0);
        assert_eq!(s[1].key(), Some("instr:a"));
        assert_eq!(s[1].text, "instr a(f: hz) { osc(sine, f) }");
        assert_eq!(&src[s[1].offset..s[1].offset + 5], "instr");
        assert_eq!(
            (s[2].key(), s[2].binding()),
            (Some("bind:riff"), Some("riff"))
        );
        assert_eq!(s[3].binding(), Some("c"));
        assert_eq!(s[4].kind(), StatementKind::Command);
        assert_eq!(
            s[4].plays(),
            Some(&PlayInfo {
                node: "lead".into(),
                what: "riff.fast(2)".into()
            })
        );
        assert_eq!(s[4].quantize.unwrap().count, 4.0);
        assert_eq!(s[5].kind(), StatementKind::Command);
        assert!(s[5].plays().is_none());
        assert_eq!(s[6].kind(), StatementKind::Note);
        assert!(!s[6].has_at());
        assert!(analyze("at 2bars lead.mute").unwrap()[0].has_at());
        let later = &analyze("at +2bars unsolo").unwrap()[0];
        assert!(!later.has_at(), "relative to when the code lands");
        assert_eq!(later.kind(), StatementKind::Command);
        assert!(analyze("at 1beat a(f: 1hz) for 1beat").unwrap()[0].has_at());
        assert!(s[6].key().is_none());
    }

    #[test]
    fn syntax_errors_come_back_as_diagnostics() {
        assert!(analyze("instr (").is_err());
        assert!(analyze("").unwrap().is_empty());
    }
}
