//! Envelope literals and the note, clock and unit context a body can read.

use super::*;

impl Lower<'_> {
    pub(super) fn env(&mut self, env: &EnvLit) -> Res<Sig> {
        // each step: its value now, and the signal it is read from at note start if it isn't known yet
        let mut inputs: Vec<(EnvField, Src)> = Vec::new();
        let start = self.env_step(&env.start, false)?;
        let unit = start.unit;
        if start.konst.is_none() {
            inputs.push((EnvField::Start, start.srcs[0]));
        }

        let mut segments = Vec::with_capacity(env.segments.len());
        for (k, seg) in env.segments.iter().enumerate() {
            let level = self.env_step(&seg.level, false)?;
            if level.unit != unit {
                return Err(Diagnostic::new(
                    format!(
                        "envelope levels must all have the same unit: this is {}, the first was {}",
                        level.unit.describe(),
                        unit.describe()
                    ),
                    seg.level.span(),
                ));
            }
            let dur = self.env_step(&seg.dur, true)?;
            if dur.konst.is_none() {
                inputs.push((EnvField::Dur(k), dur.srcs[0]));
            }
            if level.konst.is_none() {
                inputs.push((EnvField::Level(k), level.srcs[0]));
            }
            segments.push(Segment::new(
                dur.konst.unwrap_or(0.0) as f32,
                level.konst.unwrap_or(0.0) as f32,
                curve_of(seg.curve),
            ));
        }
        if inputs.len() > niminal_engine::MAX_INPUTS {
            return Err(Diagnostic::new(
                format!(
                    "an envelope can take at most {} of its steps from names; write the others as numbers",
                    niminal_engine::MAX_INPUTS
                ),
                env.start.span(),
            ));
        }

        let looped = env.looped;
        let mut op = Env::new(start.konst.unwrap_or(0.0) as f32, segments, env.sustain_at);
        if looped {
            op = op.looped();
        }
        let (fields, srcs): (Vec<EnvField>, Vec<Src>) = inputs.into_iter().unzip();
        let ports: Vec<String> = (0..srcs.len()).map(|i| format!("v{i}")).collect();
        let wired: Vec<(&str, Src)> = ports.iter().map(String::as_str).zip(srcs).collect();
        let src = self
            .g
            .add(op.with_inputs(fields), &wired)
            .expect("one port per step read at note start");
        Ok(Sig::signal(src, unit))
    }

    /// One step of an envelope: a duration in seconds, or a level.
    pub(super) fn env_step(&mut self, v: &EnvValue, duration: bool) -> Res<Sig> {
        let sig = match v {
            EnvValue::Lit(n) => self.literal(n.value, n.unit.as_deref(), n.span)?,
            EnvValue::Expr(e) => self.expr(e)?,
        };
        let span = v.span();
        if sig.count() != 1 {
            return Err(Diagnostic::new(
                "an envelope step is one value, not several channels",
                span,
            ));
        }
        if duration && sig.unit != Unit::Time {
            return Err(Diagnostic::new(
                format!("expected a duration here, found {}", sig.unit.describe()),
                span,
            )
            .with_help("levels and durations alternate; durations use `ms`, `sec` or `beats`"));
        }
        if !duration && sig.unit == Unit::Time {
            return Err(
                Diagnostic::new("expected a level here, found a duration", span).with_help(
                    "levels and durations alternate: two durations in a row is an error",
                ),
            );
        }
        if !duration && sig.unit == Unit::Db {
            return Err(Diagnostic::new(
                "db levels in envelopes aren't supported yet",
                span,
            ));
        }
        Ok(sig)
    }

    /// `x.hz`, `n.ms`, `(-6db).num`...: give a plain value a unit, or take it away.
    pub(super) fn unit_method(&mut self, name: &Ident, args: &[Arg]) -> Res<Option<Datum>> {
        if !funcs::UNIT_WORDS.contains(&name.name.as_str())
            || !(args.len() == 1 && args[0].receiver)
        {
            return Ok(None);
        }
        let value = self.value(&args[0].value)?;
        let span = args[0].value.span;
        self.map_value(value, span, |this, value| {
            this.unit_value(name, value, span)
        })
        .map(Some)
    }

    pub(super) fn unit_value(&mut self, name: &Ident, v: Sig, span: Span) -> Res<Sig> {
        if name.name == "num" {
            if v.unit == Unit::Db {
                // a db value is held as a gain factor
                let ln = self.func(
                    Fn1::Log,
                    &Sig {
                        unit: Unit::Num,
                        ..v
                    },
                );
                let k = Sig::constant(Unit::Num, 20.0 / std::f64::consts::LN_10);
                return self.mul(&ln, &k, Unit::Num, span);
            }
            return Ok(Sig {
                unit: Unit::Num,
                ..v
            });
        }
        if v.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "`.{}` gives a plain number a unit, but this is {}",
                    name.name,
                    v.unit.describe()
                ),
                span,
            )
            .with_help("take the unit away first with `.num`"));
        }
        let (unit, factor) = literal_sig(1.0, Some(&name.name), self.tempo, name.span)?;
        if unit == Unit::Db {
            let k = Sig::constant(Unit::Num, std::f64::consts::LN_10 / 20.0);
            let e = self.mul(&v, &k, Unit::Num, span)?;
            let g = self.func(Fn1::Exp, &e);
            return Ok(Sig {
                unit: Unit::Db,
                ..g
            });
        }
        self.mul(&v, &Sig::constant(Unit::Num, factor), unit, span)
    }

    /// `note.time`, `note.released`, `note.start`, `note.dur` and `note.index`: the running voice.
    pub(super) fn note_context(&mut self, name: &Ident) -> Res<Sig> {
        if self.mode != Mode::Instrument || self.in_fn {
            return Err(
                Diagnostic::new("`note` is only available in an instrument", name.span)
                    .with_help("pass `note.time` or `note.released` to a function as an argument"),
            );
        }
        match name.name.as_str() {
            "time" => Ok(Sig::signal(
                self.g.add(NoteTime::default(), &[]).expect("no ports"),
                Unit::Time,
            )),
            "released" => Ok(Sig::signal(
                self.g.add(Released, &[]).expect("no ports"),
                Unit::Num,
            )),
            "start" => Ok(Sig::signal(
                self.g.add(NoteStart::default(), &[]).expect("no ports"),
                Unit::Num,
            )),
            "dur" => Ok(self.read_context(Context::NoteDur, Unit::Time, Rate::Init)),
            "index" => Ok(self.read_context(Context::NoteIndex, Unit::Num, Rate::Init)),
            _ => Err(no_field(
                "note",
                name,
                &["time", "released", "start", "dur", "index"],
            )),
        }
    }

    pub(super) fn read_context(&mut self, what: Context, unit: Unit, rate: Rate) -> Sig {
        let src = self.g.add(ReadContext::new(what), &[]).expect("no ports");
        self.note_rate(src, rate);
        Sig::signal(src, unit)
    }

    /// `clock.time`: where the session's transport is, in seconds.
    pub(super) fn clock_context(&mut self, name: &Ident) -> Res<Sig> {
        if self.mode == Mode::Plain {
            return Err(Diagnostic::new(
                "`clock` is only available in an instrument, node or bus",
                name.span,
            ));
        }
        match name.name.as_str() {
            "time" => Ok(self.read_context(Context::ClockTime, Unit::Time, Rate::Audio)),
            _ => Err(no_field("clock", name, &["time"])),
        }
    }

    /// `late(x)`, in a bus send: `x` as it was a block ago, which breaks a loop through buses.
    pub(super) fn late(&mut self, name: &Ident, args: &[Arg]) -> Res<Sig> {
        let [arg] = args else {
            return Err(Diagnostic::new("`late` takes one value", name.span));
        };
        let Some(ctx) = &mut self.in_send else {
            return Err(Diagnostic::new("`late` is only for a send to a bus", name.span)
                .with_help("for a loop through buses, write `echo += late(out)`; for one sample ago, use `prev`"));
        };
        *ctx = true;
        self.expr(&arg.value)
    }

    /// `prev(x)`: what `x` was one sample ago.
    pub(super) fn prev(&mut self, name: &Ident, args: &[Arg]) -> Res<Sig> {
        let [arg] = args else {
            return Err(Diagnostic::new("`prev` takes one value", name.span));
        };
        if self.in_send.is_some() {
            return Err(Diagnostic::new(
                "a bus runs in blocks, so a send can't read one sample ago",
                name.span,
            )
            .with_help("use `late(x)` for a send that arrives a block late: `echo += late(out)`"));
        }
        if let ExprKind::Name(n) = &arg.value.kind
            && !self.scope.contains_key(n)
            && self.names.controls.iter().all(|c| c.name != *n)
            && self.names.bus(n).is_none()
            && !self.names.consts.contains_key(n)
        {
            return Err(Diagnostic::new(
                format!("`prev({n})` reads `{n}`, which isn't defined here"),
                arg.value.span,
            )
            .with_help(format!(
                "a loop is written on one line: `{n} = x + prev({n}) * 0.5`"
            )));
        }
        let x = self.expr(&arg.value)?;
        let srcs: Array<Src> = x
            .srcs
            .iter()
            .map(|&s| {
                self.g
                    .add(Delay1::default(), &[("x", s)])
                    .expect("prev ports")
            })
            .collect();
        Ok(Sig {
            srcs,
            konst: None,
            ..x
        })
    }
}
