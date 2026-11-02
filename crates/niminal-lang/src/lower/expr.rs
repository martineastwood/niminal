//! Expressions: values, signals, indexing and table reads.

use super::*;

impl Lower<'_> {
    pub(crate) fn param_default(
        &mut self,
        e: &Expr,
        unit: Unit,
        range: Option<(f64, f64)>,
        name: &str,
    ) -> Res<f32> {
        let sig = self.expr(e)?;
        let Some(v) = sig.konst else {
            return Err(Diagnostic::new(
                format!("the default for `{name}` must be a constant"),
                e.span,
            ));
        };
        // A gain parameter may default to a plain linear factor.
        let ok = sig.unit == unit || (unit == Unit::Db && sig.unit == Unit::Num);
        if !ok {
            return Err(self.unit_mismatch(name, unit, sig.unit, e));
        }
        if let Some((lo, hi)) = range
            && !(lo..=hi).contains(&v)
        {
            return Err(Diagnostic::new(
                format!("the default for `{name}` is outside {lo}..{hi}"),
                e.span,
            ));
        }
        Ok(v as f32)
    }

    pub(super) fn literal(&self, value: f64, unit: Option<&str>, span: Span) -> Res<Sig> {
        literal_sig(value, unit, self.tempo, span).map(|(unit, v)| Sig::constant(unit, v))
    }

    /// The signal an expression makes. A list becomes a multichannel signal, one channel per entry.
    pub(super) fn expr(&mut self, e: &Expr) -> Res<Sig> {
        let v = self.value(e)?;
        self.signal_of(v, e.span)
    }

    /// A signal, a list of values, or a range.
    pub(super) fn value(&mut self, e: &Expr) -> Res<Datum> {
        match &e.kind {
            ExprKind::Name(name) if self.names.buffer_controls.contains_key(name) => {
                let choices = self.names.buffer_controls[name].clone();
                let selector = self.name(name, e.span)?;
                Ok(Datum::BufferRef { selector, choices })
            }
            ExprKind::Name(name) => match self.scope.get(name) {
                Some(v) => Ok(v.clone()),
                None => match self.names.consts.get(name) {
                    Some(v) => Ok(v.clone()),
                    None => match self.names.buffers.get(name) {
                        Some(&(id, ref b)) => Ok(Datum::Buffer {
                            id,
                            data: b.clone(),
                        }),
                        None => match self.names.fns.get(name) {
                            Some(def) => Ok(Datum::Function {
                                def: def.clone(),
                                captures: HashMap::new(),
                            }),
                            None => self.name(name, e.span).map(Datum::Sig),
                        },
                    },
                },
            },
            ExprKind::Array(items) => items
                .iter()
                .map(|i| self.value(i))
                .collect::<Res<_>>()
                .map(|items: Vec<_>| Datum::array(items)),
            ExprKind::Range { lo, hi, inclusive } => {
                let (l, h) = (self.expr(lo)?, self.expr(hi)?);
                if l.unit != h.unit {
                    return Err(Diagnostic::new(
                        format!(
                            "the ends of a range need the same unit: {} and {}",
                            l.unit.describe(),
                            h.unit.describe()
                        ),
                        e.span,
                    ));
                }
                Ok(Datum::Range {
                    lo: l,
                    hi: h,
                    inclusive: *inclusive,
                })
            }
            ExprKind::Index { list, index } => self.index(list, index, e.span),
            ExprKind::Neg(inner) => {
                let v = self.value(inner)?;
                self.negate_value(v, e.span)
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let (a, b) = (self.value(lhs)?, self.value(rhs)?);
                self.zip_values(a, b, e.span, |this, a, b| this.compare(*op, a, b, e.span))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                self.binary_value(*op, l, r, e.span)
            }
            ExprKind::Logic { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                let (l, r) = (self.truthy(l, lhs.span)?, self.truthy(r, rhs.span)?);
                Ok(Datum::Sig(match op {
                    LogicOp::And => self.mul(&l, &r, Unit::Num, e.span)?,
                    LogicOp::Or => self.max_sig(&l, &r, e.span)?,
                }))
            }
            ExprKind::Not(inner) => {
                let v = self.expr(inner)?;
                let zero = Sig::constant(v.unit, 0.0);
                self.compare(CmpOp::Eq, v, zero, e.span).map(Datum::Sig)
            }
            ExprKind::If { .. } => self.if_value(e),
            ExprKind::For { var, iter, body } => {
                self.check_loop_name(var)?;
                let d = self.value(iter)?;
                let mut results = Vec::new();
                for entry in self.entries(d, iter.span)? {
                    self.scope.insert(var.name.clone(), entry);
                    let v = self.block(body, true, e.span);
                    self.scope.remove(&var.name);
                    results.push(v?.expect("a value"));
                }
                Ok(Datum::array(results))
            }
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Lambda { params, body } => {
                let def = funcs::lambda(params, body, e.span);
                let mut captures: HashMap<String, Datum> = self
                    .scope
                    .iter()
                    .filter(|(n, _)| funcs::reads(&def, n))
                    .map(|(n, v)| (n.clone(), v.clone()))
                    .collect();
                // an fx's parameters are controls, not locals, so they are captured by reading them here
                let controls: Vec<String> = self
                    .names
                    .controls
                    .iter()
                    .filter(|c| !self.scope.contains_key(&c.name) && funcs::reads(&def, &c.name))
                    .map(|c| c.name.clone())
                    .collect();
                for name in controls {
                    let sig = self.name(&name, e.span)?;
                    captures.insert(name, Datum::Sig(sig));
                }
                Ok(Datum::Function { def, captures })
            }
            _ => self.signal(e).map(Datum::Sig),
        }
    }

    /// 1 where `s` is not 0, 0 where it is.
    pub(super) fn truthy(&mut self, s: Sig, span: Span) -> Res<Sig> {
        let zero = Sig::constant(s.unit, 0.0);
        self.compare(CmpOp::Ne, s, zero, span)
    }

    pub(super) fn max_sig(&mut self, a: &Sig, b: &Sig, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(a.unit, x.max(y)));
        }
        self.elementwise(a, b, a.unit, span, |g, x, y| {
            g.add(Max, &[("a", x), ("b", y)]).expect("max ports")
        })
    }

    pub(super) fn min_sig(&mut self, a: &Sig, b: &Sig, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(a.unit, x.min(y)));
        }
        self.elementwise(a, b, a.unit, span, |g, x, y| {
            g.add(Min, &[("a", x), ("b", y)]).expect("min ports")
        })
    }

    /// An expression that makes a signal directly: not a list, and not arithmetic on one.
    pub(super) fn signal(&mut self, e: &Expr) -> Res<Sig> {
        match &e.kind {
            ExprKind::Num { value, unit } => self.literal(*value, unit.as_deref(), e.span),
            ExprKind::Call { .. } => unreachable!("`value` handles calls"),
            ExprKind::Env(env) => self.env(env),
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can only be an argument that asks for it",
                e.span,
            )),
            ExprKind::Pattern(_)
            | ExprKind::Grid(_)
            | ExprKind::Rows(_)
            | ExprKind::Steps(_)
            | ExprKind::FxLanes(_)
            | ExprKind::Events(_)
            | ExprKind::Rest
            | ExprKind::ClipBlock(_)
            | ExprKind::SceneBlock(_) => Err(Diagnostic::new(
                "composition syntax can't be used in a sound definition",
                e.span,
            )),
            ExprKind::Compare { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                self.compare(*op, l, r, e.span)
            }
            ExprKind::Name(_)
            | ExprKind::Neg(_)
            | ExprKind::Binary { .. }
            | ExprKind::Logic { .. }
            | ExprKind::Not(_)
            | ExprKind::If { .. }
            | ExprKind::For { .. }
            | ExprKind::Array(_)
            | ExprKind::Range { .. }
            | ExprKind::Index { .. }
            | ExprKind::Lambda { .. } => unreachable!("`value` handles these"),
        }
    }

    /// A list as a signal, one channel per entry. The entries share a unit, so `[800hz, 1200hz]` is a
    /// two-channel frequency.
    pub(super) fn signal_of(&self, v: Datum, span: Span) -> Res<Sig> {
        match v {
            Datum::Function { .. } => Err(Diagnostic::new("a function is not a signal", span)),
            Datum::Sig(sig) => Ok(sig),
            Datum::Record(fields) => self.signal_of(fields[0].1.clone(), span),
            Datum::Table(_) => Err(Diagnostic::new(
                "a table isn't a signal; play it with `osc(table, freq)` or read it with `.at`",
                span,
            )),
            Datum::Buffer { .. } | Datum::BufferRef { .. } => Err(Diagnostic::new(
                "a buffer isn't a signal; read it with `buf[i]` or `buf.at(pos)`",
                span,
            )),
            Datum::Array(items) => {
                let mut entries = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Datum::Function { .. } => return Err(Diagnostic::new("a function is not a signal", span)),
                        Datum::Record(fields) => entries.push((self.signal_of(fields[0].1.clone(), span)?, span)),
                        Datum::Sig(sig) => entries.push((sig, span)),
                        Datum::Table(_) | Datum::Buffer { .. } | Datum::BufferRef { .. } => {
                            return Err(Diagnostic::new(
                                "an array of tables can't be used as a signal",
                                span,
                            ));
                        }
                        Datum::Array(_) => {
                            return Err(Diagnostic::new("an array of arrays can't be used as a signal", span)
                                .with_help("sum the inner arrays, or index into them"));
                        }
                        Datum::Range { .. } => {
                            return Err(Diagnostic::new("an array of ranges can't be used as a signal", span));
                        }
                    }
                }
                join_channels(&entries, span)
            }
            Datum::Range { .. } => Err(Diagnostic::new("a range isn't a signal", span).with_help(
                "use it in `.range(...)`, `.clamp(...)`, a `for` loop or a slice such as `xs[0..<4]`",
            )),
        }
    }

    /// `list[i]`, counting from 0 and wrapping at both ends, or `list[a..<b]`.
    pub(super) fn index(&mut self, list: &Expr, index: &Expr, span: Span) -> Res<Datum> {
        let l = self.value(list)?;
        if let Datum::BufferRef { selector, choices } = &l {
            let pos = self.expr(index)?;
            return self
                .read_buffer_ref(selector, choices, &pos, Interp::None, span)
                .map(Datum::Sig);
        }
        if let Datum::Buffer { id, data: buffer } = &l {
            if let ExprKind::Range { .. } = index.kind {
                return Err(Diagnostic::new(
                    "slicing a shared buffer isn't supported; read with `buf[i]` or `buf.at(pos)`",
                    span,
                ));
            }
            let i = self.expr(index)?;
            if i.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!("an index is a plain number, found {}", i.unit.describe()),
                    index.span,
                ));
            }
            self.g.touch_buffer_read(*id);
            let rate = self.sig_rate(&i);
            let srcs: Array<Src> = i
                .srcs
                .iter()
                .map(|&p| {
                    let src = self
                        .g
                        .add(BufferRead::new(buffer.clone(), Interp::None), &[("pos", p)])
                        .expect("buffer ports");
                    self.note_rate(src, rate);
                    src
                })
                .collect();
            return Ok(Datum::Sig(Sig {
                srcs,
                layout: i.layout,
                unit: Unit::Num,
                konst: None,
            }));
        }
        let items = self.entries(l, list.span)?;
        let n = items.len();
        if let ExprKind::Range { .. } = index.kind {
            let picks = self.value(index)?;
            let picks = self.entries(picks, index.span)?;
            if n == 0 && !picks.is_empty() {
                return Err(Diagnostic::new("this array is empty", span));
            }
            let mut out = Vec::with_capacity(picks.len());
            for p in picks {
                let Datum::Sig(Sig { konst: Some(k), .. }) = p else {
                    unreachable!("a range gives numbers")
                };
                out.push(items[niminal_engine::wrapped_index(k, n)].clone());
            }
            return Ok(Datum::array(out));
        }
        if n == 0 {
            return Err(Diagnostic::new("this array is empty", span));
        }
        let i = self.expr(index)?;
        if i.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("an index is a plain number, found {}", i.unit.describe()),
                index.span,
            ));
        }
        if let Some(k) = i.konst {
            return Ok(items[niminal_engine::wrapped_index(k, n)].clone());
        }
        self.index_values(items, &i, span)
    }

    /// Transpose nested values into scalar reads. Each read shares the same
    /// wrapping rules and takes constant time, regardless of array length.
    pub(super) fn index_values(&mut self, items: Vec<Datum>, index: &Sig, span: Span) -> Res<Datum> {
        let Some(first) = items.first() else {
            return Err(Diagnostic::new("this array is empty", span));
        };
        match first {
            Datum::Sig(first) => {
                let width = first.count();
                let layout = first.layout;
                let unit = first.unit;
                let signals = items
                    .into_iter()
                    .map(|v| match v {
                        Datum::Sig(s) if s.count() == width && s.unit == unit => Ok(s),
                        _ => Err(Diagnostic::new(
                            "an index that changes needs matching element shapes and units",
                            span,
                        )),
                    })
                    .collect::<Res<Vec<_>>>()?;
                if width == 1 {
                    return self
                        .read_values(signals, index, Interp::None, span)
                        .map(Datum::Sig);
                }
                if index.count() != 1 {
                    return Err(Diagnostic::new(
                        "indexing channel arrays needs one index",
                        span,
                    ));
                }
                let srcs = (0..width)
                    .map(|channel| {
                        let elements = signals.iter().map(|s| Self::element(s, channel)).collect();
                        self.read_values(elements, index, Interp::None, span)
                            .map(|s| s.srcs[0])
                    })
                    .collect::<Res<_>>()?;
                Ok(Datum::Sig(Sig {
                    srcs,
                    layout,
                    unit,
                    konst: None,
                }))
            }
            Datum::Array(first) => {
                let width = first.len();
                let rows = items
                    .into_iter()
                    .map(|v| match v {
                        Datum::Array(row) if row.len() == width => Ok(row),
                        _ => Err(Diagnostic::new(
                            "an index that changes needs matching element shapes",
                            span,
                        )),
                    })
                    .collect::<Res<Vec<_>>>()?;
                Ok(Datum::Array(
                    (0..width)
                        .map(|column| {
                            let elements = rows.iter().map(|row| row[column].clone()).collect();
                            self.index_values(elements, index, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "an index that changes needs numeric elements or nested arrays",
                span,
            )),
        }
    }

    pub(super) fn read_values(&mut self, sigs: Vec<Sig>, pos: &Sig, interp: Interp, span: Span) -> Res<Sig> {
        let Some(first) = sigs.first() else {
            return Err(Diagnostic::new("this array is empty", span));
        };
        let unit = first.unit;
        if sigs.iter().any(|s| s.count() != 1 || s.unit != unit) {
            return Err(Diagnostic::new(
                "this operation needs a one-dimensional array with one unit",
                span,
            ));
        }
        if sigs.iter().all(|s| s.konst.is_some()) {
            let data = Arc::new(Table::from_f64(sigs.iter().map(|s| s.konst.unwrap()), None));
            return self.table_read(data, unit, pos, interp, Edge::Wrap, span);
        }
        let op = ArrayRead::new(sigs.len(), interp);
        let names: Vec<_> = (0..sigs.len()).map(|i| format!("e{i}")).collect();
        let rate = sigs
            .iter()
            .fold(self.sig_rate(pos), |rate, sig| rate.max(self.sig_rate(sig)));
        let srcs = pos
            .srcs
            .iter()
            .map(|&pos| {
                let mut wires = vec![("pos", pos)];
                wires.extend(
                    names
                        .iter()
                        .zip(&sigs)
                        .map(|(name, sig)| (name.as_str(), sig.srcs[0])),
                );
                let src = self
                    .g
                    .add(op.clone(), &wires)
                    .expect("one port per array element");
                self.note_rate(src, rate);
                src
            })
            .collect();
        Ok(Sig {
            srcs,
            unit,
            layout: pos.layout,
            konst: None,
        })
    }

    /// A table read at `pos`, wrapping, one read per channel of `pos`.
    pub(super) fn table_read(
        &mut self,
        data: Arc<Table>,
        unit: Unit,
        pos: &Sig,
        interp: Interp,
        edge: Edge,
        _span: Span,
    ) -> Res<Sig> {
        if let Some(position) = pos.konst {
            return Ok(Sig::constant(
                unit,
                data.read_at_edge(position, interp, edge),
            ));
        }
        let rate = self.sig_rate(pos);
        let srcs: Array<Src> = pos
            .srcs
            .iter()
            .map(|&p| {
                let src = self
                    .g
                    .add(
                        TableRead::new(data.clone(), interp).with_edge(edge),
                        &[("pos", p)],
                    )
                    .expect("table ports");
                self.note_rate(src, rate);
                src
            })
            .collect();
        Ok(Sig {
            srcs,
            layout: pos.layout,
            unit,
            konst: None,
        })
    }

    /// `f(x)` on every channel, worked out while compiling where `x` is known.
    pub(super) fn func(&mut self, f: Fn1, x: &Sig) -> Sig {
        if let Some(k) = x.konst {
            return Sig::constant(Unit::Num, f.apply(k));
        }
        let rate = self.sig_rate(x);
        let srcs: Array<Src> = x
            .srcs
            .iter()
            .map(|&s| {
                let out = self.g.add(Func(f), &[("x", s)]).expect("func ports");
                self.note_rate(out, rate);
                out
            })
            .collect();
        Sig {
            srcs,
            layout: x.layout,
            unit: Unit::Num,
            konst: None,
        }
    }

    /// How often a source can change.
    pub(super) fn src_rate(&self, src: Src) -> Rate {
        match src {
            Src::Const(_) => Rate::Const,
            Src::Param(i) => self.param_rates.get(i).copied().unwrap_or(Rate::Audio),
            Src::Node(i) => self.rates.get(&i).copied().unwrap_or(Rate::Audio),
            Src::Input(_) => Rate::Audio,
        }
    }

    pub(super) fn sig_rate(&self, s: &Sig) -> Rate {
        if s.konst.is_some() {
            return Rate::Const;
        }
        s.srcs
            .iter()
            .map(|&x| self.src_rate(x))
            .max()
            .unwrap_or(Rate::Const)
    }

    pub(super) fn note_rate(&mut self, src: Src, rate: Rate) {
        if let Src::Node(i) = src
            && rate < Rate::Audio
        {
            self.rates.insert(i, rate);
        }
    }

    /// A signal seen at a slower rate: `init` holds its value from note start, `control` reads it
    /// once per block.
    pub(super) fn at_rate(&mut self, s: Sig, rate: Option<Rate>) -> Sig {
        let Some(rate) = rate else { return s };
        if self.sig_rate(&s) <= rate || rate == Rate::Const {
            return s;
        }
        let srcs: Array<Src> = s
            .srcs
            .iter()
            .map(|&x| {
                let out = match rate {
                    Rate::Init => self.g.add(Hold::default(), &[("x", x)]),
                    _ => self.g.add(BlockHold, &[("x", x)]),
                }
                .expect("hold ports");
                self.note_rate(out, rate);
                out
            })
            .collect();
        Sig {
            srcs,
            konst: None,
            ..s
        }
    }

    /// Channel `k` of a signal, with its value known if it was.
    pub(super) fn element(sig: &Sig, k: usize) -> Sig {
        match sig.srcs[k] {
            Src::Const(v) => Sig::constant(sig.unit, f64::from(v)),
            src => Sig::signal(src, sig.unit),
        }
    }

}
