//! Expressions in a kernel: values, names, maths, indexing and array reads.

use super::*;

impl Kb<'_> {
    // ---- expressions ---------------------------------------------------

    /// A number or a signal: anything else is an error.
    pub(super) fn num(&mut self, e: &Expr) -> Res<Value> {
        match self.value(e)? {
            KVal::Num(v) => Ok(v),
            KVal::Record(fields) => match &fields[0].1 {
                KVal::Num(v) => Ok(*v),
                _ => Err(not_a_number(e)),
            },
            _ => Err(not_a_number(e)),
        }
    }

    pub(super) fn value(&mut self, e: &Expr) -> Res<KVal> {
        match &e.kind {
            ExprKind::Num { value, unit } => {
                let (unit, value) = literal_sig(*value, unit.as_deref(), self.tempo, e.span)?;
                Ok(KVal::Num(self.constant(unit, value)))
            }
            ExprKind::Name(name) => self.name(name, e.span),
            ExprKind::Neg(inner) => {
                let value = self.value(inner)?;
                self.negate_value(value, e.span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                self.binary_value(*op, l, r, e.span)
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let (a, b) = (self.value(lhs)?, self.value(rhs)?);
                self.zip_values(a, b, e.span, |this, a, b| {
                    if a.unit != b.unit {
                        return Err(Diagnostic::new(
                            format!(
                                "can't compare {} and {}",
                                a.unit.describe(),
                                b.unit.describe()
                            ),
                            e.span,
                        ));
                    }
                    Ok(this.cmp(cmp_op(*op), a, b))
                })
            }
            ExprKind::Logic { op, lhs, rhs } => {
                let (l, r) = (self.num(lhs)?, self.num(rhs)?);
                let (l, r) = (self.truthy(l), self.truthy(r));
                Ok(KVal::Num(match op {
                    LogicOp::And => self.bin(Op::Mul, l, r, Unit::Num),
                    LogicOp::Or => self.bin(Op::Max, l, r, Unit::Num),
                }))
            }
            ExprKind::Not(inner) => {
                let v = self.num(inner)?;
                let zero = self.constant(v.unit, 0.0);
                Ok(KVal::Num(self.cmp(Cmp::Eq, v, zero)))
            }
            ExprKind::If {
                cond,
                then,
                otherwise,
            } => {
                let Some(otherwise) = otherwise else {
                    return Err(Diagnostic::new(
                        "an `if` used as a value needs an `else`",
                        e.span,
                    ));
                };
                let c = self.num(cond)?;
                if let Some(k) = self.konst(&c) {
                    let taken = if k != 0.0 { then } else { otherwise };
                    return Ok(self.block(taken, true, e.span)?.expect("a value"));
                }
                let saved = self.guard;
                let saved_scope = self.frame.scope.clone();
                let saved_out = self.frame.out;
                let g = self.guard_with(c);
                self.guard = Some(g.reg);
                self.begin_branch(g.reg);
                let a = self.block(then, true, e.span)?.expect("a value");
                self.end_branch();
                let then_scope = std::mem::replace(&mut self.frame.scope, saved_scope.clone());
                let then_out = std::mem::replace(&mut self.frame.out, saved_out);
                let not_c = {
                    let zero = self.constant(c.unit, 0.0);
                    self.cmp(Cmp::Eq, c, zero)
                };
                self.guard = saved;
                let g = self.guard_with(not_c);
                self.guard = Some(g.reg);
                self.begin_branch(g.reg);
                let b = self.block(otherwise, true, e.span)?.expect("a value");
                self.end_branch();
                let else_scope = std::mem::replace(&mut self.frame.scope, saved_scope);
                let else_out = std::mem::replace(&mut self.frame.out, saved_out);
                self.guard = saved;
                self.merge_branch_bindings(c, then_scope, else_scope, then_out, else_out, e.span)?;
                self.select_value(c, a, b, e.span)
            }
            ExprKind::For { var, iter, body } => {
                self.check_name(var)?;
                let mut results = Vec::new();
                for entry in self.entries(iter)? {
                    self.frame.scope.insert(var.name.clone(), entry);
                    let v = self.block(body, true, e.span);
                    self.frame.scope.remove(&var.name);
                    results.push(self.snapshot(v?.expect("a value"), e.span)?);
                }
                Ok(KVal::array(results))
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let (l, h) = (self.num(lo)?, self.num(hi)?);
                let (Some(a), Some(b)) = (self.konst(&l), self.konst(&h)) else {
                    return Err(Diagnostic::new(
                        "the ends of a range must be known while compiling",
                        e.span,
                    ));
                };
                if l.unit != h.unit {
                    return Err(Diagnostic::new(
                        "the ends of a range need the same unit",
                        e.span,
                    ));
                }
                Ok(KVal::Range {
                    unit: l.unit,
                    lo: a,
                    hi: b,
                    inclusive: *inclusive,
                })
            }
            ExprKind::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    let value = self.value(i)?;
                    out.push(self.snapshot(value, i.span)?);
                }
                Ok(KVal::array(out))
            }
            ExprKind::Index { list, index } => self.index(list, index, e.span),
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Lambda { params, body } => {
                let def = funcs::lambda(params, body, e.span);
                let captures = self
                    .frame
                    .scope
                    .iter()
                    .filter(|(n, _)| funcs::reads(&def, n))
                    .map(|(n, v)| (n.clone(), v.clone()))
                    .collect();
                Ok(KVal::Function { def, captures })
            }
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can't be used inside a function with state",
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
            ExprKind::Env(env) if self.instrument_scope => self.envelope(env, e.span),
            ExprKind::Env(_) => Err(Diagnostic::new(
                "envelopes can't be used inside a function with state",
                e.span,
            )
            .with_help("make the envelope in the instrument and pass it in as an argument")),
        }
    }

    pub(super) fn emit_native(
        &mut self,
        op: Box<dyn niminal_engine::Opcode>,
        inputs: Vec<NativeInput>,
        unit: Unit,
    ) -> KVal {
        let index = self.natives.len();
        self.natives.push(Native { op, inputs });
        let guard = self.guard;
        let reg = self.emit(Rate::Audio, |dst| Instr::Native { dst, index, guard });
        KVal::Num(Value {
            reg,
            unit,
            rate: Rate::Audio,
            port: None,
        })
    }

    pub(super) fn envelope(&mut self, env: &EnvLit, span: Span) -> Res<KVal> {
        use niminal_engine::ops::{Env, EnvField, Segment};
        let mut values = Vec::new();
        for step in env.values() {
            let value = match step {
                EnvValue::Lit(n) => {
                    let (unit, value) =
                        literal_sig(n.value, n.unit.as_deref(), self.tempo, n.span)?;
                    self.constant(unit, value)
                }
                EnvValue::Expr(e) => self.num(e)?,
            };
            values.push(value);
        }
        let start = values[0];
        if start.unit == Unit::Time || start.unit == Unit::Db {
            return Err(Diagnostic::new(
                "an envelope level cannot be a duration or db",
                span,
            ));
        }
        let mut fields = Vec::new();
        let mut inputs = Vec::new();
        if self.konst(&start).is_none() {
            fields.push(EnvField::Start);
            inputs.push(NativeInput::Reg(start.reg));
        }
        let mut segments = Vec::new();
        for (i, segment) in env.segments.iter().enumerate() {
            if values[1 + i * 2].unit != Unit::Time {
                return Err(Diagnostic::new(
                    "an envelope duration needs a time",
                    segment.dur.span(),
                ));
            }
            if values[2 + i * 2].unit != start.unit {
                return Err(Diagnostic::new(
                    "envelope levels need the same unit",
                    segment.level.span(),
                ));
            }
            let duration = values[1 + i * 2];
            let level = values[2 + i * 2];
            if self.konst(&duration).is_none() {
                fields.push(EnvField::Dur(i));
                inputs.push(NativeInput::Reg(duration.reg));
            }
            if self.konst(&level).is_none() {
                fields.push(EnvField::Level(i));
                inputs.push(NativeInput::Reg(level.reg));
            }
            segments.push(Segment::new(
                self.konst(&duration).unwrap_or(0.0) as f32,
                self.konst(&level).unwrap_or(0.0) as f32,
                crate::lower::curve_of(segment.curve),
            ));
        }
        if fields.len() > niminal_engine::MAX_INPUTS {
            return Err(Diagnostic::new(
                "too many envelope inputs in an fx scope",
                span,
            ));
        }
        let mut op = Env::new(
            self.konst(&start).unwrap_or(0.0) as f32,
            segments,
            env.sustain_at,
        )
        .with_inputs(fields);
        if env.looped {
            op = op.looped();
        }
        Ok(self.emit_native(Box::new(op), inputs, start.unit))
    }

    pub(super) fn name(&mut self, name: &str, span: Span) -> Res<KVal> {
        if let Some(v) = self.frame.scope.get(name).cloned() {
            return match v {
                other if self.force.is_some() => {
                    self.map_value(other, span, |this, value| this.read(value, span))
                }
                other => Ok(other),
            };
        }
        if let Some((_, buffer)) = self.names.buffers.get(name).cloned() {
            return Ok(KVal::Buffer {
                selector: self.constant(Unit::Num, 0.0),
                choices: crate::lower::Choices::new(vec![(name.to_string(), buffer)]),
            });
        }
        if let Some(def) = self.names.fns.get(name) {
            return Ok(KVal::Function {
                def: def.clone(),
                captures: HashMap::new(),
            });
        }
        match name {
            "pi" => return Ok(KVal::Num(self.constant(Unit::Num, std::f64::consts::PI))),
            "true" => return Ok(KVal::Num(self.constant(Unit::Num, 1.0))),
            "false" => return Ok(KVal::Num(self.constant(Unit::Num, 0.0))),
            "sample_rate" => {
                let reg = self.emit(Rate::Init, |dst| Instr::SampleRate { dst });
                return Ok(KVal::Num(Value {
                    reg,
                    unit: Unit::Hz,
                    rate: Rate::Init,
                    port: None,
                }));
            }
            "note" => {
                return Err(
                    Diagnostic::new("`note` is only available in an instrument", span)
                        .with_help("pass `note.time` or `note.released` in as an argument"),
                );
            }
            "clock" => {
                return Err(Diagnostic::new(
                    "`clock` is only available in an instrument, node or bus",
                    span,
                )
                .with_help("pass `clock.time` in as an argument"));
            }
            _ => {}
        }
        if let Some(v) = self.const_tables.get(name) {
            return Ok(v.clone());
        }
        if let Some(d) = self.names.consts.get(name) {
            let v = self.const_value(d, span)?;
            if matches!(v, KVal::Table { .. }) {
                self.const_tables.insert(name.to_string(), v.clone());
            }
            return Ok(v);
        }
        if let Ok(note @ niminal_score::Value::Note(_)) = name.parse::<niminal_score::Value>() {
            let value = note
                .as_hz_in(&self.tempo.tuning)
                .expect("a note is a frequency");
            return Ok(KVal::Num(self.constant(Unit::Hz, value)));
        }
        let known = self
            .frame
            .scope
            .keys()
            .map(String::as_str)
            .chain(["pi", "sample_rate"]);
        Err(unknown_name(name, span, known))
    }

    /// A constant from a `const` definition.
    pub(super) fn const_value(&mut self, d: &Datum, span: Span) -> Res<KVal> {
        match d {
            Datum::Function { .. } => Err(Diagnostic::new(
                "a function is not a numeric constant",
                span,
            )),
            Datum::Record(fields) => Ok(KVal::Record(
                fields
                    .iter()
                    .map(|(name, v)| Ok((name.clone(), self.const_value(v, span)?)))
                    .collect::<Res<_>>()?,
            )),
            Datum::Sig(s) => match s.konst() {
                Some(k) => Ok(KVal::Num(self.constant(s.unit(), k))),
                None => Err(Diagnostic::new("this constant isn't a number", span)),
            },
            Datum::Table(table) => Ok(self.table(Unit::Num, table.clone())),
            Datum::Buffer { .. } | Datum::BufferRef { .. } => Err(Diagnostic::new(
                "a shared buffer isn't a compile-time constant",
                span,
            )),
            Datum::Range { lo, hi, inclusive } => match (lo.konst(), hi.konst()) {
                (Some(a), Some(b)) => Ok(KVal::Range {
                    unit: lo.unit(),
                    lo: a,
                    hi: b,
                    inclusive: *inclusive,
                }),
                _ => Err(Diagnostic::new(
                    "this range isn't known while compiling",
                    span,
                )),
            },
            Datum::Array(items) => {
                let numbers: Option<Vec<(Unit, f64)>> = items
                    .iter()
                    .map(|i| match i {
                        Datum::Sig(s) => s.konst().map(|k| (s.unit(), k)),
                        _ => None,
                    })
                    .collect();
                match numbers {
                    Some(n) if !n.is_empty() && n.iter().all(|(u, _)| *u == n[0].0) => {
                        let values = Arc::new(Table::from_f64(n.iter().map(|(_, k)| *k), None));
                        Ok(self.table(n[0].0, values))
                    }
                    _ => {
                        let mut out = Vec::with_capacity(items.len());
                        for i in items {
                            out.push(self.const_value(i, span)?);
                        }
                        Ok(KVal::array(out))
                    }
                }
            }
        }
    }

    pub(super) fn extreme_value(
        &mut self,
        a: KVal,
        b: KVal,
        minimum: bool,
        span: Span,
    ) -> Res<KVal> {
        let op = if minimum { Op::Min } else { Op::Max };
        if let Some(lane) = self.lane_binary(op, |a, b| (a == b).then_some(a), &a, &b, span)? {
            return Ok(lane);
        }
        self.zip_values(a, b, span, |this, a, b| {
            if a.unit != b.unit {
                return Err(Diagnostic::new(
                    "array elements must have the same unit",
                    span,
                ));
            }
            Ok(this.bin(if minimum { Op::Min } else { Op::Max }, a, b, a.unit))
        })
    }

    pub(super) fn math_value(&mut self, f: Fn1, value: KVal, span: Span) -> Res<KVal> {
        if let KVal::State {
            unit: Unit::Num, ..
        }
        | KVal::Lane {
            unit: Unit::Num, ..
        } = value
            && self.force.is_none()
            && let Some(input) = self.lane_input(&value)
        {
            let len = input.len.expect("an array has a length");
            let rate = self.rate(input.rate);
            let dst = self.new_lane(len, rate);
            self.push(
                rate,
                Instr::LaneCall {
                    dst,
                    f,
                    a: input.src,
                },
            );
            return Ok(KVal::Lane {
                id: dst,
                unit: Unit::Num,
                len,
            });
        }
        self.map_value(value, span, |this, value| {
            if value.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` expects a plain number, found {}",
                        f.name(),
                        value.unit.describe()
                    ),
                    span,
                ));
            }
            Ok(this.call1(f, value))
        })
    }

    pub(super) fn select_value(
        &mut self,
        condition: Value,
        a: KVal,
        b: KVal,
        span: Span,
    ) -> Res<KVal> {
        if let (KVal::Num(a), KVal::Num(b)) = (&a, &b) {
            if a.unit != b.unit {
                return Err(Diagnostic::new("both branches need the same unit", span));
            }
            return Ok(KVal::Num(self.select(condition, *a, *b)));
        }
        if Self::is_array(&a) && Self::is_array(&b) {
            let a = self.array_elements(a, span)?;
            let b = self.array_elements(b, span)?;
            if a.len() != b.len() {
                return Err(Diagnostic::new(
                    "both branches need the same array shape",
                    span,
                ));
            }
            return Ok(KVal::Array(
                a.into_iter()
                    .zip(b)
                    .map(|(a, b)| self.select_value(condition, a, b, span))
                    .collect::<Res<_>>()?,
            ));
        }
        Err(Diagnostic::new(
            "both branches need matching numbers or array shapes",
            span,
        ))
    }

    pub(super) fn negate_value(&mut self, value: KVal, span: Span) -> Res<KVal> {
        if let KVal::State { unit, .. } | KVal::Lane { unit, .. } = value
            && unit != Unit::Db
            && self.force.is_none()
        {
            let minus_one = self.constant(Unit::Num, -1.0);
            if let Some(lane) =
                self.lane_binary(Op::Mul, |a, _| Some(a), &value, &KVal::Num(minus_one), span)?
            {
                return Ok(lane);
            }
        }
        self.map_value(value, span, |this, v| {
            if v.unit == Unit::Db {
                let one = this.constant(Unit::Num, 1.0);
                return Ok(this.bin(Op::Div, one, v, Unit::Db));
            }
            if let Some(k) = this.konst(&v) {
                return Ok(this.constant(v.unit, -k));
            }
            let rate = this.rate(v.rate);
            let reg = this.emit(rate, |dst| Instr::Neg { dst, a: v.reg });
            Ok(Value {
                reg,
                unit: v.unit,
                rate,
                port: None,
            })
        })
    }

    pub(super) fn add(&mut self, a: Value, b: Value, span: Span) -> Res<Value> {
        self.binary(BinOp::Add, a, b, span)
    }

    pub(super) fn binary_value(&mut self, op: BinOp, a: KVal, b: KVal, span: Span) -> Res<KVal> {
        type UnitRule = fn(Unit, Unit) -> Option<Unit>;
        let (engine_op, unit_of): (Op, UnitRule) = match op {
            BinOp::Add => (Op::Add, |a, b| (a == b && a != Unit::Db).then_some(a)),
            BinOp::Sub => (Op::Sub, |a, b| (a == b && a != Unit::Db).then_some(a)),
            BinOp::Mul => (Op::Mul, crate::unit::mul_unit),
            BinOp::Div => (Op::Div, crate::unit::div_unit),
        };
        if let Some(lane) = self.lane_binary(engine_op, unit_of, &a, &b, span)? {
            return Ok(lane);
        }
        self.zip_values(a, b, span, |this, a, b| this.binary(op, a, b, span))
    }

    pub(super) fn zip_values(
        &mut self,
        a: KVal,
        b: KVal,
        span: Span,
        operation: impl Fn(&mut Self, Value, Value) -> Res<Value> + Copy,
    ) -> Res<KVal> {
        let a = if Self::is_array(&a) {
            KVal::array(self.array_elements(a, span)?)
        } else {
            a
        };
        let b = if Self::is_array(&b) {
            KVal::array(self.array_elements(b, span)?)
        } else {
            b
        };
        match (a, b) {
            (KVal::Num(a), KVal::Num(b)) => operation(self, a, b).map(KVal::Num),
            (KVal::Record(fields), b) => self.zip_values(fields[0].1.clone(), b, span, operation),
            (a, KVal::Record(fields)) => self.zip_values(a, fields[0].1.clone(), span, operation),
            (KVal::Array(a), KVal::Array(b)) => {
                if a.len() != b.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "can't combine an array of {} with an array of {}",
                            a.len(),
                            b.len()
                        ),
                        span,
                    ));
                }
                Ok(KVal::Array(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| self.zip_values(a, b, span, operation))
                        .collect::<Res<_>>()?,
                ))
            }
            (KVal::Array(a), b @ KVal::Num(_)) => Ok(KVal::Array(
                a.into_iter()
                    .map(|a| self.zip_values(a, b.clone(), span, operation))
                    .collect::<Res<_>>()?,
            )),
            (a @ KVal::Num(_), KVal::Array(b)) => Ok(KVal::Array(
                b.into_iter()
                    .map(|b| self.zip_values(a.clone(), b, span, operation))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new("arithmetic doesn't work on ranges", span)),
        }
    }

    pub(super) fn map_value(
        &mut self,
        value: KVal,
        span: Span,
        operation: impl Fn(&mut Self, Value) -> Res<Value> + Copy,
    ) -> Res<KVal> {
        match value {
            KVal::Num(value) => operation(self, value).map(KVal::Num),
            KVal::Record(fields) => self.map_value(fields[0].1.clone(), span, operation),
            other if Self::is_array(&other) => {
                let items = self.array_elements(other, span)?;
                Ok(KVal::Array(
                    items
                        .into_iter()
                        .map(|v| self.map_value(v, span, operation))
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "this operation needs a number or an array",
                span,
            )),
        }
    }

    pub(super) fn binary(&mut self, op: BinOp, l: Value, r: Value, span: Span) -> Res<Value> {
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            BinOp::Mul => "multiply",
            BinOp::Div => "divide",
        };
        let mismatch = || {
            Diagnostic::new(
                format!(
                    "can't {verb} {} and {}",
                    l.unit.describe(),
                    r.unit.describe()
                ),
                span,
            )
        };
        match op {
            BinOp::Add | BinOp::Sub => {
                if l.unit == Unit::Hz && r.unit == Unit::Semitones {
                    return Ok(self.transpose(l, r, op == BinOp::Sub));
                }
                if l.unit != r.unit {
                    return Err(mismatch());
                }
                if l.unit == Unit::Db {
                    // adding decibels multiplies gain factors
                    let r = if op == BinOp::Sub {
                        let one = self.constant(Unit::Num, 1.0);
                        self.bin(Op::Div, one, r, Unit::Db)
                    } else {
                        r
                    };
                    return Ok(self.bin(Op::Mul, l, r, Unit::Db));
                }
                let o = if op == BinOp::Add { Op::Add } else { Op::Sub };
                Ok(self.bin(o, l, r, l.unit))
            }
            BinOp::Mul => {
                let unit = crate::unit::mul_unit(l.unit, r.unit).ok_or_else(mismatch)?;
                Ok(self.bin(Op::Mul, l, r, unit))
            }
            BinOp::Div => {
                let unit = crate::unit::div_unit(l.unit, r.unit).ok_or_else(mismatch)?;
                Ok(self.bin(Op::Div, l, r, unit))
            }
        }
    }

    /// `freq + 7st`: a frequency scaled by 2^(st/12).
    pub(super) fn transpose(&mut self, freq: Value, interval: Value, down: bool) -> Value {
        let k = if down { -1.0 } else { 1.0 } * std::f64::consts::LN_2 / 12.0;
        let scale = self.constant(Unit::Num, k);
        let e = self.bin(Op::Mul, interval, scale, Unit::Num);
        let ratio = self.call1(Fn1::Exp, e);
        self.bin(Op::Mul, freq, ratio, Unit::Hz)
    }

    pub(super) fn index(&mut self, list: &Expr, index: &Expr, span: Span) -> Res<KVal> {
        if self.instrument_scope
            && let ExprKind::Name(name) = &list.kind
            && let Some((_, buffer)) = self.names.buffers.get(name).cloned()
        {
            let index = self.num(index)?;
            if index.unit != Unit::Num {
                return Err(Diagnostic::new("a buffer index is a plain number", span));
            }
            return Ok(self.emit_native(
                Box::new(niminal_engine::ops::BufferRead::new(buffer, Interp::None)),
                vec![NativeInput::Reg(index.reg)],
                Unit::Num,
            ));
        }
        let l = self.value(list)?;
        if let KVal::Buffer { selector, choices } = l {
            let pos = self.num(index)?;
            return self.read_buffer_ref(selector, choices, pos, Interp::None, span);
        }
        if let ExprKind::Range { .. } = index.kind {
            let KVal::Range {
                lo, hi, inclusive, ..
            } = self.value(index)?
            else {
                unreachable!()
            };
            let picks = range_numbers(lo, hi, inclusive, index.span)?;
            let entries = self.array_elements(l, list.span)?;
            let n = entries.len();
            if n == 0 && !picks.is_empty() {
                return Err(Diagnostic::new("this array is empty", span));
            }
            return Ok(KVal::Array(
                picks
                    .into_iter()
                    .map(|k| entries[niminal_engine::wrapped_index(k, n)].clone())
                    .collect(),
            ));
        }
        let i = self.num(index)?;
        if i.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("an index is a plain number, found {}", i.unit.describe()),
                index.span,
            ));
        }
        match l {
            KVal::Array(items) => {
                if items.is_empty() {
                    return Err(Diagnostic::new("this array is empty", span));
                }
                let n = items.len();
                if let Some(k) = self.konst(&i) {
                    return Ok(items[niminal_engine::wrapped_index(k, n)].clone());
                }
                self.index_values(items.into_vec(), i, span)
            }
            KVal::Table { id, unit, values } => {
                if values.is_empty() {
                    return Err(Diagnostic::new("this array is empty", span));
                }
                if let Some(k) = self.konst(&i) {
                    let n = values.len();
                    return Ok(KVal::Num(self.constant(
                        unit,
                        f64::from(values.get(niminal_engine::wrapped_index(k, n))),
                    )));
                }
                Ok(KVal::Num(self.read_array(
                    ArraySource::Table(id),
                    unit,
                    i,
                    None,
                )))
            }
            KVal::State { id, unit, len } => {
                if len == 0 {
                    return Err(Diagnostic::new("this array is empty", span));
                }
                Ok(KVal::Num(self.read_array(
                    ArraySource::State(id),
                    unit,
                    i,
                    None,
                )))
            }
            KVal::Lane { id, unit, .. } => Ok(KVal::Num(self.read_array(
                ArraySource::Lane(id),
                unit,
                i,
                None,
            ))),
            KVal::Num(v) => Ok(KVal::Num(v)),
            _ => Err(Diagnostic::new("only an array can be indexed", list.span)),
        }
    }

    pub(super) fn index_values(&mut self, items: Vec<KVal>, index: Value, span: Span) -> Res<KVal> {
        if items.iter().all(|v| matches!(v, KVal::Num(_))) {
            let (array, unit) = self.numeric_array(KVal::array(items), span)?;
            return Ok(KVal::Num(self.read_array(array, unit, index, None)));
        }
        let rows = items
            .into_iter()
            .map(|v| {
                if !Self::is_array(&v) {
                    return Err(Diagnostic::new(
                        "an index that changes needs matching element shapes",
                        span,
                    ));
                }
                self.array_elements(v, span)
            })
            .collect::<Res<Vec<_>>>()?;
        let Some(first) = rows.first() else {
            return Err(Diagnostic::new("this array is empty", span));
        };
        let width = first.len();
        if rows.iter().any(|row| row.len() != width) {
            return Err(Diagnostic::new(
                "an index that changes needs matching element shapes",
                span,
            ));
        }
        Ok(KVal::Array(
            (0..width)
                .map(|column| {
                    let elements = rows.iter().map(|row| row[column].clone()).collect();
                    self.index_values(elements, index, span)
                })
                .collect::<Res<_>>()?,
        ))
    }

    pub(super) fn numeric_array(&mut self, value: KVal, span: Span) -> Res<(ArraySource, Unit)> {
        match value {
            KVal::Table { values, .. } if values.is_empty() => {
                Err(Diagnostic::new("this array is empty", span))
            }
            KVal::State { len: 0, .. } => Err(Diagnostic::new("this array is empty", span)),
            KVal::Table { id, unit, .. } => Ok((ArraySource::Table(id), unit)),
            KVal::State { id, unit, .. } => Ok((ArraySource::State(id), unit)),
            KVal::Lane { id, unit, .. } => Ok((ArraySource::Lane(id), unit)),
            other => {
                let items = self.array_elements(other, span)?;
                let mut registers = Vec::with_capacity(items.len());
                let mut unit = None;
                let mut rate = Rate::Const;
                for item in items {
                    let KVal::Num(v) = item else {
                        return Err(Diagnostic::new(
                            "this operation needs a one-dimensional numeric array",
                            span,
                        ));
                    };
                    if unit.is_some_and(|unit| unit != v.unit) {
                        return Err(Diagnostic::new(
                            "array elements must have the same unit",
                            span,
                        ));
                    }
                    unit = Some(v.unit);
                    rate = rate.max(v.rate);
                    registers.push(v.reg);
                }
                let Some(unit) = unit else {
                    return Err(Diagnostic::new("this array is empty", span));
                };
                let id = self.values.len();
                self.values.push(registers.into());
                self.value_rates.push(rate);
                Ok((ArraySource::Values(id), unit))
            }
        }
    }

    pub(super) fn read_array(
        &mut self,
        list: ArraySource,
        unit: Unit,
        i: Value,
        interp: Option<Interp>,
    ) -> Value {
        if let Some(pos) = self.konst(&i) {
            let data = match list {
                ArraySource::Table(id) => Some(self.tables[id].as_f64()),
                ArraySource::Values(id) => self.values[id]
                    .iter()
                    .map(|reg| self.consts.get(reg).copied())
                    .collect::<Option<Vec<_>>>(),
                ArraySource::State(_) | ArraySource::Lane(_) => None,
            };
            if let Some(data) = data {
                return self.constant(
                    unit,
                    niminal_engine::ops::read_at(&data, pos, interp.unwrap_or(Interp::None)),
                );
            }
        }
        let rate = match list {
            // a state list can change every sample
            ArraySource::State(_) => Rate::Audio,
            ArraySource::Lane(id) => self.rate(i.rate.max(self.lane_rates[id])),
            ArraySource::Table(_) => self.rate(i.rate),
            ArraySource::Values(id) => self.rate(i.rate.max(self.value_rates[id])),
        };
        let reg = match interp {
            None => self.emit(rate, |dst| Instr::Read {
                dst,
                list,
                idx: i.reg,
            }),
            Some(interp) => self.emit(rate, |dst| Instr::ReadAt {
                dst,
                list,
                pos: i.reg,
                interp,
            }),
        };
        Value {
            reg,
            unit,
            rate,
            port: None,
        }
    }
}
