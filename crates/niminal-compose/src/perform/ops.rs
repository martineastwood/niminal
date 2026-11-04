//! Pattern operations (`op`) and `on_pattern`.

use super::*;

impl Perform<'_> {
    // ---- operations ----------------------------------------------------------

    pub(super) fn op(&self, name: &Ident, args: &[Arg], span: Span, it: Option<&PerfValue>) -> Res<PerfValue> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                format!(
                    "`{}` works on a pattern: write it after one, as in `riff.{}`",
                    name.name, name.name
                ),
                name.span,
            ));
        };
        let rest = &args[1..];
        match self.eval(&receiver.value, it)? {
            // `set` gives lanes to a clip; a pattern is a clip with only its notes
            PerfValue::Pattern(p) if name.name == "set" => {
                let clip = Clip {
                    name: String::new(),
                    length: Rational::ONE,
                    endless: true,
                    plays: None,
                    then: Vec::new(),
                    lanes: vec![("notes".to_string(), p)],
                    scored: Vec::new(),
                    keys: None,
                    curves: Vec::new(),
                    signals: Vec::new(),
                    fx_params: Vec::new(),
                };
                self.on_clip(name, rest, clip, span, it)
                    .map(PerfValue::Clip)
            }
            PerfValue::Pattern(p) => self.on_pattern(name, rest, p, span, it),
            PerfValue::Grid(g) if name.name == "invert" => {
                Ok(PerfValue::Grid(g.invert(Hit::new(Strength::Normal))))
            }
            PerfValue::Grid(g) => match self.generic(name, rest, g, span, it)? {
                Some(g) => Ok(PerfValue::Grid(g)),
                None => Err(self.not_for(name, "a grid")),
            },
            PerfValue::Clip(clip) => self
                .on_clip(name, rest, clip, span, it)
                .map(PerfValue::Clip),
            PerfValue::Num(Value::Num(n)) if name.name == "mod" => {
                let by = self.arg(name, rest, "by", span)?;
                match self.eval(by, it)?.as_num() {
                    Some(Value::Num(m)) if m != 0.0 => {
                        Ok(PerfValue::Num(Value::Num(n.rem_euclid(m))))
                    }
                    _ => Err(Diagnostic::new("`mod` takes a number above zero", by.span)),
                }
            }
            other => Err(self.not_for(name, other.describe())),
        }
    }

    pub(super) fn not_for(&self, name: &Ident, what: &str) -> Diagnostic {
        Diagnostic::new(
            format!("`{}` can't be used on {what}", name.name),
            name.span,
        )
    }

    pub(super) fn on_pattern(
        &self,
        name: &Ident,
        rest: &[Arg],
        p: Pattern<Step>,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        if let Some(out) = self.generic(name, rest, p.clone(), span, it)? {
            return Ok(PerfValue::Pattern(out));
        }
        let out = match name.name.as_str() {
            "transpose" => {
                let by = self.value_param(
                    self.arg(name, rest, "by", span)?,
                    it,
                    |v| match v {
                        Value::Semitones(st) => Some(st),
                        _ => None,
                    },
                    "`transpose` takes an interval in semitones",
                    "for example `transpose(+5st)` or `transpose([<0st +7st>])`",
                )?;
                by.apply(&p, |p, st| p.map(on_value(move |v| transpose(v, st))))
            }
            "add" | "sub" | "mul" | "div" => {
                let op = match name.name.as_str() {
                    "add" => BinOp::Add,
                    "sub" => BinOp::Sub,
                    "mul" => BinOp::Mul,
                    _ => BinOp::Div,
                };
                let structure = match self.find(name, rest, "timing") {
                    Some(e) => *self.word(e, "timing", &TIMINGS)?,
                    None => Structure::Left,
                };
                let other = self.eval(self.arg(name, rest, "other", span)?, it)?;
                return self.arithmetic(op, structure, PerfValue::Pattern(p), other, span);
            }
            "echo" => {
                let times = self.whole(self.arg(name, rest, "times", span)?, "echo")?;
                let gap = self.cycles(self.arg(name, rest, "time", span)?)?;
                let fade = match self.find(name, rest, "fade") {
                    Some(e) => match self.quantity(e)? {
                        (d, Some(u)) if u == "db" => d.to_f64(),
                        _ => {
                            return Err(Diagnostic::new(
                                "`fade` is how much quieter each echo gets, in db",
                                e.span,
                            )
                            .with_help("for example `fade: -6db`"));
                        }
                    },
                    None => -6.0,
                };
                let mut copies = vec![p.clone()];
                for i in 1..=times {
                    copies.push(p.shift(gap * Rational::int(i)).map(move |s| Step {
                        gain: Some(s.gain.unwrap_or(0.0) + fade * i as f64),
                        ..s
                    }));
                }
                Pattern::stack(copies)
            }
            "spread_evenly" => {
                let hits = self.num_param(self.arg(name, rest, "hits", span)?, it)?;
                let steps_arg = self.arg(name, rest, "steps", span)?;
                let steps = self.num_param(steps_arg, it)?;
                let rotation = match self.find(name, rest, "rotation") {
                    Some(e) => self.num_param(e, it)?,
                    None => Param::Const(Rational::ZERO),
                };
                if let Param::Const(s) = &steps
                    && *s > Rational::int(MAX_REPEATS)
                {
                    return Err(Diagnostic::new(
                        format!("`spread_evenly` takes at most {MAX_REPEATS} steps"),
                        name.span,
                    ));
                }
                for (arg, what) in [(&hits, "hits"), (&steps, "steps"), (&rotation, "rotation")] {
                    if let Param::Const(r) = arg
                        && (!r.is_integer() || (*r < Rational::ZERO && what != "hits"))
                    {
                        return Err(Diagnostic::new(
                            format!("`{what}` must be a whole number"),
                            name.span,
                        ));
                    }
                }
                p.rhythm(&Pattern::<bool>::euclid_with(&hits, &steps, &rotation))
            }
            "with_rhythm_of" => {
                p.rhythm(&self.structure(self.arg(name, rest, "pattern", span)?, it)?)
            }
            "keep_where" => p.mask(&self.structure(self.arg(name, rest, "pattern", span)?, it)?),
            "range" => {
                let (from, to) = (
                    self.arg(name, rest, "from", span)?,
                    self.arg(name, rest, "to", span)?,
                );
                let (lo, hi) = (self.constant(from, it)?, self.constant(to, it)?);
                if blend(lo, hi, 0.0).is_none() {
                    return Err(Diagnostic::new(
                        "`range` needs two values in the same unit",
                        to.span,
                    )
                    .with_help("for example `range(300hz, 3khz)`"));
                }
                p.map(on_value(move |v| match v {
                    Value::Num(x) => blend(lo, hi, x).unwrap_or(v),
                    other => other,
                }))
            }
            "in_scale" => {
                let scale: &[i32] =
                    self.word(self.arg(name, rest, "scale", span)?, "scale", &SCALES)?;
                let root = match self.find(name, rest, "root") {
                    Some(e) => match self.constant(e, it)? {
                        Value::Note(m) => m,
                        _ => return Err(Diagnostic::new("`root` is a note, such as `c3`", e.span)),
                    },
                    None => 60,
                };
                let len = scale.len() as i64;
                p.map(on_value(move |v| match v {
                    Value::Num(degree) => {
                        let d = degree.floor() as i64;
                        Value::Note(
                            root + scale[d.rem_euclid(len) as usize]
                                + 12 * d.div_euclid(len) as i32,
                        )
                    }
                    other => other,
                }))
            }
            "snap_to" => {
                let scale: &[i32] =
                    self.word(self.arg(name, rest, "scale", span)?, "scale", &SCALES)?;
                let root = match self.find(name, rest, "root") {
                    Some(e) => match self.constant(e, it)? {
                        Value::Note(m) => m,
                        _ => return Err(Diagnostic::new("`root` is a note, such as `c3`", e.span)),
                    },
                    None => 60,
                };
                p.map(on_value(move |v| match v {
                    Value::Note(m) => Value::Note(snap_to_scale(m, root, scale)),
                    other => other,
                }))
            }
            "chord" => {
                let shape: &[i32] =
                    self.word(self.arg(name, rest, "kind", span)?, "chord", &CHORDS)?;
                p.expand(move |s| match s.value {
                    Value::Note(m) => shape.iter().map(|i| s.with(Value::Note(m + i))).collect(),
                    _ => vec![s],
                })
            }
            "arpeggiate" => {
                let e = self.arg(name, rest, "order", span)?;
                let order: &str = self.word(e, "arpeggio order", &ARP_ORDERS)?;
                p.spread(move |notes| arpeggio_order(order, notes))
            }
            "voice_lead" => p.rewrite_cycles(|events, _| {
                // each chord is moved by octaves to sit as close as it can to the one before
                let mut out: Vec<Step> = Vec::with_capacity(events.len());
                let (mut start, mut centre) = (0, None::<f64>);
                while start < events.len() {
                    let end = (start..events.len())
                        .find(|&i| events[i].whole != events[start].whole)
                        .unwrap_or(events.len());
                    let group: Vec<Step> = events[start..end]
                        .iter()
                        .map(|h| match (h.value.value, centre) {
                            (Value::Note(m), Some(c)) => h.value.with(Value::Note(
                                m + 12 * ((c - f64::from(m)) / 12.0).round() as i32,
                            )),
                            _ => h.value,
                        })
                        .collect();
                    let notes: Vec<f64> = group
                        .iter()
                        .filter_map(|s| {
                            if let Value::Note(m) = s.value {
                                Some(f64::from(m))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !notes.is_empty() {
                        centre = Some(notes.iter().sum::<f64>() / notes.len() as f64);
                    }
                    out.extend(group);
                    start = end;
                }
                out
            }),
            "invert_melody" => {
                let around = self.constant(self.arg(name, rest, "around", span)?, it)?;
                let Value::Note(axis) = around else {
                    return Err(Diagnostic::new(
                        "`around` is a note, such as `c4`",
                        name.span,
                    ));
                };
                let axis_hz = around.as_hz().unwrap_or(440.0);
                p.map(on_value(move |v| match v {
                    Value::Note(m) => Value::Note(2 * axis - m),
                    Value::Hz(h) => Value::Hz(axis_hz * axis_hz / h),
                    other => other,
                }))
            }
            "markov" => {
                let seed = self.seed(name, rest, 0x4D41_524B)?;
                p.rewrite_cycles(move |events, cycle| {
                    // the notes follow each other as they do in this cycle, in a new order
                    let values: Vec<Value> = events.iter().map(|h| h.value.value).collect();
                    let Some(&first) = values.first() else {
                        return Vec::new();
                    };
                    let mut chain = vec![first];
                    for i in 1..values.len() {
                        let current = chain[i - 1];
                        let next: Vec<Value> = (0..values.len())
                            .filter(|&j| values[j] == current)
                            .map(|j| values[(j + 1) % values.len()])
                            .collect();
                        let pick = (chance(seed, cycle, i) * next.len() as f64) as usize;
                        chain.push(next.get(pick).copied().unwrap_or(first));
                    }
                    events
                        .iter()
                        .zip(chain)
                        .map(|(h, v)| h.value.with(v))
                        .collect()
                })
            }
            "random_walk" => {
                let e = self.arg(name, rest, "step", span)?;
                let (st, unit) = self.quantity(e)?;
                if unit.as_deref() != Some("st") || st < Rational::ZERO {
                    return Err(Diagnostic::new(
                        "`random_walk` takes the biggest step in semitones",
                        e.span,
                    )
                    .with_help("for example `random_walk(step: 2st)`"));
                }
                let step = st.floor();
                let seed = self.seed(name, rest, 0x5741_4C4B)?;
                p.rewrite_cycles(move |events, cycle| {
                    // from the first note, each note moves by up to `step` semitones either way
                    let mut at = None::<i32>;
                    events
                        .iter()
                        .enumerate()
                        .map(|(i, h)| match (h.value.value, at) {
                            (Value::Note(m), None) => {
                                at = Some(m);
                                h.value
                            }
                            (Value::Note(_), Some(prev)) => {
                                let by =
                                    (chance(seed, cycle, i) * (2 * step + 1) as f64) as i64 - step;
                                at = Some(prev + by as i32);
                                h.value.with(Value::Note(prev + by as i32))
                            }
                            _ => h.value,
                        })
                        .collect()
                })
            }
            "where" => {
                // `where(note < c3, change)`: change only the notes that pass the test
                let e = self.arg(name, rest, "when", span)?;
                let bad = || {
                    Diagnostic::new(
                        "`where` takes a test on the note, such as `note < c3`",
                        e.span,
                    )
                    .with_help("write `note` or `note.freq`, a comparison and a note or value")
                };
                let ExprKind::Compare { op, lhs, rhs } = &e.kind else {
                    return Err(bad());
                };
                let subject = match &lhs.kind {
                    ExprKind::Name(n) => n == "note",
                    ExprKind::Call { name, args } => {
                        name.name == PITCH_PARAM
                            && matches!(args.as_slice(), [a] if matches!(&a.value.kind, ExprKind::Name(n) if n == "note"))
                    }
                    _ => false,
                };
                if !subject {
                    return Err(bad());
                }
                let limit = pitch(&self.constant(rhs, it)?);
                let op = *op;
                let passes = move |s: &Step| {
                    let x = pitch(&s.value);
                    match op {
                        CmpOp::Lt => x < limit,
                        CmpOp::Gt => x > limit,
                        CmpOp::Le => x <= limit,
                        CmpOp::Ge => x >= limit,
                        CmpOp::Eq => x == limit,
                        CmpOp::Ne => x != limit,
                    }
                };
                let change = self.transformer::<Step>(self.arg(name, rest, "change", span)?, &p)?;
                Pattern::stack(vec![
                    p.filter(move |s| !passes(s)),
                    change(p.filter(passes)),
                ])
            }
            "gain" | "pan" | "begin" => {
                let control = match name.name.as_str() {
                    "gain" => Control::Gain,
                    "pan" => Control::Pan,
                    _ => Control::Begin,
                };
                let e = self.arg(name, rest, control.param(), span)?;
                let values = match self.eval(e, it)? {
                    PerfValue::Pattern(p) => p,
                    PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                    _ => {
                        return Err(Diagnostic::new(
                            format!("`{}` takes {}", name.name, control.describe()),
                            e.span,
                        ));
                    }
                };
                // say now, not when the note plays, if the value is of the wrong kind
                let cycle = niminal_pattern::Span::new(Rational::ZERO, Rational::ONE);
                if let Some(bad) = values
                    .query(cycle)
                    .iter()
                    .find(|h| control.read(h.value.value).is_none())
                {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` takes {}, found `{}`",
                            name.name,
                            control.describe(),
                            bad.value.value
                        ),
                        e.span,
                    ));
                }
                p.set_from(&values, move |s, o| match control.read(o.value) {
                    Some(x) => control.set(*s, x),
                    None => *s,
                })
            }
            "stereo_copy" => {
                // the original on the left and the changed copy on the right
                let angle = match self.find(name, rest, "angle") {
                    Some(e) => match self.quantity(e)? {
                        (a, Some(u)) if u == "deg" => a.to_f64(),
                        _ => {
                            return Err(Diagnostic::new(
                                "`angle` is a direction in degrees",
                                e.span,
                            )
                            .with_help("for example `angle: 30deg`"));
                        }
                    },
                    None => 30.0,
                };
                let change = self.transformer::<Step>(self.arg(name, rest, "change", span)?, &p)?;
                let placed = |p: Pattern<Step>, pan: f64| {
                    p.map(move |s| Step {
                        pan: Some(pan),
                        ..s
                    })
                };
                Pattern::stack(vec![placed(p.clone(), -angle), placed(change(p), angle)])
            }
            "chop" => {
                let n = self.whole(self.arg(name, rest, "pieces", span)?, "chop")?;
                // each piece plays the next stretch of the sample
                p.pieces(n, |s, i, n| Step {
                    begin: Some(slice_start(s.begin, i, n)),
                    ..*s
                })
            }
            "interleave_chops" => {
                let n = self.whole(self.arg(name, rest, "pieces", span)?, "interleave_chops")?;
                // the pattern again for each stretch of the sample, in turn
                Pattern::fastcat(
                    (0..n)
                        .map(|i| {
                            p.map(move |s| Step {
                                begin: Some(slice_start(s.begin, i, n)),
                                ..s
                            })
                        })
                        .collect(),
                )
            }
            "invert" => {
                return Err(Diagnostic::new(
                    "`invert` works on grids, as in `grid[x... .x..].invert`",
                    name.span,
                ));
            }
            other => return Err(self.unknown_op(other, name.span)),
        };
        Ok(PerfValue::Pattern(out))
    }

    /// A grid (or pattern) read as on/off.
    pub(super) fn structure(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Pattern<bool>> {
        match self.eval(e, it)? {
            PerfValue::Grid(g) => Ok(g.map(|_| true)),
            PerfValue::Pattern(p) => Ok(p.map(|_| true)),
            other => Err(Diagnostic::new(
                format!("expected a grid, found {}", other.describe()),
                e.span,
            )),
        }
    }
}
