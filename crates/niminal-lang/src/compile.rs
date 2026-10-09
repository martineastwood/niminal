//! Unit checking and lowering, in one pass: each expression is typed and turned
//! into engine graph nodes as it is visited.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use niminal_engine::ops::{Add, Curve, Env, Mul, Segment, Sub};
use niminal_engine::{GraphBuilder, Src};
use niminal_score::{Event, Tempo, Time, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::kernel::compile_opcode;
use crate::opcodes::{self, Kind, Registry};
use crate::parser;
use crate::program::{Instrument, Param, Program};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

const DEFAULT_BPM: f64 = 120.0;
const UNIT_NAMES: [&str; 8] = ["hz", "khz", "db", "sec", "ms", "beat", "beats", "bpm"];

pub fn compile(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let items = parser::parse(source).map_err(|d| vec![d])?;
    let mut errors = Vec::new();

    let tempo = match tempo_of(&items) {
        Ok(t) => t,
        Err(d) => {
            errors.push(d);
            Tempo { bpm: DEFAULT_BPM }
        }
    };

    let mut registry = Registry::builtin();
    for item in &items {
        let Item::Opcode(def) = item else { continue };
        match compile_opcode(def, &registry, tempo) {
            Ok((spec, body_error)) => {
                errors.extend(body_error);
                registry.add(spec);
            }
            Err(d) => errors.push(d),
        }
    }

    let mut instruments: Vec<Instrument> = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    for item in &items {
        let Item::Instr(def) = item else { continue };
        let name = def.name.name.as_str();
        if instruments.iter().any(|i| i.name == name) || failed.contains(&name) {
            errors.push(Diagnostic::new(format!("instrument `{name}` is defined twice"), def.name.span));
            continue;
        }
        match compile_instr(def, tempo, &registry) {
            Ok(i) => instruments.push(i),
            Err(d) => {
                errors.push(d);
                failed.push(name);
            }
        }
    }

    let mut notes = Vec::new();
    for item in &items {
        let Item::Note(note) = item else { continue };
        if failed.contains(&note.target.name.as_str()) {
            continue; // already reported against the instrument
        }
        match check_note(note, &instruments, tempo) {
            Ok(e) => notes.push(e),
            Err(d) => errors.push(d),
        }
    }

    if errors.is_empty() { Ok(Program { tempo, instruments, notes }) } else { Err(errors) }
}

// ---- program-level statements -------------------------------------------

fn tempo_of(items: &[Item]) -> Res<Tempo> {
    let mut tempo = None;
    for item in items {
        let Item::Tempo(e) = item else { continue };
        if tempo.is_some() {
            return Err(Diagnostic::new("tempo is set more than once", e.span));
        }
        match &e.kind {
            ExprKind::Num { value, unit: Some(u) } if u == "bpm" && *value > 0.0 => {
                tempo = Some(Tempo { bpm: *value });
            }
            _ => {
                return Err(Diagnostic::new("tempo must be a positive number of bpm", e.span)
                    .with_help("for example `tempo 120bpm`"));
            }
        }
    }
    Ok(tempo.unwrap_or(Tempo { bpm: DEFAULT_BPM }))
}

fn check_note(note: &NoteStmt, instruments: &[Instrument], tempo: Tempo) -> Res<Event> {
    let name = &note.target;
    let Some(instr) = instruments.iter().find(|i| i.name == name.name) else {
        let mut d = Diagnostic::new(format!("no instrument named `{}`", name.name), name.span);
        if let Some(c) = closest(&name.name, instruments.iter().map(|i| i.name.as_str())) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        return Err(d);
    };

    let at = note.at.as_ref().map_or(Ok(Time::Beats(0.0)), literal_time)?;
    let dur = literal_time(&note.dur)?;

    let mut args = BTreeMap::new();
    for arg in &note.args {
        let Some(arg_name) = &arg.name else {
            return Err(Diagnostic::new("name the arguments of a note", arg.value.span)
                .with_help("for example `lead(freq: c4)`"));
        };
        let (_, param) = instr.param(&arg_name.name).map_err(|e| arg_error(e, arg_name.span))?;
        let value = literal_value(&arg.value)?;
        instr.param_value(param, value, tempo).map_err(|e| arg_error(e, arg.value.span))?;
        if args.insert(arg_name.name.clone(), value).is_some() {
            return Err(Diagnostic::new(format!("argument `{}` given more than once", arg_name.name), arg_name.span));
        }
    }
    instr.bind_args(&args, tempo).map_err(|e| arg_error(e, note.span))?;

    Ok(Event { target: name.name.clone(), at, dur, args })
}

fn arg_error(e: crate::program::ArgError, span: Span) -> Diagnostic {
    let d = Diagnostic::new(e.message, span);
    match e.help {
        Some(h) => d.with_help(h),
        None => d,
    }
}

fn literal_time(e: &Expr) -> Res<Time> {
    if let ExprKind::Num { value, unit: Some(u) } = &e.kind {
        match u.as_str() {
            "sec" => return Ok(Time::Seconds(*value)),
            "ms" => return Ok(Time::Seconds(value / 1000.0)),
            "beat" | "beats" => return Ok(Time::Beats(*value)),
            _ => {}
        }
    }
    Err(Diagnostic::new("expected a time", e.span).with_help("for example `1beat`, `2beats` or `250ms`"))
}

/// Note arguments are plain values for now: a number with a unit, or a note name.
fn literal_value(e: &Expr) -> Res<Value> {
    let bad = || {
        Diagnostic::new("note arguments must be literal values", e.span)
            .with_help("for example `440hz`, `-6db`, `0.5` or `a3`")
    };
    match &e.kind {
        ExprKind::Num { value, unit } => match unit.as_deref() {
            None => Ok(Value::Num(*value)),
            Some("hz") => Ok(Value::Hz(*value)),
            Some("khz") => Ok(Value::Hz(value * 1000.0)),
            Some("db") => Ok(Value::Db(*value)),
            Some("sec") => Ok(Value::Seconds(*value)),
            Some("ms") => Ok(Value::Seconds(value / 1000.0)),
            Some("beat" | "beats") => Ok(Value::Beats(*value)),
            Some(u) => Err(unknown_unit(u, e.span)),
        },
        ExprKind::Name(n) => match n.parse::<Value>() {
            Ok(v @ Value::Note(_)) => Ok(v),
            _ => Err(bad()),
        },
        ExprKind::Neg(inner) => match literal_value(inner)? {
            Value::Num(n) => Ok(Value::Num(-n)),
            Value::Hz(n) => Ok(Value::Hz(-n)),
            Value::Db(n) => Ok(Value::Db(-n)),
            Value::Seconds(n) => Ok(Value::Seconds(-n)),
            Value::Beats(n) => Ok(Value::Beats(-n)),
            Value::Note(_) => Err(bad()),
        },
        _ => Err(bad()),
    }
}

/// A literal's unit and value. Decibels become gain factors and beats become
/// seconds at `tempo`.
pub(crate) fn literal_sig(value: f64, unit: Option<&str>, tempo: Tempo, span: Span) -> Res<(Unit, f64)> {
    Ok(match unit {
        None => (Unit::Num, value),
        Some("hz") => (Unit::Hz, value),
        Some("khz") => (Unit::Hz, value * 1000.0),
        Some("db") => (Unit::Db, 10f64.powf(value / 20.0)),
        Some("sec") => (Unit::Time, value),
        Some("ms") => (Unit::Time, value / 1000.0),
        Some("beat" | "beats") => (Unit::Time, value * 60.0 / tempo.bpm),
        Some("bpm") => {
            return Err(Diagnostic::new("`bpm` can only be used with `tempo`", span)
                .with_help("for example `tempo 120bpm`"));
        }
        Some(u) => return Err(unknown_unit(u, span)),
    })
}

pub(crate) fn unknown_name<'a>(name: &str, span: Span, known: impl IntoIterator<Item = &'a str>) -> Diagnostic {
    let mut d = Diagnostic::new(format!("`{name}` is not defined"), span);
    if let Some(c) = closest(name, known) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

fn unknown_unit(unit: &str, span: Span) -> Diagnostic {
    let mut d = Diagnostic::new(format!("unknown unit `{unit}`"), span);
    if let Some(c) = closest(unit, UNIT_NAMES) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

// ---- instruments -----------------------------------------------------------

#[derive(Clone, Copy)]
struct Sig {
    src: Src,
    unit: Unit,
    /// Known at compile time. Decibel values are held as linear gain factors.
    konst: Option<f64>,
}

impl Sig {
    fn constant(unit: Unit, v: f64) -> Sig {
        Sig { src: Src::Const(v as f32), unit, konst: Some(v) }
    }

    fn signal(src: Src, unit: Unit) -> Sig {
        Sig { src, unit, konst: None }
    }
}

pub(crate) struct Lower<'a> {
    registry: &'a Registry,
    g: GraphBuilder,
    tempo: Tempo,
    scope: HashMap<String, Sig>,
}

fn compile_instr(def: &InstrDef, tempo: Tempo, registry: &Registry) -> Res<Instrument> {
    let mut lower = Lower::new(registry, tempo);
    let mut params: Vec<Param> = Vec::new();

    for p in &def.params {
        if params.iter().any(|q| q.name == p.name.name) {
            return Err(Diagnostic::new(format!("parameter `{}` is declared twice", p.name.name), p.name.span));
        }
        let Some(ty) = &p.ty else {
            return Err(Diagnostic::new(format!("parameter `{}` needs a type", p.name.name), p.name.span)
                .with_help(format!("for example `{}: hz`", p.name.name)));
        };
        let (unit, range) = param_type(ty)?;
        let default = match &p.default {
            None => None,
            Some(e) => Some(lower.param_default(e, unit, range, &p.name.name)?),
        };
        let src = lower
            .g
            .param(&p.name.name, default.unwrap_or(0.0))
            .expect("duplicates checked above");
        lower.scope.insert(p.name.name.clone(), Sig::signal(src, unit));
        params.push(Param { name: p.name.name.clone(), unit, range, default });
    }

    let Some((last, init)) = def.body.split_last() else {
        return Err(Diagnostic::new(format!("instrument `{}` is empty", def.name.name), def.name.span)
            .with_help("the last line of an instrument is the signal it outputs"));
    };
    for stmt in init {
        match stmt {
            Stmt::Bind { name, value } => {
                let sig = lower.expr(value)?;
                lower.scope.insert(name.name.clone(), sig);
            }
            Stmt::State { name, .. } => return Err(state_outside_opcode(name)),
            Stmt::Expr(e) => {
                return Err(Diagnostic::new("this value is never used", e.span)
                    .with_help("bind it with `name = ...`, or make it the last line"));
            }
        }
    }
    let out_expr = match last {
        Stmt::Expr(e) => e,
        Stmt::State { name, .. } => return Err(state_outside_opcode(name)),
        Stmt::Bind { name, .. } => {
            return Err(Diagnostic::new("an instrument must end with the signal it outputs", name.span)
                .with_help(format!("add a last line such as `{}`", name.name)));
        }
    };
    let out = lower.expr(out_expr)?;
    if out.unit != Unit::Num {
        return Err(Diagnostic::new(
            format!("an instrument outputs a plain signal, but this is {}", out.unit.describe()),
            out_expr.span,
        ));
    }

    Ok(Instrument { name: def.name.name.clone(), graph: Arc::new(lower.g.build(out.src)), params })
}

fn state_outside_opcode(name: &Ident) -> Diagnostic {
    Diagnostic::new("`state` can only be used inside an `opcode`", name.span)
        .with_help("an instrument's filters and delays keep their own state")
}

pub(crate) fn param_type(ty: &TypeSpec) -> Res<(Unit, Option<(f64, f64)>)> {
    match ty {
        TypeSpec::Range { lo, hi, span } => {
            if lo >= hi {
                return Err(Diagnostic::new("the lower bound of a range must be below the upper bound", *span));
            }
            Ok((Unit::Num, Some((*lo, *hi))))
        }
        TypeSpec::Unit(id) => match id.name.as_str() {
            "hz" => Ok((Unit::Hz, None)),
            "db" => Ok((Unit::Db, None)),
            "sec" => Ok((Unit::Time, None)),
            other => Err(Diagnostic::new(format!("unknown parameter type `{other}`"), id.span)
                .with_help("use a unit (`hz`, `db`, `sec`) or a range such as `0..1`")),
        },
    }
}

impl<'a> Lower<'a> {
    pub(crate) fn new(registry: &'a Registry, tempo: Tempo) -> Self {
        Lower { registry, g: GraphBuilder::new(), tempo, scope: HashMap::new() }
    }

    pub(crate) fn param_default(&mut self, e: &Expr, unit: Unit, range: Option<(f64, f64)>, name: &str) -> Res<f32> {
        let sig = self.expr(e)?;
        let Some(v) = sig.konst else {
            return Err(Diagnostic::new(format!("the default for `{name}` must be a constant"), e.span));
        };
        // A gain parameter may default to a plain linear factor.
        let ok = sig.unit == unit || (unit == Unit::Db && sig.unit == Unit::Num);
        if !ok {
            return Err(self.unit_mismatch(name, unit, sig.unit, e));
        }
        if let Some((lo, hi)) = range
            && !(lo..=hi).contains(&v)
        {
            return Err(Diagnostic::new(format!("the default for `{name}` is outside {lo}..{hi}"), e.span));
        }
        Ok(v as f32)
    }

    fn literal(&self, value: f64, unit: Option<&str>, span: Span) -> Res<Sig> {
        literal_sig(value, unit, self.tempo, span).map(|(unit, v)| Sig::constant(unit, v))
    }

    fn expr(&mut self, e: &Expr) -> Res<Sig> {
        match &e.kind {
            ExprKind::Num { value, unit } => self.literal(*value, unit.as_deref(), e.span),
            ExprKind::Name(name) => self.name(name, e.span),
            ExprKind::Neg(inner) => {
                let s = self.expr(inner)?;
                self.negate(s, e.span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                self.binary(*op, l, r, e.span)
            }
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Env(env) => self.env(env),
        }
    }

    fn name(&self, name: &str, span: Span) -> Res<Sig> {
        if let Some(s) = self.scope.get(name) {
            return Ok(*s);
        }
        if let Ok(note @ Value::Note(_)) = name.parse::<Value>() {
            return Ok(Sig::constant(Unit::Hz, note.as_hz().expect("a note is a frequency")));
        }
        if opcodes::wave(name).is_some() {
            return Err(Diagnostic::new(format!("`{name}` is a waveform, not a value"), span)
                .with_help("waveforms go in the first slot of `osc`, as in `osc(saw, freq)`"));
        }
        Err(unknown_name(name, span, self.scope.keys().map(String::as_str)))
    }

    // ---- arithmetic ----------------------------------------------------

    fn mul(&mut self, a: Sig, b: Sig, unit: Unit) -> Sig {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Sig::constant(unit, x * y);
        }
        let src = self.g.add(Mul, &[("a", a.src), ("b", b.src)]).expect("mul ports");
        Sig::signal(src, unit)
    }

    fn negate(&mut self, s: Sig, span: Span) -> Res<Sig> {
        match (s.unit, s.konst) {
            // A decibel value is held as a gain factor, so negating it inverts it.
            (Unit::Db, Some(k)) => Ok(Sig::constant(Unit::Db, 1.0 / k)),
            (Unit::Db, None) => Err(Diagnostic::new("negating a changing db value isn't supported yet", span)),
            (unit, _) => Ok(self.mul(s, Sig::constant(Unit::Num, -1.0), unit)),
        }
    }

    fn binary(&mut self, op: BinOp, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            BinOp::Mul => "multiply",
            BinOp::Div => "divide",
        };
        let mismatch = || {
            Diagnostic::new(format!("can't {verb} {} and {}", l.unit.describe(), r.unit.describe()), span)
        };

        match op {
            BinOp::Add | BinOp::Sub => {
                if l.unit != r.unit {
                    return Err(mismatch());
                }
                if l.unit == Unit::Db {
                    // Adding decibels multiplies gain factors.
                    let r = if op == BinOp::Sub { self.invert(r, span)? } else { r };
                    return Ok(self.mul(l, r, Unit::Db));
                }
                if let (Some(x), Some(y)) = (l.konst, r.konst) {
                    let v = if op == BinOp::Add { x + y } else { x - y };
                    return Ok(Sig::constant(l.unit, v));
                }
                let src = if op == BinOp::Add {
                    self.g.add(Add, &[("a", l.src), ("b", r.src)])
                } else {
                    self.g.add(Sub, &[("a", l.src), ("b", r.src)])
                };
                Ok(Sig::signal(src.expect("add/sub ports"), l.unit))
            }
            BinOp::Mul => match (l.unit, r.unit) {
                // Applying a gain to a plain value.
                (Unit::Num, Unit::Db) | (Unit::Db, Unit::Num) => Ok(self.mul(l, r, Unit::Num)),
                (Unit::Db, _) | (_, Unit::Db) => Err(mismatch()),
                (Unit::Num, u) | (u, Unit::Num) => Ok(self.mul(l, r, u)),
                _ => Err(mismatch()),
            },
            BinOp::Div => {
                let Some(k) = r.konst else {
                    return Err(Diagnostic::new("dividing by a changing signal isn't supported yet", span));
                };
                if k == 0.0 {
                    return Err(Diagnostic::new("division by zero", span));
                }
                let unit = match (l.unit, r.unit) {
                    (Unit::Db, _) | (_, Unit::Db) => return Err(mismatch()),
                    (u, Unit::Num) => u,
                    (a, b) if a == b => Unit::Num,
                    _ => return Err(mismatch()),
                };
                Ok(self.mul(l, Sig::constant(Unit::Num, 1.0 / k), unit))
            }
        }
    }

    fn invert(&mut self, s: Sig, span: Span) -> Res<Sig> {
        match s.konst {
            Some(k) => Ok(Sig::constant(s.unit, 1.0 / k)),
            None => Err(Diagnostic::new("subtracting a changing db value isn't supported yet", span)),
        }
    }

    // ---- calls ----------------------------------------------------------

    fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Sig> {
        let registry = self.registry;
        let Some(spec) = registry.find(&name.name) else {
            let mut d = Diagnostic::new(format!("unknown opcode `{}`", name.name), name.span);
            if let Some(c) = closest(&name.name, registry.names()) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let op = spec.name.as_str();

        // Match arguments to parameters.
        let mut bound: Vec<Option<&Arg>> = vec![None; spec.params.len()];
        let mut next_positional = 0;
        for arg in args {
            let index = match &arg.name {
                None => {
                    let i = next_positional;
                    next_positional += 1;
                    if i >= spec.positional || i >= spec.params.len() {
                        let mut d = Diagnostic::new(format!("`{op}` takes its other arguments by name"), arg.value.span);
                        if let Some(p) = spec.params.get(i) {
                            d = d.with_help(format!("write `{}: ...`", p.name));
                        }
                        return Err(d);
                    }
                    i
                }
                Some(n) => match spec.params.iter().position(|p| p.name == n.name) {
                    Some(i) => i,
                    None => {
                        let mut d = Diagnostic::new(format!("`{op}` has no argument named `{}`", n.name), n.span);
                        if let Some(c) = closest(&n.name, spec.params.iter().map(|p| p.name.as_str())) {
                            d = d.with_help(format!("did you mean `{c}`?"));
                        }
                        return Err(d);
                    }
                },
            };
            if bound[index].is_some() {
                return Err(Diagnostic::new(
                    format!("`{op}` argument `{}` given more than once", spec.params[index].name),
                    arg.value.span,
                ));
            }
            bound[index] = Some(arg);
        }

        let mut wave = None;
        let mut wired: Vec<(&str, Src)> = Vec::new();
        for (param, arg) in spec.params.iter().zip(bound) {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(format!("`{op}` needs an argument `{}`", param.name), span));
                }
                continue;
            };
            match param.kind {
                Kind::Wave => {
                    let found = match &arg.value.kind {
                        ExprKind::Name(n) => opcodes::wave(n),
                        _ => None,
                    };
                    let Some(w) = found else {
                        let names: Vec<_> = opcodes::WAVES.iter().map(|(n, _)| *n).collect();
                        return Err(Diagnostic::new("expected a waveform", arg.value.span)
                            .with_help(format!("one of {}", names.join(", "))));
                    };
                    wave = Some(w);
                }
                Kind::Signal(expected) => {
                    let sig = self.expr(&arg.value)?;
                    if sig.unit != expected {
                        return Err(self.unit_mismatch(&param.name, expected, sig.unit, &arg.value));
                    }
                    wired.push((&param.name, sig.src));
                }
                Kind::Gain => {
                    let sig = self.expr(&arg.value)?;
                    if !matches!(sig.unit, Unit::Db | Unit::Num) {
                        return Err(Diagnostic::new(
                            format!("`{}` expects db or a plain factor, found {}", param.name, sig.unit.describe()),
                            arg.value.span,
                        ));
                    }
                    wired.push((&param.name, sig.src));
                }
            }
        }

        let Some(opcode) = spec.instantiate(wave) else {
            // The opcode's own body is broken and already reported.
            return Ok(Sig::constant(Unit::Num, 0.0));
        };
        let src = self.g.add(opcode, &wired).expect("ports match the opcode table");
        Ok(Sig::signal(src, Unit::Num))
    }

    fn unit_mismatch(&self, param: &str, expected: Unit, found: Unit, arg: &Expr) -> Diagnostic {
        let mut d = Diagnostic::new(
            format!("`{param}` expects {}, found {}", expected.describe(), found.describe()),
            arg.span,
        );
        if found == Unit::Num {
            let literal = match &arg.kind {
                ExprKind::Num { value, .. } => Some(*value),
                ExprKind::Neg(inner) => match inner.kind {
                    ExprKind::Num { value, .. } => Some(-value),
                    _ => None,
                },
                _ => None,
            };
            d = match (literal, expected.suffix()) {
                (Some(v), Some(suffix)) => d.with_help(format!("did you mean `{v}{suffix}`?")),
                (None, Some(suffix)) => d.with_help(format!("multiply by a value with a unit, such as `* 1{suffix}`")),
                _ => d,
            };
        }
        d
    }

    // ---- envelopes -------------------------------------------------------

    fn env(&mut self, env: &EnvLit) -> Res<Sig> {
        let start = self.env_level(&env.start)?;
        let unit = start.unit;

        let mut segments = Vec::with_capacity(env.segments.len());
        for seg in &env.segments {
            let level = self.env_level(&seg.level)?;
            if level.unit != unit {
                return Err(Diagnostic::new(
                    format!(
                        "envelope levels must all have the same unit: this is {}, the first was {}",
                        level.unit.describe(),
                        unit.describe()
                    ),
                    seg.level.span,
                ));
            }
            let dur = self.literal(seg.dur.value, seg.dur.unit.as_deref(), seg.dur.span)?;
            let curve = match seg.curve {
                None => Curve::Linear,
                Some(CurveKind::Exp) => Curve::Exp,
                Some(CurveKind::Log) => Curve::Log,
                Some(CurveKind::Custom(k)) => Curve::Custom(k as f32),
            };
            segments.push(Segment::new(dur.konst.expect("a literal") as f32, level.konst.unwrap() as f32, curve));
        }

        let env = Env::new(start.konst.unwrap() as f32, segments, env.sustain_at);
        let src = self.g.add(env, &[]).expect("env has no ports");
        Ok(Sig::signal(src, unit))
    }

    fn env_level(&self, n: &EnvNum) -> Res<Sig> {
        let sig = self.literal(n.value, n.unit.as_deref(), n.span)?;
        if sig.unit == Unit::Db {
            return Err(Diagnostic::new("db levels in envelopes aren't supported yet", n.span));
        }
        Ok(sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Program {
        compile(src).unwrap_or_else(|errs| {
            panic!("{}", errs.iter().map(|d| d.render("t", src)).collect::<String>());
        })
    }

    fn errors(src: &str) -> Vec<Diagnostic> {
        compile(src).err().expect("expected compile errors")
    }

    fn first_error(src: &str) -> Diagnostic {
        errors(src).remove(0)
    }

    fn msg(src: &str) -> String {
        first_error(src).message
    }

    fn help(src: &str) -> Option<String> {
        first_error(src).help
    }

    const SAW_LEAD: &str = "
instr saw_lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level
}
saw_lead(freq: a3) for 1beat
";

    #[test]
    fn compiles_the_basic_synth() {
        let p = ok(SAW_LEAD);
        let i = p.instrument("saw_lead").unwrap();
        assert_eq!(i.params.len(), 2);
        assert_eq!(i.params[0], Param { name: "freq".into(), unit: Unit::Hz, range: None, default: None });
        let amp = i.params[1].default.unwrap();
        assert!((amp - 0.501_187).abs() < 1e-5, "-6db is stored as a gain factor: {amp}");

        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].dur, Time::Beats(1.0));
        assert_eq!(p.notes[0].args["freq"], Value::Note(57));
    }

    #[test]
    fn arithmetic_with_units() {
        ok("instr a(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq * 2).lpf(cutoff: freq * (2 + bright * 6)) }");
        ok("instr a(freq: hz, amp: db = -6db) { osc(sine, freq).gain(amp - 6db).gain(0.5) }");
        ok("instr a(freq: hz) { osc(sine, freq + 3hz - 1hz) }");
        ok("instr a(freq: hz) { osc(sine, freq / 2) }");
        ok("instr a() { 0.5 }");
    }

    #[test]
    fn tempo_converts_beats_inside_instruments() {
        let src = "instr a() { osc(sine, 100hz) * env[0 1beat 1] }\na() for 1beat";
        assert_eq!(ok(&format!("tempo 60bpm\n{src}")).tempo.bpm, 60.0);
        assert_eq!(ok(src).tempo.bpm, 120.0);
        assert_eq!(msg("tempo 120bpm\ntempo 90bpm"), "tempo is set more than once");
        assert_eq!(msg("tempo 120"), "tempo must be a positive number of bpm");
        assert_eq!(msg("instr a() { osc(sine, 100bpm) }"), "`bpm` can only be used with `tempo`");
    }

    #[test]
    fn bare_literal_in_a_unit_slot_gets_a_quick_fix() {
        let src = "instr a(f: hz) { osc(saw, f).lpf(cutoff: 800) }";
        let d = first_error(src);
        assert_eq!(d.message, "`cutoff` expects hz, found a plain number");
        assert_eq!(d.help.as_deref(), Some("did you mean `800hz`?"));
        assert_eq!(&src[d.span.start..d.span.end], "800");

        let d = first_error("instr a() { osc(saw, 440) }");
        assert_eq!(d.help.as_deref(), Some("did you mean `440hz`?"));
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: sine) }"), "`sine` is a waveform, not a value");
    }

    #[test]
    fn wrong_units_are_errors() {
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: 6db) }"), "`cutoff` expects hz, found db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).gain(440hz) }"), "`gain` expects db or a plain factor, found hz");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f + 6db) }"), "can't add hz and db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f * f) }"), "can't multiply hz and hz");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f + 1) }"), "can't add hz and a plain number");
        assert_eq!(msg("instr a(f: hz) { f }"), "an instrument outputs a plain signal, but this is hz");
        assert_eq!(msg("instr a(f: hz = 6db) { osc(saw, f) }"), "`f` expects hz, found db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).gain(f * 2db) }"), "can't multiply hz and db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f / 0) }"), "division by zero");
    }

    #[test]
    fn name_errors_suggest_fixes() {
        let d = first_error("instr a(freq: hz) { osc(saw, frq) }");
        assert_eq!(d.message, "`frq` is not defined");
        assert_eq!(d.help.as_deref(), Some("did you mean `freq`?"));
        assert_eq!(help("instr a(f: hz) { osc(saw, f).lpf(cuttoff: 1khz) }").as_deref(), Some("did you mean `cutoff`?"));
        assert_eq!(help("instr a(f: hz) { osc(saw, f).lfp(cutoff: 1khz) }").as_deref(), Some("did you mean `lpf`?"));
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).wobble() }"), "unknown opcode `wobble`");
        assert_eq!(msg("instr a(f: hz) { saw }"), "`saw` is a waveform, not a value");
        assert_eq!(msg("instr a(f: hz) { osc(wobble, f) }"), "expected a waveform");
        assert_eq!(msg("instr a(f: hertz) { osc(saw, f) }"), "unknown parameter type `hertz`");
        assert_eq!(msg("instr a(f: hz) { osc(saw, 440herz) }"), "unknown unit `herz`");
    }

    #[test]
    fn argument_binding_rules() {
        let d = first_error("instr a(f: hz) { osc(saw, f).lpf(2khz) }");
        assert_eq!(d.message, "`lpf` takes its other arguments by name");
        assert_eq!(d.help.as_deref(), Some("write `cutoff: ...`"));
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(res: 0.1) }"), "`lpf` needs an argument `cutoff`");
        assert_eq!(msg("instr a(f: hz) { osc(saw) }"), "`osc` needs an argument `freq`");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f, f) }"), "`osc` takes its other arguments by name");
        assert_eq!(msg("instr a(f: hz) { osc(saw, freq: f, freq: f) }"), "`osc` argument `freq` given more than once");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f, freq: f) }"), "`osc` argument `freq` given more than once");
        // the receiver fills the first parameter, so naming it as well clashes
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(x: f, cutoff: 1hz) }"), "`lpf` argument `x` given more than once");
    }

    #[test]
    fn instrument_shape_errors() {
        assert_eq!(msg("instr a() { }"), "instrument `a` is empty");
        assert_eq!(msg("instr a() { x = 0.5 }"), "an instrument must end with the signal it outputs");
        assert_eq!(msg("instr a() { 0.5\n0.5 }"), "this value is never used");
        assert_eq!(msg("instr a(f: hz, f: hz) { 0.5 }"), "parameter `f` is declared twice");
        assert_eq!(msg("instr a() { 0.5 }\ninstr a() { 0.5 }"), "instrument `a` is defined twice");
        assert_eq!(msg("instr a(b: 0..1 = 2) { b }"), "the default for `b` is outside 0..1");
        assert_eq!(msg("instr a(b: 1..0) { b }"), "the lower bound of a range must be below the upper bound");
    }

    #[test]
    fn envelope_unit_rules() {
        ok("instr a() { osc(sine, env[200hz 10ms 6khz 300ms.exp 900hz]) }");
        assert!(msg("instr a() { osc(sine, env[0 10ms 6khz]) }").starts_with("envelope levels must all have the same unit"));
        assert_eq!(msg("instr a() { env[-6db 10ms 0db] }"), "db levels in envelopes aren't supported yet");
    }

    #[test]
    fn note_checking() {
        let base = "instr lead(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq) }\n";
        let note = |n: &str| format!("{base}{n}");

        let p = ok(&note("at 2beats lead(freq: c4, bright: 0.2) for 250ms"));
        assert_eq!(p.notes[0].at, Time::Beats(2.0));
        assert_eq!(p.notes[0].dur, Time::Seconds(0.25));

        let d = first_error(&note("lead(freq: 440) for 1beat"));
        assert_eq!(d.message, "argument `freq`: expected a frequency (hz or a note name), got `440`");
        assert_eq!(d.help.as_deref(), Some("did you mean `440hz`?"));

        assert_eq!(msg(&note("lead() for 1beat")), "`lead` needs an argument `freq`");
        assert_eq!(msg(&note("lead(freq: c4, bright: 1.5) for 1beat")), "argument `bright` must be between 0 and 1, got 1.5");
        assert_eq!(msg(&note("lead(freq: c4, brigt: 1) for 1beat")), "`lead` has no parameter `brigt`");
        assert_eq!(help(&note("lead(freq: c4, brigt: 1) for 1beat")).as_deref(), Some("did you mean `bright`?"));
        assert_eq!(msg(&note("lede(freq: c4) for 1beat")), "no instrument named `lede`");
        assert_eq!(msg(&note("lead(c4) for 1beat")), "name the arguments of a note");
        assert_eq!(msg(&note("lead(freq: c4) for 440hz")), "expected a time");
        assert_eq!(msg(&note("lead(freq: c4, freq: c5) for 1beat")), "argument `freq` given more than once");
        assert_eq!(msg(&note("lead(freq: c4 + 1st) for 1beat")), "note arguments must be literal values");
    }

    const SPEC_OPCODES: &str = "
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}

