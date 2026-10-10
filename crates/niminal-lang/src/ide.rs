//! What an editor wants to know about code: the names it defines and the
//! opcodes it can call.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::opcodes::{Kind, Registry, WAVES};
use crate::parser::parse_spanned;
use crate::unit::Unit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Instrument,
    Opcode,
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
    /// For an instrument or opcode, its parameters as written (`amp: db = -6db`).
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
        let first_line = || source[span.start..span.end].lines().next().unwrap_or_default().trim().to_string();
        let mut param_list = Vec::new();
        let (name, kind, detail) = match &item {
            Item::Instr(d) => {
                param_list = params(source, &d.params);
                (&d.name, SymbolKind::Instrument, format!("instr {}({})", d.name.name, labels(&param_list)))
            }
            Item::Opcode(d) => {
                param_list = params(source, &d.params);
                (&d.name, SymbolKind::Opcode, format!("opcode {}({})", d.name.name, labels(&param_list)))
            }
            Item::Control { name, .. } => (name, SymbolKind::Control, first_line()),
            Item::Bus(d) => (&d.name, SymbolKind::Bus, first_line()),
            Item::Track(d) => (&d.name, SymbolKind::Track, format!("track {}", d.name.name)),
            Item::Sample(d) => (&d.name, if d.is_kit { SymbolKind::Kit } else { SymbolKind::Sample }, first_line()),
            Item::Clip(b) => {
                param_list = clip_lanes(source, b, &items);
                (&b.name, SymbolKind::Clip, format!("clip {}", b.name.name))
            }
            Item::Scene(b) => (&b.name, SymbolKind::Scene, format!("scene {}", b.name.name)),
            Item::Arrangement(d) => (&d.name, SymbolKind::Arrangement, format!("arrangement {}", d.name.name)),
            Item::Bind { name, .. } => (name, SymbolKind::Pattern, first_line()),
            _ => continue,
        };
        out.push(Symbol { name: name.name.clone(), kind, name_span: name.span, span, detail, params: param_list });
    }
    Ok(out)
}

/// The lanes a clip with an `instrument:` can be given: the instrument's parameters, and the lanes every note has.
fn clip_lanes(source: &str, clip: &NamedBlock, items: &[(Item, Span)]) -> Vec<SymbolParam> {
    let Some((_, instrument)) = clip.entries.iter().find(|(key, _)| key.name == "instrument") else { return Vec::new() };
    let name = match &instrument.kind {
        ExprKind::Call { name, .. } => name.name.as_str(),
        ExprKind::Name(name) => name.as_str(),
        _ => return Vec::new(),
    };
    let Some(def) = items.iter().find_map(|(item, _)| match item {
        Item::Instr(d) if d.name.name == name => Some(d),
        _ => None,
    }) else {
        return Vec::new();
    };
    let mut lanes: Vec<SymbolParam> = params(source, &def.params).into_iter().filter(|p| p.name != "freq").collect();
    lanes.extend(crate::perform::UNIVERSAL_LANES.iter().map(|l| SymbolParam { name: (*l).to_string(), label: (*l).to_string() }));
    lanes
}

fn params(source: &str, params: &[ParamDef]) -> Vec<SymbolParam> {
    params
        .iter()
        .map(|p| {
            let mut label = p.name.name.clone();
            match &p.ty {
                Some(TypeSpec::Unit(unit)) => label += &format!(": {}", unit.name),
                Some(TypeSpec::Range { lo, hi, .. }) => label += &format!(": {lo}..{hi}"),
                None => {}
            }
            if let Some(default) = &p.default {
                label += &format!(" = {}", &source[default.span.start..default.span.end]);
            }
            SymbolParam { name: p.name.name.clone(), label }
        })
        .collect()
}

fn labels(params: &[SymbolParam]) -> String {
    params.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join(", ")
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

/// The operations on patterns, clips and grids, written after one: `riff.every(2, it.reverse)`.
/// The pattern itself is the receiver, so it is not among the parameters.
pub fn pattern_ops() -> Vec<OpInfo> {
    let op = |name: &str, doc: &str, params: &[(&'static str, &'static str, bool)]| OpInfo {
        name: name.to_string(),
        doc: doc.to_string(),
        positional: 1 + params.len(),
        params: params.iter().map(|&(name, ty, required)| ParamInfo { name: name.to_string(), ty, required }).collect(),
    };
    vec![
        op("fast", "Play it this many times faster.", &[("by", "number", true)]),
        op("slow", "Play it this many times slower.", &[("by", "number", true)]),
        op("over", "Stretch it to fill this long, as in `over(4 bars)`.", &[("length", "bars", true)]),
        op("reverse", "Play each cycle backwards.", &[]),
        op("shift", "Move it later by this many bars (earlier if negative).", &[("by", "bars", true)]),
        op("repeat_each", "Play each note this many times within its own length.", &[("times", "number", true)]),
        op("thin", "Drop some notes at random, the same ones every time: `thin(25%)`.", &[("amount", "percent", true)]),
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
}

pub fn builtin_ops() -> Vec<OpInfo> {
    let registry = Registry::builtin();
    registry
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
                    ty: match p.kind {
                        Kind::Wave => "wave",
                        Kind::Gain => "gain",
                        Kind::Layout => "layout",
                        Kind::Signal(Unit::Num) => "number",
                        Kind::Signal(Unit::Hz) => "hz",
                        Kind::Signal(Unit::Db) => "db",
                        Kind::Signal(Unit::Time) => "time",
                        Kind::Signal(Unit::Angle) => "angle",
                        Kind::Signal(Unit::Semitones) => "semitones",
                    },
                    required: p.required,
                })
                .collect(),
        })
        .collect()
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
        assert_eq!(names, [("cutoff", SymbolKind::Control), ("tone", SymbolKind::Instrument), ("lead", SymbolKind::Track), ("riff", SymbolKind::Pattern)]);
        assert_eq!(s[1].detail, "instr tone(freq: hz, amp: db = -6db)");
        assert_eq!(&src[s[1].name_span.start..s[1].name_span.end], "tone");
        assert!(symbols("instr (").is_err());
    }

    #[test]
    fn builtin_opcodes_are_documented() {
        let ops = builtin_ops();
        assert!(ops.len() >= 10);
        assert!(ops.iter().all(|o| !o.doc.is_empty()), "{:?}", ops.iter().filter(|o| o.doc.is_empty()).map(|o| &o.name).collect::<Vec<_>>());
        let lpf = ops.iter().find(|o| o.name == "lpf").unwrap();
        assert_eq!(lpf.signature(), "lpf(x: number, cutoff: hz, res?: number)");
    }
}
