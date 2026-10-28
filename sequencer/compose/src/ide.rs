//! What an editor wants to know about a composition: the names it defines and the operations on patterns.

use std::sync::OnceLock;

use crate::ast::*;
use crate::parser::parse_spanned;
use niminal_lang::{Diagnostic, OpInfo, ParamInfo, Span};

/// Names that mean something to the language itself: reserved for rename checks,
/// and offered as keyword completions in the editor.
pub const RESERVED_WORDS: &[&str] = &[
    "after",
    "and",
    "arrangement",
    "as",
    "at",
    "buffer",
    "bus",
    "capture",
    "clip",
    "clock",
    "config",
    "const",
    "control",
    "ctl",
    "else",
    "env",
    "false",
    "fn",
    "for",
    "hush",
    "if",
    "in",
    "init",
    "instr",
    "it",
    "kit",
    "last",
    "match",
    "meter",
    "midi_out",
    "macro",
    "mono",
    "new",
    "next",
    "on",
    "not",
    "note",
    "now",
    "or",
    "out",
    "over",
    "oversample",
    "panic",
    "reset",
    "reset_on",
    "sample",
    "scene",
    "spawn",
    "state",
    "tempo",
    "fx",
    "true",
    "unsolo",
    "use",
    "cycle",
    "def",
    "events",
    "grid",
    "node",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Instrument,
    Function,
    Control,
    Bus,
    Fx,
    Sample,
    Kit,
    Clip,
    Scene,
    Arrangement,
    Pattern,
}

/// A name defined at the top level of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub name_span: Span,
    /// The whole definition.
    pub span: Span,
    /// A signature or the first line of the definition.
    pub detail: String,
    /// For an instrument or function, its parameters as written (`amp: db = -6db`).
    pub params: Vec<SymbolParam>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolParam {
    pub name: String,
    pub label: String,
}

