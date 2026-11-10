//! Statements in a kernel: bindings, state, `if` and `for`.

use super::*;

impl Kb<'_> {
    // ---- statements ----------------------------------------------------

    /// Run a statement. The value of a trailing expression, if `last`.
    pub(super) fn stmt(&mut self, stmt: &Stmt, last: bool) -> Res<Option<KVal>> {
        match stmt {
            Stmt::Bind { name, value } => {
                self.bind(name, value)?;
                Ok(None)
            }
            Stmt::State { name, init } => {
                self.declare_state(name, init)?;
                Ok(None)
            }
            Stmt::AddAssign { name, value } => {
                if name.name != "out" {
                    return Err(Diagnostic::new(
                        format!("`+=` in a function only adds to `out`, not `{}`", name.name),
                        name.span,
                    )
                    .with_help(if self.names.bus(&name.name).is_some() {
                        "functions can't send to buses; send from the instrument that calls it"
                            .to_string()
                    } else {
                        format!("write `{0} = {0} + ...`", name.name)
                    }));
                }
                let v = self.num(value)?;
                let sum = match self.frame.out {
                    None => v,
                    Some(prev) => self.add(prev, v, value.span)?,
                };
                self.frame.out = Some(sum);
                Ok(None)
            }
            Stmt::IndexAssign { name, index, value } => {
                if !self.frame.buffers.contains(&name.name) {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` is not a state array, so it can't be written",
                            name.name
                        ),
                        name.span,
                    ));
                }
                let Some(KVal::State { id, unit, len }) = self.frame.scope.get(&name.name).cloned()
                else {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` is not a state array, so it can't be written",
                            name.name
                        ),
                        name.span,
                    )
                    .with_help(format!("declare it with `state {} = zeros(64)`", name.name)));
                };
                if len == 0 {
                    return Err(Diagnostic::new("this array is empty", name.span));
                }
                let i = self.num(index)?;
                if i.unit != Unit::Num {
                    return Err(Diagnostic::new("an index is a plain number", index.span));
                }
                let v = self.num(value)?;
                if v.unit != unit {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` holds {}, but this is {}",
                            name.name,
                            unit.describe(),
                            v.unit.describe()
                        ),
                        value.span,
                    ));
                }
                let cond = self.guard;
                self.code.push(Instr::Write {
                    list: id,
                    idx: i.reg,
                    src: v.reg,
                    cond,
                });
                Ok(None)
            }
            Stmt::Destructure { names, value } => {
                let array = self.value(value)?;
                let entries = self.array_elements(array, value.span)?;
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
                for (n, v) in names.iter().zip(entries) {
                    self.check_name(n)?;
                    self.frame.scope.insert(n.name.clone(), v);
                }
                Ok(None)
            }
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
                    otherwise, then, ..
                } if !(last && otherwise.is_some() && ends_with_value(then)) => {
                    self.if_stmt(e)?;
                    Ok(None)
                }
                ExprKind::For { body, .. } if !(last && ends_with_value(body)) => {
                    self.for_stmt(e)?;
                    Ok(None)
                }
                _ if last => Ok(Some(self.value(e)?)),
                _ => Err(Diagnostic::new("this value is never used", e.span)
                    .with_help("bind it with `name = ...`, or make it the last line")),
            },
        }
    }

    pub(super) fn check_name(&self, name: &Ident) -> Res<()> {
        if matches!(
            name.name.as_str(),
            "in" | "it"
                | "note"
                | "clock"
                | "state"
                | "pi"
                | "sample_rate"
                | "true"
                | "false"
                | "reset_on"
                | "spawn"
                | "oversample"
                | "control"
        ) {
            return Err(Diagnostic::new(
                format!("`{}` is a reserved word", name.name),
                name.span,
            ));
        }
        if self.frame.results.contains(&name.name) {
            return Ok(());
        }
        self.names.check_local_in(name, true)
    }

    pub(super) fn bind(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        if name.name == "out" {
            let v = self.num(value)?;
            self.frame.out = Some(v);
            return Ok(());
        }
        self.check_name(name)?;
        let v = self.value(value)?;
        if let Some(KVal::State { id, unit, len }) = self.frame.scope.get(&name.name).cloned() {
            if !self.frame.buffers.contains(&name.name) {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` is not a state array, so it can't be written",
                        name.name
                    ),
                    name.span,
                ));
            }
            // `buf = expr` writes every entry: an array of the same length, or one value for all
            let input = self.lane_of(v, value.span)?;
            if input.len.is_some_and(|n| n != len) {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` has {len} entries, but this array has {}",
                        name.name,
                        input.len.unwrap_or(0)
                    ),
                    value.span,
                ));
            }
            if input.unit != unit {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` holds {}, but this is {}",
                        name.name,
                        unit.describe(),
                        input.unit.describe()
                    ),
                    value.span,
                ));
            }
            let cond = self.guard;
            self.code.push(Instr::LaneStore {
                list: id,
                src: input.src,
                cond,
            });
            return Ok(());
        }
        if let Some(s) = self.frame.states.iter().find(|s| s.name == name.name) {
            let KVal::Num(v) = &v else {
                return Err(not_a_number(value));
            };
            if v.unit != s.unit {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` holds {}, but this is {}",
                        name.name,
                        s.unit.describe(),
                        v.unit.describe()
                    ),
                    value.span,
                ));
            }
        }
        let v = self.snapshot(v, value.span)?;
        self.frame.scope.insert(name.name.clone(), v);
        Ok(())
    }

    pub(super) fn declare_state(&mut self, name: &Ident, init: &Expr) -> Res<()> {
        self.check_name(name)?;
        if self.frame.scope.contains_key(&name.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already defined", name.name),
                name.span,
            ));
        }
        // `state s = x * 0` is zeros shaped like x. Only the shape of x is needed, so x may be a signal
        // computed in the body, which a value read at note start could not be.
        let zeros_like = match &init.kind {
            ExprKind::Binary {
                op: BinOp::Mul,
                lhs,
                rhs,
            } => match (&lhs.kind, &rhs.kind) {
                (_, ExprKind::Num { value, unit: None }) if *value == 0.0 => Some(lhs),
                (ExprKind::Num { value, unit: None }, _) if *value == 0.0 => Some(rhs),
                _ => None,
            },
            _ => None,
        };
        let start = if let Some(shape) = zeros_like {
            self.value(shape).and_then(|shape| {
                self.map_value(shape, init.span, |this, v| Ok(this.constant(v.unit, 0.0)))
            })
        } else {
            let saved = self.force.replace(Section::Init);
            let start = self.value(init);
            self.force = saved;
            start
        };
        match start? {
            KVal::Num(v) => {
                let slot = self.state_init.len();
                match self.konst(&v) {
                    Some(k) => self.state_init.push(k),
                    None => {
                        self.state_init.push(0.0);
                        self.init.push(Instr::StoreState { slot, src: v.reg });
                    }
                }
                let loaded = self.registers;
                self.registers += 1;
                self.code.push(Instr::LoadState { dst: loaded, slot });
                self.frame.states.push(StateVar {
                    name: name.name.clone(),
                    slot,
                    loaded,
                    unit: v.unit,
                });
                self.frame.scope.insert(
                    name.name.clone(),
                    KVal::Num(Value {
                        reg: loaded,
                        unit: v.unit,
                        rate: Rate::Audio,
                        port: None,
                    }),
                );
            }
            KVal::Table { unit, values, .. } => {
                let id = self.buffers.len();
                self.buffers.push(values.as_f64().into());
                self.frame.buffers.insert(name.name.clone());
                self.frame.scope.insert(
                    name.name.clone(),
                    KVal::State {
                        id,
                        unit,
                        len: values.len(),
                    },
                );
            }
            KVal::Array(items) => {
                let unit = match items.first() {
                    Some(KVal::Num(first)) => first.unit,
                    None => Unit::Num,
                    _ => {
                        return Err(Diagnostic::new(
                            "a state array needs numeric elements",
                            init.span,
                        ));
                    }
                };
                let id = self.buffers.len();
                let len = items.len();
                let mut values = Vec::with_capacity(len);
                for (i, item) in items.into_iter().enumerate() {
                    let KVal::Num(value) = item else {
                        return Err(Diagnostic::new(
                            "a state array needs numeric elements",
                            init.span,
                        ));
                    };
                    if value.unit != unit {
                        return Err(Diagnostic::new(
                            "state array elements must have the same unit",
                            init.span,
                        ));
                    }
                    values.push(self.konst(&value).unwrap_or(0.0));
                    if self.konst(&value).is_none() {
                        let index = self.constant(Unit::Num, i as f64);
                        self.init.push(Instr::Write {
                            list: id,
                            idx: index.reg,
                            src: value.reg,
                            cond: None,
                        });
                    }
                }
                self.buffers.push(values.into());
                self.frame.buffers.insert(name.name.clone());
                self.frame
                    .scope
                    .insert(name.name.clone(), KVal::State { id, unit, len });
            }
            _ => {
                return Err(Diagnostic::new(
                    "a state starts as a number, or as an array such as `zeros(64)`",
                    init.span,
                ));
            }
        }
        Ok(())
    }

    pub(super) fn block(&mut self, stmts: &[Stmt], value: bool, span: Span) -> Res<Option<KVal>> {
        let outer: Vec<String> = self.frame.scope.keys().cloned().collect();
        let mut tail = None;
        for (i, s) in stmts.iter().enumerate() {
            if matches!(s, Stmt::State { .. }) {
                let Stmt::State { name, .. } = s else {
                    unreachable!()
                };
                return Err(Diagnostic::new(
                    "state is declared at the top of a function, not inside a block",
                    name.span,
                ));
            }
            let last = value && i + 1 == stmts.len();
            if let Some(v) = self.stmt(s, last)? {
                tail = Some(v);
            }
        }
        self.frame.scope.retain(|k, _| outer.contains(k));
        if value && tail.is_none() {
            return Err(Diagnostic::new("this block must end with a value", span));
        }
        Ok(tail)
    }

    pub(super) fn if_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::If {
            cond,
            then,
            otherwise,
        } = &e.kind
        else {
            unreachable!()
        };
        let c = self.num(cond)?;
        if let Some(k) = self.konst(&c) {
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
        let saved_scope = self.frame.scope.clone();
        let saved_out = self.frame.out;
        let saved_guard = self.guard;

        let g = self.guard_with(c);
        self.guard = Some(g.reg);
        self.begin_branch(g.reg);
        self.block(then, false, e.span)?;
        self.end_branch();
        let then_scope = std::mem::replace(&mut self.frame.scope, saved_scope.clone());
        let then_out = std::mem::replace(&mut self.frame.out, saved_out);

        let not_c = {
            let zero = self.constant(c.unit, 0.0);
            self.cmp(Cmp::Eq, c, zero)
        };
        self.guard = saved_guard;
        let g = self.guard_with(not_c);
        self.guard = Some(g.reg);
        self.begin_branch(g.reg);
        if let Some(stmts) = otherwise {
            self.block(stmts, false, e.span)?;
        }
        self.end_branch();
        let else_scope = std::mem::replace(&mut self.frame.scope, saved_scope);
        let else_out = std::mem::replace(&mut self.frame.out, saved_out);
        self.guard = saved_guard;

        self.merge_branch_bindings(c, then_scope, else_scope, then_out, else_out, e.span)
    }

    /// A value-producing `if` must merge assignments just as a statement `if` does.
    pub(super) fn merge_branch_bindings(
        &mut self,
        c: Value,
        then_scope: HashMap<String, KVal>,
        else_scope: HashMap<String, KVal>,
        then_out: Option<Value>,
        else_out: Option<Value>,
        span: Span,
    ) -> Res<()> {
        let names: Vec<String> = self.frame.scope.keys().cloned().collect();
        for name in names {
            let (a, b) = (&then_scope[&name], &else_scope[&name]);
            let merged = if same(a, b) {
                a.clone()
            } else {
                self.select_value(c, a.clone(), b.clone(), span)?
            };
            self.frame.scope.insert(name, merged);
        }
        self.frame.out = match (then_out, else_out) {
            (Some(a), Some(b)) => Some(self.select(c, a, b)),
            (Some(a), None) => {
                let zero = self.constant(a.unit, 0.0);
                Some(self.select(c, a, zero))
            }
            (None, Some(b)) => {
                let zero = self.constant(b.unit, 0.0);
                Some(self.select(c, zero, b))
            }
            (None, None) => None,
        };
        Ok(())
    }

    /// The entries a `for` runs over.
    /// Materialize a fixed-length array's current elements. Storage is an
    /// implementation choice, never a restriction on array operations.
    pub(super) fn array_elements(&mut self, value: KVal, span: Span) -> Res<Vec<KVal>> {
        match value {
            KVal::Array(items) => Ok(items.into_vec()),
            KVal::Table { unit, values, .. } => Ok(values
                .as_f64()
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect()),
            KVal::State { id, unit, len } => Ok((0..len)
                .map(|k| {
                    let index = self.constant(Unit::Num, k as f64);
                    KVal::Num(self.read_array(ArraySource::State(id), unit, index, None))
                })
                .collect()),
            KVal::Lane { id, unit, len } => Ok((0..len)
                .map(|k| {
                    let index = self.constant(Unit::Num, k as f64);
                    KVal::Num(self.read_array(ArraySource::Lane(id), unit, index, None))
                })
                .collect()),
            KVal::Num(v) => Ok(vec![KVal::Num(v)]),
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => Ok(range_numbers(lo, hi, inclusive, span)?
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect()),
            _ => Err(Diagnostic::new("expected an array here", span)),
        }
    }

    pub(super) fn is_array(value: &KVal) -> bool {
        matches!(
            value,
            KVal::Array(_) | KVal::Table { .. } | KVal::State { .. } | KVal::Lane { .. }
        )
    }

    pub(super) fn entries(&mut self, e: &Expr) -> Res<Vec<KVal>> {
        let value = self.value(e)?;
        match value {
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => Ok(range_numbers(lo, hi, inclusive, e.span)?
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect()),
            other => self.array_elements(other, e.span),
        }
    }

    pub(super) fn for_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::For { var, iter, body } = &e.kind else {
            unreachable!()
        };
        self.check_name(var)?;
        if self.frame.scope.contains_key(&var.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already in use", var.name),
                var.span,
            ));
        }
        for entry in self.entries(iter)? {
            self.frame.scope.insert(var.name.clone(), entry);
            self.block(body, false, e.span)?;
            self.frame.scope.remove(&var.name);
        }
        Ok(())
    }
}
