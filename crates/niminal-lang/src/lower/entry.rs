//! Entry points: compiling controls, instruments and MIDI outs.

use super::*;

pub(crate) fn compile_control(
    name: &Ident,
    value: &Expr,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<ControlDef> {
    if crate::names::is_reserved(&name.name) {
        return Err(super::reserved(name));
    }
    let mut lower = Lower::new(registry, names, tempo);
    let mut initial = value.clone();
    let mut smooth_seconds = 0.005;
    let (smooth_expr, negative) = match &value.kind {
        ExprKind::Neg(inner) => (inner.as_ref(), true),
        _ => (value, false),
    };
    if let ExprKind::Call { name: method, args } = &smooth_expr.kind
        && method.name == "smooth"
    {
        let [receiver, duration] = args.as_slice() else {
            return Err(Diagnostic::new(
                "a control's .smooth takes one duration",
                value.span,
            ));
        };
        if !receiver.receiver || duration.name.is_some() {
            return Err(Diagnostic::new(
                "write ctl name = value.smooth(5ms)",
                value.span,
            ));
        }
        initial = receiver.value.clone();
        if negative {
            initial = Expr {
                kind: ExprKind::Neg(Box::new(initial)),
                span: value.span,
            };
        }
        let smooth = lower.expr(&duration.value)?;
        if smooth.unit != Unit::Time {
            return Err(Diagnostic::new(
                "control smoothing needs a duration, such as 5ms",
                duration.value.span,
            ));
        }
        smooth_seconds = smooth
            .konst
            .ok_or_else(|| Diagnostic::new("smoothing must be constant", duration.value.span))?;
        if !smooth_seconds.is_finite() || !(0.0..=60.0).contains(&smooth_seconds) {
            return Err(Diagnostic::new(
                "control smoothing must be between 0ms and 60sec",
                duration.value.span,
            ));
        }
    }
    let sig = lower.expr(&initial)?;
    let value = sig.konst.ok_or_else(|| {
        Diagnostic::new("a control's initial value must be constant", initial.span)
    })? as f32;
    if !value.is_finite() || (sig.unit == Unit::Db && value <= 0.0) {
        return Err(Diagnostic::new(
            "control values must be finite and representable",
            initial.span,
        ));
    }
    Ok(ControlDef {
        range: None,
        name: name.name.clone(),
        key: format!("ctl:{}:{:?}", name.name, sig.unit),
        unit: sig.unit,
        initial: value,
        smooth_seconds,
    })
}

pub(crate) fn compile_instr(
    def: &InstrDef,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Instrument> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut params: Vec<Param> = Vec::new();
    let mut arrays = std::collections::BTreeMap::new();
    let mut resources = std::collections::BTreeMap::new();
    let mut resource_keys = std::collections::BTreeMap::new();

    let signature = funcs::signature(&def.params, tempo)?;
    for p in &signature {
        names.check_local(&p.name)?;
        if p.kind == PKind::Buffer && p.rate != Some(Rate::Const) {
            let default = p.default.as_ref().ok_or_else(|| {
                Diagnostic::new("a buffer parameter needs a default resource", p.name.span)
            })?;
            let Datum::Buffer { data, .. } = lower.value(default)? else {
                return Err(Diagnostic::new(
                    "buffer default is a named shared buffer",
                    default.span,
                ));
            };
            let resource = names
                .buffers
                .iter()
                .find(|(_, (_, b))| Arc::ptr_eq(b, &data))
                .map(|(n, _)| n.as_str())
                .ok_or_else(|| {
                    Diagnostic::new("buffer default is a named shared buffer", default.span)
                })?;
            let owner = resource.split_once("::").map(|(o, _)| o).unwrap_or("");
            let mut choices: Vec<_> = names
                .buffers
                .iter()
                .filter(|(n, _)| n.split_once("::").map(|(o, _)| o).unwrap_or("") == owner)
                .map(|(n, (id, b))| (*id, n.clone(), b.clone()))
                .collect();
            choices.sort_by_key(|v| v.0);
            let choices: BufferChoices = Choices::named(
                choices.into_iter().map(|(_, n, b)| (n, b)).collect(),
                &format!("{}::{}", def.name.name, p.name.name),
            );
            let index = choices
                .iter()
                .position(|(_, b)| Arc::ptr_eq(b, &data))
                .expect("the owner's choices include the buffer they were chosen by");
            let src = lower
                .g
                .param(&p.name.name, index as f32)
                .expect("unique parameter");
            lower.param_rates.push(Rate::Audio);
            params.push(Param {
                name: p.name.name.clone(),
                unit: Unit::Num,
                range: Some((0.0, (choices.len() - 1) as f64)),
                default: Some(index as f32),
            });
            resource_keys.insert(p.name.name.clone(), choices.key);
            resources.insert(
                p.name.name.clone(),
                choices.iter().map(|(n, _)| n.clone()).collect(),
            );
            let selector = lower.at_rate(Sig::signal(src, Unit::Num), p.rate);
            lower
                .scope
                .insert(p.name.name.clone(), Datum::BufferRef { selector, choices });
            continue;
        }
        if matches!(p.kind, PKind::Array(_)) && p.rate != Some(Rate::Const) {
            let default = p.default.as_ref().ok_or_else(|| {
                Diagnostic::new(
                    "a live array parameter needs a default to fix its shape",
                    p.name.span,
                )
            })?;
            let value = lower.value(default)?;
            let value = lower.check_param(&def.name.name, p, value, default)?;
            let start = params.len();
            let value = lower.live_array(
                value,
                &p.name.name,
                &p.kind,
                p.rate,
                &mut params,
                default.span,
            )?;
            arrays.insert(
                p.name.name.clone(),
                params[start..].iter().map(|p| p.name.clone()).collect(),
            );
            lower.scope.insert(p.name.name.clone(), value);
            continue;
        }
        // Structural arguments specialize the graph. Notes only see the remaining scalar ports.
        if p.rate == Some(Rate::Const)
            || matches!(p.kind, PKind::Choice(_) | PKind::Function | PKind::Array(_))
        {
            let default = p.default.as_ref().ok_or_else(|| {
                Diagnostic::new(
                    format!("structural parameter `{}` needs a default", p.name.name),
                    p.name.span,
                )
                .with_help("override it by calling this instrument inside another instrument")
            })?;
            let value = if let PKind::Choice(options) = &p.kind {
                let ExprKind::Name(n) = &default.kind else {
                    unreachable!("signature checked")
                };
                for (i, option) in options.iter().enumerate() {
                    lower.scope.insert(
                        option.clone(),
                        Datum::Sig(Sig::constant(Unit::Num, i as f64)),
                    );
                }
                Datum::Sig(Sig::constant(
                    Unit::Num,
                    options.iter().position(|o| o == n).expect("the branch checked n is an option") as f64,
                ))
            } else {
                lower.value(default)?
            };
            let mut structural = p.clone();
            structural.rate = Some(Rate::Const);
            let value = lower.check_param(&def.name.name, &structural, value, default)?;
            lower.scope.insert(p.name.name.clone(), value);
            continue;
        }
        let (unit, range) = match &p.kind {
            PKind::Unit { unit, .. } => (*unit, None),
            PKind::Range { unit, lo, hi } => (*unit, Some((*lo, *hi))),
            PKind::Any => match &p.default {
                Some(e) => (lower.expr(e)?.unit, None),
                None if p.name.name == "sample" => (Unit::Num, None),
                None => {
                    return Err(Diagnostic::new(
                        format!("parameter `{}` needs a type", p.name.name),
                        p.name.span,
                    )
                    .with_help(format!("for example `{}: hz`", p.name.name)));
                }
            },
            PKind::Buffer | PKind::Choice(_) | PKind::Array(_) | PKind::Function => {
                unreachable!("structural arguments handled above")
            }
        };
        let default = match &p.default {
            None => None,
            Some(e) => Some(lower.param_default(e, unit, range, &p.name.name)?),
        };
        let src = lower
            .g
            .param(&p.name.name, default.unwrap_or(0.0))
            .expect("duplicates checked above");
        lower
            .g
            .set_param_range(&p.name.name, range.map(|(lo, hi)| (lo as f32, hi as f32)));
        lower.param_rates.push(Rate::Audio);
        let sig = lower.at_rate(Sig::signal(src, unit), p.rate);
        lower.scope.insert(p.name.name.clone(), Datum::Sig(sig));
        params.push(Param {
            name: p.name.name.clone(),
            unit,
            range,
            default,
        });
    }
    if let Some(tail) = def
        .options
        .iter()
        .find(|o| o.name.as_ref().is_some_and(|n| n.name == "tail"))
    {
        let t = lower.expr(&tail.value)?;
        match (t.unit, t.konst) {
            (Unit::Time, Some(seconds)) if (0.0..=600.0).contains(&seconds) => {
                lower.g.set_tail(seconds as f32)
            }
            _ => {
                return Err(Diagnostic::new(
                    "`tail` is a time from 0 to 600sec, such as `2sec`",
                    tail.value.span,
                ));
            }
        }
    }

    // Every note accepts a `gain` control, applied after the instrument, unless the instrument takes it
    // over by declaring its own.
    let own_gain = params.iter().find(|p| p.name == GAIN_PARAM);
    if let Some(p) = own_gain
        && p.unit != Unit::Db
    {
        let span = def
            .params
            .iter()
            .find(|q| q.name.name == GAIN_PARAM)
            .map_or(def.name.span, |q| q.name.span);
        return Err(Diagnostic::new(
            format!("an instrument's own `{GAIN_PARAM}` is in db, like the one every note has"),
            span,
        )
        .with_help(format!("`{GAIN_PARAM}: db = 0db`")));
    }
    let gain = if own_gain.is_none() {
        let gain = lower.g.param(GAIN_PARAM, 1.0).expect("name checked above");
        lower.param_rates.push(Rate::Audio);
        params.push(Param {
            name: GAIN_PARAM.to_string(),
            unit: Unit::Db,
            range: None,
            default: Some(1.0),
        });
        Some(gain)
    } else {
        None
    };

    if def.body.is_empty() {
        return Err(Diagnostic::new(
            format!("instrument `{}` is empty", def.name.name),
            def.name.span,
        )
        .with_help("the last line of an instrument is the signal it outputs"));
    }
    let body: Vec<&Stmt> = def.body.iter().collect();
    let tail = lower.run_body(&body)?;

    let (result, span) = match (lower.out.take(), tail) {
        (Some(_), Some((_, span))) => {
            return Err(Diagnostic::new(
                "this instrument sets `out`, so its last line can't also be a value",
                span,
            )
            .with_help("assign it to `out`, or remove the `out` lines"));
        }
        (Some(out), None) => (out, def.name.span),
        (None, Some((sig, span))) => {
            if sig.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "an instrument outputs a plain signal, but this is {}",
                        sig.unit.describe()
                    ),
                    span,
                ));
            }
            (sig, span)
        }
        // Only sends: the instrument makes no sound of its own.
        (None, None) if lower.sends > 0 => (Sig::constant(Unit::Num, 0.0), def.name.span),
        (None, None) => {
            let name = match body.last() {
                Some(Stmt::Bind { name, .. }) => name,
                _ => &def.name,
            };
            return Err(Diagnostic::new(
                "an instrument must end with the signal it outputs",
                name.span,
            )
            .with_help(format!("add a last line such as `{}`", name.name)));
        }
    };

    // Every voice leaves the instrument in the master's layout.
    let result = lower.convert(&result, names.master, span)?;
    let result = match gain {
        Some(gain) => lower.mul(&result, &Sig::signal(gain, Unit::Num), Unit::Num, span)?,
        None => result,
    };
    let mut instrument = Instrument::new(
        def.name.name.clone(),
        Arc::new(lower.g.build_channels(&result.srcs)),
        params,
    );
    if let Some((members, chokes)) = lower.kit {
        instrument.members = members;
        instrument.chokes = chokes;
    }
    instrument.taps = lower.taps;
    instrument.rate_hints = lower.rate_hints;
    instrument.arrays = arrays;
    instrument.resources = resources;
    instrument.resource_keys = resource_keys;
    Ok(instrument)
}

