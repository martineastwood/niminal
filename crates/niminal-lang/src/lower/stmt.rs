//! Statements: blocks, `if`/`for`, assignment, feedback, destructuring, layout conversion.

use super::*;

impl Lower<'_> {
    /// Run a body's statements, returning its trailing value as a signal, if there is one.
    pub(super) fn run_body(&mut self, body: &[&Stmt]) -> Res<Option<(Sig, Span)>> {
        match self.run_body_value(body)? {
            Some((d, span)) => Ok(Some((self.signal_of(d, span)?, span))),
            None => Ok(None),
        }
    }

    /// Run a body's statements, returning its trailing value, if there is one.
    pub(super) fn run_body_value(&mut self, body: &[&Stmt]) -> Res<Option<(Datum, Span)>> {
        let n = body.len();
        let mut tail = None;
        for (i, stmt) in body.iter().enumerate() {
            if let Some(v) = self.stmt(stmt, i + 1 == n)? {
                tail = Some(v);
            }
        }
        Ok(tail)
    }

    /// Run one statement. The value of a trailing expression, if `last`.
    pub(super) fn stmt(&mut self, stmt: &Stmt, last: bool) -> Res<Option<(Datum, Span)>> {
        match stmt {
            Stmt::State { name, .. } => Err(state_outside_fn(name)),
            Stmt::IndexAssign { name, index, value } => {
                if let Some((id, buffer)) = self.names.buffers.get(&name.name).cloned() {
                    if self.in_fn {
                        return Err(Diagnostic::new(
                            "a shared buffer can't be written inside a function",
                            name.span,
                        )
                        .with_help("write it from an instrument or node"));
                    }
                    let idx = self.expr(index)?;
                    let val = self.expr(value)?;
                    if idx.unit != Unit::Num || val.unit != Unit::Num {
                        return Err(Diagnostic::new(
                            "a buffer write needs plain numbers for the index and value",
                            name.span,
                        ));
                    }
                    self.g.touch_buffer_write(id);
                    for (i, v) in idx.srcs.iter().zip(&val.srcs) {
                        let mut ports = vec![("idx", *i), ("value", *v)];
                        if let Some(gate) = self.write_gate {
                            ports.push(("gate", gate));
                        }
                        let _ = self
                            .g
                            .add(BufferWrite::new(buffer.clone()), &ports)
                            .map_err(|e| Diagnostic::new(e.to_string(), name.span))?;
                    }
                    return Ok(None);
                }
                Err(Diagnostic::new(
                    format!(
                        "only a state array or shared buffer can be written, and `{}` isn't one",
                        name.name
                    ),
                    name.span,
                )
                .with_help(
                    "state arrays belong to a `fn`: `state buf = zeros(64)`, then `buf[i] = x`",
                ))
            }
            Stmt::Bind { name, value } => self.assign(name, value).map(|()| None),
            Stmt::Destructure { names, value } => self.destructure(names, value).map(|()| None),
            Stmt::AddAssign { name, value } => self.add_assign(name, value).map(|()| None),
            Stmt::Expr(e) => match &e.kind {
                ExprKind::If {
                    then,
                    otherwise: None,
                    ..
                } if last && ends_with_value(then) => Err(Diagnostic::new(
                    "an `if` used as a value needs an `else`",
                    e.span,
                )
                .with_help("it makes a value, so both ways need one: `if x > 0 { a } else { b }`")),
                ExprKind::If {
                    then, otherwise, ..
                } if !(last && otherwise.is_some() && ends_with_value(then)) => {
                    self.if_stmt(e).map(|()| None)
                }
                ExprKind::For { body, .. } if !(last && ends_with_value(body)) => {
                    self.for_stmt(e).map(|()| None)
                }
                _ if self.mode == Mode::Node && !self.in_fn => {
                    Err(Diagnostic::new("a node makes sound through `out`", e.span)
                        .with_help("write `out = ...`"))
                }
                _ if last => Ok(Some((self.value(e)?, e.span))),
                _ => Err(Diagnostic::new("this value is never used", e.span)
                    .with_help("bind it with `name = ...`, or make it the last line")),
            },
        }
    }

    /// Run a block's statements. Names first bound inside it stay inside it. The trailing value,
    /// if `value` is asked for.
    pub(super) fn block(&mut self, stmts: &[Stmt], value: bool, span: Span) -> Res<Option<Datum>> {
        let outer: Vec<String> = self.scope.keys().cloned().collect();
        let mut tail = None;
        let n = stmts.len();
        for (i, s) in stmts.iter().enumerate() {
            if let Some((v, _)) = self.stmt(s, value && i + 1 == n)? {
                tail = Some(v);
            }
        }
        self.scope.retain(|k, _| outer.contains(k));
        if value && tail.is_none() {
            return Err(Diagnostic::new("this block must end with a value", span));
        }
        Ok(tail)
    }

    /// The condition of an `if`, as a plain signal.
    pub(super) fn condition(&mut self, cond: &Expr) -> Res<Sig> {
        let c = self.expr(cond)?;
        if c.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "an `if` needs a plain number to decide on, but this is {}",
                    c.unit.describe()
                ),
                cond.span,
            )
            .with_help("a comparison gives 1 or 0: `if freq < 2khz`"));
        }
        Ok(c)
    }

    /// `if cond { ... } else { ... }` as a statement: each name a branch changes takes the value of the
    /// branch that is taken.
    pub(super) fn if_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::If {
            cond,
            then,
            otherwise,
        } = &e.kind
        else {
            unreachable!()
        };
        let c = self.condition(cond)?;
        if let Some(k) = c.konst {
            let taken = if k != 0.0 {
                Some(then)
            } else {
                otherwise.as_ref()
            };
            if let Some(stmts) = taken {
                self.block(stmts, false, e.span)?;
            }
            return Ok(());
        }
        let sends = self.sends;
        let saved_scope = self.scope.clone();
        let saved_out = self.out.clone();
        // a write to a shared buffer happens only in the branch that is taken
        let outer_gate = self.write_gate;
        let taken = self.truthy(c.clone(), e.span)?;
        let (then_gate, else_gate) = self.branch_gates(outer_gate, &taken, e.span)?;
        self.write_gate = Some(then_gate);
        let then_run = self.block(then, false, e.span);
        self.write_gate = outer_gate;
        then_run?;
        let then_scope = std::mem::replace(&mut self.scope, saved_scope.clone());
        let then_out = std::mem::replace(&mut self.out, saved_out.clone());
        if let Some(stmts) = otherwise {
            self.write_gate = Some(else_gate);
            let else_run = self.block(stmts, false, e.span);
            self.write_gate = outer_gate;
            else_run?;
        }
        let else_scope = std::mem::replace(&mut self.scope, saved_scope);
        let else_out = std::mem::replace(&mut self.out, saved_out);
        if self.sends != sends {
            return Err(Diagnostic::new(
                "a send inside an `if` that changes while playing would always be made",
                e.span,
            )
            .with_help("send the `if` instead: `space += if c { a } else { b }`"));
        }
        let names: Vec<String> = self.scope.keys().cloned().collect();
        for name in names {
            let (a, b) = (then_scope[&name].clone(), else_scope[&name].clone());
            if same_datum(&a, &b) {
                continue;
            }
            let merged = self.select_value(&c, a, b, e.span)?;
            self.scope.insert(name, merged);
        }
        self.out = match (then_out, else_out) {
            (Some(a), Some(b)) => Some(self.select_sig(&c, a, b, e.span)?),
            (Some(a), None) => {
                let zero = Sig::constant(a.unit, 0.0);
                Some(self.select_sig(&c, a, zero, e.span)?)
            }
            (None, Some(b)) => {
                let zero = Sig::constant(b.unit, 0.0);
                Some(self.select_sig(&c, zero, b, e.span)?)
            }
            (None, None) => None,
        };
        Ok(())
    }

    /// Where each branch of an `if` is taken, inside whatever `outer` already says.
    pub(super) fn branch_gates(&mut self, outer: Option<Src>, taken: &Sig, span: Span) -> Res<(Src, Src)> {
        let zero = Sig::constant(Unit::Num, 0.0);
        let not_taken = self.compare(CmpOp::Eq, taken.clone(), zero, span)?;
        let (mut then_gate, mut else_gate) = (
            Sig::signal(taken.pick(0), Unit::Num),
            Sig::signal(not_taken.pick(0), Unit::Num),
        );
        if let Some(outer) = outer {
            let outer = Sig::signal(outer, Unit::Num);
            then_gate = self.mul(&outer, &then_gate, Unit::Num, span)?;
            else_gate = self.mul(&outer, &else_gate, Unit::Num, span)?;
        }
        Ok((then_gate.pick(0), else_gate.pick(0)))
    }

    /// The entries of a list or range, for `for`, `map` and the like.
    pub(super) fn entries(&mut self, d: Datum, span: Span) -> Res<Vec<Datum>> {
        match d {
            Datum::BufferRef { .. } => Err(Diagnostic::new(
                "a buffer reference is read with indexing or .at",
                span,
            )),
            Datum::Function { .. } => Err(Diagnostic::new("a function is not an array", span)),
            Datum::Record(fields) => self.entries(fields[0].1.clone(), span),
            Datum::Array(items) => Ok(items.into_vec()),
            Datum::Table(table) => Ok(table_entries(&table)),
            Datum::Buffer { data: buf, .. } => Ok((0..buf.len())
                .map(|i| Datum::Sig(Sig::constant(Unit::Num, f64::from(buf.read(i)))))
                .collect()),
            Datum::Range { lo, hi, inclusive } => {
                let (Some(a), Some(b)) = (lo.konst, hi.konst) else {
                    return Err(Diagnostic::new(
                        "the ends of a range must be known while compiling",
                        span,
                    ));
                };
                Ok(range_numbers(a, b, inclusive, span)?
                    .into_iter()
                    .map(|k| Datum::Sig(Sig::constant(lo.unit, k)))
                    .collect())
            }
            Datum::Sig(sig) => Ok((0..sig.count())
                .map(|k| Datum::Sig(Self::element(&sig, k)))
                .collect()),
        }
    }

    /// A compile-time numeric table, or a constant numeric array coerced into one.
    pub(super) fn as_table(&mut self, value: Datum, span: Span) -> Res<Arc<Table>> {
        match value {
            Datum::Table(t) => Ok(t),
            Datum::Buffer { data: b, .. } => Ok(Arc::new(b.snapshot())),
            Datum::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items.iter() {
                    match item {
                        Datum::Sig(Sig {
                            konst: Some(k),
                            unit: Unit::Num,
                            ..
                        }) => values.push(*k),
                        Datum::Sig(Sig { konst: None, .. }) => {
                            return Err(Diagnostic::new(
                                "the table must be known while compiling",
                                span,
                            ));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                "a table is a one-dimensional array of plain numbers",
                                span,
                            ));
                        }
                    }
                }
                if values.is_empty() {
                    return Err(Diagnostic::new("a table can't be empty", span));
                }
                Ok(Arc::new(Table::from_f64(values, None)))
            }
            _ => Err(Diagnostic::new(
                "expected a table of plain numbers known while compiling",
                span,
            )),
        }
    }

    pub(super) fn for_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::For { var, iter, body } = &e.kind else {
            unreachable!()
        };
        self.check_loop_name(var)?;
        let d = self.value(iter)?;
        for entry in self.entries(d, iter.span)? {
            self.scope.insert(var.name.clone(), entry);
            let r = self.block(body, false, e.span);
            self.scope.remove(&var.name);
            r?;
        }
        Ok(())
    }

    pub(super) fn check_loop_name(&self, var: &Ident) -> Res<()> {
        self.names.check_local_in(var, self.in_fn)?;
        if self.scope.contains_key(&var.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already in use", var.name),
                var.span,
            ));
        }
        Ok(())
    }

    pub(super) fn assign(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        match name.name.as_str() {
            "out" => {
                let sig = self.out_value(value)?;
                self.set_out(sig, value.span)
            }
            "in" => Err(Diagnostic::new(
                "`in` is an input and can't be assigned",
                name.span,
            )),
            "instrument" => Err(Diagnostic::new(
                "`instrument` is only available in a node",
                name.span,
            )),
            _ => {
                self.names.check_local_in(name, self.in_fn)?;
                let v = if reads_prev_of(value, &name.name) && self.in_send.is_none() {
                    self.feedback(name, value)?
                } else {
                    self.value(value)?
                };
                if !self.in_fn
                    && let Datum::Sig(sig) = &v
                {
                    let rate = self.sig_rate(sig);
                    if rate > Rate::Const {
                        self.rate_hints.push(crate::program::RateHint {
                            span: name.span,
                            rate: rate.keyword(),
                        });
                    }
                }
                self.scope.insert(name.name.clone(), v);
                Ok(())
            }
        }
    }

    /// `y = x + prev(y) * 0.5`: a loop one sample long, compiled as a function with state whose
    /// inputs are the names the line reads.
    pub(super) fn feedback(&mut self, name: &Ident, value: &Expr) -> Res<Datum> {
        let mut read = Vec::new();
        names_read(value, &mut read);
        let inputs: Vec<String> = read
            .into_iter()
            .filter(|n| *n != name.name && self.scope.contains_key(n))
            .collect();
        let span = value.span;
        let def = FnDef {
            name: name.clone(),
            params: inputs
                .iter()
                .map(|n| ParamDef {
                    name: Ident {
                        name: n.clone(),
                        span,
                    },
                    rate: None,
                    ty: None,
                    default: None,
                })
                .collect(),
            result: FnResult::Inferred,
            body: vec![
                Stmt::Bind {
                    name: name.clone(),
                    value: value.clone(),
                },
                Stmt::Expr(Expr {
                    kind: ExprKind::Name(name.name.clone()),
                    span,
                }),
            ],
            span,
        };
        let params = funcs::signature(&def.params, self.tempo)?;
        let given = inputs
            .iter()
            .map(|n| {
                Some((
                    self.scope[n].clone(),
                    Expr {
                        kind: ExprKind::Name(n.clone()),
                        span,
                    },
                ))
            })
            .collect();
        self.call_kernel(&def, name, &params, given, span)
    }

    /// `[l, r] = src`: one name per channel, or per entry of a list.
    pub(super) fn destructure(&mut self, names: &[Ident], value: &Expr) -> Res<()> {
        let entries = match self.value(value)? {
            Datum::Function { .. } => {
                return Err(Diagnostic::new(
                    "a function cannot be destructured",
                    value.span,
                ));
            }
            Datum::Record(fields) => fields.into_iter().map(|(_, v)| v).collect(),
            Datum::Range { .. } => {
                return Err(Diagnostic::new(
                    "a range can't be taken apart with `[a, b] = ...`",
                    value.span,
                ));
            }
            Datum::Table(table) => {
                let items = table_entries(&table);
                if items.len() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this array has {} entries, but {} name(s) are given",
                            items.len(),
                            names.len()
                        ),
                        value.span,
                    ));
                }
                items
            }
            Datum::Buffer { .. } | Datum::BufferRef { .. } => {
                return Err(Diagnostic::new(
                    "a shared buffer can't be taken apart with `[a, b] = ...`",
                    value.span,
                )
                .with_help("read it with `buf[i]` or `buf.at(pos)`"));
            }
            Datum::Array(items) => {
                if items.len() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this array has {} entries, but {} name(s) are given",
                            items.len(),
                            names.len()
                        ),
                        value.span,
                    ));
                }
                items.into_vec()
            }
            Datum::Sig(sig) => {
                self.require_signal("a channel", &sig, value.span)?;
                if sig.count() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this signal has {} channel(s), but {} name(s) are given",
                            sig.count(),
                            names.len()
                        ),
                        value.span,
                    ));
                }
                (0..sig.count())
                    .map(|k| Datum::Sig(Self::element(&sig, k)))
                    .collect()
            }
        };
        if entries.len() != names.len() {
            return Err(Diagnostic::new(
                format!(
                    "this array has {} entries, but {} name(s) are given",
                    entries.len(),
                    names.len()
                ),
                value.span,
            ));
        }
        for (name, entry) in names.iter().zip(entries) {
            self.names.check_local_in(name, self.in_fn)?;
            self.scope.insert(name.name.clone(), entry);
        }
        Ok(())
    }

    pub(super) fn add_assign(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        if name.name == "out" {
            let sig = self.expr(value)?;
            self.require_signal("out", &sig, value.span)?;
            let sum = match self.out.take() {
                None => sig,
                Some(prev) => self.binary(BinOp::Add, prev, sig, value.span)?,
            };
            self.out = Some(sum);
            return Ok(());
        }
        if let Some(bus) = self.names.bus(&name.name) {
            if self.in_fn {
                return Err(Diagnostic::new(
                    format!("a function can't send to the bus `{}`", name.name),
                    name.span,
                )
                .with_help("send from the instrument that calls it"));
            }
            self.in_send = Some(false);
            let sig = self.expr(value);
            let delayed = self.in_send.take() == Some(true);
            let sig = sig?;
            self.require_signal(&name.name, &sig, value.span)?;
            let sig = self.convert(&sig, self.names.bus_layouts[bus], value.span)?;
            for (channel, src) in sig.srcs.iter().enumerate() {
                if delayed {
                    self.g.send_channel_delayed(bus, channel, *src);
                } else {
                    self.g.send_channel(bus, channel, *src);
                }
            }
            self.sends += 1;
            return Ok(());
        }
        let mut d = Diagnostic::new(
            format!("`+=` can only add to `out` or a bus, not `{}`", name.name),
            name.span,
        );
        if self.scope.contains_key(&name.name) {
            d = d.with_help(format!(
                "to change a local, write `{0} = {0} + ...`",
                name.name
            ));
        } else if let Some(c) = closest(&name.name, self.names.buses.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean the bus `{c}`?"));
        }
        Err(d)
    }

    pub(super) fn require_signal(&self, what: &str, sig: &Sig, span: Span) -> Res<()> {
        if sig.unit == Unit::Num {
            Ok(())
        } else {
            Err(Diagnostic::new(
                format!(
                    "`{what}` carries a plain signal, but this is {}",
                    sig.unit.describe()
                ),
                span,
            ))
        }
    }

    pub(super) fn set_out(&mut self, sig: Sig, span: Span) -> Res<()> {
        self.require_signal("out", &sig, span)?;
        self.out = Some(sig);
        Ok(())
    }

    /// The value assigned to `out`. In a node, `x.to(bus)` routes the node's
    /// output to a bus instead of the master.
    pub(super) fn out_value(&mut self, value: &Expr) -> Res<Sig> {
        let ExprKind::Call { name, args } = &value.kind else {
            return self.expr(value);
        };
        if name.name != "to" || self.mode != Mode::Node {
            return self.expr(value);
        }
        let [source, target] = args.as_slice() else {
            return Err(Diagnostic::new("`to` takes a bus", name.span)
                .with_help("for example `out = in.to(space)`"));
        };
        let bus = match (&target.name, &target.value.kind) {
            (None, ExprKind::Name(b)) => self.names.bus(b).ok_or_else(|| {
                unknown_name(
                    b,
                    target.value.span,
                    self.names.buses.iter().map(String::as_str),
                )
            })?,
            _ => {
                return Err(Diagnostic::new(
                    "`to` takes the name of a bus",
                    target.value.span,
                ));
            }
        };
        self.route = Route::Bus(bus);
        self.expr(&source.value)
    }

    /// A bundle of a node chain's external inputs, made on first use. Each
    /// channel is its own mono input.
    pub(super) fn input_bundle(
        &mut self,
        key: &str,
        layout: Layout,
        source: impl Fn(usize) -> ChainInput,
    ) -> Sig {
        if let Some(sig) = self.input_sigs.get(key) {
            return sig.clone();
        }
        let srcs: Array<Src> = (0..layout.channels())
            .map(|c| {
                self.chain_inputs.push(source(c));
                self.g.input()
            })
            .collect();
        let sig = Sig::channels(srcs, layout);
        self.input_sigs.insert(key.to_string(), sig.clone());
        sig
    }

    /// `in`: what the node's voices produced, in the master's layout.
    pub(super) fn node_input(&mut self) -> Sig {
        self.input_bundle("in", self.names.master, |channel| ChainInput::It {
            channel,
        })
    }

    /// Turn a signal into another layout using the automatic rules, as gain and
    /// sum nodes. An unnamed `channels(n)` signal may be taken as any layout
    /// with `n` channels.
    pub(super) fn convert(&mut self, sig: &Sig, to: Layout, span: Span) -> Res<Sig> {
        if sig.layout == to {
            return Ok(sig.clone());
        }
        if matches!(sig.layout, Layout::Channels(_)) && sig.count() == to.channels() {
            return Ok(Sig {
                layout: to,
                ..sig.clone()
            });
        }
        let Some(matrix) = conversion(sig.layout, to) else {
            return Err(Diagnostic::new(
                format!("can't convert {} to {} automatically", sig.layout, to),
                span,
            )
            .with_help(
                "pan the signal, or take its channels apart with `[a, b] = ...` and rebuild them",
            ));
        };

        let mut srcs = Vec::with_capacity(to.channels());
        for row in &matrix {
            let mut sum: Option<Src> = None;
            for (input, &coeff) in row.iter().enumerate() {
                if coeff == 0.0 {
                    continue;
                }
                let term = if coeff == 1.0 {
                    sig.srcs[input]
                } else {
                    self.g
                        .add(
                            Mul,
                            &[("a", sig.srcs[input]), ("b", Src::Const(coeff as f32))],
                        )
                        .expect("mul ports")
                };
                sum = Some(match sum {
                    None => term,
                    Some(prev) => self
                        .g
                        .add(Add, &[("a", prev), ("b", term)])
                        .expect("add ports"),
                });
            }
            srcs.push(sum.unwrap_or(Src::Const(0.0)));
        }
        Ok(Sig::channels(srcs, to))
    }

}
