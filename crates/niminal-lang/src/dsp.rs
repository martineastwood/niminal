//! The boundary between a client and the audio server: the server takes DSP definitions, with
//! musical time resolved by the client.

use crate::ast::{Item, Stmt};
use crate::parser::parse_spanned;
use crate::{Diagnostic, Tok, lex};
use niminal_score::{Tempo, Value};

/// Resolve musical parameter values before sending them to the audio server.
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

/// Refuse client constructs and unresolved musical time at the server boundary.
pub fn validate_dsp(source: &str) -> Result<(), Diagnostic> {
    for (item, span) in parse_spanned(source)? {
        let allowed = match &item {
            Item::Instr(i) => !i.options.iter().any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize")),
            Item::Node(t) | Item::Processor(t) => {
                !t.options.iter().any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize"))
                    && !t.body.iter().any(|s| matches!(s, Stmt::Bind { name, .. } if name.name == "notes"))
            }
            Item::Sample(s) => !s.options.iter().any(|a| a.name.as_ref().is_some_and(|n| n.name == "beats")),
            _ => true,
        };
        if !allowed {
            return Err(Diagnostic::new(
                "the audio server accepts only DSP definitions; sequencing and musical timing belong to the client",
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
                && matches!(tokens.get(i+2).map(|t| &t.tok), Some(Tok::Ident(n)) if matches!(n.as_str(), "beat" | "bar" | "bpm")));
        if musical {
            return Err(Diagnostic::new(
                "resolve musical time in the client before sending DSP definitions",
                t.span,
            ));
        }
    }
    Ok(())
}
