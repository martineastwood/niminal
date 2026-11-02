//! Program-wide settings: master layout, tuning and scales.

use super::*;

/// The master layout from the project's `config`, mono if there is none.
pub(super) fn master_layout(items: &[Item], default: Layout) -> Res<Layout> {
    let mut layout = None;
    let mut seen = false;
    for item in items {
        let Item::Config(config) = item else { continue };
        if seen {
            return Err(Diagnostic::new(
                "a project has only one `config`",
                config.span,
            ));
        }
        seen = true;
        for (key, value) in &config.entries {
            match key.name.as_str() {
                "channels" => layout = Some(layout_from_expr(value)?),
                other => {
                    let mut d = Diagnostic::new(format!("unknown setting `{other}`"), key.span);
                    d = d.with_help("the settings so far are: channels");
                    return Err(d);
                }
            }
        }
    }
    Ok(layout.unwrap_or(default))
}

/// Cents above the root (c) for the twelve notes of each named scale.
pub(super) const NAMED_SCALES: [(&str, [f64; 12]); 4] = [
    (
        "equal",
        [
            0.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 1100.0,
        ],
    ),
    (
        "just",
        [
            0.0, 111.73, 203.91, 315.64, 386.31, 498.04, 590.22, 701.96, 813.69, 884.36, 1017.60,
            1088.27,
        ],
    ),
    (
        "pythagorean",
        [
            0.0, 113.69, 203.91, 294.13, 407.82, 498.04, 611.73, 701.96, 815.64, 905.87, 996.09,
            1109.78,
        ],
    ),
    (
        "meantone",
        [
            0.0, 76.05, 193.16, 310.26, 386.31, 503.42, 579.47, 696.58, 772.63, 889.74, 1006.84,
            1082.89,
        ],
    ),
];

/// `config { tuning: { a4: 432hz, scale: just, period: 1200ct } }`: how note names become frequencies.
/// `scale` is a name (`equal`, `just`, `pythagorean`, `meantone`), `cents(0, 112, ...)` or
/// `ratios(1, 16/15, ...)`, one entry per note number from c up to the period.
pub(super) fn tuning_of(items: &[Item]) -> Res<Tuning> {
    let default = Tuning::default();
    let (mut a4, mut period, mut scale) = (default.a4, default.period, NAMED_SCALES[0].1.to_vec());
    for item in items {
        let Item::Config(config) = item else { continue };
        for (key, value) in &config.tuning {
            match key.name.as_str() {
                "a4" => match &value.kind {
                    ExprKind::Num {
                        value: v,
                        unit: Some(u),
                    } if (u == "hz" || u == "khz") && *v > 0.0 => {
                        a4 = if u == "khz" { v * 1000.0 } else { *v };
                    }
                    _ => {
                        return Err(Diagnostic::new("`a4` is a frequency", value.span)
                            .with_help("for example `a4: 432hz`"));
                    }
                },
                "period" => {
                    let cents = cents_of(value)?;
                    if cents <= 0.0 {
                        return Err(Diagnostic::new("`period` must be above 0ct", value.span));
                    }
                    period = cents;
                }
                "scale" => scale = scale_of(value)?,
                other => {
                    return Err(Diagnostic::new(
                        format!("unknown tuning setting `{other}`"),
                        key.span,
                    )
                    .with_help("the settings are: a4, scale, period"));
                }
            }
        }
    }
    Ok(Tuning::new(a4, period, &scale))
}

/// An interval written in cents (`702ct`) or semitones (`7st`), as cents.
pub(super) fn cents_of(e: &Expr) -> Res<f64> {
    match &e.kind {
        ExprKind::Num {
            value,
            unit: Some(u),
        } if u == "ct" => Ok(*value),
        ExprKind::Num {
            value,
            unit: Some(u),
        } if u == "st" => Ok(*value * 100.0),
        ExprKind::Neg(inner) => cents_of(inner).map(|c| -c),
        _ => Err(Diagnostic::new(
            "expected an interval in cents, such as `702ct`",
            e.span,
        )),
    }
}

pub(super) fn scale_of(e: &Expr) -> Res<Vec<f64>> {
    let check = |degrees: Vec<f64>, span| -> Res<Vec<f64>> {
        if degrees.is_empty() || degrees.len() > MAX_DEGREES {
            return Err(Diagnostic::new(
                format!("a scale has between 1 and {MAX_DEGREES} degrees"),
                span,
            ));
        }
        if degrees[0] != 0.0 {
            return Err(Diagnostic::new(
                "a scale starts at the root: its first degree is 0 cents (or a ratio of 1)",
                span,
            ));
        }
        Ok(degrees)
    };
    match &e.kind {
        ExprKind::Name(n) => NAMED_SCALES
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, cents)| cents.to_vec())
            .ok_or_else(|| {
                let names = NAMED_SCALES.map(|(name, _)| name);
                let mut d = Diagnostic::new(format!("unknown scale `{n}`"), e.span)
                    .with_help(format!("the named scales are: {}", names.join(", ")));
                if let Some(c) = closest(n, names) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            }),
        ExprKind::Call { name, args } if name.name == "cents" => {
            // inside `cents(...)` a bare number is already in cents
            let cents = |a: &Arg| match &a.value.kind {
                ExprKind::Num { value, unit: None } => Ok(*value),
                _ => cents_of(&a.value),
            };
            check(args.iter().map(cents).collect::<Res<_>>()?, e.span)
        }
        ExprKind::Call { name, args } if name.name == "ratios" => {
            let ratio = |a: &Arg| -> Res<f64> {
                let value = match &a.value.kind {
                    ExprKind::Num { value, unit: None } => Some(*value),
                    ExprKind::Binary {
                        op: BinOp::Div,
                        lhs,
                        rhs,
                    } => match (&lhs.kind, &rhs.kind) {
                        (
                            ExprKind::Num {
                                value: n,
                                unit: None,
                            },
                            ExprKind::Num {
                                value: d,
                                unit: None,
                            },
                        ) if *d != 0.0 => Some(n / d),
                        _ => None,
                    },
                    _ => None,
                };
                match value {
                    Some(r) if r > 0.0 => Ok(1200.0 * r.log2()),
                    _ => Err(Diagnostic::new(
                        "expected a ratio above 0, such as `3/2`",
                        a.value.span,
                    )),
                }
            };
            check(args.iter().map(ratio).collect::<Res<_>>()?, e.span)
        }
        _ => Err(
            Diagnostic::new("a scale is a name, `cents(...)` or `ratios(...)`", e.span).with_help(
                "for example `scale: just` or `scale: ratios(1, 9/8, 5/4, 4/3, 3/2, 5/3, 15/8)`",
            ),
        ),
    }
}
