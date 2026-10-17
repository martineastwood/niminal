//! What an editor wants to know about code: the names it defines and the
//! built-in functions it can call.

use std::sync::OnceLock;

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::opcodes::{Kind, Registry, WAVES};
use crate::parser::parse_spanned;
use crate::unit::Unit;

/// Names that mean something to the language itself: reserved for rename checks,
/// and offered as keyword completions in the editor.
pub const RESERVED_WORDS: &[&str] = &[
    "and",
    "arrangement",
    "as",
    "at",
    "buffer",
    "bus",
    "capture",
    "clip",
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
    "meter",
    "midi_out",
    "mono",
    "new",
    "next",
    "not",
    "note",
    "now",
    "or",
    "out",
    "over",
    "panic",
    "reset",
    "reset_on",
    "sample",
    "scene",
    "state",
    "tempo",
    "track",
    "true",
    "unsolo",
    "use",
    "with",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Instrument,
    Function,
    Control,
    Bus,
    Track,
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
                            outputs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                    };
                    format!("fn {}({}){arrow}", d.name.name, labels(&param_list))
                })
            }
            Item::Const { name, .. } => (name, SymbolKind::Pattern, first_line()),
            Item::Control { name, .. } => (name, SymbolKind::Control, first_line()),
            Item::Bus(d) => (&d.name, SymbolKind::Bus, first_line()),
            Item::Track(d) => (&d.name, SymbolKind::Track, format!("track {}", d.name.name)),
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
        SampleSource::Slices { count, .. } if *count >= 1.0 && count.fract() == 0.0 => {
            (0..*count as usize)
                .map(|i| {
                    let name = format!("s{i}");
                    SymbolParam {
                        name: name.clone(),
                        label: name,
                    }
                })
                .collect()
        }
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
    let mut lanes: Vec<SymbolParam> = params(source, &def.params)
        .into_iter()
        .filter(|p| p.name != "freq")
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

/// A built-in opcode, as an editor shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpInfo {
    pub name: String,
    pub doc: String,
    pub params: Vec<ParamInfo>,
    /// How many leading arguments may go without a name. The receiver of `x.f()` is the first.
    pub positional: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    pub name: String,
    /// `hz`, `db`, `time`, `angle`, `semitones`, `number`, `gain`, `wave` or `layout`.
    pub ty: &'static str,
    pub required: bool,
}

