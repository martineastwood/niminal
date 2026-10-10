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
    for (item, span) in parse_spanned(source)? {
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
            Item::Clip(b) => (&b.name, SymbolKind::Clip, format!("clip {}", b.name.name)),
            Item::Scene(b) => (&b.name, SymbolKind::Scene, format!("scene {}", b.name.name)),
            Item::Arrangement(d) => (&d.name, SymbolKind::Arrangement, format!("arrangement {}", d.name.name)),
            Item::Bind { name, .. } => (name, SymbolKind::Pattern, first_line()),
            _ => continue,
        };
        out.push(Symbol { name: name.name.clone(), kind, name_span: name.span, span, detail, params: param_list });
    }
    Ok(out)
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
