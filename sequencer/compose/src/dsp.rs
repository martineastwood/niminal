//! The part of a composition that is a sound-server definition: instruments, nodes, buses and the
//! rest, with the client's musical time resolved to seconds.

use crate::ast::{Arg, Expr, ExprKind, Item};
use crate::lexer::{Tok, lex};
use crate::parser::parse_spanned;
use niminal_lang::{Diagnostic, Span};
use niminal_score::Tempo;

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
        let end = if span.end > span.start {
            at(span.end - 1, true)
        } else {
            start
        };
        Span::new(start, end.max(start))
    }
}

pub(crate) fn control_value(source: &str, expr: &Expr) -> String {
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

/// An effect parameter (`Some(effect)`) or a `ctl` (`None`) that starts at a length in beats or bars, with the
/// number and whether it is `beats` or `bars`. The server keeps one to the tempo when the client sets it in
/// those units, though the DSP it is sent has the default in seconds.
pub struct BeatControl {
    pub effect: Option<String>,
    pub name: String,
    pub value: f64,
    pub unit: &'static str,
}

/// The effect parameters and controls whose default is written in beats or bars.
pub fn beat_controls(source: &str) -> Vec<BeatControl> {
    let Ok(items) = parse_spanned(source) else {
        return Vec::new();
    };
    let musical = |expr: &Expr| match &expr.kind {
        ExprKind::Num { value, unit: Some(unit) } if matches!(unit.as_str(), "beat" | "beats" | "bar" | "bars") => {
            Some((*value, if unit.starts_with("bar") { "bars" } else { "beats" }))
        }
        _ => None,
    };
    let mut found = Vec::new();
    for (item, _) in &items {
        match item {
            Item::Fx(fx) => {
                for param in &fx.params {
                    if let Some((value, unit)) = param.default.as_ref().and_then(musical) {
                        found.push(BeatControl { effect: Some(fx.name.name.clone()), name: param.name.name.clone(), value, unit });
                    }
                }
            }
            Item::Control { name, value, input: None } => {
                if let Some((value, unit)) = musical(value) {
                    found.push(BeatControl { effect: None, name: name.name.clone(), value, unit });
                }
            }
            _ => {}
        }
    }
    found
}

/// The effect parameters declared as a `division` (a length snapped to a musical division), as (effect, name).
pub fn division_controls(source: &str) -> Vec<(String, String)> {
    let Ok(items) = parse_spanned(source) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (item, _) in &items {
        if let Item::Fx(fx) = item {
            for param in &fx.params {
                if matches!(&param.ty, Some(crate::ast::TypeSpec::Unit(t)) if t.name == "division") {
                    found.push((fx.name.name.clone(), param.name.name.clone()));
                }
            }
        }
    }
    found
}

/// Project a composition's source into DSP-only source. The client owns this conversion; the server
/// never accepts the composition constructs it removes.
pub fn dsp_source(source: &str, tempo: Tempo) -> Result<String, Diagnostic> {
    let items = parse_spanned(source)?;
    Ok(project(source, &items, tempo)?.text)
}

/// `text` with its lengths in beats and bars written out in seconds, and nothing else changed: a beat is a
/// quarter note at `bpm`, and a bar is `beats_per_bar` of them (3 in 6/8, 3.5 in 7/8). For a client that sends
/// the audio server a definition on its own, with no composition around it.
pub fn resolve_musical_time(text: &str, bpm: f64, beats_per_bar: f64) -> Result<String, Diagnostic> {
    let tempo = Tempo {
        beats_per_bar,
        ..Tempo::new(bpm)
    };
    let mut edits = token_edits(text, tempo)?;
    edits.retain(|(span, _)| {
        let written = &text[span.start..span.end];
        ["beat", "beats", "bar", "bars"].iter().any(|unit| written.ends_with(unit))
    });
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (span, replacement) in edits {
        out.push_str(&text[at..span.start]);
        out.push_str(&replacement);
        at = span.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// Like [`dsp_source`], for items already parsed.
pub fn project(
    source: &str,
    items: &[(Item, Span)],
    tempo: Tempo,
) -> Result<DspSource, Diagnostic> {
    let mut pieces = Vec::new();
    let mut silent_defined = false;
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
            | Item::On(_)
            | Item::Note(_)
            | Item::For { .. }
            | Item::If { .. } => {}
            Item::Clip(c) => {
                if let Some(instrument) = instrument_of(c, items, 0) {
                    pieces.push(derived(format!(
                        "node {} {{ instrument = {} }}",
                        c.name.name,
                        &source[instrument.span.start..instrument.span.end]
                    )));
                } else if is_gesture(c, items, 0) {
                    // a clip that only moves an effect's controls plays this, which makes no sound
                    if !std::mem::replace(&mut silent_defined, true) {
                        pieces.push(derived(
                            "instr __gesture(freq: hz = 440hz) { out = osc(sine, freq) * 0 }".into(),
                        ));
                    }
                    pieces.push(derived(format!("node {} {{ instrument = __gesture }}", c.name.name)));
                }
            }
            // a macro is a control from 0 to 1 here; the composition keeps what it moves
            Item::Macro(block) => {
                let value = match block.entries.iter().find(|(key, _)| key.name == "smooth") {
                    Some((_, smooth)) => format!("(0).smooth({})", control_value(source, smooth)),
                    None => "0".to_string(),
                };
                pieces.push(derived(format!("ctl {} = {value}", block.name.name)));
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
            Item::Fx(p) => {
                let cuts = p
                    .options
                    .iter()
                    .filter_map(|a| setting_range(a, "quantize"))
                    .collect();
                pieces.push(without(text, span.start, cuts, verbatim, derived));
            }
            Item::Instr(i) => {
                let cuts = i
                    .options
                    .iter()
                    .filter_map(|a| setting_range(a, "quantize"))
                    .collect();
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
                        // Keep duration connected to the live tempo, including on held voices.
                        text.replace_range(
                            name.span.start - span.start..arg.value.span.end - span.start,
                            &format!("duration({}sec / clock.bpm", value * 60.0),
                        );
                        changed = true;
                    }
                }
                pieces.push(if changed {
                    derived(text)
                } else {
                    verbatim(text)
                });
            }
            _ => pieces.push(verbatim(text.to_string())),
        }
    }

    let mut uses_grids = false;
    for piece in &mut pieces {
        let lowered = crate::rhythm_gate::lower(&piece.text).map_err(|mut d| {
            d.span = match piece.verbatim_at {
                Some(base) => Span::new(base + d.span.start, base + d.span.end),
                None => piece.span,
            };
            d
        })?;
        if let Some(text) = lowered {
            piece.text = text;
            piece.verbatim_at = None;
            uses_grids = true;
        }
    }
    if uses_grids {
        pieces.push(Piece {
            text: crate::rhythm_gate::HELPER.into(),
            verbatim_at: None,
            span: Span::new(0, 0),
        });
    }

    // Musical time becomes seconds. Tokens preserve strings, comments and paths, including words
    // that resemble musical units.
    let mut text = String::new();
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
        let mut at = 0;
        let mut push =
            |out: &mut String, dsp_text: &str, from: usize, len: usize, replaced: bool| {
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
            push(
                &mut text,
                &piece.text[at..edit.start],
                at,
                edit.start - at,
                false,
            );
            push(
                &mut text,
                replacement,
                edit.start,
                edit.end - edit.start,
                true,
            );
            at = edit.end;
        }
        push(
            &mut text,
            &piece.text[at..],
            at,
            piece.text.len() - at,
            false,
        );
    }
    Ok(DspSource { text, segments })
}