/// `midi_out drums(port: "IAC Driver", channel: 10)`: an instrument whose notes go to external gear.
/// It is silent here; a note's `freq` is the key that is pressed and its `gain` the velocity.
pub(crate) fn compile_midi_out(decl: &MidiOutDecl, names: &Names) -> Res<Instrument> {
    let (mut port, mut channel) = (None, 1u8);
    for arg in &decl.args {
        let Some(option) = &arg.name else {
            return Err(Diagnostic::new(
                "an argument is a name and a value, as in `port: \"IAC Driver\"`",
                arg.value.span,
            ));
        };
        match option.name.as_str() {
            "port" => match &arg.value.kind {
                ExprKind::Str(text) if !text.trim().is_empty() => port = Some(text.clone()),
                _ => {
                    return Err(Diagnostic::new(
                        "the port is the name of a MIDI output, in quotes",
                        arg.value.span,
                    ));
                }
            },
            "channel" => match &arg.value.kind {
                ExprKind::Num { value, unit: None }
                    if value.fract() == 0.0 && (1.0..=16.0).contains(value) =>
                {
                    channel = *value as u8
                }
                _ => {
                    return Err(Diagnostic::new(
                        "a MIDI channel is a whole number from 1 to 16",
                        arg.value.span,
                    ));
                }
            },
            other => {
                let mut d =
                    Diagnostic::new(format!("`midi_out` has no argument `{other}`"), option.span);
                if let Some(c) = closest(other, ["port", "channel"]) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
            }
        }
    }
    let Some(port) = port else {
        return Err(Diagnostic::new(
            format!("`{}` needs a `port:`", decl.name.name),
            decl.name.span,
        )
        .with_help("for example `midi_out drums(port: \"IAC Driver\", channel: 10)`"));
    };
    let mut g = GraphBuilder::new();
    g.param("freq", 440.0).expect("a fresh graph");
    g.param(GAIN_PARAM, 1.0).expect("a fresh graph");
    let silence = vec![Src::Const(0.0); names.master.channels()];
    let mut instrument = Instrument::new(
        decl.name.name.clone(),
        Arc::new(g.build_channels(&silence)),
        vec![
            Param {
                name: "freq".into(),
                unit: Unit::Hz,
                range: None,
                default: None,
            },
            Param {
                name: GAIN_PARAM.into(),
                unit: Unit::Db,
                range: None,
                default: Some(1.0),
            },
        ],
    );
    instrument.midi = Some(MidiTarget {
        port: port.into(),
        channel,
    });
    Ok(instrument)
}