impl OpInfo {
    /// `lpf(x, cutoff: hz, res?: number)`
    pub fn signature(&self) -> String {
        let args: Vec<String> = self
            .params
            .iter()
            .map(|p| format!("{}{}: {}", p.name, if p.required { "" } else { "?" }, p.ty))
            .collect();
        format!("{}({})", self.name, args.join(", "))
    }
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

/// The operations on patterns, clips and grids, written after one: `riff.every(2, it.reverse)`.
/// The pattern itself is the receiver, so it is not among the parameters.
pub fn pattern_ops() -> Vec<OpInfo> {
    static OPS: OnceLock<Vec<OpInfo>> = OnceLock::new();
    OPS.get_or_init(|| {
        let op = |name, doc, params| op_info(name, doc, true, params);
        vec![
        op("fast", "Play it this many times faster.", &[("by", "number", true)]),
        op("slow", "Play it this many times slower.", &[("by", "number", true)]),
        op("over", "Stretch it to fill this long, as in `over(4bars)`.", &[("length", "bars", true)]),
        op("reverse", "Play each cycle backwards.", &[]),
        op("shift", "Move it later by this many bars (earlier if negative).", &[("by", "bars", true)]),
        op("repeat_each", "Play each note this many times within its own length.", &[("times", "number", true)]),
        op(
            "thin",
            "Drop some notes at random, the same ones every time: `thin(25%)`. A different `seed` drops different ones.",
            &[("amount", "percent", true), ("seed", "number", false)],
        ),
        op(
            "sometimes",
            "Change a random share of the notes: `sometimes(25%, it.transpose(+12st))`.",
            &[("chance", "percent", true), ("change", "transform", true), ("seed", "number", false)],
        ),
        op(
            "within",
            "Change only the notes that start between two points of the bar: `within(0%, 50%, it.fast(2))`.",
            &[("from", "percent", true), ("to", "percent", true), ("change", "transform", true)],
        ),
        op(
            "swing",
            "Delay every other slot: `swing(amount: 20%, every: 1/8beat)`.",
            &[("amount", "percent", true), ("every", "bars", true)],
        ),
        op(
            "rotate",
            "Start a part later each cycle: `rotate(4)` moves the start on by a quarter every cycle, wrapping after four.",
            &[("parts", "number", true)],
        ),
        op(
            "in_turn",
            "Change a different part of the cycle each cycle: `in_turn(4, it.fast(2))`.",
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
        op("repeat_start", "Loop the start of each cycle to fill it: `repeat_start(25%)`.", &[("length", "percent", true)]),
        op(
            "where",
            "Change only the notes that pass a test: `where(it < c3, it.transpose(+12st))`. The test compares `it` (or `it.freq`) with a note or value.",
            &[("when", "test", true), ("change", "transform", true)],
        ),
        op("gain", "Set how loud each note is: `gain(-6db)` or `gain([0db -6db])`.", &[("level", "db", true)]),
        op("pan", "Place each note among the speakers: `pan(-30deg)` or `pan([-30deg 30deg])`.", &[("angle", "degrees", true)]),
        op("begin", "Start each sample note part way in: `begin(25%)`.", &[("share", "percent", true)]),
        op(
            "left_right",
            "Play the original on the left and a changed copy on the right: `left_right(it.reverse)`. `angle` sets how far out.",
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
        op("palindrome", "Play forwards one cycle, backwards the next.", &[]),
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
            "Apply a change on every nth cycle, starting with the first: `every(2, it.reverse)`. `it` is the pattern.",
            &[("n", "number", true), ("change", "transform", true)],
        ),
        op(
            "layer",
            "Play it together with a changed copy, optionally later: `layer(change: it.transpose(+7st))`.",
            &[("after", "bars", false), ("change", "transform", true)],
        ),
        op("transpose", "Move the notes by an interval: `transpose(+5st)`.", &[("by", "semitones", true)]),
        op(
            "euclid",
            "Spread this many hits over this many steps as evenly as possible, and place the notes on them.",
            &[("hits", "number", true), ("steps", "number", true), ("rotation", "number", false)],
        ),
        op("rhythm", "Give the notes the rhythm of a grid or pattern.", &[("structure", "grid", true)]),
        op("mask", "Keep only the notes where a grid or pattern has a hit.", &[("mask", "grid", true)]),
        op(
            "set",
            "Give the clip these lanes instead of its own, a number or a pattern each: `set(bright: 0.9)`, `set(gain: [0db -6db])`.",
            &[("lane", "value", true)],
        ),
        ]
    })
    .clone()
}

fn kind_ty(kind: Kind) -> &'static str {
    match kind {
        Kind::Wave => "wave",
        Kind::Gain => "gain",
        Kind::Layout => "layout",
        Kind::Signal(Unit::Num) => "number",
        Kind::Signal(Unit::Hz) => "hz",
        Kind::Signal(Unit::Db) => "db",
        Kind::Signal(Unit::Time) => "time",
        Kind::Signal(Unit::Angle) => "angle",
        Kind::Signal(Unit::Semitones) => "semitones",
    }
}

pub fn builtin_ops() -> Vec<OpInfo> {
    static OPS: OnceLock<Vec<OpInfo>> = OnceLock::new();
    OPS.get_or_init(|| {
        let registry = Registry::builtin();
        let mut ops: Vec<OpInfo> = registry
            .names()
            .filter_map(|name| registry.find(name))
            .map(|spec| OpInfo {
                name: spec.name.clone(),
                doc: spec.doc.clone(),
                positional: spec.positional,
                params: spec
                    .params
                    .iter()
                    .map(|p| ParamInfo {
                        name: p.name.clone(),
                        ty: kind_ty(p.kind),
                        required: p.required,
                    })
                    .collect(),
            })
            .collect();
        ops.extend(envelope_ops());
        ops.extend(array_ops());
        ops.push(op_info("reset_on", "Restart state created inside `reset_on(trigger) { ... }` at each positive edge, before that sample. Signals outside the scope keep running.", false, &[("trigger", "number", true), ("body", "function", true)]));
        ops
    })
    .clone()
}

/// Collection operations exposed alongside scalar DSP operations.
fn array_ops() -> Vec<OpInfo> {
    let method = |name, doc, params| op_info(name, doc, true, params);
    vec![
        method("len", "The fixed length of an array or number of signal channels.", &[]),
        method("indices", "The array's indices as a range, from zero to its length (excluded).", &[]),
        method("sum", "Add array elements or mix signal channels. Also works on writable buffers.", &[]),
        method("min", "The smallest array element, or element-wise minimum with another value.", &[("other", "value", false)]),
        method("max", "The largest array element, or element-wise maximum with another value.", &[("other", "value", false)]),
        method("map", "Apply a function to each current array element: `xs.map(it * 2)`.", &[("function", "function", true)]),
        method("channels", "A signal's channels as an ordinary array.", &[]),
        method("at", "Read an array at a fractional, wrapping position. Interpolation: none, linear (default) or cubic.", &[("position", "number", true), ("interp", "interpolation", false)]),
        op_info("zeros", "A fixed-length array of zeros, including an empty array for zero.", false, &[("count", "integer", true)]),
    ]
}

/// `adsr` and `perc`: shortcuts for envelope literals, not wired opcodes.
fn envelope_ops() -> Vec<OpInfo> {
    let op = |name, doc, params| op_info(name, doc, false, params);
    vec![
        op(
            "adsr",
            "Attack, decay, sustain and release: the same as `env[0 attack 1 decay sustain | release 0]`.",
            &[
                ("attack", "time", true),
                ("decay", "time", true),
                ("sustain", "number", true),
                ("release", "time", true),
            ],
        ),
        op(
            "perc",
            "A one-shot attack and exponential decay: the same as `env[0 attack 1 decay.exp 0]`. `attack` defaults to 2ms.",
            &[("attack", "time", false), ("decay", "time", true)],
        ),
        op(
            "range",
            "Map a plain signal from 0 to 1 to a range, evenly (decibels evenly in db): `knob.range(200hz..4khz)`.",
            &[
                ("x", "number", true),
                ("from", "value", true),
                ("to", "value", true),
            ],
        ),
        op(
            "osc_out",
            "Pass a track's signal through and send its value over OSC: `out.osc_out(\"/vj/level\", port: 9000)`. `host` is this computer unless given; `rate` is 30hz unless given.",
            &[
                ("x", "number", true),
                ("address", "text", true),
                ("port", "number", true),
                ("host", "text", false),
                ("rate", "hz", false),
            ],
        ),
        op(
            "input",
            "Channels of the audio input, in a track: `input(1)` or `input(1..2)`.",
            &[("first", "number", true), ("last", "number", false)],
        ),
    ]
}

/// The waveform names `osc` accepts.
pub fn waves() -> impl Iterator<Item = &'static str> {
    WAVES.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_name_what_a_file_defines() {
        let src = "ctl cutoff = 1khz\ninstr tone(freq: hz, amp: db = -6db) { osc(sine, freq) }\ntrack lead { instrument = tone }\nriff = [c4 e4]";
        let s = symbols(src).unwrap();
        let names: Vec<_> = s.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            [
                ("cutoff", SymbolKind::Control),
                ("tone", SymbolKind::Instrument),
                ("lead", SymbolKind::Track),
                ("riff", SymbolKind::Pattern)
            ]
        );
        assert_eq!(s[1].detail, "instr tone(freq: hz, amp: db = -6db)");
        assert_eq!(&src[s[1].name_span.start..s[1].name_span.end], "tone");
        assert!(symbols("instr (").is_err());
    }

    #[test]
    fn builtin_opcodes_are_documented() {
        let ops = builtin_ops();
        assert!(ops.len() >= 10);
        assert!(
            ops.iter().all(|o| !o.doc.is_empty()),
            "{:?}",
            ops.iter()
                .filter(|o| o.doc.is_empty())
                .map(|o| &o.name)
                .collect::<Vec<_>>()
        );
        let lpf = ops.iter().find(|o| o.name == "lpf").unwrap();
        assert_eq!(lpf.signature(), "lpf(x: number, cutoff: hz, res?: number)");
    }
}
