//! `adsr(...)` and `perc(...)` stand for envelope literals.
//!
//! Each call is rewritten to `ExprKind::Env` once user opcodes are known, so a
//! user `opcode adsr` / `opcode perc` wins and lowering only sees `env[...]`.

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::opcodes::Registry;

type Res<T> = Result<T, Diagnostic>;

/// Rewrite `adsr` / `perc` calls in instrument, track and control bodies.
/// Skips a name that is already a user opcode. Binding mistakes become
/// diagnostics; the call is still replaced so later stages do not re-report it.
pub fn desugar(items: &mut [Item], registry: &Registry) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for item in items {
        match item {
            Item::Instr(d) => d
                .body
                .iter_mut()
                .for_each(|s| rewrite_stmt(s, registry, &mut errors)),
            Item::Track(d) => d
                .body
                .iter_mut()
                .for_each(|s| rewrite_stmt(s, registry, &mut errors)),
            Item::Control { value, .. } => rewrite_expr(value, registry, &mut errors),
            _ => {}
        }
    }
    errors
}

fn rewrite_stmt(stmt: &mut Stmt, registry: &Registry, errors: &mut Vec<Diagnostic>) {
    match stmt {
        Stmt::Bind { value, .. }
        | Stmt::AddAssign { value, .. }
        | Stmt::Destructure { value, .. }
        | Stmt::Expr(value) => {
            rewrite_expr(value, registry, errors);
        }
        Stmt::State { init, .. } => rewrite_expr(init, registry, errors),
    }
}

fn rewrite_expr(e: &mut Expr, registry: &Registry, errors: &mut Vec<Diagnostic>) {
    match &mut e.kind {
        ExprKind::Call { name, args } => {
            for arg in args.iter_mut() {
                rewrite_expr(&mut arg.value, registry, errors);
            }
            if !matches!(name.name.as_str(), "adsr" | "perc") || registry.find(&name.name).is_some()
            {
                return;
            }
            let span = name.span;
            let result = env_lit(name, args);
            e.kind = match result {
                Ok(lit) => ExprKind::Env(lit),
                Err(d) => {
                    errors.push(d);
                    ExprKind::Env(dummy_env(span))
                }
            };
        }
        ExprKind::Neg(inner) => rewrite_expr(inner, registry, errors),
        ExprKind::Index { list, index } => {
            rewrite_expr(list, registry, errors);
            rewrite_expr(index, registry, errors);
        }
        ExprKind::Lambda { body, .. } => rewrite_expr(body, registry, errors),
        ExprKind::Range { lo, hi } => {
            rewrite_expr(lo, registry, errors);
            rewrite_expr(hi, registry, errors);
        }
        ExprKind::Channels(items) => items
            .iter_mut()
            .for_each(|i| rewrite_expr(i, registry, errors)),
        ExprKind::Binary { lhs, rhs, .. } => {
            rewrite_expr(lhs, registry, errors);
            rewrite_expr(rhs, registry, errors);
        }
        _ => {}
    }
}

fn dummy_env(span: Span) -> EnvLit {
    let n = |value: f64, unit: Option<&str>| EnvNum {
        value,
        unit: unit.map(str::to_string),
        span,
    };
    EnvLit {
        start: n(0.0, None),
        segments: vec![EnvSegment {
            dur: n(1.0, Some("ms")),
            curve: None,
            level: n(0.0, None),
        }],
        sustain_at: None,
        looped: false,
    }
}

/// `adsr(...)` → `env[0 attack 1 decay sustain | release 0]`,
/// `perc(...)` → `env[0 attack 1 decay.exp 0]` (attack defaults to 2ms).
fn env_lit(name: &Ident, args: &[Arg]) -> Res<EnvLit> {
    let level = |value: f64| EnvNum {
        value,
        unit: None,
        span: name.span,
    };
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

fn bind<const N: usize>(
    name: &Ident,
    args: &[Arg],
    params: &[&str; N],
    defaults: &[(&str, f64, &str)],
) -> Res<[EnvNum; N]> {
    let mut given: [Option<EnvNum>; N] = std::array::from_fn(|_| None);
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
        let ExprKind::Num { value, unit } = &arg.value.kind else {
            return Err(Diagnostic::new(
                "an envelope's times and levels must be plain values such as `5ms` or `0.4`",
                arg.value.span,
            ));
        };
        if given[slot]
            .replace(EnvNum {
                value: *value,
                unit: unit.clone(),
                span: arg.value.span,
            })
            .is_some()
        {
            return Err(Diagnostic::new(
                format!("`{}` is given twice", params[slot]),
                arg.value.span,
            ));
        }
    }
    let mut out = std::array::from_fn(|_| EnvNum {
        value: 0.0,
        unit: None,
        span: name.span,
    });
    for (i, param) in params.iter().enumerate() {
        out[i] = given[i]
            .take()
            .or_else(|| {
                defaults
                    .iter()
                    .find(|d| d.0 == *param)
                    .map(|&(_, value, unit)| EnvNum {
                        value,
                        unit: Some(unit.into()),
                        span: name.span,
                    })
            })
            .ok_or_else(|| {
                Diagnostic::new(format!("`{}` needs `{param}`", name.name), name.span)
            })?;
    }
    Ok(out)
}
