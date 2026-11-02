//! Folding literal expressions and arguments to values at compile time.

use super::*;

/// Literal named arguments, checked against `instr`'s parameters.
pub(super) fn literal_args(
    args: &[Arg],
    instr: &Instrument,
    sample_names: &[String],
    tempo: Tempo,
) -> Res<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for arg in args {
        let Some(arg_name) = &arg.name else {
            return Err(
                Diagnostic::new("name the arguments of a note", arg.value.span)
                    .with_help("for example `lead(freq: c4)`"),
            );
        };
        if let Some(paths) = instr.arrays.get(&arg_name.name) {
            fn leaves(e: &Expr, out: &mut Vec<Expr>) -> Res<()> {
                match &e.kind {
                    ExprKind::Array(items) => {
                        for item in items {
                            leaves(item, out)?;
                        }
                    }
                    _ => out.push(e.clone()),
                }
                Ok(())
            }
            let mut values = Vec::new();
            leaves(&arg.value, &mut values)?;
            if values.len() != paths.len() {
                return Err(Diagnostic::new(
                    format!("array `{}` needs {} values", arg_name.name, paths.len()),
                    arg.value.span,
                ));
            }
            for (path, e) in paths.iter().zip(values) {
                let value = literal_value(&e).map_err(|mut d| {
                    d.span = arg.value.span;
                    d
                })?;
                instr
                    .param_value(instr.param(path).unwrap().1, value, tempo)
                    .map_err(|e| arg_error(e, arg.value.span))?;
                if out.insert(path.clone(), value).is_some() {
                    return Err(Diagnostic::new(
                        "array argument given more than once",
                        arg_name.span,
                    ));
                }
            }
            continue;
        }
        let (_, param) = instr
            .param(&arg_name.name)
            .map_err(|e| arg_error(e, arg_name.span))?;
        let value = literal_arg_value(&arg.value, &arg_name.name, instr, sample_names)?;
        instr
            .param_value(param, value, tempo)
            .map_err(|e| arg_error(e, arg.value.span))?;
        if out.insert(arg_name.name.clone(), value).is_some() {
            return Err(Diagnostic::new(
                format!("argument `{}` given more than once", arg_name.name),
                arg_name.span,
            ));
        }
    }
    Ok(out)
}

/// A note argument: a literal, or a kit sample name for `sample:`.
pub(super) fn literal_arg_value(
    e: &Expr,
    param: &str,
    instr: &Instrument,
    _sample_names: &[String],
) -> Res<Value> {
    if let Some(resources) = instr.resources.get(param) {
        let ExprKind::Name(name) = &e.kind else {
            return Err(Diagnostic::new(
                "buffer arguments are named shared buffers",
                e.span,
            ));
        };
        let index = resources
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| Diagnostic::new(format!("unknown buffer `{name}`"), e.span))?;
        return Ok(Value::Num(index as f64));
    }
    if param == "sample"
        && let ExprKind::Name(name) = &e.kind
        && !instr.members.is_empty()
    {
        if let Some(local) = instr.members.iter().position(|m| m == name) {
            return Ok(Value::Num(local as f64));
        }
        let mut d = Diagnostic::new(
            format!("there is no sample `{name}` in the kit `{}`", instr.name),
            e.span,
        );
        if let Some(c) = closest(name, instr.members.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        return Err(d);
    }
    literal_value(e)
}

/// A note's argument problem as a diagnostic at `span`.
pub(super) fn arg_error(e: crate::program::ArgError, span: Span) -> Diagnostic {
    let d = Diagnostic::new(e.message, span);
    match e.help {
        Some(h) => d.with_help(h),
        None => d,
    }
}

pub(super) fn literal_time(e: &Expr) -> Res<Time> {
    if let Ok((value, unit)) = folded_quantity(e) {
        return match unit.as_deref() {
            Some("sec") => Ok(Time::Seconds(value)),
            Some("ms") => Ok(Time::Seconds(value / 1000.0)),
            Some("beat" | "beats") => Ok(Time::Beats(value)),
            Some("bar" | "bars") => Ok(Time::Bars(value)),
            _ => Err(Diagnostic::new("expected a time", e.span)
                .with_help("for example `1beat`, `2bars` or `250ms`")),
        };
    }
    Err(Diagnostic::new("expected a time", e.span)
        .with_help("for example `1beat`, `2bars` or `250ms`"))
}

/// A number with a unit, including constant arithmetic such as `i * 1/2beat`.
pub(super) fn folded_quantity(e: &Expr) -> Res<(f64, Option<String>)> {
    match &e.kind {
        ExprKind::Num { value, unit } => Ok((*value, unit.clone())),
        ExprKind::Neg(inner) => {
            let (v, u) = folded_quantity(inner)?;
            Ok((-v, u))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let ((l, lu), (r, ru)) = (folded_quantity(lhs)?, folded_quantity(rhs)?);
            let bad = || Diagnostic::new("can't combine these units", e.span);
            match op {
                BinOp::Add | BinOp::Sub if lu == ru => {
                    Ok((if *op == BinOp::Add { l + r } else { l - r }, lu))
                }
                BinOp::Mul if ru.is_none() => Ok((l * r, lu)),
                BinOp::Mul if lu.is_none() => Ok((l * r, ru)),
                BinOp::Div if ru.is_none() && r != 0.0 => Ok((l / r, lu)),
                BinOp::Div if lu == ru && r != 0.0 => Ok((l / r, None)),
                _ => Err(bad()),
            }
        }
        _ => Err(Diagnostic::new("expected a number", e.span)),
    }
}

