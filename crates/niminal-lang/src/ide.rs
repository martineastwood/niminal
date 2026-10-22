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
    "after",
    "and",
    "as",
    "buffer",
    "bus",
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
    "fx",
    "if",
    "in",
    "init",
    "instr",
    "it",
    "kit",
    "last",
    "match",
    "midi_out",
    "mono",
    "not",
    "note",
    "or",
    "out",
    "oversample",
    "reset_on",
    "sample",
    "spawn",
    "state",
    "node",
    "true",
    "use",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Instrument,
    Function,
    Control,
    Bus,
    Node,
    Fx,
    Sample,
    Kit,
    Constant,
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
            Item::Const { name, .. } => (name, SymbolKind::Constant, first_line()),
            Item::Control { name, .. } => (name, SymbolKind::Control, first_line()),
            Item::Bus(d) => (&d.name, SymbolKind::Bus, first_line()),
            Item::Buffer(d) => (
                &d.name,
                SymbolKind::Constant,
                format!("buffer {}", d.name.name),
            ),
            Item::Node(d) => (&d.name, SymbolKind::Node, format!("node {}", d.name.name)),
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
        ops.push(op_info("spawn", "Start an independent layer on each positive trigger edge: `spawn(trigger, dur: 100ms, max: 8) { ... }`. Inputs are captured at the trigger; a full pool drops new triggers.", false, &[("trigger", "number", true), ("dur", "time", true), ("max", "constant integer", false), ("body", "function", true)]));
        ops.push(op_info("control", "Run a body once per block instead of every sample: `control { osc(sine, 2hz) }`. Oscillators, envelopes and state inside advance by a block's time, values from outside are read at the start of the block, and the result is held through the block (use `.smooth` or `.smooth(5ms)` on it where a step would be heard). Cheap for slow modulation.", false, &[("body", "function", true)]));
        ops.push(op_info("oversample", "Run sample DSP at 2x, 4x or 8x with filtered rate conversion: `oversample(4) { ... }`. Adds 16 samples of conversion latency; parallel paths are aligned automatically.", false, &[("factor", "constant integer", true), ("body", "function", true)]));
        ops.push(op_info("reset_on", "Restart state created inside `reset_on(trigger) { ... }` at each positive edge, before that sample. Signals outside the scope keep running.", false, &[("trigger", "number", true), ("body", "function", true)]));
        ops.push(op_info("late", "In a send to a bus, a copy that arrives one block late: `echo += late(out)`. Breaks a loop between buses; `prev` is one sample.", false, &[("x", "number", true)]));
        ops.push(op_info("after", "Run `after(delay) { ... }` from when the note is `delay` old: the body starts with fresh state and is silent until then.", false, &[("delay", "time", true), ("body", "function", true)]));
        ops
    })
    .clone()
}

