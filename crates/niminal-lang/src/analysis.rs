//! Splitting source into its top-level statements, for a live session that
//! applies a snippet to a running project.

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::parser::parse_spanned;
use crate::perform::convert_quantize;
use crate::performance::Quantize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    /// Defines something: an instrument, track, clip, binding, setting...
    Definition,
    /// Does something once: `play`, `mute`, `launch`...
    Command,
    /// A note written out: `lead(freq: c4) for 1beat`.
    Note,
}

/// A command that starts a clip on a track, and the source of what it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayInfo {
    pub track: String,
    pub what: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    /// What a definition defines, as `instr:name`, `clip:name`, `config`...
    /// Defining the same key again replaces the earlier definition.
    pub key: Option<String>,
    /// Pending writes to this destination supersede older pending writes.
    /// Explicitly timed score commands and notes are never superseded.
    pub write_key: Option<String>,
    /// The name a pattern, clip or scene definition binds.
    pub binding: Option<String>,
    pub text: String,
    /// The `@ ...` written after the statement, if any.
    pub quantize: Option<Quantize>,
    pub plays: Option<PlayInfo>,
    /// Whether the statement says when it happens (`at bar 5 ...`).
    pub has_at: bool,
    /// Where the statement starts in the source.
    pub offset: usize,
}

/// Split `source` into statements.
pub fn analyze(source: &str) -> Result<Vec<Statement>, Diagnostic> {
    let mut out = Vec::new();
    for (item, span) in parse_spanned(source)? {
        let text = source[span.start..span.end].to_string();
        let mut s = Statement {
            kind: StatementKind::Definition,
            key: None,
            write_key: None,
            binding: None,
            text,
            quantize: None,
            plays: None,
            has_at: false,
            offset: span.start,
        };
        match &item {
            Item::Control { name, .. } => s.key = Some(format!("ctl:{}", name.name)),
            Item::Instr(d) => s.key = Some(format!("instr:{}", d.name.name)),
            Item::Opcode(d) => s.key = Some(format!("opcode:{}", d.name.name)),
            Item::Sample(d) => s.key = Some(format!("{}:{}", if d.is_kit { "kit" } else { "sample" }, d.name.name)),
            Item::Arrangement(d) => s.key = Some(format!("arrangement:{}", d.name.name)),
            Item::Bus(d) => s.key = Some(format!("bus:{}", d.name.name)),
            Item::Track(d) => s.key = Some(format!("track:{}", d.name.name)),
            Item::Config(_) => s.key = Some("config".into()),
            Item::Meter(_) => s.key = Some("meter".into()),
            Item::Tempo(t) => {
                s.key = Some("tempo".into());
                s.quantize = t.quantize.map(convert_quantize);
            }
            Item::Bind { name, .. } => {
                s.key = Some(format!("bind:{}", name.name));
                s.binding = Some(name.name.clone());
            }
            Item::Clip(b) => {
                s.key = Some(format!("clip:{}", b.name.name));
                s.binding = Some(b.name.name.clone());
            }
            Item::Scene(b) => {
                s.key = Some(format!("scene:{}", b.name.name));
                s.binding = Some(b.name.name.clone());
            }
            Item::Command(c) => {
                s.kind = StatementKind::Command;
                s.has_at = c.at.is_some();
                s.quantize = c.quantize.map(convert_quantize);
                if c.at.is_none() {
                    s.write_key = match &c.command {
                        Command::Play { track, .. } | Command::Stop(track) => Some(format!("play:{}", track.name)),
                        Command::Mute(track) | Command::Unmute(track) => Some(format!("mute:{}", track.name)),
                        Command::Solo(_) | Command::Unsolo(_) => Some("solo".into()),
                        Command::Launch(_) => Some("launch".into()),
                        Command::Hush | Command::Panic => None,
                    };
                }
                if let Command::Play { track, what } = &c.command {
                    s.plays = Some(PlayInfo {
                        track: track.name.clone(),
                        what: source[what.span.start..what.span.end].to_string(),
                    });
                }
            }
            Item::Note(n) => {
                s.kind = StatementKind::Note;
                s.has_at = n.at.is_some();
            }
        }
        if s.kind == StatementKind::Definition { s.write_key.clone_from(&s.key); }
        out.push(s);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_classified_with_their_source() {
        let src = "
tempo 120bpm @ next bar
instr a(f: hz) { osc(sine, f) }
riff = [c4 e4]
clip c { notes: [c4] }
play lead = riff.fast(2) @ next 4 bars
mute lead
a(f: 440hz) for 1beat
";
        let s = analyze(src).unwrap();
        assert_eq!(s.len(), 7);
        assert_eq!(s[0].key.as_deref(), Some("tempo"));
        assert_eq!(s[0].quantize.unwrap().count, 1.0);
        assert_eq!(s[1].key.as_deref(), Some("instr:a"));
        assert_eq!(s[1].text, "instr a(f: hz) { osc(sine, f) }");
        assert_eq!(&src[s[1].offset..s[1].offset + 5], "instr");
        assert_eq!((s[2].key.as_deref(), s[2].binding.as_deref()), (Some("bind:riff"), Some("riff")));
        assert_eq!(s[3].binding.as_deref(), Some("c"));
        assert_eq!(s[4].kind, StatementKind::Command);
        assert_eq!(s[4].plays, Some(PlayInfo { track: "lead".into(), what: "riff.fast(2)".into() }));
        assert_eq!(s[4].quantize.unwrap().count, 4.0);
        assert_eq!(s[5].kind, StatementKind::Command);
        assert!(s[5].plays.is_none());
        assert_eq!(s[6].kind, StatementKind::Note);
        assert!(!s[6].has_at);
        assert!(analyze("at 2 bars mute lead").unwrap()[0].has_at);
        assert!(analyze("at 1beat a(f: 1hz) for 1beat").unwrap()[0].has_at);
        assert!(s[6].key.is_none());
    }

    #[test]
    fn syntax_errors_come_back_as_diagnostics() {
        assert!(analyze("instr (").is_err());
        assert!(analyze("").unwrap().is_empty());
    }
}
