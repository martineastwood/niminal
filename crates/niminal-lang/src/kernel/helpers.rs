//! Small helpers shared by the kernel builder and the lowering pass.

use super::*;

/// Whether two values are the same, for names a branch didn't change.
pub(super) fn same(a: &KVal, b: &KVal) -> bool {
    match (a, b) {
        (KVal::Num(x), KVal::Num(y)) => x.reg == y.reg,
        (KVal::Table { id: a, .. }, KVal::Table { id: b, .. })
        | (KVal::State { id: a, .. }, KVal::State { id: b, .. }) => a == b,
        (KVal::Array(a), KVal::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (KVal::Range { lo: a, hi: b, .. }, KVal::Range { lo: c, hi: d, .. }) => a == c && b == d,
        (KVal::Record(a), KVal::Record(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.0 == y.0 && same(&x.1, &y.1))
        }
        _ => false,
    }
}

/// Whether a block's last statement is an expression that gives a value.
pub(crate) fn ends_with_value(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(Stmt::Expr(e)) => match &e.kind {
            ExprKind::If {
                then,
                otherwise: Some(o),
                ..
            } => ends_with_value(then) && ends_with_value(o),
            ExprKind::If {
                otherwise: None, ..
            } => false,
            ExprKind::For { body, .. } => ends_with_value(body),
            _ => true,
        },
        _ => false,
    }
}

pub(super) fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Expr(e) => e.span,
        Stmt::Bind { name, .. }
        | Stmt::State { name, .. }
        | Stmt::AddAssign { name, .. }
        | Stmt::IndexAssign { name, .. } => name.span,
        Stmt::Destructure { value, .. } => value.span,
    }
}

pub(crate) fn cmp_op(op: CmpOp) -> Cmp {
    match op {
        CmpOp::Lt => Cmp::Lt,
        CmpOp::Gt => Cmp::Gt,
        CmpOp::Le => Cmp::Le,
        CmpOp::Ge => Cmp::Ge,
        CmpOp::Eq => Cmp::Eq,
        CmpOp::Ne => Cmp::Ne,
    }
}

/// The whole numbers a range runs over.
pub(crate) fn range_numbers(lo: f64, hi: f64, inclusive: bool, span: Span) -> Res<Vec<f64>> {
    if lo.fract() != 0.0 || hi.fract() != 0.0 {
        return Err(Diagnostic::new(
            "a range runs over whole numbers; use `.step(...)` for others",
            span,
        ));
    }
    let end = if inclusive { hi } else { hi - 1.0 };
    if end < lo {
        return Ok(Vec::new());
    }
    let count = (end - lo) as usize + 1;
    if count > 1 << 20 {
        return Err(Diagnostic::new("this range is too long", span));
    }
    Ok((0..count).map(|i| lo + i as f64).collect())
}

/// The values a range takes in steps of `step`: `(0..1).step(0.25)` is 0, 0.25, 0.5, 0.75 and 1.
pub(crate) fn step_numbers(
    lo: f64,
    hi: f64,
    inclusive: bool,
    step: f64,
    span: Span,
) -> Res<Vec<f64>> {
    if step <= 0.0 {
        return Err(Diagnostic::new("a step must be above 0", span));
    }
    let count = ((hi - lo) / step + 1e-9).floor();
    if count < 0.0 {
        return Ok(Vec::new());
    }
    if count > 65_536.0 {
        return Err(Diagnostic::new("this range has too many steps", span));
    }
    let mut values: Vec<f64> = (0..=count as usize).map(|i| lo + i as f64 * step).collect();
    // `..<` leaves the end out, even when a step lands on it
    if !inclusive
        && values
            .last()
            .is_some_and(|last| (hi - last).abs() < step * 1e-9)
    {
        values.pop();
    }
    Ok(values)
}

/// `interp: none`, `linear` or `cubic`.
pub(crate) fn interp_of(e: &Expr) -> Res<Interp> {
    match &e.kind {
        ExprKind::Name(n) if n == "none" => Ok(Interp::None),
        ExprKind::Name(n) if n == "linear" => Ok(Interp::Linear),
        ExprKind::Name(n) if n == "cubic" => Ok(Interp::Cubic),
        _ => Err(Diagnostic::new(
            "`interp` is `none`, `linear` or `cubic`",
            e.span,
        )),
    }
}

pub(crate) fn edge_of(e: &Expr) -> Res<niminal_engine::Edge> {
    match &e.kind {
        ExprKind::Name(n) if n == "wrap" => Ok(niminal_engine::Edge::Wrap),
        ExprKind::Name(n) if n == "clamp" => Ok(niminal_engine::Edge::Clamp),
        _ => Err(Diagnostic::new("`edge` is `wrap` or `clamp`", e.span)),
    }
}

/// A function given to `map`: `fn(x) { ... }`, or an expression of `it`.
pub(crate) enum Body<'e> {
    Block(&'e [Stmt]),
    Expr(&'e Expr),
}

pub(crate) fn lambda_of(e: &Expr) -> Res<(String, Body<'_>)> {
    match &e.kind {
        ExprKind::Lambda { params, body } => {
            let [param] = params.as_slice() else {
                return Err(Diagnostic::new(
                    "the function given to `map` takes one parameter",
                    e.span,
                ));
            };
            Ok((param.name.clone(), Body::Block(body)))
        }
        _ => Ok(("it".to_string(), Body::Expr(e))),
    }
}