opcode drive(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 9)).tanh
}

opcode dc_block(x) {
  state x1 = 0.0
  state y1 = 0.0
  y = x - x1 + 0.995 * y1
  x1 = x
  y1 = y
  y
}

opcode fold(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 8)).sin
}
";

    #[test]
    fn the_specs_example_opcodes_compile_and_can_be_used() {
        ok(&format!(
            "{SPEC_OPCODES}
instr a(freq: hz) {{ osc(saw, freq).drive(amount: 0.3).one_pole(cutoff: 800hz) }}
instr b(freq: hz) {{ osc(sine, freq).fold(amount: env[0 2sec 1 2sec 0]).dc_block }}
instr c(freq: hz) {{ drive(osc(saw, freq)) }}
a(freq: 110hz) for 1beat"
        ));
    }

    #[test]
    fn opcode_arguments_are_checked_like_any_other() {
        let with = |call: &str| format!("{SPEC_OPCODES}\ninstr a(freq: hz) {{ {call} }}");
        assert_eq!(msg(&with("osc(saw, freq).one_pole(cutoff: 800)")), "`cutoff` expects hz, found a plain number");
        assert_eq!(help(&with("osc(saw, freq).one_pole(cutoff: 800)")).as_deref(), Some("did you mean `800hz`?"));
        assert_eq!(msg(&with("osc(saw, freq).one_pole()")), "`one_pole` needs an argument `cutoff`");
        assert_eq!(msg(&with("osc(saw, freq).drive(amont: 0.3)")), "`drive` has no argument named `amont`");
        assert_eq!(msg(&with("osc(saw, freq).drive(0.3)")), "`drive` takes its other arguments by name");
        ok(&with("osc(saw, freq).drive"));
    }

    #[test]
    fn opcode_body_errors() {
        let body = |b: &str| format!("opcode f(x, amount: 0..1 = 0.5, cutoff: hz = 100hz) {{ {b} }}");
        assert_eq!(msg(&body("x * undefined_thing")), "`undefined_thing` is not defined");
        assert_eq!(msg(&body("exp(cutoff)")), "`exp` expects a plain number, found hz");
        assert_eq!(msg(&body("x + cutoff")), "can't add a plain number and hz");
        assert_eq!(msg(&body("cutoff")), "an opcode outputs a plain signal, but this is hz");
        assert_eq!(msg(&body("x.lpf(cutoff: 100hz)")), "`lpf` can't be used inside an opcode");
        assert_eq!(msg(&body("x.exxp")), "`exxp` can't be used inside an opcode");
        assert_eq!(help(&body("x.exxp")).as_deref(), Some("did you mean `exp`?"));
        assert_eq!(msg(&body("exp(x, x)")), "`exp` takes one argument");
        assert_eq!(msg(&body("env[0 1sec 1]")), "envelopes can't be used inside an opcode");
        assert_eq!(msg(&body("")), "opcode `f` is empty");
        assert_eq!(msg(&body("state s = x\ns")), "the starting value of a state must be a number");
        assert_eq!(msg(&body("state s = 0.0\ns = cutoff\ns")), "`s` holds a plain number, but this is hz");
        assert_eq!(msg(&body("state x = 0.0\nx")), "`x` is already defined");
        assert_eq!(msg(&body("x = 1")), "an opcode must end with the value it outputs");
        assert_eq!(msg(&body("x\nx")), "this value is never used");
    }

    #[test]
    fn opcode_signature_errors() {
        assert_eq!(msg("opcode lpf(x) { x }"), "`lpf` is already an opcode");
        assert_eq!(msg("opcode f(x) { x }\nopcode f(x) { x }"), "`f` is already an opcode");
        assert_eq!(msg("opcode f(x, x) { x }"), "parameter `x` is declared twice");
        assert_eq!(msg("opcode f(x, a: 0..1 = 3) { x }"), "the default for `a` is outside 0..1");
        assert_eq!(msg("opcode f(x, a: furlongs) { x }"), "unknown parameter type `furlongs`");
    }

    #[test]
    fn state_and_untyped_parameters_belong_to_the_right_definitions() {
        assert_eq!(msg("instr a() { state y = 0.0\n0.5 }"), "`state` can only be used inside an `opcode`");
        assert_eq!(msg("instr a(freq) { osc(saw, freq) }"), "parameter `freq` needs a type");
    }

    #[test]
    fn a_broken_opcode_is_reported_once_even_if_it_is_used() {
        let errs = errors("
opcode bad(x) { x + undefined_thing }
instr a(f: hz) { osc(saw, f).bad }
a(f: 100hz) for 1beat
");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].message, "`undefined_thing` is not defined");
    }

    #[test]
    fn db_gains_apply_to_plain_values() {
        ok("instr a(f: hz, amp: db = -6db) { osc(saw, f) * amp }");
        ok("opcode g(x, amp: db = -6db) { x * amp }\ninstr a(f: hz) { osc(saw, f).g(amp: -3db) }");
    }

    #[test]
    fn reports_errors_from_several_instruments_without_cascading() {
        let errs = errors("
instr a() { osc(saw, 440) }
instr b() { osc(saw, 440) }
a() for 1beat
");
        assert_eq!(errs.len(), 2, "one per broken instrument, none for the note: {errs:?}");
    }
}