/// What is worth a second look in `source` without being wrong: a clip with no instrument exists but plays
/// nothing.
pub fn warnings(source: &str) -> Vec<Diagnostic> {
    let Ok(items) = parse_spanned(source) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|(item, _)| match item {
            Item::Clip(c) if instrument_of(c, &items, 0).is_none() && !is_gesture(c, &items, 0) => Some(
                Diagnostic::new(
                    format!(
                        "clip `{}` has no instrument, so it plays nothing",
                        c.name.name
                    ),
                    c.name.span,
                )
                .with_help(format!("add `instrument: ...` to `{}`", c.name.name)),
            ),
            _ => None,
        })
        .collect()
}

/// Does the clip have rows for an effect, such as `stutter_fx { hold: ..x. }`, or a lane for one of the
/// `macros`? With no instrument it is a gesture: it plays no notes and only moves those controls.
pub fn drives_effects(clip: &crate::ast::NamedBlock, macros: &[&str]) -> bool {
    clip.entries
        .iter()
        .any(|(k, v)| matches!(v.kind, ExprKind::FxLanes(_)) || macros.contains(&k.name.as_str()))
}

/// Is the clip a gesture: rows for an effect of its own, or made from a clip that has them?
fn is_gesture(clip: &crate::ast::NamedBlock, items: &[(Item, Span)], depth: usize) -> bool {
    let macros: Vec<&str> = items
        .iter()
        .filter_map(|(item, _)| match item {
            Item::Macro(b) => Some(b.name.name.as_str()),
            _ => None,
        })
        .collect();
    if drives_effects(clip, &macros) {
        return true;
    }
    let Some(root) = clip.base.as_deref().and_then(|b| b.root_name()) else {
        return false;
    };
    depth <= 16
        && root != clip.name.name
        && items.iter().any(|(item, _)| matches!(item, Item::Clip(base) if base.name.name == root && is_gesture(base, items, depth + 1)))
}

/// The instrument a clip plays: its own `instrument:`, or its base's when it is made from another clip
/// (`clip bass_chorus = bass.transpose(+5st)`).
fn instrument_of<'a>(
    clip: &'a crate::ast::NamedBlock,
    items: &'a [(Item, Span)],
    depth: usize,
) -> Option<&'a Expr> {
    if let Some((_, instrument)) = clip.entries.iter().find(|(n, _)| n.name == "instrument") {
        return Some(instrument);
    }
    let root = clip.base.as_deref()?.root_name()?;
    // a clip made from itself, or a long chain of them, has no instrument to find
    if depth > 16 || root == clip.name.name {
        return None;
    }
    items.iter().find_map(|(item, _)| match item {
        Item::Clip(base) if base.name.name == root => instrument_of(base, items, depth + 1),
        _ => None,
    })
}

/// The musical units in `text`, each with what stands for it in seconds.
fn token_edits(text: &str, tempo: Tempo) -> Result<Vec<(Span, String)>, Diagnostic> {
    let tokens = lex(text)?;
    let mut edits = Vec::new();
    for token in &tokens {
        if let Tok::Num {
            value,
            unit: Some(unit),
        } = &token.tok
        {
            if unit == "%" {
                edits.push((token.span, (value / 100.0).to_string()));
            }
            let seconds = match unit.as_str() {
                "beat" | "beats" => Some(value * 60.0 / tempo.bpm),
                "bar" | "bars" => Some(value * tempo.beats_per_bar * 60.0 / tempo.bpm),
                _ => None,
            };
            if let Some(seconds) = seconds {
                edits.push((token.span, format!("{seconds}sec")));
            }
            if unit == "bpm" {
                return Err(Diagnostic::new(
                    "`bpm` can only be used with `tempo`",
                    token.span,
                ));
            }
        }
    }
    Ok(edits)
}