/// Note arguments are plain values for now: a number with a unit, or a note name.
pub(super) fn literal_value(e: &Expr) -> Res<Value> {
    let bad = || {
        Diagnostic::new("note arguments must be literal values", e.span)
            .with_help("for example `440hz`, `-6db`, `0.5` or `a3`")
    };
    match &e.kind {
        // Note args don't take `%`; value_of does (for pattern atoms).
        ExprKind::Num { value, unit } => match unit.as_deref() {
            Some("%") => Err(unknown_unit("%", e.span)),
            _ => value_of(*value, unit.as_deref(), e.span),
        },
        ExprKind::Name(n) => match n.parse::<Value>() {
            Ok(v @ Value::Note(_)) => Ok(v),
            _ => Err(bad()),
        },
        ExprKind::Neg(inner) => match literal_value(inner)? {
            Value::Note(_) | Value::Sample(_) => Err(bad()),
            v => Ok(negate(v)),
        },
        ExprKind::Binary { op, lhs, rhs } => {
            let (a, b) = (literal_value(lhs)?, literal_value(rhs)?);
            match op {
                BinOp::Add => fold_add(a, b, e.span),
                BinOp::Sub => fold_sub(a, b, e.span),
                BinOp::Mul => fold_mul(a, b, e.span),
                BinOp::Div => fold_div(a, b, e.span),
            }
        }
        _ => Err(bad()),
    }
}

pub(super) fn fold_add(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) | (Value::Semitones(st), Value::Note(n)) => {
            Ok(transpose(Value::Note(n), st))
        }
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a + b)),
        (Value::Db(a), Value::Db(b)) => Ok(Value::Db(a + b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a + b)),
        _ => Err(Diagnostic::new("can't add these note arguments", span)),
    }
}

pub(super) fn fold_sub(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) => Ok(transpose(Value::Note(n), -st)),
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a - b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a - b)),
        _ => Err(Diagnostic::new("can't subtract these note arguments", span)),
    }
}

pub(super) fn fold_mul(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a * b)),
        (Value::Semitones(s), Value::Num(n)) | (Value::Num(n), Value::Semitones(s)) => {
            Ok(Value::Semitones(s * n))
        }
        _ => Err(Diagnostic::new("can't multiply these note arguments", span)),
    }
}

pub(super) fn fold_div(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) if b != 0.0 => Ok(Value::Num(a / b)),
        _ => Err(Diagnostic::new("can't divide these note arguments", span)),
    }
}

/// A number with a unit as a score value: `440hz`, `-6db`, `2bars`.
pub fn value_of(value: f64, unit: Option<&str>, span: Span) -> Res<Value> {
    Ok(match unit {
        None => Value::Num(value),
        Some("hz") => Value::Hz(value),
        Some("khz") => Value::Hz(value * 1000.0),
        Some("db") => Value::Db(value),
        Some("sec") => Value::Seconds(value),
        Some("ms") => Value::Seconds(value / 1000.0),
        Some("beat" | "beats") => Value::Beats(value),
        Some("bar" | "bars") => Value::Bars(value),
        Some("deg") => Value::Degrees(value),
        Some("st") => Value::Semitones(value),
        Some("ct") => Value::Semitones(value / 100.0),
        Some("%") => Value::Num(value / 100.0),
        Some(u) => return Err(crate::lower::unknown_unit(u, span)),
    })
}

/// The same quantity the other way round. A note or a sample keeps its sign.
pub fn negate(v: Value) -> Value {
    match v {
        Value::Num(n) => Value::Num(-n),
        Value::Hz(n) => Value::Hz(-n),
        Value::Db(n) => Value::Db(-n),
        Value::Seconds(n) => Value::Seconds(-n),
        Value::Beats(n) => Value::Beats(-n),
        Value::Bars(n) => Value::Bars(-n),
        Value::Semitones(n) => Value::Semitones(-n),
        Value::Degrees(n) => Value::Degrees(-n),
        keep @ (Value::Note(_) | Value::Sample(_)) => keep,
    }
}

/// Move a pitch by `st` semitones. Notes stay notes when the interval is whole.
pub fn transpose(v: Value, st: f64) -> Value {
    match v {
        Value::Note(m) if st.fract() == 0.0 => Value::Note(m + st as i32),
        Value::Note(_) | Value::Hz(_) => {
            let hz = v.as_hz().expect("notes and frequencies have a frequency");
            Value::Hz(hz * 2f64.powf(st / 12.0))
        }
        other => other,
    }
}
