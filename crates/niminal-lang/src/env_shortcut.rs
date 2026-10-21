//! `adsr(...)` and `perc(...)` stand for envelope literals.
//!
//! Each call is rewritten to `ExprKind::Env` once user functions are known, so a
//! user `fn adsr` / `fn perc` wins and lowering only sees `env[...]`.

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};

type Res<T> = Result<T, Diagnostic>;

/// Rewrite `adsr` / `perc` calls in instrument, function, node and control bodies.
/// Skips a name that is a user function. Binding mistakes become diagnostics;
/// the call is still replaced so later stages do not re-report it.
pub fn desugar(items: &mut [Item], fns: &HashMap<String, FnDef>) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    let skip = |name: &str| fns.contains_key(name);
    for item in items {
        let stmts = match item {
            Item::Instr(d) => &mut d.body,
            Item::Node(d) | Item::Processor(d) => &mut d.body,
            Item::Fn(d) => &mut d.body,
            Item::Control { value, .. } | Item::Const { value, .. } | Item::Buffer(BufferDecl { value, .. }) => {
                rewrite_expr(value, &skip, &mut errors);
                continue;
            }
            _ => continue,
        };
        for s in stmts {
            for e in s.exprs_mut() {
                rewrite_expr(e, &skip, &mut errors);
            }
        }
    }
    errors
}

fn rewrite_expr(e: &mut Expr, skip: &dyn Fn(&str) -> bool, errors: &mut Vec<Diagnostic>) {
    e.for_each_child_mut(&mut |c| rewrite_expr(c, skip, errors));
    let ExprKind::Call { name, args } = &e.kind else {
        return;
    };
    if !matches!(name.name.as_str(), "adsr" | "perc") || skip(&name.name) {
        return;
    }
    let span = name.span;
    e.kind = match env_lit(name, args) {
        Ok(lit) => ExprKind::Env(lit),
        Err(d) => {
            errors.push(d);
            ExprKind::Env(dummy_env(span))
        }
    };
}

fn lit(value: f64, unit: Option<&str>, span: Span) -> EnvValue {
    EnvValue::Lit(EnvNum {
        value,
        unit: unit.map(str::to_string),
        span,
    })
}

fn dummy_env(span: Span) -> EnvLit {
    EnvLit {
        start: lit(0.0, None, span),
        segments: vec![EnvSegment {
            dur: lit(1.0, Some("ms"), span),
            curve: None,
            level: lit(0.0, None, span),
        }],
        sustain_at: None,
        looped: false,
    }
}

/// `adsr(...)` → `env[0 attack 1 decay sustain | release 0]`,
/// `perc(...)` → `env[0 attack 1 decay.exp 0]` (attack defaults to 2ms).
fn env_lit(name: &Ident, args: &[Arg]) -> Res<EnvLit> {
    let level = |value: f64| lit(value, None, name.span);
    match name.name.as_str() {
        "adsr" => {
            let [attack, decay, sustain, release] =
                bind(name, args, &["attack", "decay", "sustain", "release"], &[])?;
            Ok(EnvLit {
                start: level(0.0),
                segments: vec![
                    EnvSegment {
                        dur: attack,
                        curve: None,
                        level: level(1.0),
                    },
                    EnvSegment {
                        dur: decay,
                        curve: None,
                        level: sustain,
                    },
                    EnvSegment {
                        dur: release,
                        curve: None,
                        level: level(0.0),
                    },
                ],
                sustain_at: Some(2),
                looped: false,
            })
        }
        "perc" => {
            let [attack, decay] = bind(name, args, &["attack", "decay"], &[("attack", 2.0, "ms")])?;
            Ok(EnvLit {
                start: level(0.0),
                segments: vec![
                    EnvSegment {
                        dur: attack,
                        curve: None,
                        level: level(1.0),
                    },
                    EnvSegment {
                        dur: decay,
                        curve: Some(CurveKind::Exp),
                        level: level(0.0),
                    },
                ],
                sustain_at: None,
                looped: false,
            })
        }
        _ => unreachable!("not an envelope shortcut"),
    }
}

/// The step each parameter gets: a literal stays one, anything else is read at note start.
fn bind<const N: usize>(
    name: &Ident,
    args: &[Arg],
    params: &[&str; N],
    defaults: &[(&str, f64, &str)],
) -> Res<[EnvValue; N]> {
    let mut given: [Option<EnvValue>; N] = std::array::from_fn(|_| None);
    let mut next_positional = 0;
    for arg in args {
        let slot = match &arg.name {
            None => {
                let i = next_positional;
                next_positional += 1;
                if i >= N {
                    return Err(Diagnostic::new(
                        format!("`{}` takes {}", name.name, params.join(", ")),
                        arg.value.span,
                    ));
                }
                i
            }
            Some(n) => match params.iter().position(|p| *p == n.name) {
                Some(i) => i,
                None => {
                    let mut d =
                        Diagnostic::new(format!("`{}` has no `{}`", name.name, n.name), n.span);
                    d = if let Some(c) = closest(&n.name, params.iter().copied()) {
                        d.with_help(format!("did you mean `{c}`?"))
                    } else {
                        d.with_help(format!("it takes {}", params.join(", ")))
                    };
                    return Err(d);
                }
            },
        };
        let step = match &arg.value.kind {
            ExprKind::Num { value, unit } => EnvValue::Lit(EnvNum {
                value: *value,
                unit: unit.clone(),
                span: arg.value.span,
            }),
            _ => EnvValue::Expr(Box::new(arg.value.clone())),
        };
        if given[slot].replace(step).is_some() {
            return Err(Diagnostic::new(
                format!("`{}` is given twice", params[slot]),
                arg.value.span,
            ));
        }
    }
    let mut out = std::array::from_fn(|_| lit(0.0, None, name.span));
    for (i, param) in params.iter().enumerate() {
        out[i] = given[i]
            .take()
            .or_else(|| {
                defaults
                    .iter()
                    .find(|d| d.0 == *param)
                    .map(|&(_, value, unit)| lit(value, Some(unit), name.span))
            })
            .ok_or_else(|| {
                Diagnostic::new(format!("`{}` needs `{param}`", name.name), name.span)
            })?;
    }
    Ok(out)
}
