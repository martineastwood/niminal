//! The boundary between a client and the audio server: the server takes DSP definitions, with
//! musical time resolved by the client.

use crate::ast::{Item, Stmt};
use crate::parser::parse_spanned;
use crate::{Diagnostic, Span, Tok, lex};
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

/// A connection's tempo, as the DSP it sends reads it through `clock.bpm`, `clock.beat` and `clock.bar`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockState {
    pub bpm: f64,
    pub beats_per_bar: f64,
    /// The engine time, in seconds, at which beat 0 fell: `clock.beat` counts from it at the tempo.
    pub origin: f64,
}

impl Default for ClockState {
    fn default() -> Self {
        ClockState { bpm: 120.0, beats_per_bar: 4.0, origin: 0.0 }
    }
}

impl ClockState {
    /// The beat at engine time `t` seconds.
    pub fn beat_at(&self, t: f64) -> f64 {
        (t - self.origin) * self.bpm / 60.0
    }

    /// The tempo changed to `bpm` and `beats_per_bar` at `t` seconds. With a `beat` the count is set to it
    /// there; without, the beat in progress carries on from where it was.
    pub fn changed(&self, t: f64, bpm: f64, beats_per_bar: f64, beat: Option<f64>) -> ClockState {
        let beat = beat.unwrap_or_else(|| self.beat_at(t));
        ClockState { bpm, beats_per_bar, origin: t - beat * 60.0 / bpm }
    }
}

pub const CLOCK_BPM: &str = "__clock_bpm";
pub const CLOCK_ORIGIN: &str = "__clock_origin";
pub const CLOCK_METER: &str = "__clock_meter";

/// `source` with `clock.bpm`, `clock.beat`, `clock.bar`, `clock.meter` (beats in a bar), `clock.beat_length` and
/// `clock.bar_length` (in seconds) made into reads of the connection's tempo
/// controls, which are declared at the end with the values of `clock`. The server does this, so a client
/// only says what the tempo is.
pub fn lower_clock(source: &str, clock: ClockState) -> Result<String, Diagnostic> {
    lower_clock_edits(source, clock).map(|(text, _)| text)
}

/// A stretch of source that [`lower_clock_edits`] wrote something else in place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockEdit {
    /// Where it started in the source, and how long it was there and is now.
    pub start: usize,
    pub old_len: usize,
    pub new_len: usize,
}

/// Where `span` of the text [`lower_clock_edits`] made is in the source it was made from, so that a
/// problem found in the lowered text can be pointed at what was written.
pub fn unlowered_span(edits: &[ClockEdit], span: Span, source_len: usize) -> Span {
    let unmap = |pos: usize, end: bool| {
        let mut shift: isize = 0;
        for e in edits {
            let from = (e.start as isize + shift) as usize;
            if pos < from {
                break;
            }
            if pos < from + e.new_len {
                return if end { e.start + e.old_len } else { e.start };
            }
            shift += e.new_len as isize - e.old_len as isize;
        }
        ((pos as isize - shift).max(0) as usize).min(source_len)
    };
    let start = unmap(span.start, false);
    Span::new(start, unmap(span.end, true).max(start))
}

/// Where `clock.beat` and the like are in `source`, and the reads of the tempo controls they become.
fn clock_reads(source: &str) -> Result<Vec<(usize, usize, String)>, Diagnostic> {
    let tokens = lex(source)?;
    let beat = format!("((clock.time - {CLOCK_ORIGIN}) / 1sec * {CLOCK_BPM} / 60)");
    let mut edits = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if !matches!(&t.tok, Tok::Ident(n) if n == "clock")
            || !matches!(tokens.get(i + 1).map(|t| &t.tok), Some(Tok::Dot))
        {
            continue;
        }
        let Some(field) = tokens.get(i + 2) else { continue };
        let replacement = match &field.tok {
            Tok::Ident(n) if n == "beat" => beat.clone(),
            Tok::Ident(n) if n == "bar" => format!("({beat} / {CLOCK_METER})"),
            Tok::Ident(n) if n == "bpm" => CLOCK_BPM.to_string(),
            Tok::Ident(n) if n == "beat_length" => format!("(60sec / {CLOCK_BPM})"),
            Tok::Ident(n) if n == "bar_length" => format!("(60sec / {CLOCK_BPM} * {CLOCK_METER})"),
            Tok::Ident(n) if n == "meter" => CLOCK_METER.to_string(),
            _ => continue,
        };
        edits.push((t.span.start, field.span.end, replacement));
    }
    Ok(edits)
}

/// `text` with its reads of the clock made into reads of the tempo controls, and nothing declared: for
/// the library functions that come with the language, whose file declares the controls.
pub(crate) fn lower_clock_reads(text: &str) -> String {
    let mut text = text.to_string();
    for (start, end, replacement) in clock_reads(&text).unwrap_or_default().into_iter().rev() {
        text.replace_range(start..end, &replacement);
    }
    text
}

/// [`lower_clock`], with where it changed the text.
pub fn lower_clock_edits(source: &str, clock: ClockState) -> Result<(String, Vec<ClockEdit>), Diagnostic> {
    let edits = clock_reads(source)?;
    // a file that calls a library function that reads the clock needs the controls too
    let declared = source.contains(&format!("ctl {CLOCK_BPM} "));
    if declared || (edits.is_empty() && !crate::prelude::reads_clock(source)) {
        return Ok((source.to_string(), Vec::new()));
    }
    let mut text = source.to_string();
    let made = edits
        .iter()
        .map(|(start, end, replacement)| ClockEdit { start: *start, old_len: end - start, new_len: replacement.len() })
        .collect();
    for (start, end, replacement) in edits.into_iter().rev() {
        text.replace_range(start..end, &replacement);
    }
    text.push_str(&format!(
        "\nctl {CLOCK_BPM} = {}\nctl {CLOCK_ORIGIN} = {}sec\nctl {CLOCK_METER} = {}\n",
        clock.bpm, clock.origin, clock.beats_per_bar
    ));
    Ok((text, made))
}

/// Refuse client constructs and unresolved musical time at the server boundary.
pub fn validate_dsp(source: &str) -> Result<(), Diagnostic> {
    for (item, span) in parse_spanned(source)? {
        let allowed = match &item {
            Item::Instr(i) => !i
                .options
                .iter()
                .any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize")),
            Item::Node(t) | Item::Fx(t) => {
                !t.options
                    .iter()
                    .any(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize"))
                    && !t
                        .body
                        .iter()
                        .any(|s| matches!(s, Stmt::Bind { name, .. } if name.name == "notes"))
            }
            Item::Sample(s) => !s
                .options
                .iter()
                .any(|a| a.name.as_ref().is_some_and(|n| n.name == "beats")),
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
;
        if musical {
            return Err(Diagnostic::new(
                "resolve musical time in the client before sending DSP definitions",
                t.span,
            ));
        }
    }
    Ok(())
}
