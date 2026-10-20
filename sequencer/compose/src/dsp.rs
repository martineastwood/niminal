//! The part of a composition that is a sound-server definition: instruments, tracks, buses and the
//! rest, with the client's musical time resolved to seconds.

use crate::ast::{Arg, Expr, ExprKind, Item, Stmt};
use crate::lexer::{Tok, lex};
use crate::parser::parse_spanned;
use niminal_lang::{Diagnostic, Span};
use niminal_score::Tempo;

pub const LIVE_BPM: &str = "__niminal_live_bpm";
pub const LIVE_ORIGIN: &str = "__niminal_live_origin";
pub const LIVE_METER: &str = "__niminal_live_meter";

/// DSP source made from a composition, and where each part of it came from.
pub struct DspSource {
    pub text: String,
    segments: Vec<Segment>,
}

/// A stretch of the DSP text. `src` is where it starts in the composition when it was copied
/// from there as it stands; otherwise the whole stretch stands for `src..src_end`.
struct Segment {
    dsp: usize,
    len: usize,
    src: usize,
    src_end: usize,
    verbatim: bool,
}

impl DspSource {
    /// Where a span of the DSP text is in the composition it was made from.
    pub fn locate(&self, span: Span) -> Span {
        let at = |offset: usize, end: bool| {
            let found = self
                .segments
                .iter()
                .rev()
                .find(|s| s.dsp <= offset && (offset < s.dsp + s.len || s.len == 0));
            match found {
                Some(s) if s.verbatim => s.src + (offset - s.dsp) + usize::from(end),
                Some(s) => {
                    if end {
                        s.src_end
                    } else {
                        s.src
                    }
                }
                None => 0,
            }
        };
        let start = at(span.start, false);
        let end = if span.end > span.start { at(span.end - 1, true) } else { start };
        Span::new(start, end.max(start))
    }
}

fn control_value(source: &str, expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Num { value, unit } => format!("{value}{}", unit.as_deref().unwrap_or("")),
        ExprKind::Call { name, args } if name.name == "smooth" => format!(
            "({}).smooth({})",
            control_value(source, &args[0].value),
            control_value(source, &args[1].value)
        ),
        _ => source[expr.span.start..expr.span.end].to_string(),
    }
}

/// The text of the setting `name` in `arg`, from its name to the end of its value.
fn setting_range(arg: &Arg, name: &str) -> Option<(usize, usize)> {
    let key = arg.name.as_ref().filter(|n| n.name == name)?;
    Some((key.span.start, arg.value.span.end))
}

/// `text`, which starts at `base` in the composition, without the stretches in `cuts`.
fn without(
    text: &str,
    base: usize,
    mut cuts: Vec<(usize, usize)>,
    verbatim: impl Fn(String) -> Piece,
    derived: impl Fn(String) -> Piece,
) -> Piece {
    if cuts.is_empty() {
        return verbatim(text.to_string());
    }
    cuts.sort();
    let mut text = text.to_string();
    for (start, end) in cuts.into_iter().rev() {
        text.replace_range(start - base..end - base, "");
    }
    derived(text)
}

/// One DSP definition and where it came from.
struct Piece {
    text: String,
    /// Where `text` starts in the composition, if it is a copy of what is there.
    verbatim_at: Option<usize>,
    span: Span,
}

/// Project a composition's source into DSP-only source. The client owns this conversion; the server
/// never accepts the composition constructs it removes.
pub fn dsp_source(source: &str, tempo: Tempo) -> Result<String, Diagnostic> {
    let items = parse_spanned(source)?;
    Ok(project(source, &items, tempo, &[], false)?.text)
}

