//! The boundary between a composition and sound-server definitions.

use crate::ast::{Arg, Expr, ExprKind, Item, Stmt};
use crate::parser::parse_spanned;
use crate::{Diagnostic, Span, Tok, lex};
use niminal_score::{Tempo, Value};

pub const LIVE_BPM: &str = "__niminal_live_bpm";
pub const LIVE_ORIGIN: &str = "__niminal_live_origin";
pub const LIVE_METER: &str = "__niminal_live_meter";

/// Resolve musical parameter values before sending them to the sound server.
pub fn dsp_value(value: Value, tempo: Tempo) -> Value {
    match value {
        Value::Beats(n) => Value::Seconds(n * 60.0 / tempo.bpm),
        Value::Bars(n) => Value::Seconds(n * tempo.beats_per_bar * 60.0 / tempo.bpm),
        Value::Note(_) => Value::Hz(
            value
                .as_hz_in(&tempo.tuning)
                .expect("a note name is a frequency"),
        ),
        other => other,
    }
}

fn options(source: &str, args: &[Arg]) -> String {
    let args: Vec<String> = args
        .iter()
        .filter(|a| a.name.as_ref().is_none_or(|n| n.name != "quantize"))
        .map(|a| match &a.name {
            Some(n) => format!(
                "{}: {}",
                n.name,
                &source[a.value.span.start..a.value.span.end]
            ),
            None => source[a.value.span.start..a.value.span.end].to_string(),
        })
        .collect();
    if args.is_empty() {
        String::new()
    } else {
        format!(" with({})", args.join(", "))
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

/// Project a compiled client's source into DSP-only source. The client owns this conversion;
/// the server never accepts the composition constructs it removes.
pub fn dsp_source(source: &str, tempo: Tempo) -> Result<String, Diagnostic> {
    let mut out = Vec::new();
    for (item, span) in parse_spanned(source)? {
        let text = &source[span.start..span.end];
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
                    out.push(format!(
                        "track {} {{ instrument = {} }}{}",
                        c.name.name,
                        &source[instrument.span.start..instrument.span.end],
                        options(source, &c.options)
                    ));
                }
            }
            Item::Control { name, value, .. } => out.push(format!(
                "ctl {} = {}",
                name.name,
                control_value(source, &value)
            )),
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
                    out.push(format!("config {{\n{}\n}}", entries.join("\n")));
                }
            }
            Item::Track(t) => {
                let Some(end) = text.rfind('}').map(|n| n + 1) else {
                    out.push(text.to_string());
                    continue;
                };
                let mut body = text[..end].to_string();
                for stmt in t.body.iter().rev() {
                    if let Stmt::Bind { name, value } = stmt
                        && name.name == "notes"
                    {
                        body.replace_range(
                            name.span.start - span.start..value.span.end - span.start,
                            "",
                        );
                    }
                }
                out.push(format!("{body}{}", options(source, &t.options)));
            }
            Item::Instr(i) => {
                if i.options
                    .iter()
                    .any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize"))
                {
                    let tokens = lex(text)?;
                    let start = tokens
                        .iter()
                        .position(|t| matches!(&t.tok, Tok::Ident(n) if n == "with"))
                        .expect("options have with");
                    let mut depth = 0;
                    let end = tokens[start + 1..]
                        .iter()
                        .find(|t| {
                            match t.tok {
                                Tok::LParen => depth += 1,
                                Tok::RParen => depth -= 1,
                                _ => {}
                            }
                            depth == 0
                        })
                        .expect("options close")
                        .span
                        .end;
                    let mut text = text.to_string();
                    text.replace_range(tokens[start].span.start..end, &options(source, &i.options));
                    out.push(text);
                } else {
                    out.push(text.to_string());
                }
            }
            Item::Sample(sample) => {
                let mut text = text.to_string();
                for arg in sample.options.iter().rev() {
                    if let Some(name) = &arg.name
                        && name.name == "beats"
                    {
                        if let ExprKind::Num { value, .. } = arg.value.kind {
                            text.replace_range(
                                name.span.start - span.start..arg.value.span.end - span.start,
                                &format!("duration: {}sec", value * 60.0 / tempo.bpm),
                            );
                        }
                    }
                }
                out.push(text);
            }
            _ => out.push(text.to_string()),
        }
    }
    // Tokens preserve strings, comments and paths, including words that resemble musical units.
    let mut source = out.join("\n");
    let tokens = lex(&source)?;
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
    for (span, replacement) in edits.into_iter().rev() {
        source.replace_range(span.start..span.end, &replacement);
    }
    Ok(source)
}

/// Refuse client constructs and unresolved musical time at the server boundary.
pub fn validate_dsp(source: &str) -> Result<(), Diagnostic> {
    for (item, span) in parse_spanned(source)? {
        let allowed = match &item {
            Item::Instr(i) => !i
                .options
                .iter()
                .any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize")),
            Item::Track(t) => {
                !t.options
                    .iter()
                    .any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize"))
                    && !t
                        .body
                        .iter()
                        .any(|s| matches!(s, Stmt::Bind { name, .. } if name.name == "notes"))
            }
            Item::Control { input, .. } => input.is_none(),
            Item::Config(c) => c.quantize.is_empty(),
            Item::Fn(_)
            | Item::Const { .. }
            | Item::Bus(_)
            | Item::Buffer(_)
            | Item::MidiOut(_) => true,
            Item::Sample(s) => !s
                .options
                .iter()
                .any(|a| a.name.as_ref().is_some_and(|n| n.name == "beats")),
            _ => false,
        };
        if !allowed {
            return Err(Diagnostic::new(
                "the sound server accepts only DSP definitions; composition, input bindings and musical timing belong to the client",
                span,
            ));
        }
    }
    let tokens = lex(source)?;
    for (i, t) in tokens.iter().enumerate() {
        let musical = (i > 0
            && matches!(tokens[i - 1].tok, Tok::Dot)
            && matches!(&t.tok, Tok::Ident(u) if matches!(u.as_str(), "beat" | "beats" | "bar" | "bars")))
            || matches!(&t.tok, Tok::Num { unit: Some(u), .. } if matches!(u.as_str(), "beat" | "beats" | "bar" | "bars" | "bpm"))
            || (matches!(&t.tok, Tok::Ident(n) if n == "clock")
                && matches!(tokens.get(i + 1).map(|t| &t.tok), Some(Tok::Dot))
                && matches!(tokens.get(i+2).map(|t| &t.tok), Some(Tok::Ident(n)) if n != "time"));
        if musical {
            return Err(Diagnostic::new(
                "resolve musical time in the client before sending DSP definitions",
                t.span,
            ));
        }
    }
    Ok(())
}
