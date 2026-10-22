//! Function signatures: what each parameter accepts, and how a call's arguments
//! are matched to its parameters. Shared by instruments, functions and kernels.

use niminal_score::Tempo;

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::lower::literal_sig;
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

/// What a parameter accepts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// Untyped: any unit, worked out at each call.
    Any,
    Buffer,
    Function,
    /// A value of this unit. `whole` for `int`.
    Unit {
        unit: Unit,
        whole: bool,
    },
    /// A value of this unit between two ends, clamped to them.
    Range {
        unit: Unit,
        lo: f64,
        hi: f64,
    },
    /// One of these names, known while compiling. Inside the body each name is its index.
    Choice(Vec<String>),
    /// A list whose entries have this kind.
    Array(Box<Kind>),
}

impl Kind {
    /// The unit a value must have, if the kind fixes one.
    pub fn unit(&self) -> Option<Unit> {
        match self {
            Kind::Unit { unit, .. } | Kind::Range { unit, .. } => Some(*unit),
            Kind::Choice(_) => Some(Unit::Num),
            Kind::Any | Kind::Buffer | Kind::Function | Kind::Array(_) => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Kind::Any => "any value".into(),
            Kind::Buffer => "a shared buffer".into(),
            Kind::Function => "a function".into(),
            Kind::Unit { whole: true, .. } => "a whole number".into(),
            Kind::Unit { unit, .. } => unit.describe().into(),
            Kind::Range { unit, .. } => unit.describe().into(),
            Kind::Choice(names) => format!("one of {}", names.join(", ")),
            Kind::Array(inner) => format!("an array of {}", inner.describe()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub name: Ident,
    /// The slowest rate the body sees: `const`, `init`, `control`, or none for whatever is passed.
    pub rate: Option<Rate>,
    pub kind: Kind,
    pub default: Option<Expr>,
}

impl Param {
    pub fn required(&self) -> bool {
        self.default.is_none()
    }

    /// The rate the value must be known at: a choice is always known while compiling.
    pub fn rate(&self) -> Option<Rate> {
        if matches!(self.kind, Kind::Choice(_) | Kind::Function) {
            Some(Rate::Const)
        } else {
            self.rate
        }
    }
}

/// A unit written as a type: `hz`, `db`, `sec`, `deg`, `st`, `num` or `int`.
pub(crate) fn unit_type(id: &Ident) -> Res<(Unit, bool)> {
    Ok(match id.name.as_str() {
        "num" => (Unit::Num, false),
        "int" => (Unit::Num, true),
        "hz" => (Unit::Hz, false),
        "db" => (Unit::Db, false),
        "sec" => (Unit::Time, false),
        "deg" => (Unit::Angle, false),
        "st" => (Unit::Semitones, false),
        other => {
            let mut d = Diagnostic::new(format!("unknown type `{other}`"), id.span).with_help(
                "use a unit (`hz`, `db`, `sec`, `deg`, `st`, `num`, `int`), a range such as `0..1`, a choice such as `(low | high)`, or a list such as `[num]`",
            );
            if let Some(c) = closest(other, ["num", "int", "hz", "db", "sec", "deg", "st"]) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        }
    })
}

/// A literal number, with an optional unit and sign, as written in a type.
fn type_number(e: &Expr, tempo: Tempo) -> Res<(Unit, f64)> {
    match &e.kind {
        ExprKind::Num { value, unit } => literal_sig(*value, unit.as_deref(), tempo, e.span),
        ExprKind::Neg(inner) => {
            let (unit, v) = type_number(inner, tempo)?;
            Ok((unit, if unit == Unit::Db { 1.0 / v } else { -v }))
        }
        _ => Err(Diagnostic::new(
            "the ends of a range type are numbers, such as `0..1` or `20hz..20khz`",
            e.span,
        )),
    }
}

pub(crate) fn kind_of(ty: &TypeSpec, tempo: Tempo) -> Res<Kind> {
    Ok(match ty {
        TypeSpec::Unit(id) if id.name == "fn" => Kind::Function,
        TypeSpec::Unit(id) if id.name == "buffer" => Kind::Buffer,
        TypeSpec::Unit(id) => {
            let (unit, whole) = unit_type(id)?;
            Kind::Unit { unit, whole }
        }
        TypeSpec::Range { lo, hi, span } => {
            let ((lu, l), (hu, h)) = (type_number(lo, tempo)?, type_number(hi, tempo)?);
            if lu != hu {
                return Err(Diagnostic::new(
                    format!(
                        "the ends of a range need the same unit: {} and {}",
                        lu.describe(),
                        hu.describe()
                    ),
                    *span,
                ));
            }
            if l >= h {
                return Err(Diagnostic::new(
                    "the lower end of a range must be below the upper end",
                    *span,
                ));
            }
            Kind::Range {
                unit: lu,
                lo: l,
                hi: h,
            }
        }
        TypeSpec::Choice(names) => {
            for (i, n) in names.iter().enumerate() {
                if names[..i].iter().any(|m| m.name == n.name) {
                    return Err(Diagnostic::new(
                        format!("`{}` is listed twice", n.name),
                        n.span,
                    ));
                }
            }
            Kind::Choice(names.iter().map(|n| n.name.clone()).collect())
        }
        TypeSpec::Array(inner, _) => Kind::Array(Box::new(kind_of(inner, tempo)?)),
    })
}

/// The parameters of a function or instrument, checked.
pub(crate) fn signature(params: &[ParamDef], tempo: Tempo) -> Res<Vec<Param>> {
    let mut out: Vec<Param> = Vec::with_capacity(params.len());
    for p in params {
        if out.iter().any(|q| q.name.name == p.name.name) {
            return Err(Diagnostic::new(
                format!("parameter `{}` is declared twice", p.name.name),
                p.name.span,
            ));
        }
        let kind = match &p.ty {
            Some(ty) => kind_of(ty, tempo)?,
            None => Kind::Any,
        };
        if let (Kind::Function, Some((rate, span))) = (&kind, p.rate)
            && rate != Rate::Const
        {
            return Err(Diagnostic::new(
                "a function is always known while compiling",
                span,
            ));
        }
        if let (Kind::Choice(_), Some((rate, span))) = (&kind, p.rate)
            && rate != Rate::Const
        {
            return Err(
                Diagnostic::new("a choice is always known while compiling", span)
                    .with_help("leave out the rate: `mode: (low | high) = low`"),
            );
        }
        if let (Kind::Choice(names), Some(default)) = (&kind, &p.default) {
            match &default.kind {
                ExprKind::Name(n) if names.contains(n) => {}
                _ => {
                    return Err(Diagnostic::new(
                        format!(
                            "the default for `{}` is one of {}",
                            p.name.name,
                            names.join(", ")
                        ),
                        default.span,
                    ));
                }
            }
        }
        out.push(Param {
            name: p.name.clone(),
            rate: p.rate.map(|(r, _)| r),
            kind,
            default: p.default.clone(),
        });
    }
    Ok(out)
}

/// The unit a function's `-> unit` declares.
pub(crate) fn result_unit(result: &FnResult) -> Res<Option<Unit>> {
    match result {
        FnResult::Unit(id) => {
            let (unit, _) = unit_type(id)?;
            Ok(Some(unit))
        }
        FnResult::Inferred | FnResult::Named(_) => Ok(None),
    }
}

/// How many arguments a callee takes without a name: its leading required parameters, and at
/// least the first (the receiver of a method call).
pub(crate) fn positional(required: impl IntoIterator<Item = bool>) -> usize {
    required.into_iter().take_while(|r| *r).count().max(1)
}

/// Which argument goes with each parameter. `params` are the parameter names in order; the
/// first `positional` may be given without a name.
pub(crate) fn bind<'a>(
    callee: &str,
    callee_span: Span,
    params: &[&str],
    positional: usize,
    args: &'a [Arg],
) -> Res<Vec<Option<&'a Arg>>> {
    let mut bound: Vec<Option<&Arg>> = vec![None; params.len()];
    let mut next_positional = 0;
    for arg in args {
        let index = match &arg.name {
            None => {
                let i = next_positional;
                next_positional += 1;
                if i >= positional || i >= params.len() {
                    let mut d = Diagnostic::new(
                        if params.is_empty() {
                            format!("`{callee}` takes no arguments")
                        } else {
                            format!("`{callee}` takes its other arguments by name")
                        },
                        arg.value.span,
                    );
                    if let Some(p) = params.get(i) {
                        d = d.with_help(format!("write `{p}: ...`"));
                    }
                    return Err(d);
                }
                i
            }
            Some(n) => params.iter().position(|p| *p == n.name).ok_or_else(|| {
                let mut d = Diagnostic::new(
                    format!("`{callee}` has no argument named `{}`", n.name),
                    n.span,
                );
                if let Some(c) = closest(&n.name, params.iter().copied()) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            })?,
        };
        if bound[index].is_some() {
            return Err(Diagnostic::new(
                format!(
                    "`{callee}` argument `{}` given more than once",
                    params[index]
                ),
                arg.value.span,
            ));
        }
        bound[index] = Some(arg);
    }
    let _ = callee_span;
    Ok(bound)
}

/// Whether a function needs a per-sample kernel: it has `state`, `prev` or writes a state list,
/// or calls a function that does.
pub(crate) fn has_state(def: &FnDef, fns: &std::collections::HashMap<String, FnDef>) -> bool {
    fn visit(
        stmts: &[Stmt],
        fns: &std::collections::HashMap<String, FnDef>,
        seen: &mut Vec<String>,
    ) -> bool {
        stmts.iter().any(|s| {
            matches!(s, Stmt::State { .. } | Stmt::IndexAssign { .. })
                || s.exprs().into_iter().any(|e| expr(e, fns, seen))
        })
    }
    fn expr(
        e: &Expr,
        fns: &std::collections::HashMap<String, FnDef>,
        seen: &mut Vec<String>,
    ) -> bool {
        let blocks: Vec<&[Stmt]> = match &e.kind {
            ExprKind::If {
                then, otherwise, ..
            } => std::iter::once(then.as_slice())
                .chain(otherwise.as_deref())
                .collect(),
            ExprKind::For { body, .. } | ExprKind::Lambda { body, .. } => vec![body],
            _ => Vec::new(),
        };
        if blocks.into_iter().any(|b| visit(b, fns, seen)) {
            return true;
        }
        if let ExprKind::Call { name, args } = &e.kind {
            if name.name == "control"
                && matches!(args.as_slice(), [a] if !a.receiver && matches!(a.value.kind, ExprKind::Lambda { .. }))
            {
                // a control scope owns the state inside it
                return false;
            }
            if matches!(
                name.name.as_str(),
                "prev" | "dot" | "fir" | "taps" | "matvec" | "frames" | "fft" | "ifft"
            ) {
                return true;
            }
            if let Some(def) = fns.get(&name.name)
                && !seen.contains(&name.name)
            {
                seen.push(name.name.clone());
                if visit(&def.body, fns, seen) {
                    return true;
                }
            }
        }
        let mut found = false;
        e.for_each_child(&mut |c| found = found || expr(c, fns, seen));
        found
    }
    visit(&def.body, fns, &mut vec![def.name.name.clone()])
}

/// Words that turn a number into a quantity, or take it away: `440.hz`, `2.bars`, `x.num`.
pub(crate) const UNIT_WORDS: [&str; 14] = [
    "num", "hz", "khz", "db", "sec", "ms", "beat", "beats", "bar", "bars", "bpm", "deg", "st", "ct",
];

/// Calls that are not opcodes but are built into the compiler.
const BUILTIN_CALLS: &[&str] = &[
    // arrays and signals
    "map",
    "sum",
    "len",
    "channels",
    "indices",
    "at",
    "phase",
    "step",
    "active",
    "normalize",
    "reverse",
    "rotate",
    "resample",
    "concat",
    "integrate",
    "mix",
    "record",
    "data",
    // numbers
    "min",
    "max",
    "pow",
    "mod",
    "atan2",
    "clamp",
    "hold",
    "smooth",
    "init",
    "control",
    "late",
    "prev",
    "range",
    "zeros",
    // vectors and frames
    "dot",
    "fir",
    "taps",
    "matvec",
    "frames",
    "fft",
    "ifft",
    // tables
    "harmonics",
    "partials",
    "cosines",
    "curve",
    "steps",
    "poly",
    "ramp",
    "cheby",
    "window",
    "random",
    "file",
    "bank",
    // scopes and taps
    "reset_on",
    "spawn",
    "oversample",
    "after",
    "report",
    "osc_out",
    "input",
];

/// Why `name` can't be the name of a function: it is a unit word or something built in.
pub(crate) fn reserved_function_name(
    name: &str,
    registry: &crate::opcodes::Registry,
) -> Option<String> {
    if UNIT_WORDS.contains(&name) {
        return Some(format!(
            "`{name}` is a unit, so it can't be the name of a function"
        ));
    }
    (registry.find(name).is_some()
        || crate::opcodes::math_fn(name).is_some()
        || BUILTIN_CALLS.contains(&name))
    .then(|| format!("`{name}` is a built-in function, so it can't be defined again"))
}

/// Anonymous functions use the same call machinery and state ownership as named functions.
pub(crate) fn lambda(params: &[Ident], body: &[Stmt], span: Span) -> FnDef {
    FnDef {
        name: Ident {
            name: format!("<fn@{}>", span.start),
            span,
        },
        params: params
            .iter()
            .map(|name| ParamDef {
                name: name.clone(),
                rate: None,
                ty: None,
                default: None,
            })
            .collect(),
        result: FnResult::Inferred,
        body: body.to_vec(),
        span,
    }
}

/// Capture only names read by the body. Bindings are resolved before capture is used.
pub(crate) fn reads(def: &FnDef, name: &str) -> bool {
    fn visit(e: &Expr, name: &str) -> bool {
        let mut found = matches!(&e.kind, ExprKind::Name(n) if n == name)
            || matches!(&e.kind, ExprKind::Call { name: n, .. } if n.name == name);
        e.for_each_child(&mut |c| found |= visit(c, name));
        found
    }
    !def.params.iter().any(|p| p.name.name == name)
        && def
            .body
            .iter()
            .flat_map(Stmt::exprs)
            .any(|e| visit(e, name))
}

pub(crate) fn reset_args(args: &[Arg], span: Span) -> Res<(&Expr, &[Stmt])> {
    if let [trigger, body] = args
        && trigger.name.is_none()
        && body.name.is_none()
        && let ExprKind::Lambda { params, body } = &body.value.kind
        && params.is_empty()
    {
        return Ok((&trigger.value, body));
    }
    Err(Diagnostic::new("write `reset_on(trigger) { ... }`", span))
}

/// A named function supplied to map is applied to its implicit element.
pub(crate) fn mapped_call(name: &str, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Call {
            name: Ident {
                name: name.into(),
                span,
            },
            args: vec![Arg {
                name: None,
                receiver: false,
                value: Expr {
                    kind: ExprKind::Name("it".into()),
                    span,
                },
            }],
        },
        span,
    }
}