/// Like [`dsp_source`], for items already parsed. `implicit` names the instruments that are played
/// directly and so get a track of their own. With `standalone`, the controls that `clock.beat` and the
/// like are made from are defined too, at the tempo given; otherwise whoever plays the DSP defines them.
pub fn project(
    source: &str,
    items: &[(Item, Span)],
    tempo: Tempo,
    implicit: &[String],
    standalone: bool,
) -> Result<DspSource, Diagnostic> {
    let mut pieces = Vec::new();
    for (item, span) in items {
        let text = &source[span.start..span.end];
        let verbatim = |text: String| Piece {
            text,
            verbatim_at: Some(span.start),
            span: *span,
        };
        let derived = |text: String| Piece {
            text,
            verbatim_at: None,
            span: *span,
        };
        match item {
            Item::Tempo(_)
            | Item::Meter(_)
            | Item::Bind { .. }
            | Item::Scene(_)
            | Item::Arrangement(_)
            | Item::Def(_)
            | Item::Command(_)
            | Item::Note(_)
            | Item::For { .. }
            | Item::If { .. } => {}
            Item::Clip(c) => {
                if let Some((_, instrument)) =
                    c.entries.iter().find(|(n, _)| n.name == "instrument")
                {
                    pieces.push(derived(format!(
                        "track {} {{ instrument = {} }}",
                        c.name.name,
                        &source[instrument.span.start..instrument.span.end]
                    )));
                }
            }
            Item::Control { name, value, .. } => pieces.push(derived(format!(
                "ctl {} = {}",
                name.name,
                control_value(source, value)
            ))),
            Item::Config(c) => {
                let mut entries: Vec<String> = c
                    .entries
                    .iter()
                    .map(|(n, v)| format!("{}: {}", n.name, &source[v.span.start..v.span.end]))
                    .collect();
                if !c.tuning.is_empty() {
                    entries.push(format!(
                        "tuning: {{ {} }}",
                        c.tuning
                            .iter()
                            .map(|(n, v)| format!(
                                "{}: {}",
                                n.name,
                                &source[v.span.start..v.span.end]
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                }
                if !entries.is_empty() {
                    pieces.push(derived(format!("config {{\n{}\n}}", entries.join("\n"))));
                }
            }
            Item::Track(t) => {
                // the client's own settings and its `notes` are not the server's
                let mut cuts: Vec<(usize, usize)> = t
                    .options
                    .iter()
                    .filter_map(|a| setting_range(a, "quantize"))
                    .collect();
                for stmt in &t.body {
                    if let Stmt::Bind { name, value } = stmt
                        && name.name == "notes"
                    {
                        cuts.push((name.span.start, value.span.end));
                    }
                }
                pieces.push(without(text, span.start, cuts, verbatim, derived));
            }
            Item::Instr(i) => {
                let cuts = i.options.iter().filter_map(|a| setting_range(a, "quantize")).collect();
                pieces.push(without(text, span.start, cuts, verbatim, derived));
            }
            Item::Sample(sample) => {
                let mut changed = false;
                let mut text = text.to_string();
                for arg in sample.options.iter().rev() {
                    if let Some(name) = &arg.name
                        && name.name == "beats"
                    {
                        let ExprKind::Num { value, unit: None } = arg.value.kind else {
                            return Err(Diagnostic::new(
                                "`beats` is how many beats the loop lasts, such as `4`",
                                arg.value.span,
                            ));
                        };
                        if value <= 0.0 {
                            return Err(Diagnostic::new(
                                "`beats` is how many beats the loop lasts, such as `4`",
                                arg.value.span,
                            ));
                        }
                        // `beats(4)` becomes `duration(2sec)`
                        text.replace_range(
                            name.span.start - span.start..arg.value.span.end - span.start,
                            &format!("duration({}sec", value * 60.0 / tempo.bpm),
                        );
                        changed = true;
                    }
                }
                pieces.push(if changed { derived(text) } else { verbatim(text) });
            }
            _ => pieces.push(verbatim(text.to_string())),
        }
    }
    for name in implicit {
        pieces.push(Piece {
            text: format!("track {name} {{ instrument = {name} }}"),
            verbatim_at: None,
            span: Span::new(0, 0),
        });
    }

    // Musical time becomes seconds. Tokens preserve strings, comments and paths, including words
    // that resemble musical units.
    let mut text = String::new();
    let mut uses_clock = false;
    let mut segments = Vec::new();
    for (n, piece) in pieces.iter().enumerate() {
        if n > 0 {
            text.push('\n');
        }
        let edits = token_edits(&piece.text, tempo).map_err(|mut d| {
            // placed in the composition, if the text is a copy of what is there
            d.span = match piece.verbatim_at {
                Some(base) => Span::new(base + d.span.start, base + d.span.end),
                None => piece.span,
            };
            d
        })?;
        uses_clock |= edits.iter().any(|(_, replacement)| replacement.contains(LIVE_BPM));
        let mut at = 0;
        let mut push = |out: &mut String, dsp_text: &str, from: usize, len: usize, replaced: bool| {
            if dsp_text.is_empty() && !replaced {
                return;
            }
            let (src, src_end, verbatim) = match piece.verbatim_at {
                Some(base) if !replaced => (base + from, base + from + len, true),
                Some(base) => (base + from, base + from + len, false),
                None => (piece.span.start, piece.span.end, false),
            };
            segments.push(Segment {
                dsp: out.len(),
                len: dsp_text.len(),
                src,
                src_end,
                verbatim,
            });
            out.push_str(dsp_text);
        };
        for (edit, replacement) in &edits {
            push(&mut text, &piece.text[at..edit.start], at, edit.start - at, false);
            push(&mut text, replacement, edit.start, edit.end - edit.start, true);
            at = edit.end;
        }
        push(&mut text, &piece.text[at..], at, piece.text.len() - at, false);
    }
    if standalone && uses_clock {
        let own = Span::new(0, 0);
        for line in [
            format!("ctl {LIVE_BPM} = {}", tempo.bpm),
            format!("ctl {LIVE_ORIGIN} = 0sec"),
            format!("ctl {LIVE_METER} = {}", tempo.beats_per_bar),
        ] {
            text.push('\n');
            segments.push(Segment { dsp: text.len(), len: line.len(), src: own.start, src_end: own.end, verbatim: false });
            text.push_str(&line);
        }
    }
    Ok(DspSource { text, segments })
}

/// The musical units in `text`, each with what stands for it in seconds.
fn token_edits(text: &str, tempo: Tempo) -> Result<Vec<(Span, String)>, Diagnostic> {
    let tokens = lex(text)?;
    let mut edits = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if let Tok::Num {
            value,
            unit: Some(unit),
        } = &token.tok
        {
            let seconds = match unit.as_str() {
                "beat" | "beats" => Some(value * 60.0 / tempo.bpm),
                "bar" | "bars" => Some(value * tempo.beats_per_bar * 60.0 / tempo.bpm),
                _ => None,
            };
            if let Some(seconds) = seconds {
                edits.push((token.span, format!("{seconds}sec")));
            }
            if unit == "bpm" {
                return Err(Diagnostic::new("`bpm` can only be used with `tempo`", token.span));
            }
        }
        if matches!(&token.tok, Tok::Ident(n) if n == "clock")
            && matches!(tokens.get(i + 1).map(|t| &t.tok), Some(Tok::Dot))
        {
            let Some(field) = tokens.get(i + 2) else {
                continue;
            };
            let beat = format!("((clock.time - {LIVE_ORIGIN}) / 1sec * {LIVE_BPM} / 60)");
            let replacement = match &field.tok {
                Tok::Ident(n) if n == "beat" => Some(beat),
                Tok::Ident(n) if n == "bar" => Some(format!("({beat} / {LIVE_METER})")),
                Tok::Ident(n) if n == "bpm" => Some(LIVE_BPM.to_string()),
                _ => None,
            };
            if let Some(replacement) = replacement {
                edits.push((Span::new(token.span.start, field.span.end), replacement));
            }
        }
    }
    Ok(edits)
}
