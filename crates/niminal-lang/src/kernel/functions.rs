//! Function bodies, parameter checks and calls inside a kernel.

use super::*;

impl Kb<'_> {
    // ---- functions -----------------------------------------------------

    /// Run a function's body in the current frame, which holds its parameters, and give its result.
    pub(super) fn body(&mut self, def: &FnDef) -> Res<KVal> {
        let n = def.body.len();
        let mut tail = None;
        for (i, stmt) in def.body.iter().enumerate() {
            let last = i + 1 == n && matches!(def.result, FnResult::Inferred | FnResult::Unit(_));
            if let Some(v) = self.stmt(stmt, last)? {
                tail = Some((v, stmt_span(stmt)));
            }
        }
        let result = match &def.result {
            FnResult::Named(outputs) => {
                let mut fields = Vec::with_capacity(outputs.len());
                for out in outputs {
                    let Some(v) = self.frame.scope.get(&out.name).cloned() else {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` is a result of `{}`, but the body never defines it",
                                out.name, def.name.name
                            ),
                            out.span,
                        ));
                    };
                    fields.push((out.name.clone(), v));
                }
                KVal::Record(fields)
            }
            _ => {
                let v = match (tail, self.frame.out) {
                    (Some(_), Some(_)) => {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` sets `out`, so its last line can't also be a value",
                                def.name.name
                            ),
                            def.name.span,
                        ));
                    }
                    (Some((v, span)), None) => (v, span),
                    (None, Some(out)) => (KVal::Num(out), def.name.span),
                    (None, None) => {
                        return Err(Diagnostic::new(
                            format!("`{}` must end with the value it gives", def.name.name),
                            def.name.span,
                        )
                        .with_help("the last line of a function is its result"));
                    }
                };
                if let Some(unit) = funcs::result_unit(&def.result)? {
                    self.check_result_unit(&v.0, unit, v.1)?;
                }
                v.0
            }
        };
        let result = self.snapshot(result, def.name.span)?;
        self.finish_frame()?;
        Ok(result)
    }

    pub(super) fn check_result_unit(&self, value: &KVal, unit: Unit, span: Span) -> Res<()> {
        match value {
            KVal::Num(v) if v.unit == unit => Ok(()),
            KVal::Table { unit: u, .. } | KVal::State { unit: u, .. } if *u == unit => Ok(()),
            KVal::Array(items) => {
                for item in items {
                    self.check_result_unit(item, unit, span)?;
                }
                Ok(())
            }
            _ => Err(Diagnostic::new(
                format!("the result must have unit {}", unit.describe()),
                span,
            )),
        }
    }

    /// Store each state's value for the next sample: only where the guard holds, if there is one.
    pub(super) fn finish_frame(&mut self) -> Res<()> {
        let guard = self.guard;
        let states = std::mem::take(&mut self.frame.states);
        for s in &states {
            let Some(KVal::Num(v)) = self.frame.scope.get(&s.name).cloned() else {
                continue;
            };
            let src = self.guarded(guard, v.reg, s.loaded);
            self.code.push(Instr::StoreState { slot: s.slot, src });
        }
        let forward: Vec<(String, (usize, Span))> = self.frame.forward.drain().collect();
        for (name, (slot, span)) in forward {
            let v = match self.frame.scope.get(&name) {
                Some(KVal::Num(v)) => *v,
                _ => {
                    return Err(Diagnostic::new(
                        format!("`prev({name})` reads `{name}`, which is never defined"),
                        span,
                    ));
                }
            };
            if v.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "`prev({name})` is read before `{name}` is defined, so it is a plain number, but `{name}` is {}",
                        v.unit.describe()
                    ),
                    span,
                ));
            }
            let loaded = self.registers;
            self.registers += 1;
            self.code.push(Instr::LoadState { dst: loaded, slot });
            let src = self.guarded(guard, v.reg, loaded);
            self.code.push(Instr::StoreState { slot, src });
        }
        for (slot, reg) in std::mem::take(&mut self.frame.deferred) {
            self.code.push(Instr::StoreState { slot, src: reg });
        }
        Ok(())
    }

    pub(super) fn guarded(&mut self, guard: Option<usize>, new: usize, old: usize) -> usize {
        match guard {
            None => new,
            Some(cond) => {
                let dst = self.registers;
                self.registers += 1;
                self.code.push(Instr::Select {
                    dst,
                    cond,
                    a: new,
                    b: old,
                });
                dst
            }
        }
    }

    pub(super) fn bind_choices(&mut self, options: &[String]) {
        for (i, o) in options.iter().enumerate() {
            let v = self.constant(Unit::Num, i as f64);
            self.frame.scope.insert(o.clone(), KVal::Num(v));
        }
    }

    /// Check a value against a parameter, and fit it to the parameter's rate and range.
    pub(super) fn check_param(
        &mut self,
        callee: &str,
        param: &Param,
        v: &KVal,
        span: Span,
    ) -> Res<KVal> {
        let name = &param.name.name;
        let mismatch = |found: &str| {
            Diagnostic::new(
                format!("`{name}` expects {}, found {found}", param.kind.describe()),
                span,
            )
            .with_help(format!("in the call of `{callee}`"))
        };
        match (&param.kind, v) {
            (Kind::Buffer | Kind::Any, KVal::Buffer { .. }) => return Ok(v.clone()),
            (Kind::Buffer, _) | (_, KVal::Buffer { .. }) => return Err(mismatch("a value")),
            (Kind::Function, KVal::Function { .. }) => return Ok(v.clone()),
            (Kind::Function, _) => return Err(mismatch("a value")),
            (_, KVal::Function { .. }) if !matches!(param.kind, Kind::Any) => {
                return Err(mismatch("a function"));
            }
            (Kind::Array(inner), value) => {
                if let Kind::Unit { unit, whole } = **inner {
                    match value {
                        KVal::Table {
                            unit: found,
                            values,
                            ..
                        } => {
                            if *found != unit {
                                return Err(mismatch(found.describe()));
                            }
                            if whole && values.as_f64().iter().any(|v| v.fract() != 0.0) {
                                return Err(mismatch("a fractional array element"));
                            }
                            return Ok(value.clone());
                        }
                        KVal::State { unit: found, .. } if param.rate().is_none() => {
                            if *found != unit {
                                return Err(mismatch(found.describe()));
                            }
                            return Ok(value.clone());
                        }
                        _ => {}
                    }
                }
                let entries = self.array_elements(value.clone(), span)?;
                let element_param = Param {
                    kind: *inner.clone(),
                    ..param.clone()
                };
                let checked = entries
                    .into_iter()
                    .map(|v| self.check_param(callee, &element_param, &v, span))
                    .collect::<Res<_>>()?;
                return Ok(KVal::array(checked));
            }
            (_, KVal::Num(value)) => {
                if let Some(unit) = param.kind.unit() {
                    let fits = value.unit == unit || (unit == Unit::Db && value.unit == Unit::Num);
                    if !fits {
                        return Err(mismatch(value.unit.describe()));
                    }
                }
                if param.rate() == Some(Rate::Const) && self.konst(value).is_none() {
                    return Err(Diagnostic::new(
                        format!("`{name}` must be known while compiling"),
                        span,
                    ));
                }
                if let Kind::Unit { whole: true, .. } = param.kind
                    && let Some(k) = self.konst(value)
                    && k.fract() != 0.0
                {
                    return Err(mismatch(&format!("{k}")));
                }
                let mut value = *value;
                if let Kind::Range { lo, hi, .. } = param.kind {
                    if let Some(k) = self.konst(&value) {
                        if !(lo..=hi).contains(&k) {
                            return Err(Diagnostic::new(
                                format!("`{name}` is between {lo} and {hi}, but this is {k}"),
                                span,
                            ));
                        }
                    } else {
                        let (l, h) = (self.constant(value.unit, lo), self.constant(value.unit, hi));
                        let unit = value.unit;
                        value = self.bin(Op::Max, value, l, unit);
                        value = self.bin(Op::Min, value, h, unit);
                    }
                }
                return Ok(KVal::Num(value));
            }
            (Kind::Any, _) => {}
            (_, other) if Self::is_array(other) => {
                let entries = self.array_elements(other.clone(), span)?;
                return Ok(KVal::Array(
                    entries
                        .into_iter()
                        .map(|v| self.check_param(callee, param, &v, span))
                        .collect::<Res<_>>()?,
                ));
            }
            (_, KVal::Range { .. }) => return Err(mismatch("a range")),
            (_, _) => return Err(mismatch("an array")),
        }
        Ok(v.clone())
    }

    /// A call of another user function: its body is compiled here, with its own state.
    pub(super) fn call_fn(&mut self, def: &FnDef, name: &Ident, args: &[Arg]) -> Res<KVal> {
        self.call_captured(def, name, args, HashMap::new())
    }

    pub(super) fn call_captured(
        &mut self,
        def: &FnDef,
        name: &Ident,
        args: &[Arg],
        captures: HashMap<String, KVal>,
    ) -> Res<KVal> {
        if self.stack.contains(&def.name.name) {
            let mut path = self.stack.clone();
            path.push(def.name.name.clone());
            return Err(Diagnostic::new(
                format!("functions can't call themselves: {}", path.join(" -> ")),
                name.span,
            ));
        }
        let params = funcs::signature(&def.params, self.tempo)?;
        let names: Vec<&str> = params.iter().map(|p| p.name.name.as_str()).collect();
        let positional = funcs::positional(params.iter().map(Param::required));
        let bound = funcs::bind(&name.name, name.span, &names, positional, args)?;
        let mut values = Vec::with_capacity(params.len());
        for (param, arg) in params.iter().zip(&bound) {
            values.push(match arg {
                Some(arg) => {
                    let value = self.arg_value(param, &arg.value)?;
                    Some((self.snapshot(value, arg.value.span)?, arg.value.span))
                }
                None => None,
            });
        }
        let saved = std::mem::take(&mut self.frame);
        self.frame.scope = captures;
        self.stack.push(def.name.name.clone());
        let result = (|| {
            for (param, given) in params.iter().zip(values) {
                let (v, span) = match given {
                    Some(g) => g,
                    None => {
                        let Some(default) = &param.default else {
                            return Err(Diagnostic::new(
                                format!("`{}` needs an argument `{}`", name.name, param.name.name),
                                name.span,
                            ));
                        };
                        (self.arg_value(param, default)?, default.span)
                    }
                };
                let v = self.check_param(&name.name, param, &v, span)?;
                let v = self.fit_rate(param, v, span)?;
                if let Kind::Choice(options) = &param.kind {
                    self.bind_choices(options);
                }
                self.frame.scope.insert(param.name.name.clone(), v);
            }
            self.body(def)
        })();
        self.stack.pop();
        self.frame = saved;
        result
    }

    /// A parameter's argument: a choice's name is its index.
    pub(super) fn arg_value(&mut self, param: &Param, e: &Expr) -> Res<KVal> {
        if let Kind::Choice(options) = &param.kind {
            return match &e.kind {
                ExprKind::Name(n) if options.contains(n) => {
                    let i = options.iter().position(|o| o == n).expect("the guard checked n is an option");
                    Ok(KVal::Num(self.constant(Unit::Num, i as f64)))
                }
                _ => Err(Diagnostic::new(
                    format!("`{}` is one of {}", param.name.name, options.join(", ")),
                    e.span,
                )),
            };
        }
        self.value(e)
    }

    /// A parameter with a rate sees its value at that rate.
    pub(super) fn fit_rate(&mut self, param: &Param, v: KVal, span: Span) -> Res<KVal> {
        let Some(rate) = param.rate() else {
            return Ok(v);
        };
        match v {
            KVal::Num(value) => Ok(KVal::Num(match rate {
                Rate::Init => self.at_init(value),
                Rate::Control => self.at_control(value, span)?,
                Rate::Const | Rate::Audio => value,
            })),
            other if Self::is_array(&other) => {
                let entries = self.array_elements(other, span)?;
                Ok(KVal::Array(
                    entries
                        .into_iter()
                        .map(|v| self.fit_rate(param, v, span))
                        .collect::<Res<_>>()?,
                ))
            }
            other => Ok(other),
        }
    }
}
