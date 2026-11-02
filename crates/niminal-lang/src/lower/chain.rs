//! Effect chains and channel joining.

use super::*;

pub(super) const MAX_SAMPLE_CHANNELS: usize = 16;

/// Envelope curve written in source → engine curve.
pub(crate) fn curve_of(kind: Option<CurveKind>) -> Curve {
    match kind {
        None => Curve::Linear,
        Some(CurveKind::Exp) => Curve::Exp,
        Some(CurveKind::Log) => Curve::Log,
        Some(CurveKind::Custom(k)) => Curve::Custom(k as f32),
    }
}

/// A compiled node: its processing, what feeds that processing, and where
/// the result goes.
pub(crate) struct Chain {
    pub graph: Option<Arc<Graph>>,
    pub inputs: Vec<ChainInput>,
    pub route: Route,
    pub taps: Vec<crate::program::OscTap>,
}

/// A node's mixer lines: `gain = -3db` and `pan = 20deg`, applied to what the node puts out.
#[derive(Default)]
pub(crate) struct Mixer<'a> {
    pub gain: Option<&'a Expr>,
    pub pan: Option<&'a Expr>,
}

/// Compile a node's statements (everything except its `instrument` line).
pub(crate) fn compile_chain(
    node: &Ident,
    body: &[&Stmt],
    has_instrument: bool,
    mixer: &Mixer,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Chain> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Node;

    let mixed = mixer.gain.is_some() || mixer.pan.is_some();
    if body.is_empty() && !mixed {
        return if has_instrument {
            Ok(Chain {
                graph: None,
                inputs: Vec::new(),
                route: Route::Master,
                taps: Vec::new(),
            })
        } else {
            Err(
                Diagnostic::new(format!("node `{}` does nothing", node.name), node.span)
                    .with_help("give it an `instrument = ...`, or an `out = ...` that reads a bus"),
            )
        };
    }
    lower.run_body(body)?;

    let out = match lower.out.take() {
        Some(out) => out,
        None if has_instrument => lower.node_input(),
        None if lower.sends > 0 => Sig::constant(Unit::Num, 0.0),
        None => {
            return Err(Diagnostic::new(
                format!("node `{}` never sets `out`", node.name),
                node.span,
            )
            .with_help(
                "add `out = it` to pass its sound through, or send to a bus with `bus += ...`",
            ));
        }
    };
    // The mixer's lines come last: the fader, then the placement.
    let mut out = out;
    if mixed {
        lower.out = Some(out);
        let at = |kind: ExprKind, span: Span| Expr { kind, span };
        let call = |name: &str, args: Vec<Arg>, span: Span| {
            at(
                ExprKind::Call {
                    name: Ident {
                        name: name.into(),
                        span,
                    },
                    args,
                },
                span,
            )
        };
        let positional = |value: Expr| Arg {
            name: None,
            value,
            receiver: false,
        };
        let receiver = |value: Expr| Arg {
            name: None,
            value,
            receiver: true,
        };
        let mut signal = at(ExprKind::Name("out".into()), node.span);
        if let Some(g) = mixer.gain {
            signal = call(
                "gain",
                vec![receiver(signal), positional(g.clone())],
                g.span,
            );
        }
        if let Some(p) = mixer.pan {
            // a pan places one signal, so a wider node is mixed down first, as loud as one of its channels
            let mono = if names.master.channels() > 1 {
                let to_mono = call(
                    "to_layout",
                    vec![
                        receiver(signal),
                        positional(at(ExprKind::Name("mono".into()), p.span)),
                    ],
                    p.span,
                );
                let level = at(
                    ExprKind::Num {
                        value: (names.master.channels() as f64).sqrt(),
                        unit: None,
                    },
                    p.span,
                );
                call("gain", vec![receiver(to_mono), positional(level)], p.span)
            } else {
                signal
            };
            let angle = Arg {
                name: Some(Ident {
                    name: "angle".into(),
                    span: p.span,
                }),
                value: p.clone(),
                receiver: false,
            };
            signal = call("pan", vec![receiver(mono), angle], p.span);
        }
        out = lower.expr(&signal)?;
    }
    // The chain delivers in its destination's layout.
    let destination = match lower.route {
        Route::Master => names
            .node_outputs
            .get(&node.name)
            .copied()
            .unwrap_or(names.master),
        Route::Bus(b) => names.bus_layouts[b],
    };
    let out = lower.convert(&out, destination, node.span)?;
    Ok(Chain {
        graph: Some(Arc::new(lower.g.build_channels(&out.srcs))),
        inputs: lower.chain_inputs,
        route: lower.route,
        taps: lower.taps,
    })
}

/// The span of the first argument that is not the receiver, for a problem with its value.
/// Entries of one channel each, joined into a list. They share a unit, so `[800hz, 1200hz]` is a two-channel frequency.
pub(super) fn join_channels(entries: &[(Sig, Span)], span: Span) -> Res<Sig> {
    if entries.is_empty() {
        return Err(Diagnostic::new(
            "audio output needs at least one channel",
            span,
        ));
    }
    let mut srcs = Vec::with_capacity(entries.len());
    let mut unit = None;
    for (sig, at) in entries {
        if sig.count() != 1 {
            return Err(Diagnostic::new(
                format!(
                    "a channel array takes one channel per entry, but this has {}",
                    sig.count()
                ),
                *at,
            ));
        }
        match unit {
            None => unit = Some(sig.unit),
            Some(u) if u != sig.unit => {
                return Err(Diagnostic::new(
                    format!(
                        "the entries of a channel array must have the same unit: this is {}, the first was {}",
                        sig.unit.describe(),
                        u.describe()
                    ),
                    *at,
                ));
            }
            Some(_) => {}
        }
        srcs.push(sig.srcs[0]);
    }
    if srcs.len() > niminal_engine::MAX_CHANNELS {
        return Err(Diagnostic::new(
            format!(
                "audio output supports at most {} channels",
                niminal_engine::MAX_CHANNELS
            ),
            span,
        ));
    }
    let layout = Layout::from_count(srcs.len());
    Ok(Sig {
        srcs: srcs.into(),
        layout,
        unit: unit.unwrap_or(Unit::Num),
        konst: None,
    })
}
