//! Unit checking and lowering, in one pass: each expression is typed and turned
//! into engine graph nodes as it is visited.

use std::collections::HashMap;
use std::sync::Arc;

use niminal_engine::ops::{Add, Curve, Env, Mul, Segment, Sub};
use niminal_engine::{GraphBuilder, Src};
use niminal_score::{Tempo, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::opcodes::{self, Kind, Registry};
use crate::program::{Instrument, Param};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

const UNIT_NAMES: [&str; 8] = ["hz", "khz", "db", "sec", "ms", "beat", "beats", "bpm"];

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

pub(crate) fn unknown_unit(unit: &str, span: Span) -> Diagnostic {
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

pub(crate) fn compile_instr(def: &InstrDef, tempo: Tempo, registry: &Registry) -> Res<Instrument> {
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