/// The names defined at the top level of `source`. Fails if it doesn't parse.
pub fn symbols(source: &str) -> Result<Vec<Symbol>, Diagnostic> {
    let mut out = Vec::new();
    let items = parse_spanned(source)?;
    for (item, span) in items.iter().map(|(item, span)| (item.clone(), *span)) {
        let first_line = || {
            source[span.start..span.end]
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let mut param_list = Vec::new();
        let (name, kind, detail) = match &item {
            Item::Instr(d) => {
                param_list = params(source, &d.params);
                (
                    &d.name,
                    SymbolKind::Instrument,
                    format!("instr {}({})", d.name.name, labels(&param_list)),
                )
            }
            Item::Fn(d) => {
                param_list = params(source, &d.params);
                (&d.name, SymbolKind::Function, {
                    let arrow = match &d.result {
                        FnResult::Inferred => String::new(),
                        FnResult::Unit(u) => format!(" -> {}", u.name),
                        FnResult::Named(outputs) => format!(
                            " -> ({})",
                            outputs
                                .iter()
                                .map(|o| o.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    };
                    format!("fn {}({}){arrow}", d.name.name, labels(&param_list))
                })
            }
            Item::Const { name, .. } => (name, SymbolKind::Pattern, first_line()),
            Item::Control { name, .. } => (name, SymbolKind::Control, first_line()),
            Item::Bus(d) => (&d.name, SymbolKind::Bus, first_line()),
            Item::Buffer(d) => (
                &d.name,
                SymbolKind::Pattern,
                format!("buffer {}", d.name.name),
            ),
            Item::Fx(d) => {
                param_list = params(source, &d.params);
                (
                    &d.name,
                    SymbolKind::Fx,
                    format!("fx {}({})", d.name.name, labels(&param_list)),
                )
            }
            Item::MidiOut(d) => (&d.name, SymbolKind::Instrument, first_line()),
            Item::Sample(d) => {
                if d.is_kit {
                    param_list = kit_members(d);
                }
                (
                    &d.name,
                    if d.is_kit {
                        SymbolKind::Kit
                    } else {
                        SymbolKind::Sample
                    },
                    first_line(),
                )
            }
            Item::Clip(b) => {
                param_list = clip_lanes(source, b, &items);
                (&b.name, SymbolKind::Clip, format!("clip {}", b.name.name))
            }
            Item::Scene(b) => (&b.name, SymbolKind::Scene, format!("scene {}", b.name.name)),
            Item::Macro(b) => (&b.name, SymbolKind::Control, format!("macro {}", b.name.name)),
            Item::Arrangement(d) => (
                &d.name,
                SymbolKind::Arrangement,
                format!("arrangement {}", d.name.name),
            ),
            Item::Bind { name, .. } => (name, SymbolKind::Pattern, first_line()),
            _ => continue,
        };
        out.push(Symbol {
            name: name.name.clone(),
            kind,
            name_span: name.span,
            span,
            detail,
            params: param_list,
        });
    }
    Ok(out)
}

/// Members of a kit known from the source: `amen.slices(16)` gives `s0`..`s15`.
fn kit_members(d: &SampleDecl) -> Vec<SymbolParam> {
    match &d.source {
        SampleSource::Instruments(entries) => entries
            .iter()
            .map(|(name, _)| SymbolParam {
                name: name.name.clone(),
                label: name.name.clone(),
            })
            .collect(),
        SampleSource::Slices { count, .. } if *count >= 1.0 && count.fract() == 0.0 => (0..*count
            as usize)
            .map(|i| {
                let name = format!("s{i}");
                SymbolParam {
                    name: name.clone(),
                    label: name,
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The lanes a clip with an `instrument:` can be given: the instrument's parameters, and the lanes every note has.
fn clip_lanes(source: &str, clip: &NamedBlock, items: &[(Item, Span)]) -> Vec<SymbolParam> {
    let Some((_, instrument)) = clip
        .entries
        .iter()
        .find(|(key, _)| key.name == "instrument")
    else {
        return Vec::new();
    };
    // `drums.snare` is parsed as `snare(drums)`: the kit is the receiver.
    let name = match &instrument.kind {
        ExprKind::Call { args, .. }
            if args.len() == 1
                && args[0].receiver
                && let ExprKind::Name(kit) = &args[0].value.kind =>
        {
            kit.as_str()
        }
        ExprKind::Call { name, args } if !args.iter().any(|a| a.receiver) => name.name.as_str(),
        ExprKind::Name(name) => name.as_str(),
        ExprKind::Call { name, .. } => name.name.as_str(),
        _ => return Vec::new(),
    };
    let Some(def) = items.iter().find_map(|(item, _)| match item {
        Item::Instr(d) if d.name.name == name => Some(d),
        _ => None,
    }) else {
        return Vec::new();
    };
    let pitch = def
        .options
        .iter()
        .find(|o| o.name.as_ref().is_some_and(|n| n.name == "pitch"))
        .and_then(|o| match &o.value.kind {
            crate::ast::ExprKind::Name(n) => Some(n.as_str()),
            _ => None,
        })
        .unwrap_or(crate::perform::PITCH_PARAM);
    let mut lanes: Vec<SymbolParam> = params(source, &def.params)
        .into_iter()
        .filter(|p| p.name != pitch)
        .collect();
    lanes.extend(crate::perform::UNIVERSAL_LANES.iter().map(|l| SymbolParam {
        name: (*l).to_string(),
        label: (*l).to_string(),
    }));
    lanes
}

fn params(source: &str, params: &[ParamDef]) -> Vec<SymbolParam> {
    params
        .iter()
        .map(|p| {
            let mut label = p.name.name.clone();
            if p.rate.is_some() || p.ty.is_some() {
                label += ":";
            }
            if let Some((rate, _)) = p.rate {
                label += &format!(" {}", rate.keyword());
            }
            if let Some(ty) = &p.ty {
                let span = match ty {
                    TypeSpec::Unit(u) => u.span,
                    TypeSpec::Range { span, .. } | TypeSpec::Array(_, span) => *span,
                    TypeSpec::Choice(names) => names[0].span.to(names[names.len() - 1].span),
                };
                let text = &source[span.start..span.end];
                label += &match ty {
                    TypeSpec::Choice(_) => format!(" ({text})"),
                    _ => format!(" {text}"),
                };
            }
            if let Some(default) = &p.default {
                label += &format!(" = {}", &source[default.span.start..default.span.end]);
            }
            SymbolParam {
                name: p.name.name.clone(),
                label,
            }
        })
        .collect()
}

fn labels(params: &[SymbolParam]) -> String {
    params
        .iter()
        .map(|p| p.label.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn op_info(
    name: &str,
    doc: &str,
    receiver: bool,
    params: &[(&'static str, &'static str, bool)],
) -> OpInfo {
    OpInfo {
        name: name.to_string(),
        doc: doc.to_string(),
        positional: usize::from(receiver) + params.len(),
        params: params
            .iter()
            .map(|&(name, ty, required)| ParamInfo {
                name: name.to_string(),
                ty,
                required,
            })
            .collect(),
    }
}

/// The operations on patterns, clips and grids, written after one: `riff.every(2, reverse)`.
/// The pattern itself is the receiver, so it is not among the parameters.
pub fn pattern_ops() -> Vec<OpInfo> {
    static OPS: OnceLock<Vec<OpInfo>> = OnceLock::new();
    OPS.get_or_init(|| {
        let op = |name, doc, params| op_info(name, doc, true, params);
        vec![
        op("fast", "Play it this many times faster.", &[("by", "number", true)]),
        op("slow", "Play it this many times slower.", &[("by", "number", true)]),
        op("over", "Stretch it to fill this long, as in `over(4bars)`.", &[("length", "bars", true)]),
        op("width", "On a DSP grid literal, open each active step for this share of its duration: `.width(60%)`. Consecutive steps join at 100%.", &[("amount", "percent", true)]),
        op("reverse", "Play each cycle backwards.", &[]),
        op("shift", "Move it later by this many bars (earlier if negative).", &[("by", "bars", true)]),
        op("repeat_each", "Play each note this many times within its own length.", &[("times", "number", true)]),
        op(
            "drop",
            "Drop some notes at random, the same ones every time: `drop(25%)`. A different `seed` drops different ones.",
            &[("amount", "percent", true), ("seed", "number", false)],
        ),
        op(
            "sometimes",
            "Change a random share of the notes: `sometimes(25%, transpose(+12st))`. With `per: cycle`, change a random share of whole cycles instead.",
            &[("chance", "percent", true), ("change", "transform", true), ("per", "note or cycle", false), ("seed", "number", false)],
        ),
        op(
            "only_between",
            "Change only the notes that start between two points of the bar: `only_between(0%, 50%, fast(2))`.",
            &[("from", "percent", true), ("to", "percent", true), ("change", "transform", true)],
        ),
        op(
            "swing",
            "Delay every other slot: `swing(amount: 20%, every: 1/8beat)`.",
            &[("amount", "percent", true), ("every", "bars", true)],
        ),
        op(
            "start_later",
            "Start a clip later each cycle: `start_later(4)` moves the start on by a quarter every cycle, wrapping after four.",
            &[("parts", "number", true)],
        ),
        op(
            "step_through",
            "Change a different part of the cycle each cycle: `step_through(4, fast(2))`.",
            &[("parts", "number", true), ("change", "transform", true)],
        ),
        op(
            "shuffle",
            "Cut each cycle into parts and play them in a new order every cycle, each once.",
            &[("parts", "number", true), ("seed", "number", false)],
        ),
        op(
            "pick_random",
            "Cut each cycle into parts and fill each slot with a randomly chosen part, repeats allowed.",
            &[("parts", "number", true), ("seed", "number", false)],
        ),
        op("loop_start", "Loop the start of each cycle to fill it: `loop_start(25%)`.", &[("length", "percent", true)]),
        op(
            "where",
            "Change only the notes that pass a test: `where(note < c3, transpose(+12st))`. The test compares `note` (or `note.freq`) with a note or value.",
            &[("when", "test", true), ("change", "transform", true)],
        ),
        op("gain", "Set how loud each note is: `gain(-6db)` or `gain([0db -6db])`.", &[("level", "db", true)]),
        op("pan", "Place each note among the speakers: `pan(-30deg)` or `pan([-30deg 30deg])`.", &[("angle", "degrees", true)]),
        op("begin", "Start each sample note part way in: `begin(25%)`.", &[("share", "percent", true)]),
        op(
            "stereo_copy",
            "Play the original on the left and a changed copy on the right: `stereo_copy(reverse)`. `angle` sets how far out.",
            &[("change", "transform", true), ("angle", "degrees", false)],
        ),
        op(
            "chop",
            "Cut each sample note into pieces that each play the next part of the sample: `chop(8)`.",
            &[("pieces", "number", true)],
        ),
        op(
            "interleave_chops",
            "Play the pattern once for each part of the sample, in turn: `interleave_chops(4)`.",
            &[("pieces", "number", true)],
        ),
        op("take", "Play only the first part of each cycle: `take(75%)`.", &[("length", "percent", true)]),
        op("crop", "Stretch part of each cycle to fill it: `crop(25%, 75%)`.", &[("from", "percent", true), ("to", "percent", true)]),
        op("squeeze", "Squeeze each cycle into part of it, silent elsewhere: `squeeze(25%, 75%)`.", &[("from", "percent", true), ("to", "percent", true)]),
        op(
            "echo",
            "Repeat each note, quieter each time: `echo(3, 1/8beat, fade: -6db)`.",
            &[("times", "number", true), ("time", "bars", true), ("fade", "db", false)],
        ),
        op(
            "strum",
            "Strum chords: each note of a chord starts a little later than the one before, up to this share of the chord's length (default 25%).",
            &[("amount", "percent", false)],
        ),
        op(
            "cycle_through",
            "Play a different change each cycle, in turn: `cycle_through(same, reverse, fast(2))`. `same` is no change.",
            &[("change", "transform", true)],
        ),
        op(
            "times",
            "Play a clip this many times and then finish it: `times(4)`.",
            &[("count", "number", true)],
        ),
        op(
            "in_cycles",
            "Change the cycles whose number, counted in groups of `cycles`, is at or past `from`: `in_cycles(4, 3, fast(2))` changes the last cycle of every four.",
            &[("cycles", "number", true), ("from", "number", true), ("change", "transform", true)],
        ),
        op("often", "Change about 75% of the notes, at random: `often(transpose(+12st))`.", &[("change", "transform", true), ("seed", "number", false)]),
        op("rarely", "Change about 25% of the notes, at random: `rarely(fast(2))`.", &[("change", "transform", true), ("seed", "number", false)]),
        op("back_and_forth", "Play forwards one cycle, backwards the next.", &[]),
        op("to_steps", "Read a signal such as `sine` at this many steps a cycle.", &[("steps", "number", true)]),
        op("invert", "On a grid: hit where it rests, rest where it hits.", &[]),
        op("range", "Scale a 0 to 1 signal or pattern to a range: `sine.range(300hz, 3khz)`.", &[("from", "value", true), ("to", "value", true)]),
        op(
            "in_scale",
            "Turn scale degrees (0, 1, 2...) into notes: `in_scale(minor, root: c3)`. Scales: major, minor, dorian, blues...",
            &[("scale", "scale", true), ("root", "note", false)],
        ),
        op("chord", "Turn each note into a chord: `chord(minor7)`.", &[("kind", "chord", true)]),
        op(
            "arpeggiate",
            "Play chords one note at a time: `arpeggiate(up)`. Orders: up, down, updown, downup, converge, diverge.",
            &[("order", "order", true)],
        ),
        op("voice_lead", "Move each chord by octaves to sit as close as possible to the one before.", &[]),
        op("invert_melody", "Mirror the notes around a note: `invert_melody(around: c4)`.", &[("around", "note", true)]),
        op(
            "markov",
            "Play the notes in a new order each cycle, each following the notes that followed it in the original.",
            &[("seed", "number", false)],
        ),
        op(
            "random_walk",
            "Wander from the first note, each one up to this far from the last: `random_walk(step: 2st)`.",
            &[("step", "semitones", true), ("seed", "number", false)],
        ),
        op(
            "every",
            "Apply a change on every nth cycle, starting with the first: `every(2, reverse)`.",
            &[("n", "number", true), ("change", "transform", true)],
        ),
        op(
            "overlay",
            "Play it together with a changed copy, optionally later: `overlay(transpose(+7st), after: 1/8beat)`.",
            &[("change", "transform", true), ("after", "bars", false)],
        ),
        op(
            "add",
            "Add to each note: `add(7st)`, or another pattern, `add([0 3 5], timing: second)`. The same as `+`. `timing` says whose timing is kept: first (default), second or both. A plain number added to a note moves it by that many semitones.",
            &[("other", "pattern", true), ("timing", "word", false)],
        ),
        op("sub", "Subtract from each note, like `add`. The same as `-`.", &[("other", "pattern", true), ("timing", "word", false)]),
        op("mul", "Multiply each note, like `add`: `mul(2)`. The same as `*`.", &[("other", "pattern", true), ("timing", "word", false)]),
        op("div", "Divide each note, like `add`. The same as `/`.", &[("other", "pattern", true), ("timing", "word", false)]),
        op("transpose", "Move the notes by an interval: `transpose(+5st)`.", &[("by", "semitones", true)]),
        op(
            "spread_evenly",
            "Spread this many hits over this many steps as evenly as possible, and place the notes on them.",
            &[("hits", "number", true), ("steps", "number", true), ("rotation", "number", false)],
        ),
        op("with_rhythm_of", "Give the notes the rhythm of a grid or pattern.", &[("pattern", "grid", true)]),
        op("keep_where", "Keep only the notes where a grid or pattern has a hit.", &[("pattern", "grid", true)]),
        op(
            "set",
            "Give the clip these lanes instead of its own, a number or a pattern each: `set(bright: 0.9)`, `set(gain: [0db -6db])`.",
            &[("lane", "value", true)],
        ),
        ]
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_name_what_a_file_defines() {
        let src = "ctl cutoff = 1khz\ninstr tone(freq: hz, amp: db = -6db) { osc(sine, freq) }\nclip lead { instrument: tone }\nriff = [c4 e4]";
        let s = symbols(src).unwrap();
        let names: Vec<_> = s.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            [
                ("cutoff", SymbolKind::Control),
                ("tone", SymbolKind::Instrument),
                ("lead", SymbolKind::Clip),
                ("riff", SymbolKind::Pattern)
            ]
        );
        assert_eq!(s[1].detail, "instr tone(freq: hz, amp: db = -6db)");
        assert_eq!(&src[s[1].name_span.start..s[1].name_span.end], "tone");
        assert!(symbols("instr (").is_err());
    }
}
