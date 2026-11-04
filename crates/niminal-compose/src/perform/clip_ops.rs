//! Operations on clips and lanes, and the generic pattern transformers.

use super::*;

impl Perform<'_> {
    pub(super) fn on_clip(
        &self,
        name: &Ident,
        rest: &[Arg],
        clip: Clip,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<Clip> {
        if name.name == "set" {
            return self.set_lanes(rest, clip, it);
        }
        if name.name == "times" {
            let n = self.whole(self.arg(name, rest, "count", span)?, "times")?;
            return Ok(Clip {
                plays: Some(n as u32),
                endless: false,
                ..clip
            });
        }
        // The lanes span the clip, so `over` stretches them from the clip's length to the new one.
        if name.name == "over" {
            let e = self.arg(name, rest, "length", span)?;
            let length = self.cycles(e)?;
            if length <= Rational::ZERO {
                return Err(Diagnostic::new("`over` must be above zero", e.span));
            }
            let length = length.max(Rational::new(1, 1000));
            let factor = length / clip.length;
            let lanes = clip
                .lanes
                .into_iter()
                .map(|(lane, p)| (lane, p.slow(factor)))
                .collect();
            return Ok(Clip {
                name: clip.name,
                length,
                endless: false,
                plays: clip.plays,
                then: clip.then,
                lanes,
                scored: clip.scored,
                keys: clip.keys,
                curves: clip.curves,
                signals: Vec::new(),
                fx_params: Vec::new(),
            });
        }
        // Pitch operations touch only the notes; time operations touch every lane.
        let notes_only = matches!(
            name.name.as_str(),
            "transpose"
                | "echo"
                | "strum"
                | "often"
                | "rarely"
                | "add"
                | "sub"
                | "mul"
                | "div"
                | "spread_evenly"
                | "with_rhythm_of"
                | "keep_where"
                | "drop"
                | "sometimes"
                | "in_scale"
                | "snap_to"
                | "chord"
                | "arpeggiate"
                | "voice_lead"
                | "invert_melody"
                | "markov"
                | "random_walk"
                | "where"
                | "gain"
                | "pan"
                | "begin"
                | "stereo_copy"
                | "chop"
                | "interleave_chops"
        );
        let mut lanes = Vec::with_capacity(clip.lanes.len());
        for (lane, pattern) in &clip.lanes {
            if notes_only && lane != "notes" {
                lanes.push((lane.clone(), pattern.clone()));
                continue;
            }
            match self.on_pattern(name, rest, pattern.clone(), span, it)? {
                PerfValue::Pattern(p) => lanes.push((lane.clone(), p)),
                _ => unreachable!("operations on a pattern give a pattern"),
            }
        }
        let length = match name.name.as_str() {
            "fast" | "slow" => match self.num_param(self.arg(name, rest, "by", span)?, it)? {
                Param::Const(by) if name.name == "fast" => clip.length / by,
                Param::Const(by) => clip.length * by,
                Param::Pat(_) => clip.length,
            },
            "over" => self
                .cycles(self.arg(name, rest, "length", span)?)?
                .max(Rational::new(1, 1000)),
            _ => clip.length,
        };
        Ok(Clip {
            name: clip.name,
            length,
            endless: clip.endless && name.name != "over",
            plays: clip.plays,
            then: clip.then,
            lanes,
            scored: clip.scored,
            keys: clip.keys,
            curves: clip.curves,
            signals: Vec::new(),
            fx_params: Vec::new(),
        })
    }

    /// `riff.set(bright: 0.9, gain: [0db -6db])`: these lanes, in place of the clip's own.
    pub(super) fn set_lanes(&self, rest: &[Arg], mut clip: Clip, it: Option<&PerfValue>) -> Res<Clip> {
        for arg in rest {
            let Some(lane) = &arg.name else {
                return Err(Diagnostic::new(
                    "`set` takes a lane and its value, as in `set(bright: 0.9)`",
                    arg.value.span,
                ));
            };
            if lane.name == "length" {
                return Err(Diagnostic::new(
                    "a clip's length can't be set; use `over(…)`",
                    lane.span,
                ));
            }
            let pattern = match self.eval(&arg.value, it)? {
                PerfValue::Pattern(p) => p,
                PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                PerfValue::Env(_) if lane.name == "notes" => {
                    return Err(Diagnostic::new(
                        "the `notes` lane holds notes, not an envelope",
                        arg.value.span,
                    ));
                }
                PerfValue::Env(curve) => {
                    clip.lanes.retain(|(n, _)| *n != lane.name);
                    match clip.curves.iter_mut().find(|(n, _)| *n == lane.name) {
                        Some((_, existing)) => *existing = curve,
                        None => clip.curves.push((lane.name.clone(), curve)),
                    }
                    continue;
                }
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "a lane holds a pattern, a number or an envelope, but this is {}",
                            other.describe()
                        ),
                        arg.value.span,
                    ));
                }
            };
            clip.curves.retain(|(n, _)| *n != lane.name);
            let pattern = pattern.slow(clip.length);
            match clip.lanes.iter_mut().find(|(n, _)| *n == lane.name) {
                Some((_, existing)) => *existing = pattern,
                None => clip.lanes.push((lane.name.clone(), pattern)),
            }
        }
        Ok(clip)
    }

    /// Operations that work on any kind of pattern. `None` if `name` isn't one.
    pub(super) fn generic<'a, T: Kind>(
        &'a self,
        name: &Ident,
        rest: &'a [Arg],
        p: Pattern<T>,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<Option<Pattern<T>>> {
        let max = MAX_REPEATS;
        let positive = |r: Rational, what: &str, at: Span| {
            if r <= Rational::ZERO {
                Err(Diagnostic::new(format!("`{what}` must be above zero"), at))
            } else if r > Rational::int(max) || r < Rational::new(1, max) {
                Err(Diagnostic::new(
                    format!("`{what}` takes a number from 1/{max} to {max}"),
                    at,
                ))
            } else {
                Ok(r)
            }
        };
        Ok(Some(match name.name.as_str() {
            "fast" | "slow" => {
                let e = self.arg(name, rest, "by", span)?;
                let by = self.num_param(e, it)?;
                if let Param::Const(r) = &by {
                    positive(*r, &name.name, e.span)?;
                }
                let slow = name.name == "slow";
                by.apply(&p, move |p, f| {
                    let f = f.min(Rational::int(max));
                    if slow { p.slow(f) } else { p.fast(f) }
                })
            }
            "over" => {
                let e = self.arg(name, rest, "length", span)?;
                p.slow(positive(self.cycles(e)?, "over", e.span)?)
            }
            "reverse" => p.reverse(),
            "back_and_forth" => p.palindrome(),
            "shift" => p.shift(self.cycles(self.arg(name, rest, "by", span)?)?),
            "repeat_each" => {
                let e = self.arg(name, rest, "times", span)?;
                let times = self.num_param(e, it)?;
                if let Param::Const(n) = &times
                    && (!n.is_integer() || *n < Rational::ONE || *n > Rational::int(MAX_REPEATS))
                {
                    return Err(Diagnostic::new(
                        format!("`repeat_each` takes a whole number from 1 to {MAX_REPEATS}"),
                        name.span,
                    ));
                }
                times.apply(&p, |p, n| p.repeat_each(n.floor()))
            }
            "drop" => {
                let amount = self.share_param(name, self.arg(name, rest, "amount", span)?, it)?;
                // The same amount always thins the same events, wherever it is written.
                let default = match &amount {
                    Param::Const(x) => x.to_bits(),
                    Param::Pat(_) => 0x4452_4F50,
                };
                let seed = self.seed(name, rest, default)?;
                amount.apply(&p, move |p, x| p.thin(x.clamp(0.0, 1.0), seed))
            }
            "sometimes" => {
                let chance = self.share_param(name, self.arg(name, rest, "chance", span)?, it)?;
                let default = match &chance {
                    Param::Const(c) => c.to_bits(),
                    Param::Pat(_) => 0x534F_4D45,
                };
                let seed = self.seed(name, rest, default)?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                match self.find(name, rest, "per") {
                    None => p.sometimes_by(&chance, seed, change),
                    Some(e) => match &e.kind {
                        ExprKind::Name(w) if w == "note" => p.sometimes_by(&chance, seed, change),
                        ExprKind::Name(w) if w == "cycle" => p.some_cycles(&chance, seed, change),
                        _ => {
                            return Err(Diagnostic::new(
                                "`per` is `note` or `cycle`, as in `sometimes(25%, reverse, per: cycle)`",
                                e.span,
                            ));
                        }
                    },
                }
            }
            "every" => {
                let e = self.arg(name, rest, "n", span)?;
                let n = self.num_param(e, it)?;
                if let Param::Const(n) = &n
                    && (!n.is_integer() || *n < Rational::ONE)
                {
                    return Err(Diagnostic::new(
                        "`every` takes a whole number of at least 1",
                        name.span,
                    ));
                }
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.every_by(&n, change)
            }
            "overlay" => {
                let after = match self.find(name, rest, "after") {
                    Some(e) => self.cycles(e)?,
                    None => Rational::ZERO,
                };
                let f = self.arg(name, rest, "change", span)?;
                let change = self.transformer::<T>(f, &p)?;
                p.layer(after, change)
            }
            "only_between" => {
                let from = self.fraction(self.arg(name, rest, "from", span)?)?;
                let to = self.fraction(self.arg(name, rest, "to", span)?)?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.within(from, to, change)
            }
            "swing" => {
                let amount = self.share_param(name, self.arg(name, rest, "amount", span)?, it)?;
                let e = self.arg(name, rest, "every", span)?;
                let every = self.cycles(e)?;
                if every <= Rational::ZERO || every > Rational::ONE {
                    return Err(Diagnostic::new(
                        "`swing` needs `every` to be a share of a bar, such as `1/8beat`",
                        e.span,
                    ));
                }
                let slots = Rational::ONE / every;
                amount.apply(&p, move |p, x| p.swing(Rational::from_f64(x), slots))
            }
            "start_later" => {
                let times = self.num_param(self.arg(name, rest, "parts", span)?, it)?;
                times.apply(&p, |p, n| p.rotate(n.floor().min(MAX_REPEATS)))
            }
            "step_through" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "step_through")?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.in_turn(n, change)
            }
            "shuffle" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "shuffle")?;
                p.shuffle(n, self.seed(name, rest, 0)?)
            }
            "pick_random" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "pick_random")?;
                p.pick_random(n, self.seed(name, rest, 0)?)
            }
            "loop_start" => {
                let e = self.arg(name, rest, "length", span)?;
                let fraction = self.fraction(e)?;
                if fraction <= Rational::ZERO || fraction > Rational::ONE {
                    return Err(Diagnostic::new(
                        "`loop_start` takes a share of the cycle, from above 0% to 100%",
                        e.span,
                    ));
                }
                p.repeat_start(fraction)
            }
            "take" => {
                let length = self.share_param(name, self.arg(name, rest, "length", span)?, it)?;
                length.apply(&p, |p, x| {
                    if x <= 0.0 {
                        Pattern::silence()
                    } else {
                        p.trunc(Rational::from_f64(x.min(1.0)))
                    }
                })
            }
            "crop" | "squeeze" => {
                let (from_e, to_e) = (
                    self.arg(name, rest, "from", span)?,
                    self.arg(name, rest, "to", span)?,
                );
                let (from, to) = (self.fraction(from_e)?, self.fraction(to_e)?);
                if from < Rational::ZERO || to > Rational::ONE || from >= to {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` takes two points of the cycle, the first before the second, such as `{}(25%, 75%)`",
                            name.name, name.name
                        ),
                        to_e.span,
                    ));
                }
                if name.name == "crop" {
                    p.zoom(from, to)
                } else {
                    p.compress(from, to)
                }
            }
            "strum" => {
                let amount = match self.find(name, rest, "amount") {
                    Some(e) => self.share_param(name, e, it)?,
                    None => Param::Const(0.25),
                };
                amount.apply(&p, |p, x| p.rolled(Rational::from_f64(x.clamp(0.0, 1.0))))
            }
            "cycle_through" => {
                // `riff.cycle_through(same, reverse, fast(2))`: one change per cycle, in turn
                if rest.is_empty() {
                    return Err(Diagnostic::new(
                        "`cycle_through` needs changes to cycle through, as in `cycle_through(same, reverse)`",
                        name.span,
                    ));
                }
                let mut variants = Vec::with_capacity(rest.len());
                for arg in rest {
                    variants.push(self.transformer::<T>(&arg.value, &p)?(p.clone()));
                }
                Pattern::cycle_through(variants)
            }
            "in_cycles" => {
                let (a_e, b_e) = (
                    self.arg(name, rest, "cycles", span)?,
                    self.arg(name, rest, "from", span)?,
                );
                let (a, b) = (self.plain(a_e)?, self.plain(b_e)?);
                if !a.is_integer() || a < Rational::ONE || !b.is_integer() || b < Rational::ZERO {
                    return Err(Diagnostic::new(
                        "`in_cycles` takes a whole number of cycles and where to start in them, as in `in_cycles(4, 3, fast(2))`",
                        name.span,
                    ));
                }
                let (a, b) = (a.numer(), b.numer());
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.when_cycle(move |c| c.rem_euclid(a) >= b, change)
            }
            "often" | "rarely" => {
                let chance: f64 = if name.name == "often" { 0.75 } else { 0.25 };
                let seed = self.seed(name, rest, chance.to_bits())?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.sometimes(chance, seed, change)
            }
            "to_steps" => {
                let steps = self.num_param(self.arg(name, rest, "steps", span)?, it)?;
                steps.apply(&p, |p, n| {
                    p.rhythm(
                        &Pattern::pure(true)
                            .fast(n.max(Rational::ZERO).min(Rational::int(MAX_REPEATS))),
                    )
                })
            }
            _ => return Ok(None),
        }))
    }

    /// An argument with a unit, fixed (`5st`) or a pattern of them (`[<0st 5st>]`). `read` takes the
    /// number out of a value, `None` if it is the wrong kind; `bad` says what the argument should be.
    pub(super) fn value_param(
        &self,
        e: &Expr,
        it: Option<&PerfValue>,
        read: impl Fn(Value) -> Option<f64> + Send + Sync + 'static,
        bad: &str,
        help: &str,
    ) -> Res<Param<f64>> {
        let wrong = || Diagnostic::new(bad.to_string(), e.span).with_help(help.to_string());
        match self.eval(e, it)? {
            PerfValue::Num(v) => read(v).map(Param::Const).ok_or_else(wrong),
            PerfValue::Pattern(pattern) => {
                // say now, not when the note plays, if a value is of the wrong kind
                let cycle = niminal_pattern::Span::new(Rational::ZERO, Rational::ONE);
                if pattern
                    .query(cycle)
                    .iter()
                    .any(|h| read(h.value.value).is_none())
                {
                    return Err(wrong());
                }
                Ok(Param::Pat(pattern.filter_map(move |s| read(s.value))))
            }
            _ => Err(wrong()),
        }
    }

    /// A share of a cycle that can change from cycle to cycle: `20%`, `1/4`, `2beats`, or a pattern of them.
    pub(super) fn share_param(&self, name: &Ident, e: &Expr, it: Option<&PerfValue>) -> Res<Param<f64>> {
        let per_bar = self.cx.tempo.beats_per_bar;
        let share = self.value_param(
            e,
            it,
            move |v| match v {
                Value::Num(x) | Value::Bars(x) => Some(x),
                Value::Beats(b) => Some(b / per_bar),
                _ => None,
            },
            &format!("`{}` takes a share of the cycle, such as `25%`", name.name),
            "write a percentage, a fraction or a length in beats or bars",
        )?;
        if let Param::Const(x) = &share
            && !(0.0..=1.0).contains(x)
        {
            return Err(Diagnostic::new(
                format!("`{}` takes an amount from 0% to 100%", name.name),
                e.span,
            ));
        }
        Ok(share)
    }

    /// A number argument, fixed (`2`, `3/2`) or a pattern (`<2 4>`).
    pub(super) fn num_param(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Param<Rational>> {
        match self.quantity(e) {
            Ok((r, None)) => return Ok(Param::Const(r)),
            Ok((_, Some(u))) => {
                return Err(Diagnostic::new(
                    format!("expected a plain number, found a value in `{u}`"),
                    e.span,
                ));
            }
            Err(_) => {}
        }
        match self.eval(e, it)? {
            PerfValue::Num(Value::Num(n)) => Ok(Param::Const(Rational::from_f64(n))),
            PerfValue::Pattern(p) => Ok(Param::Pat(p.filter_map(|s| match s.value {
                Value::Num(n) => Some(Rational::from_f64(n)),
                _ => None,
            }))),
            other => Err(Diagnostic::new(
                format!(
                    "expected a number or a pattern of numbers, found {}",
                    other.describe()
                ),
                e.span,
            )),
        }
    }

    /// A share of a cycle: `25%`, `1/4`, or a length such as `2beats`.
    pub(super) fn fraction(&self, e: &Expr) -> Res<Rational> {
        match self.quantity(e)? {
            (r, Some(u)) if u == "%" => Ok(r / Rational::int(100)),
            (r, None) => Ok(r),
            _ => self.cycles(e),
        }
    }

    /// A whole number of pieces, from 1 to the most a step can be divided into.
    pub(super) fn whole(&self, e: &Expr, what: &str) -> Res<i64> {
        let n = self.plain(e)?;
        if n.is_integer() && n >= Rational::ONE && n <= Rational::int(MAX_REPEATS) {
            Ok(n.numer())
        } else {
            Err(Diagnostic::new(
                format!("`{what}` takes a whole number from 1 to {MAX_REPEATS}"),
                e.span,
            ))
        }
    }

    /// The `seed` argument if given: a different seed chooses differently.
    pub(super) fn seed(&self, name: &Ident, rest: &[Arg], default: u64) -> Res<u64> {
        match self.find(name, rest, "seed") {
            Some(e) => Ok(self.plain(e)?.floor() as u64 ^ default),
            None => Ok(default),
        }
    }

    /// A single value, such as the `c3` in `in_scale(minor, root: c3)`.
    pub(super) fn constant(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Value> {
        match self.eval(e, it)? {
            PerfValue::Num(v) => Ok(v),
            PerfValue::Pattern(p) => {
                let at = niminal_pattern::Span::new(Rational::ZERO, Rational::ZERO);
                p.query(at)
                    .into_iter()
                    .next()
                    .map(|h| h.value.value)
                    .ok_or_else(|| Diagnostic::new("expected a single value", e.span))
            }
            _ => Err(Diagnostic::new("expected a single value", e.span)),
        }
    }

    /// A bare word such as `minor7`, looked up in `table`.
    pub(super) fn word<'t, V: ?Sized>(
        &self,
        e: &Expr,
        what: &str,
        table: &'t [(&'static str, &'t V)],
    ) -> Res<&'t V> {
        let ExprKind::Name(word) = &e.kind else {
            return Err(Diagnostic::new(
                format!("expected the name of a {what}"),
                e.span,
            ));
        };
        table
            .iter()
            .find(|(n, _)| n == word)
            .map(|(_, v)| *v)
            .ok_or_else(|| {
                let mut d = Diagnostic::new(format!("`{word}` isn't a {what}"), e.span);
                if let Some(c) = closest(word, table.iter().map(|(n, _)| *n)) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            })
    }

    /// An argument such as `reverse` or `transpose(+7st)`, as a function from pattern to pattern.
    /// It is tried once on `sample` so that mistakes are reported now.
    pub(super) fn transformer<'a, T: Kind>(&'a self, e: &'a Expr, sample: &Pattern<T>) -> Res<Change<'a, T>> {
        let written = self.implicit_it(e);
        let apply = move |p: Pattern<T>| -> Res<Pattern<T>> {
            let e = written.as_ref().unwrap_or(e);
            let out = self.eval(e, Some(&T::wrap(p)))?;
            T::unwrap(out).ok_or_else(|| {
                Diagnostic::new("this change must give the same kind of pattern", e.span)
            })
        };
        apply(sample.clone())?;
        Ok(Box::new(move |p: Pattern<T>| apply(p.clone()).unwrap_or(p)))
    }

    /// A change is written as operations on the pattern with the pattern itself left out:
    /// `reverse`, `fast(2).reverse`. This puts `same` in front.
    /// `None` if the change already starts from something else, such as `same` itself.
    pub(super) fn implicit_it(&self, e: &Expr) -> Option<Expr> {
        let it = |span| Expr {
            kind: ExprKind::Name("same".to_string()),
            span,
        };
        let is_op = |n: &str| {
            !self.bindings.contains_key(n) && crate::ide::pattern_ops().iter().any(|o| o.name == n)
        };
        match &e.kind {
            ExprKind::Name(n) if is_op(n) => Some(Expr {
                kind: ExprKind::Call {
                    name: Ident {
                        name: n.clone(),
                        span: e.span,
                    },
                    args: vec![Arg {
                        name: None,
                        value: it(e.span),
                        receiver: true,
                    }],
                },
                span: e.span,
            }),
            ExprKind::Call { name, args } => match args.first() {
                Some(first) if first.receiver => {
                    let value = self.implicit_it(&first.value)?;
                    let mut args = args.clone();
                    args[0].value = value;
                    Some(Expr {
                        kind: ExprKind::Call {
                            name: name.clone(),
                            args,
                        },
                        span: e.span,
                    })
                }
                _ if is_op(&name.name) => {
                    let mut with_it = vec![Arg {
                        name: None,
                        value: it(name.span),
                        receiver: true,
                    }];
                    with_it.extend(args.iter().cloned());
                    Some(Expr {
                        kind: ExprKind::Call {
                            name: name.clone(),
                            args: with_it,
                        },
                        span: e.span,
                    })
                }
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn unknown_op(&self, name: &str, span: Span) -> Diagnostic {
        use std::sync::OnceLock;
        static NAMES: OnceLock<Vec<String>> = OnceLock::new();
        let names = NAMES.get_or_init(|| {
            crate::ide::pattern_ops()
                .into_iter()
                .map(|o| o.name)
                .collect()
        });
        let mut d = Diagnostic::new(format!("unknown pattern operation `{name}`"), span);
        if let Some(c) = closest(name, names.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        d
    }
}
