//! Checking `const` and `fn` definitions on their own.

use super::*;

/// Work out a `const` definition while compiling.
pub(crate) fn compile_const(
    value: &Expr,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Datum> {
    let mut lower = Lower::new(registry, names, tempo);
    let v = lower.value(value)?;
    pub(super) fn known(d: &Datum) -> bool {
        match d {
            Datum::Function { .. } => false,
            Datum::Sig(s) => s.konst.is_some(),
            Datum::Table(_) => true,
            Datum::Buffer { .. } | Datum::BufferRef { .. } => false,
            Datum::Array(items) => items.iter().all(known),
            Datum::Record(fields) => fields.iter().all(|(_, v)| known(v)),
            Datum::Range { lo, hi, .. } => lo.konst.is_some() && hi.konst.is_some(),
        }
    }
    if !known(&v) {
        return Err(
            Diagnostic::new("a `const` must be known while compiling", value.span)
                .with_help("use numbers, ranges, arrays and functions without state"),
        );
    }
    Ok(v)
}

/// Check a function where it is defined, so mistakes in its body are found even if nothing calls it.
/// Untyped parameters take their units from each call, so with one of those only mistakes in names are
/// reported here; the rest are found where it is called.
pub(crate) fn check_fn(def: &FnDef, tempo: Tempo, registry: &Registry, names: &Names) -> Res<()> {
    let params = funcs::signature(&def.params, tempo)?;
    funcs::result_unit(&def.result)?;
    if params
        .iter()
        .any(|p| matches!(p.kind, PKind::Function | PKind::Buffer))
    {
        return Ok(());
    }
    // an untyped parameter takes its unit from each call, and a list parameter its length: the body is
    // checked with two guesses, and a mistake that could come from the guess only counts if both make it
    match check_fn_with(def, tempo, registry, names, Unit::Num, 2) {
        Ok(()) => Ok(()),
        Err((d, true)) if from_a_guess(&d) && reads_untyped(def, d.span) => {
            match check_fn_with(def, tempo, registry, names, Unit::Hz, 3) {
                Err((again, _)) if again.message == d.message && again.span == d.span => Err(d),
                _ => Ok(()),
            }
        }
        Err((d, _)) => Err(d),
    }
}

/// Whether a mistake could come from guessing a unit or a list's length: one about units, or about how
/// many entries a list has.
pub(super) fn from_a_guess(d: &Diagnostic) -> bool {
    let m = d.message.as_str();
    m.starts_with("can't ")
        || m.contains(" expects ")
        || m.contains("same unit")
        || m.contains(" holds ")
        || m.contains(" gives ")
        || m.contains("plain signal")
        || m.contains(" entries")
        || m.contains("an array of ")
        || m.contains("arrays of the same length")
}

/// Whether the body reads an untyped parameter inside `span`.
pub(super) fn reads_untyped(def: &FnDef, span: Span) -> bool {
    let untyped: Vec<&str> = def
        .params
        .iter()
        .filter(|p| p.default.is_none() && matches!(p.ty, None | Some(TypeSpec::Array(..))))
        .map(|p| p.name.name.as_str())
        .collect();
    pub(super) fn visit(e: &Expr, names: &[&str], span: Span, found: &mut bool) {
        if let ExprKind::Name(n) = &e.kind
            && names.contains(&n.as_str())
            && e.span.start >= span.start
            && e.span.end <= span.end
        {
            *found = true;
        }
        e.for_each_child(&mut |c| visit(c, names, span, found));
    }
    let mut found = false;
    for s in &def.body {
        for e in s.exprs() {
            visit(e, &untyped, span, &mut found);
        }
    }
    found
}

/// The error, and whether an untyped parameter was given a signal of unit `guess`, or a list parameter
/// a list of `length` entries, to check the body with.
pub(super) fn check_fn_with(
    def: &FnDef,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
    guess: Unit,
    length: usize,
) -> Result<(), (Diagnostic, bool)> {
    let params = funcs::signature(&def.params, tempo).map_err(|d| (d, false))?;
    funcs::result_unit(&def.result).map_err(|d| (d, false))?;
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut given = Vec::with_capacity(params.len());
    let mut guessed = false;
    for p in &params {
        if p.default.is_some() {
            given.push(None);
            continue;
        }
        let kind = match &p.kind {
            PKind::Any => {
                guessed = true;
                PKind::Unit {
                    unit: guess,
                    whole: false,
                }
            }
            k => k.clone(),
        };
        if p.rate() == Some(Rate::Const)
            && let Some(unit) = kind.unit()
        {
            let v = Datum::Sig(Sig::constant(unit, 1.0));
            given.push(Some((
                v,
                Expr {
                    kind: ExprKind::Name(p.name.name.clone()),
                    span: p.name.span,
                },
            )));
            continue;
        }
        let v = match &kind {
            PKind::Unit { unit, .. } | PKind::Range { unit, .. } => {
                let src = lower
                    .g
                    .param(&p.name.name, 0.0)
                    .map_err(|e| (Diagnostic::new(e.to_string(), p.name.span), false))?;
                lower.param_rates.push(Rate::Audio);
                Some(Datum::Sig(Sig::signal(src, *unit)))
            }
            PKind::Choice(_) => Some(Datum::Sig(Sig::constant(Unit::Num, 0.0))),
            PKind::Array(inner) => {
                guessed = true;
                let unit = inner.unit().unwrap_or(guess);
                Some(Datum::array(vec![
                    Datum::Sig(Sig::constant(unit, 0.0));
                    length
                ]))
            }
            PKind::Any | PKind::Buffer | PKind::Function => None,
        };
        given.push(v.map(|v| {
            (
                v,
                Expr {
                    kind: ExprKind::Name(p.name.name.clone()),
                    span: p.name.span,
                },
            )
        }));
    }
    let name = def.name.clone();
    let result = if funcs::has_state(def, &names.fns) {
        lower
            .call_kernel(def, &name, &params, given, def.span)
            .map(|_| ())
    } else {
        lower.in_fn = true;
        lower.fn_stack.push(def.name.name.clone());
        lower.fn_body(def, &name, &params, given).map(|_| ())
    };
    result.map_err(|d| (d, guessed))
}
