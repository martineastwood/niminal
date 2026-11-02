//! Name lookup and arithmetic: binary and comparison operators, selection, transposition.

use super::*;

impl Lower<'_> {
    pub(super) fn name(&mut self, name: &str, span: Span) -> Res<Sig> {
        if let Some(v) = self.scope.get(name) {
            return self.signal_of(v.clone(), span);
        }
        match name {
            "out" => {
                return self.out.clone().ok_or_else(|| {
                    Diagnostic::new("`out` has no value yet", span)
                        .with_help("set it first, with `out = ...`")
                });
            }
            "in" => {
                return match self.mode {
                    Mode::Node => Ok(self.node_input()),
                    _ => Err(Diagnostic::new(
                        "`in` is only available inside an fx, where it is the audio coming in",
                        span,
                    )),
                };
            }
            "it" => {
                return Err(Diagnostic::new(
                    "`it` is only available inside a function given to `map`",
                    span,
                ));
            }
            "pi" => return Ok(Sig::constant(Unit::Num, std::f64::consts::PI)),
            "true" => return Ok(Sig::constant(Unit::Num, 1.0)),
            "false" => return Ok(Sig::constant(Unit::Num, 0.0)),
            "sample_rate" => {
                if self.mode == Mode::Plain {
                    return Err(Diagnostic::new(
                        "the sample rate isn't known while compiling",
                        span,
                    ));
                }
                let src = self.g.add(SampleRate, &[]).expect("no ports");
                self.note_rate(src, Rate::Init);
                return Ok(Sig::signal(src, Unit::Hz));
            }
            "note" => {
                return Err(Diagnostic::new(
                    "`note` describes the running voice: `note.time`, `note.released`, `note.start`, `note.dur` or `note.index`",
                    span,
                ));
            }
            "clock" => {
                return Err(Diagnostic::new(
                    "`clock` describes the transport: `clock.time`",
                    span,
                ));
            }
            _ => {}
        }
        if let Some(control) = self.names.controls.iter().find(|c| c.name == name) {
            if self.mode == Mode::Plain {
                return Err(Diagnostic::new(
                    "a live control cannot be used as a constant",
                    span,
                ));
            }
            let src = self
                .g
                .add(
                    niminal_engine::ops::Control::new(control.key.clone(), control.initial),
                    &[],
                )
                .map_err(|e| Diagnostic::new(e.to_string(), span))?;
            self.note_rate(src, Rate::Control);
            return Ok(Sig {
                srcs: vec![src].into(),
                layout: Layout::Mono,
                unit: control.unit,
                konst: None,
            });
        }
        if let Some(bus) = self.names.bus(name) {
            return match self.mode {
                Mode::Node => {
                    let layout = self.names.bus_layouts[bus];
                    Ok(self.input_bundle(name, layout, |channel| ChainInput::Bus { bus, channel }))
                }
                _ => Err(Diagnostic::new(
                    format!("`{name}` is a bus, and only an fx can read a bus"),
                    span,
                )
                .with_help(format!("an instrument can send to it with `{name} += ...`"))),
            };
        }
        if let Ok(note @ Value::Note(_)) = name.parse::<Value>() {
            return Ok(Sig::constant(
                Unit::Hz,
                note.as_hz_in(&self.tempo.tuning)
                    .expect("a note is a frequency"),
            ));
        }
        if self.names.fns.contains_key(name) {
            return Err(Diagnostic::new(
                format!("`{name}` is a function: call it with `{name}(...)`"),
                span,
            ));
        }
        if opcodes::wave(name).is_some() {
            return Err(
                Diagnostic::new(format!("`{name}` is a waveform, not a value"), span)
                    .with_help("waveforms go in the first slot of `osc`, as in `osc(saw, freq)`"),
            );
        }
        Err(unknown_name(
            name,
            span,
            self.scope.keys().map(String::as_str),
        ))
    }

    // ---- arithmetic ----------------------------------------------------

    /// Combine two signals channel by channel. A one-channel signal is applied
    /// to every channel of the other.
    pub(super) fn elementwise(
        &mut self,
        a: &Sig,
        b: &Sig,
        unit: Unit,
        span: Span,
        make: impl Fn(&mut GraphBuilder, Src, Src) -> Src,
    ) -> Res<Sig> {
        let n = match (a.count(), b.count()) {
            (x, y) if x == y => x,
            (1, y) => y,
            (x, 1) => x,
            (x, y) => {
                return Err(Diagnostic::new(
                    format!("can't combine a {x}-channel signal with a {y}-channel one"),
                    span,
                )
                .with_help("a one-channel signal can be combined with any; otherwise the counts must match"));
            }
        };
        let layout = if a.count() >= b.count() {
            a.layout
        } else {
            b.layout
        };
        let rate = self.sig_rate(a).max(self.sig_rate(b));
        let srcs: Vec<Src> = (0..n)
            .map(|k| make(&mut self.g, a.pick(k), b.pick(k)))
            .collect();
        for &src in &srcs {
            self.note_rate(src, rate);
        }
        Ok(Sig {
            srcs: srcs.into(),
            layout,
            unit,
            konst: None,
        })
    }

    pub(super) fn mul(&mut self, a: &Sig, b: &Sig, unit: Unit, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(unit, x * y));
        }
        self.elementwise(a, b, unit, span, |g, x, y| {
            g.add(Mul, &[("a", x), ("b", y)]).expect("mul ports")
        })
    }

    pub(super) fn negate(&mut self, s: Sig, span: Span) -> Res<Sig> {
        match (s.unit, s.konst) {
            // A decibel value is held as a gain factor, so negating it inverts it.
            (Unit::Db, Some(k)) => Ok(Sig::constant(Unit::Db, 1.0 / k)),
            (Unit::Db, None) => Err(Diagnostic::new(
                "negating a changing db value isn't supported yet",
                span,
            )),
            (unit, _) => self.mul(&s, &Sig::constant(Unit::Num, -1.0), unit, span),
        }
    }

    pub(super) fn binary(&mut self, op: BinOp, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            BinOp::Mul => "multiply",
            BinOp::Div => "divide",
        };
        let (lu, ru) = (l.unit, r.unit);
        let mismatch = || {
            Diagnostic::new(
                format!("can't {verb} {} and {}", lu.describe(), ru.describe()),
                span,
            )
        };

        match op {
            BinOp::Add | BinOp::Sub => {
                // A frequency moved by an interval.
                if lu == Unit::Hz && ru == Unit::Semitones {
                    return self.transpose(&l, &r, op == BinOp::Sub, span);
                }
                if lu == Unit::Semitones && ru == Unit::Hz && op == BinOp::Add {
                    return self.transpose(&r, &l, false, span);
                }
                if lu != ru {
                    return Err(mismatch());
                }
                if lu == Unit::Db {
                    // Adding decibels multiplies gain factors.
                    let r = if op == BinOp::Sub {
                        self.invert(&r, span)?
                    } else {
                        r
                    };
                    return self.mul(&l, &r, Unit::Db, span);
                }
                if let (Some(x), Some(y)) = (l.konst, r.konst) {
                    let v = if op == BinOp::Add { x + y } else { x - y };
                    return Ok(Sig::constant(lu, v));
                }
                if op == BinOp::Add {
                    self.elementwise(&l, &r, lu, span, |g, x, y| {
                        g.add(Add, &[("a", x), ("b", y)]).expect("add ports")
                    })
                } else {
                    self.elementwise(&l, &r, lu, span, |g, x, y| {
                        g.add(Sub, &[("a", x), ("b", y)]).expect("sub ports")
                    })
                }
            }
            BinOp::Mul => {
                let unit = crate::unit::mul_unit(lu, ru).ok_or_else(mismatch)?;
                self.mul(&l, &r, unit, span)
            }
            BinOp::Div => {
                let unit = crate::unit::div_unit(lu, ru).ok_or_else(mismatch)?;
                let Some(k) = r.konst else {
                    // a changing divisor: worked out sample by sample (0 where it is 0)
                    return self.elementwise(&l, &r, unit, span, |g, x, y| {
                        g.add(Div, &[("a", x), ("b", y)]).expect("div ports")
                    });
                };
                if k == 0.0 {
                    return Ok(Sig::constant(unit, 0.0));
                }
                self.mul(&l, &Sig::constant(Unit::Num, 1.0 / k), unit, span)
            }
        }
    }

    /// `a.mod(b)`: the remainder, with the sign of `b`.
    pub(super) fn rem(&mut self, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        let unit = crate::unit::rem_unit(l.unit, r.unit).ok_or_else(|| {
            Diagnostic::new(
                format!(
                    "can't take the remainder of {} and {}",
                    l.unit.describe(),
                    r.unit.describe()
                ),
                span,
            )
        })?;
        if let (Some(x), Some(y)) = (l.konst, r.konst) {
            return Ok(Sig::constant(unit, niminal_engine::ops::rem(x, y)));
        }
        self.elementwise(&l, &r, unit, span, |g, x, y| {
            g.add(Rem, &[("a", x), ("b", y)]).expect("rem ports")
        })
    }

    /// `a < b`: 1 where it holds, 0 where it doesn't.
    pub(super) fn compare(&mut self, op: CmpOp, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        if l.unit != r.unit {
            return Err(Diagnostic::new(
                format!(
                    "can't compare {} and {}",
                    l.unit.describe(),
                    r.unit.describe()
                ),
                span,
            ));
        }
        let cmp: Cmp = cmp_op(op);
        if let (Some(x), Some(y)) = (l.konst, r.konst) {
            return Ok(Sig::constant(
                Unit::Num,
                if cmp.holds(x, y) { 1.0 } else { 0.0 },
            ));
        }
        self.elementwise(&l, &r, Unit::Num, span, move |g, x, y| {
            g.add(Compare(cmp), &[("a", x), ("b", y)])
                .expect("compare ports")
        })
    }

    /// `if cond { a } else { b }` as a value. A condition known while compiling picks a side then and
    /// there; otherwise both are worked out and one is kept, sample by sample.
    pub(super) fn if_value(&mut self, e: &Expr) -> Res<Datum> {
        let ExprKind::If {
            cond,
            then,
            otherwise,
        } = &e.kind
        else {
            unreachable!()
        };
        let Some(otherwise) = otherwise else {
            return Err(
                Diagnostic::new("an `if` used as a value needs an `else`", e.span).with_help(
                    "it makes a value, so both ways need one: `if x > 0 { a } else { b }`",
                ),
            );
        };
        let c = self.condition(cond)?;
        if let Some(k) = c.konst {
            let taken = if k != 0.0 { then } else { otherwise };
            return Ok(self.block(taken, true, e.span)?.expect("a value"));
        }
        let sends = self.sends;
        let a = self.block(then, true, e.span)?.expect("a value");
        let b = self.block(otherwise, true, e.span)?.expect("a value");
        if self.sends != sends {
            return Err(Diagnostic::new(
                "a send inside an `if` that changes while playing would always be made",
                e.span,
            ));
        }
        self.select_value(&c, a, b, e.span)
    }

    pub(super) fn select_value(&mut self, condition: &Sig, a: Datum, b: Datum, span: Span) -> Res<Datum> {
        match (a, b) {
            (Datum::Sig(a), Datum::Sig(b)) => {
                self.select_sig(condition, a, b, span).map(Datum::Sig)
            }
            (Datum::Array(a), Datum::Array(b)) => {
                if a.len() != b.len() {
                    return Err(Diagnostic::new(
                        "both branches need the same array shape",
                        span,
                    ));
                }
                Ok(Datum::Array(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| self.select_value(condition, a, b, span))
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "both branches need matching numbers or array shapes",
                span,
            )),
        }
    }

    /// `a` where `c` is not 0, `b` elsewhere, channel by channel.
    pub(super) fn select_sig(&mut self, c: &Sig, a: Sig, b: Sig, span: Span) -> Res<Sig> {
        if a.unit != b.unit {
            return Err(Diagnostic::new(
                format!(
                    "the two ways of an `if` must have the same unit: one is {}, the other {}",
                    a.unit.describe(),
                    b.unit.describe()
                ),
                span,
            ));
        }
        if let Some(k) = c.konst {
            return Ok(if k != 0.0 { a } else { b });
        }
        let n = [c.count(), a.count(), b.count()]
            .into_iter()
            .max()
            .unwrap_or(1);
        for s in [c, &a, &b] {
            if s.count() != 1 && s.count() != n {
                return Err(Diagnostic::new(
                    format!("can't combine a {n}-channel signal with a {}-channel one", s.count()),
                    span,
                )
                .with_help("a one-channel signal can be combined with any; otherwise the counts must match"));
            }
        }
        let layout = [c, &a, &b]
            .into_iter()
            .find(|s| s.count() == n)
            .map_or(Layout::Mono, |s| s.layout);
        let rate = self
            .sig_rate(c)
            .max(self.sig_rate(&a))
            .max(self.sig_rate(&b));
        let srcs: Vec<Src> = (0..n)
            .map(|k| {
                self.g
                    .add(
                        Select,
                        &[("cond", c.pick(k)), ("a", a.pick(k)), ("b", b.pick(k))],
                    )
                    .expect("select ports")
            })
            .collect();
        for &src in &srcs {
            self.note_rate(src, rate);
        }
        Ok(Sig {
            srcs: srcs.into(),
            layout,
            unit: a.unit,
            konst: None,
        })
    }

    /// `freq + 7st`: scale a frequency by 2^(st/12).
    pub(super) fn transpose(&mut self, freq: &Sig, interval: &Sig, down: bool, span: Span) -> Res<Sig> {
        let direction = if down { -1.0 } else { 1.0 };
        if let Some(st) = interval.konst {
            let ratio = Sig::constant(Unit::Num, 2f64.powf(direction * st / 12.0));
            return self.mul(freq, &ratio, Unit::Hz, span);
        }
        // Changing intervals go through exp: 2^x = e^(x ln 2).
        let scale = Sig::constant(Unit::Num, direction * std::f64::consts::LN_2 / 12.0);
        let exponent = self.mul(interval, &scale, Unit::Num, span)?;
        let srcs: Array<Src> = exponent
            .srcs
            .iter()
            .map(|&s| {
                self.g
                    .add(Func(Fn1::Exp), &[("x", s)])
                    .expect("exp has one port")
            })
            .collect();
        let ratio = Sig {
            srcs,
            layout: exponent.layout,
            unit: Unit::Num,
            konst: None,
        };
        self.mul(freq, &ratio, Unit::Hz, span)
    }

    pub(super) fn invert(&mut self, s: &Sig, span: Span) -> Res<Sig> {
        match s.konst {
            Some(k) => Ok(Sig::constant(s.unit, 1.0 / k)),
            None => Err(Diagnostic::new(
                "subtracting a changing db value isn't supported yet",
                span,
            )),
        }
    }

    // ---- calls ----------------------------------------------------------

}
