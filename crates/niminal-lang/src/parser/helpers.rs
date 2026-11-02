//! Helpers for building and post-processing parsed items (options, sample declarations).

use super::*;

pub(super) fn logic(op: LogicOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    Expr {
        kind: ExprKind::Logic {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    }
}

pub(super) fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    Expr {
        kind: ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    }
}

/// The settings an `instr` or `mono` takes in its body, such as `tail = 3sec`.
pub(super) const INSTR_OPTIONS: &[&str] = &[
    "max",
    "steal",
    "same_note",
    "legato",
    "priority",
    "glide",
    "pitch",
    "choke",
    "compensate",
    "tail",
    "quantize",
];

/// The settings a `node` takes in its body.
pub(super) const NODE_OPTIONS: &[&str] = &["quantize"];

pub(super) fn with_gone(span: Span) -> Diagnostic {
    Diagnostic::new("`with` is gone", span)
        .with_help("a definition's settings go in its body, such as `tail = 3sec`, or `quantize: 1bar` in a clip")
}

/// Settings in a definition's body, `tail = 3sec`, are its options rather than part of its sound:
/// take them out of `body`.
pub(super) fn take_options(body: &mut Vec<Stmt>, names: &[&str]) -> Vec<Arg> {
    let mut options = Vec::new();
    let mut rest = Vec::new();
    for stmt in std::mem::take(body) {
        match stmt {
            Stmt::Bind { name, value } if names.contains(&name.name.as_str()) => {
                options.push(Arg {
                    name: Some(name),
                    value,
                    receiver: false,
                });
            }
            other => rest.push(other),
        }
    }
    *body = rest;
    options
}

