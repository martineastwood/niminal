//! Audio input, reports, OSC output and record values.

use super::*;

impl Lower<'_> {
    /// The value of `e` if it is an opcode's several results: a name bound to them, or a call that makes them.
    pub(super) fn record(&mut self, e: &Expr) -> Res<Option<Vec<(String, Datum)>>> {
        if let ExprKind::Name(name) = &e.kind {
            return Ok(match self.scope.get(name) {
                Some(Datum::Record(fields)) => Some(fields.clone()),
                _ => None,
            });
        }
        if let ExprKind::Call { name, .. } = &e.kind
            && (self
                .names
                .fns
                .get(&name.name)
                .is_some_and(|def| matches!(def.result, FnResult::Named(_)))
                || matches!(self.scope.get(&name.name), Some(Datum::Function { def, .. }) if matches!(def.result, FnResult::Named(_))))
        {
            return Ok(match self.value(e)? {
                Datum::Record(fields) => Some(fields),
                _ => None,
            });
        }
        Ok(None)
    }

    pub(super) fn audio_input(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Sig> {
        if self.in_fn {
            return Err(
                Diagnostic::new("`input` can't be used inside a function", name.span)
                    .with_help("read it in the node, and pass it in as an argument"),
            );
        }
        if self.mode != Mode::Node {
            return Err(Diagnostic::new("`input` is read in a node", name.span)
                .with_help("for example `fx voice { out = input(1).lpf(cutoff: 2khz) }`"));
        }
        let channel = |arg: &Arg| -> Res<usize> {
            match &arg.value.kind {
                ExprKind::Num { value, unit: None }
                    if arg.name.is_none()
                        && value.fract() == 0.0
                        && (1.0..=16.0).contains(value) =>
                {
                    Ok(*value as usize)
                }
                _ => Err(Diagnostic::new(
                    "an input channel is a whole number from 1 to 16",
                    arg.value.span,
                )),
            }
        };
        let ends = |arg: &Arg| -> Option<(Arg, Arg)> {
            let ExprKind::Range {
                lo,
                hi,
                inclusive: true,
            } = &arg.value.kind
            else {
                return None;
            };
            let wrap = |e: &Expr| Arg {
                name: None,
                value: e.clone(),
                receiver: false,
            };
            Some((wrap(lo), wrap(hi)))
        };
        let (first, last) = match args {
            [a] => match ends(a) {
                Some((lo, hi)) => (channel(&lo)?, channel(&hi)?),
                None => (channel(a)?, channel(a)?),
            },
            _ => {
                return Err(Diagnostic::new(
                    "`input` takes a channel, or a range of them",
                    name.span,
                )
                .with_help("for example `input(1)` or `input(1..2)`"));
            }
        };
        if last < first {
            return Err(
                Diagnostic::new("the first channel of a range comes first", span)
                    .with_help("for example `input(1..2)`"),
            );
        }
        let layout = Layout::from_count(last - first + 1);
        Ok(
            self.input_bundle(&format!("input:{first}:{last}"), layout, move |c| {
                ChainInput::Audio {
                    channel: first - 1 + c,
                }
            }),
        )
    }

    pub(super) fn report(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        if self.in_fn {
            return Err(Diagnostic::new(
                "report belongs in an instrument or fx",
                name.span,
            ));
        }
        let receiver = args
            .first()
            .filter(|a| a.receiver)
            .ok_or_else(|| Diagnostic::new("use signal.report(\"name\")", name.span))?;
        let value = self.value(&receiver.value)?;
        let mut entries = self
            .entries(value.clone(), receiver.value.span)
            .or_else(|_| Ok::<_, Diagnostic>(vec![value.clone()]))?;
        if entries.is_empty() || entries.len() > 64 {
            return Err(Diagnostic::new("report needs 1–64 values", name.span));
        }
        let mut sources = Vec::new();
        for entry in entries.drain(..) {
            let sig = self.signal_of(entry, receiver.value.span)?;
            sources.extend(sig.srcs.iter().copied());
        }
        if sources.is_empty() || sources.len() > 64 {
            return Err(Diagnostic::new(
                "report needs 1–64 scalar values",
                name.span,
            ));
        }
        let mut label = None;
        let mut rate = 30.;
        let mut trigger = None;
        for arg in &args[1..] {
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                None if label.is_none() => {
                    if let ExprKind::Str(s) = &arg.value.kind {
                        label = Some(s.clone());
                    } else {
                        return Err(Diagnostic::new("report name is text", arg.value.span));
                    }
                }
                Some("rate") => {
                    if let ExprKind::Num { value, unit } = &arg.value.kind {
                        if unit.as_deref() != Some("hz") || !(0.5..=200.).contains(value) {
                            return Err(Diagnostic::new(
                                "report rate is 0.5hz–200hz",
                                arg.value.span,
                            ));
                        }
                        rate = *value;
                    } else {
                        return Err(Diagnostic::new(
                            "report rate is a constant in hz",
                            arg.value.span,
                        ));
                    }
                }
                Some("on") => {
                    let sig = self.expr(&arg.value)?;
                    if sig.count() != 1 || sig.unit != Unit::Num {
                        return Err(Diagnostic::new(
                            "report trigger is one plain signal",
                            arg.value.span,
                        ));
                    }
                    trigger = Some(sig.srcs[0]);
                }
                _ => {
                    return Err(Diagnostic::new(
                        "unknown or repeated report argument",
                        arg.value.span,
                    ));
                }
            }
        }
        let label = label
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or_else(|| Diagnostic::new("report needs a name of 1–128 characters", name.span))?;
        let report = niminal_engine::ops::Report::new(label, sources.len());
        let mut ports: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("x{i}"), *s))
            .collect();
        if let Some(trigger) = trigger {
            ports.push(("on".into(), trigger));
        }
        let ports: Vec<_> = ports.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        self.g
            .add(
                niminal_engine::ops::ReportOut::new(report, trigger.is_some(), rate),
                &ports,
            )
            .expect("report ports");
        Ok(value)
    }

    /// `x.osc_out("/vj/level", port: 9000)`: passes `x` through and sends its value as an OSC message,
    /// 30 times a second unless `rate:` says otherwise, to `host:` (this computer if none).
    pub(super) fn osc_out(&mut self, name: &Ident, args: &[Arg]) -> Res<Sig> {
        use niminal_engine::ops::{Tap, TapOut};
        if self.in_fn {
            return Err(
                Diagnostic::new("`osc_out` can't be used inside a function", name.span)
                    .with_help("send from the node that calls it"),
            );
        }
        let Some(signal) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                "`osc_out` works on a signal: write it after one, as in `out.osc_out(...)`",
                name.span,
            ));
        };
        let mut address = None;
        let (mut host, mut port, mut rate, mut trigger) = (None, None, None, None);
        for arg in &args[1..] {
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                None => {
                    if address.is_some() {
                        return Err(Diagnostic::new(
                            "`osc_out` takes its other arguments by name",
                            arg.value.span,
                        ));
                    }
                    address = Some(&arg.value);
                }
                Some("host") => host = Some(&arg.value),
                Some("port") => port = Some(&arg.value),
                Some("rate") => rate = Some(&arg.value),
                Some("on") => trigger = Some(&arg.value),
                Some(other) => {
                    let mut d = Diagnostic::new(
                        format!("`osc_out` has no argument named `{other}`"),
                        arg.name.as_ref().map_or(arg.value.span, |n| n.span),
                    );
                    if let Some(c) = closest(other, ["host", "port", "rate", "on"]) {
                        d = d.with_help(format!("did you mean `{c}`?"));
                    }
                    return Err(d);
                }
            }
        }
        let text = |e: &Expr, what: &str| match &e.kind {
            ExprKind::Str(t) => Ok(t.clone()),
            _ => Err(Diagnostic::new(format!("{what} is text in quotes"), e.span)),
        };
        let address = match address {
            Some(e) => text(e, "an OSC address")?,
            None => {
                return Err(Diagnostic::new("`osc_out` needs an address", name.span)
                    .with_help("for example `osc_out(\"/vj/level\", port: 9000)`"));
            }
        };
        if !address.starts_with('/') {
            return Err(Diagnostic::new(
                "an OSC address starts with `/`",
                address_span(args),
            ));
        }
        let host = match host {
            Some(e) => text(e, "a host")?,
            None => "127.0.0.1".to_string(),
        };
        let number = |e: &Expr, what: &str, unit: Option<&str>| match &e.kind {
            ExprKind::Num { value, unit: u } if u.as_deref() == unit => Ok(*value),
            _ => Err(Diagnostic::new(
                format!(
                    "{what} is a number{}",
                    unit.map_or(String::new(), |u| format!(" in {u}"))
                ),
                e.span,
            )),
        };
        let port = match port {
            Some(e) => {
                let p = number(e, "a port", None)?;
                if p.fract() != 0.0 || !(1.0..=65535.0).contains(&p) {
                    return Err(Diagnostic::new(
                        "a port is a whole number from 1 to 65535",
                        e.span,
                    ));
                }
                p as u16
            }
            None => {
                return Err(Diagnostic::new("`osc_out` needs a `port:`", name.span)
                    .with_help("for example `osc_out(\"/vj/level\", port: 9000)`"));
            }
        };
        let rate = match rate {
            Some(e) => {
                let r = number(e, "a rate", Some("hz"))?;
                if !(0.5..=200.0).contains(&r) {
                    return Err(Diagnostic::new("`rate` is from 0.5hz to 200hz", e.span));
                }
                r
            }
            None => 30.0,
        };
        let sig = self.expr(&signal.value)?;
        self.require_signal("osc_out", &sig, signal.value.span)?;
        if sig.count() != 1 {
            return Err(Diagnostic::new(
                "`osc_out` sends one value, but this signal has several channels",
                signal.value.span,
            )
            .with_help("send a channel at a time: `[l, r] = ...` then `l.osc_out(...)`"));
        }
        let mut ports = vec![("x", sig.srcs[0])];
        let tap = if let Some(trigger) = trigger {
            let value = self.expr(trigger)?;
            if value.count() != 1 || value.unit != Unit::Num {
                return Err(Diagnostic::new(
                    "OSC on is one plain trigger signal",
                    trigger.span,
                ));
            }
            ports.push(("on", value.srcs[0]));
            Tap::triggered()
        } else {
            Tap::new()
        };
        let src = self
            .g
            .add(TapOut::new(tap.clone()), &ports)
            .expect("tap ports");
        self.taps.push(crate::program::OscTap {
            address,
            host,
            port,
            rate,
            value: tap,
        });
        Ok(Sig {
            srcs: vec![src].into(),
            konst: None,
            ..sig
        })
    }
}