/// Collection operations exposed alongside scalar DSP operations.
fn array_ops() -> Vec<OpInfo> {
    let method = |name, doc, params| op_info(name, doc, true, params);
    vec![
        method(
            "len",
            "The fixed length of an array or number of signal channels.",
            &[],
        ),
        method(
            "indices",
            "The array's indices as a range, from zero to its length (excluded).",
            &[],
        ),
        method(
            "sum",
            "Add array elements or mix signal channels. Also works on writable buffers.",
            &[],
        ),
        method(
            "min",
            "The smallest array element, or element-wise minimum with another value.",
            &[("other", "value", false)],
        ),
        method(
            "max",
            "The largest array element, or element-wise maximum with another value.",
            &[("other", "value", false)],
        ),
        method(
            "map",
            "Apply a function to each current array element: `xs.map(it * 2)`.",
            &[("function", "function", true)],
        ),
        method(
            "step",
            "A range as an array, one entry each `amount`: `(0..1).step(0.25)`. The amount is in the range's unit.",
            &[("amount", "number", true)],
        ),
        method(
            "active",
            "Keep the first `count` entries and silence the rest; `count` can change while playing.",
            &[("count", "number", true)],
        ),
        method("channels", "A signal's channels as an ordinary array.", &[]),
        method(
            "at",
            "Read a table at a fractional position. Interpolation: none, linear (default) or cubic; edge: wrap or clamp.",
            &[
                ("position", "number", true),
                ("interp", "interpolation", false),
                ("edge", "wrap | clamp", false),
            ],
        ),
        method(
            "phase",
            "Read a table at a normalized 0..1 position.",
            &[
                ("position", "number", true),
                ("interp", "interpolation", false),
            ],
        ),
        method(
            "waveshape",
            "Map a signal in -1..1 through a transfer table.",
            &[("table", "table", true)],
        ),
        method(
            "normalize",
            "Scale a table so its peak absolute value is 1.",
            &[],
        ),
        method("reverse", "Reverse a table.", &[]),
        method(
            "rotate",
            "Rotate a table by a whole number of samples.",
            &[("amount", "integer", true)],
        ),
        method(
            "resample",
            "Resample a table to a new length.",
            &[("size", "integer", true)],
        ),
        method(
            "concat",
            "Concatenate two tables.",
            &[("other", "table", true)],
        ),
        method("integrate", "Running sum of a table, then normalize.", &[]),
        method(
            "mix",
            "Crossfade two tables.",
            &[("other", "table", true), ("amount", "number", false)],
        ),
        method(
            "record",
            "Write a signal into a shared buffer.",
            &[("x", "number", true), ("loop", "bool", false)],
        ),
        method("data", "The underlying table of a sample.", &[]),
        op_info(
            "zeros",
            "A fixed-length array of zeros, including an empty array for zero.",
            false,
            &[("count", "integer", true)],
        ),
        op_info(
            "harmonics",
            "One cycle from harmonic amplitudes.",
            false,
            &[
                ("amps", "array", true),
                ("size", "integer", false),
                ("phases", "array", false),
            ],
        ),
        op_info(
            "partials",
            "One cycle from inharmonic partials `[ratio, amp]` or `[ratio, amp, phase]`.",
            false,
            &[("partials", "array", true), ("size", "integer", false)],
        ),
        op_info(
            "cosines",
            "One cycle from a cosine series.",
            false,
            &[("amps", "array", true), ("size", "integer", false)],
        ),
        op_info(
            "curve",
            "Breakpoint curve through `[x, y]` points.",
            false,
            &[
                ("points", "array", true),
                ("size", "integer", false),
                ("shape", "linear | exp | cubic | spline", false),
            ],
        ),
        op_info(
            "steps",
            "Step function from x and y arrays.",
            false,
            &[
                ("xs", "array", true),
                ("ys", "array", true),
                ("size", "integer", false),
            ],
        ),
        op_info(
            "poly",
            "Polynomial over -1..1.",
            false,
            &[("coeffs", "array", true), ("size", "integer", false)],
        ),
        op_info(
            "ramp",
            "Linear ramp from start to end.",
            false,
            &[
                ("start", "number", true),
                ("end", "number", true),
                ("size", "integer", false),
            ],
        ),
        op_info(
            "cheby",
            "Chebyshev transfer function for waveshaping.",
            false,
            &[("amps", "array", true), ("size", "integer", false)],
        ),
        op_info(
            "window",
            "Window function: hann, hamming, blackman, kaiser(beta), gauss, tukey, sinc, tri.",
            false,
            &[("kind", "window", true), ("size", "integer", false)],
        ),
        op_info(
            "random",
            "Deterministic random table; seed is required.",
            false,
            &[
                ("size", "integer", true),
                ("dist", "distribution", false),
                ("seed", "integer", true),
            ],
        ),
        op_info(
            "file",
            "Load one channel of a WAV, or one column of a .csv/.txt/.dat file of numbers, as a table. `{n}` or `{n:2}` in the path is a constant.",
            false,
            &[
                ("path", "text", true),
                ("channel", "integer", false),
                ("start", "integer", false),
                ("end", "integer", false),
            ],
        ),
        op_info(
            "bank",
            "A wavetable bank for osc(..., position:).",
            false,
            &[("tables", "array", true)],
        ),
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
            "Pass an instrument or fx signal through and send its value over OSC: `out.osc_out(\"/vj/level\", port: 9000)`. `host` is this computer unless given; `rate` is 30hz unless given.",
            &[
                ("x", "number", true),
                ("address", "text", true),
                ("port", "number", true),
                ("host", "text", false),
                ("rate", "hz", false),
                ("on", "trigger", false),
            ],
        ),
        op(
            "input",
            "Channels of the audio input, in an fx: `input(1)` or `input(1..2)`.",
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
        let src = "ctl cutoff = 1khz\ninstr tone(freq: hz, amp: db = -6db) { osc(sine, freq) }\nnode lead { instrument = tone }\nconst riff = 4";
        let s = symbols(src).unwrap();
        let names: Vec<_> = s.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            [
                ("cutoff", SymbolKind::Control),
                ("tone", SymbolKind::Instrument),
                ("lead", SymbolKind::Node),
                ("riff", SymbolKind::Constant)
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