/// A body line like `max_voices = 4` that sets a number and is never read is a setting spelt wrong:
/// settings are the only lines in a body that are not part of the sound, and an unknown name is
/// otherwise just a local, so it would quietly do nothing.
pub(crate) fn reject_unread_constants(body: &[Stmt]) -> Res<()> {
    fn reads(e: &Expr, name: &str) -> bool {
        let mut found = matches!(&e.kind, ExprKind::Name(n) if n == name)
            || matches!(&e.kind, ExprKind::Call { name: n, .. } if n.name == name);
        e.for_each_child(&mut |c| found |= reads(c, name));
        found
    }
    for stmt in body {
        let Stmt::Bind { name, value } = stmt else {
            continue;
        };
        let constant = match &value.kind {
            ExprKind::Num { .. } => true,
            ExprKind::Neg(inner) => matches!(inner.kind, ExprKind::Num { .. }),
            _ => false,
        };
        // `out` is the result, which nothing reads by name
        if !constant
            || name.name == "out"
            || body
                .iter()
                .flat_map(Stmt::exprs)
                .any(|e| reads(e, &name.name))
        {
            continue;
        }
        let d = Diagnostic::new(format!("`{}` is set but never used", name.name), name.span);
        return Err(match closest(&name.name, INSTR_OPTIONS.iter().copied()) {
            Some(c) => d.with_help(format!("did you mean the setting `{c}`?")),
            None => d.with_help(format!(
                "a line in an instrument's body that is not used is ignored; the settings are {}",
                INSTR_OPTIONS
                    .iter()
                    .map(|o| format!("`{o}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        });
    }
    Ok(())
}

/// `quantize: 1bar` in a clip or scene sets when its changes land, so it is not one of its entries.
pub(super) fn take_entry_options(entries: &mut Vec<(Ident, Expr)>) -> Vec<Arg> {
    let mut options = Vec::new();
    entries.retain(|(key, value)| {
        let setting = key.name == "quantize";
        if setting {
            options.push(Arg {
                name: Some(key.clone()),
                value: value.clone(),
                receiver: false,
            });
        }
        !setting
    });
    options
}

/// Whether `value` is a chain that starts with `sample(...)`, `kit(...)`, `sfz(...)` or `name.slices(n)`.
pub(super) fn is_sample_chain(value: &Expr) -> bool {
    let mut cur = value;
    while let ExprKind::Call { name, args } = &cur.kind {
        match args.first() {
            Some(first) if first.receiver => {
                if name.name == "slices" && matches!(first.value.kind, ExprKind::Name(_)) {
                    return true;
                }
                cur = &first.value;
            }
            _ => return matches!(name.name.as_str(), "sample" | "kit" | "sfz"),
        }
    }
    false
}

/// `brk = sample("break.wav").beats(4)`, `drums = kit("drums").choke(...)` or
/// `chops = brk.slices(16).choke(...)`: the chain's calls after the first are the sample's options.
pub(super) fn sample_decl(name: Ident, value: Expr) -> Res<Item> {
    let span = name.span.to(value.span);
    let not_a_sample = |at: Span| {
        Diagnostic::new("expected a sample or a kit", at).with_help(
            "for example `kick = sample(\"drums/kick.wav\")`, `drums = kit(\"drums/808\")` or `chops = amen.slices(16)`",
        )
    };
    let mut options = Vec::new();
    let mut cur = value;
    let (is_kit, source, source_span) = loop {
        let at = cur.span;
        let ExprKind::Call {
            name: call,
            mut args,
        } = cur.kind
        else {
            return Err(not_a_sample(at));
        };
        if !args.first().is_some_and(|a| a.receiver) {
            let is_kit = match call.name.as_str() {
                "sample" | "sfz" => false,
                "kit" => true,
                _ => return Err(not_a_sample(call.span)),
            };
            let is_sfz = call.name == "sfz";
            let source = match args.as_slice() {
                [
                    Arg {
                        name: None,
                        value:
                            Expr {
                                kind: ExprKind::Str(path),
                                ..
                            },
                        ..
                    },
                ] if is_sfz => SampleSource::Sfz(path.clone()),
                [
                    Arg {
                        name: None,
                        value:
                            Expr {
                                kind: ExprKind::Str(path),
                                ..
                            },
                        ..
                    },
                ] => SampleSource::Path(path.clone()),
                [
                    Arg {
                        name: None, value, ..
                    },
                ] if !is_kit && !is_sfz => SampleSource::Table(value.clone()),
                _ => {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` takes {}",
                            call.name,
                            if is_kit {
                                "a folder in quotes"
                            } else if is_sfz {
                                "the path of an .sfz file in quotes"
                            } else {
                                "a file path in quotes, or a table"
                            }
                        ),
                        at,
                    ));
                }
            };
            break (is_kit, source, at);
        }
        let receiver = args.remove(0).value;
        if call.name == "slices" {
            let usage = || {
                Diagnostic::new(
                    "`slices` cuts a sample into equal pieces, as in `amen.slices(16)`, at places in it, as in `amen.slices([0, 0.25, 0.6])`, or at the hits it finds, as in `amen.slices(detect: 16)`",
                    at,
                )
            };
            let ExprKind::Name(sample) = &receiver.kind else {
                return Err(usage());
            };
            let number = |value: &Expr| match &value.kind {
                ExprKind::Num { value, unit: None } => Some(*value),
                _ => None,
            };
            let named = |name: &str| {
                args.iter()
                    .find(|a| a.name.as_ref().is_some_and(|n| n.name == name))
            };
            let mut sensitivity = None;
            let (count, places) = if let Some(detect) = named("detect") {
                if args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .is_none_or(|n| n.name != "detect" && n.name != "sensitivity")
                }) {
                    return Err(usage());
                }
                let Some(most) = number(&detect.value) else {
                    return Err(usage());
                };
                sensitivity = Some(match named("sensitivity") {
                    Some(s) => number(&s.value)
                        .filter(|v| (0.0..=1.0).contains(v))
                        .ok_or_else(|| {
                            Diagnostic::new("`sensitivity` is a number from 0 to 1", s.value.span)
                        })?,
                    None => 0.5,
                });
                (most, Vec::new())
            } else {
                match args.as_slice() {
                    [
                        Arg {
                            name: None,
                            value:
                                Expr {
                                    kind:
                                        ExprKind::Num {
                                            value: count,
                                            unit: None,
                                        },
                                    ..
                                },
                            ..
                        },
                    ] => (*count, Vec::new()),
                    [
                        Arg {
                            name: None,
                            value:
                                Expr {
                                    kind: ExprKind::Array(items),
                                    ..
                                },
                            ..
                        },
                    ] if !items.is_empty() => {
                        let mut places = Vec::new();
                        for item in items {
                            let ExprKind::Num { value, unit: None } = &item.kind else {
                                return Err(usage());
                            };
                            places.push(*value);
                        }
                        (places.len() as f64, places)
                    }
                    // outside an instrument's body the lexer keeps a bracketed list as raw pattern text
                    [
                        Arg {
                            name: None,
                            value:
                                Expr {
                                    kind: ExprKind::Pattern(raw),
                                    ..
                                },
                            ..
                        },
                    ] => {
                        let places: Option<Vec<f64>> = raw
                            .split(',')
                            .map(str::trim)
                            .filter(|word| !word.is_empty())
                            .map(|word| word.parse().ok())
                            .collect();
                        match places {
                            Some(places) if !places.is_empty() => (places.len() as f64, places),
                            _ => return Err(usage()),
                        }
                    }
                    _ => return Err(usage()),
                }
            };
            let sample = Ident {
                name: sample.clone(),
                span: receiver.span,
            };
            break (
                true,
                SampleSource::Slices {
                    sample,
                    count,
                    at: places,
                    sensitivity,
                },
                at,
            );
        }
        let [
            Arg {
                name: None,
                value: arg,
                ..
            },
        ] = args.as_slice()
        else {
            return Err(Diagnostic::new(
                format!(
                    "`{}` takes one value, as in `.{}(...)`",
                    call.name, call.name
                ),
                at,
            ));
        };
        options.push(Arg {
            name: Some(call),
            value: arg.clone(),
            receiver: false,
        });
        cur = receiver;
    };
    options.reverse();
    Ok(Item::Sample(SampleDecl {
        is_kit,
        name,
        source,
        source_span,
        options,
        span,
    }))
}
