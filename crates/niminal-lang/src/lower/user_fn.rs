//! Calling user-defined functions, kernels and instruments.

use super::*;

impl Lower<'_> {
    pub(super) fn live_array(
        &mut self,
        value: Datum,
        path: &str,
        kind: &PKind,
        rate: Option<Rate>,
        params: &mut Vec<Param>,
        span: Span,
    ) -> Res<Datum> {
        if *kind == PKind::Buffer {
            return Err(Diagnostic::new(
                "use individual buffer parameters instead of a live array of buffers",
                span,
            ));
        }
        match value {
            Datum::Sig(sig) if sig.count() == 1 => {
                let default = sig
                    .konst
                    .ok_or_else(|| Diagnostic::new("array defaults must be constant", span))?
                    as f32;
                let src = self
                    .g
                    .param(path, default)
                    .map_err(|e| Diagnostic::new(e.to_string(), span))?;
                self.param_rates.push(Rate::Audio);
                let range = if let PKind::Range { lo, hi, .. } = kind {
                    Some((*lo, *hi))
                } else {
                    None
                };
                params.push(Param {
                    name: path.into(),
                    unit: sig.unit,
                    default: Some(default),
                    range,
                });
                Ok(Datum::Sig(self.at_rate(Sig::signal(src, sig.unit), rate)))
            }
            value => {
                let entries = self.entries(value, span)?;
                let kind = if let PKind::Array(inner) = kind {
                    inner
                } else {
                    kind
                };
                Ok(Datum::Array(
                    entries
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            self.live_array(v, &format!("{path}[{i}]"), kind, rate, params, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
        }
    }

    /// Check a value against a parameter, and fit it to the parameter's range.
    pub(super) fn check_param(
        &mut self,
        callee: &str,
        p: &PParam,
        v: Datum,
        arg: &Expr,
    ) -> Res<Datum> {
        let name = &p.name.name;
        let span = arg.span;
        let mismatch = |found: &str| {
            Diagnostic::new(
                format!("`{name}` expects {}, found {found}", p.kind.describe()),
                span,
            )
            .with_help(format!("in the call of `{callee}`"))
        };
        let sig = match (&p.kind, v) {
            (PKind::Buffer | PKind::Any, d @ (Datum::Buffer { .. } | Datum::BufferRef { .. })) => {
                return Ok(d);
            }
            (PKind::Buffer, _) => return Err(mismatch("a value")),
            (_, Datum::BufferRef { .. }) => return Err(mismatch("a buffer reference")),
            (PKind::Function | PKind::Any, d @ Datum::Function { .. }) => return Ok(d),
            (PKind::Function, _) => return Err(mismatch("a value")),
            (_, Datum::Function { .. }) => return Err(mismatch("a function")),
            (_, Datum::Record(fields)) => {
                return self.check_param(callee, p, fields[0].1.clone(), arg);
            }
            (PKind::Array(inner), value) => {
                let elements = self.entries(value, span)?;
                let element_param = PParam {
                    kind: *inner.clone(),
                    ..p.clone()
                };
                return Ok(Datum::Array(
                    elements
                        .into_iter()
                        .map(|v| self.check_param(callee, &element_param, v, arg))
                        .collect::<Res<_>>()?,
                ));
            }
            (PKind::Any, d @ (Datum::Array(_) | Datum::Table(_) | Datum::Range { .. })) => {
                return Ok(d);
            }
            (_, Datum::Range { .. }) => return Err(mismatch("a range")),
            (_, Datum::Table(table)) => {
                // Tables are arrays of plain numbers for typed `[num]` parameters.
                if matches!(
                    p.kind,
                    PKind::Unit {
                        unit: Unit::Num,
                        ..
                    }
                ) {
                    return Ok(Datum::Table(table));
                }
                return Err(mismatch("a table"));
            }
            (_, Datum::Buffer { .. }) => return Err(mismatch("a shared buffer")),
            (_, Datum::Array(items)) => {
                // a list where one value is expected: each entry is checked
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.check_param(callee, p, item, arg)?);
                }
                return Ok(Datum::array(out));
            }
            (_, Datum::Sig(s)) => s,
        };
        if let Some(unit) = p.kind.unit() {
            let fits = sig.unit == unit || (unit == Unit::Db && sig.unit == Unit::Num);
            if !fits {
                return Err(self.unit_mismatch(name, unit, sig.unit, arg));
            }
        }
        if p.rate() == Some(Rate::Const) && sig.konst.is_none() {
            return Err(Diagnostic::new(
                format!("`{name}` must be known while compiling"),
                span,
            ));
        }
        if let (PKind::Unit { whole: true, .. }, Some(k)) = (&p.kind, sig.konst)
            && k.fract() != 0.0
        {
            return Err(mismatch(&format!("{k}")));
        }
        let mut sig = sig;
        if let PKind::Range { lo, hi, .. } = p.kind {
            match sig.konst {
                Some(k) if !(lo..=hi).contains(&k) => {
                    return Err(Diagnostic::new(
                        format!("`{name}` is between {lo} and {hi}, but this is {k}"),
                        span,
                    ));
                }
                Some(_) => {}
                None => {
                    let low = Sig::constant(sig.unit, lo);
                    let high = Sig::constant(sig.unit, hi);
                    sig = self.max_sig(&sig, &low, span)?;
                    sig = self.min_sig(&sig, &high, span)?;
                }
            }
        }
        Ok(Datum::Sig(self.at_rate(sig, p.rate())))
    }

    /// The arguments of a call to a user function, worked out in the caller's scope. A choice's name
    /// is its index.
    pub(super) fn fn_args(
        &mut self,
        callee: &Ident,
        params: &[PParam],
        args: &[Arg],
    ) -> Res<BoundFnArgs> {
        let names: Vec<&str> = params.iter().map(|p| p.name.name.as_str()).collect();
        let positional = funcs::positional(params.iter().map(PParam::required));
        let bound = funcs::bind(&callee.name, callee.span, &names, positional, args)?;
        let mut given = Vec::with_capacity(params.len());
        for (p, arg) in params.iter().zip(bound) {
            let Some(arg) = arg else {
                given.push(None);
                continue;
            };
            let mut written = arg.value.clone();
            let v = if let PKind::Choice(options) = &p.kind {
                let option = funcs::pick_choice(&p.name.name, options, &arg.value, &self.chosen)?;
                let i = options.iter().position(|o| *o == option).expect("pick_choice returns an option");
                written.kind = ExprKind::Name(option);
                Datum::Sig(Sig::constant(Unit::Num, i as f64))
            } else {
                self.value(&arg.value)?
            };
            given.push(Some((v, written)));
        }
        Ok(given)
    }

    /// A call of a user function. One without state is lowered in place, its parameters standing for
    /// the arguments; one with state becomes a kernel, compiled for this call.
    pub(super) fn call_fn(
        &mut self,
        def: &FnDef,
        name: &Ident,
        args: &[Arg],
        span: Span,
        instrument: bool,
    ) -> Res<Datum> {
        if self.fn_stack.contains(&def.name.name) {
            let mut path = self.fn_stack.clone();
            path.push(def.name.name.clone());
            return Err(Diagnostic::new(
                format!("functions can't call themselves: {}", path.join(" -> ")),
                name.span,
            ));
        }
        let params = funcs::signature(&def.params, self.tempo)?;
        let given = self.fn_args(name, &params, args)?;
        if !instrument && funcs::has_state(def, &self.names.fns) {
            return self.call_kernel(def, name, &params, given, span);
        }
        let saved_scope = std::mem::take(&mut self.scope);
        let saved_out = self.out.take();
        let saved_in_fn = std::mem::replace(&mut self.in_fn, !instrument);
        self.fn_stack.push(def.name.name.clone());
        let result = self.fn_body(def, name, &params, given);
        self.fn_stack.pop();
        self.scope = saved_scope;
        self.out = saved_out;
        self.in_fn = saved_in_fn;
        result
    }

    pub(super) fn fn_body(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
    ) -> Res<Datum> {
        let results = std::mem::replace(&mut self.fn_results, super::result_names(def));
        let chosen = std::mem::take(&mut self.chosen);
        let body = self.fn_body_with_results(def, name, params, given);
        self.fn_results = results;
        self.chosen = chosen;
        body
    }

    fn fn_body_with_results(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
    ) -> Res<Datum> {
        for (p, g) in params.iter().zip(given) {
            let (v, arg) = match g {
                Some(g) => g,
                None => match &p.default {
                    Some(d) => {
                        let v = if let PKind::Choice(options) = &p.kind {
                            let ExprKind::Name(n) = &d.kind else {
                                unreachable!("checked")
                            };
                            let i = options.iter().position(|o| o == n).expect("the guard checked n is an option");
                            Datum::Sig(Sig::constant(Unit::Num, i as f64))
                        } else {
                            self.value(d)?
                        };
                        (v, d.clone())
                    }
                    None => {
                        return Err(Diagnostic::new(
                            format!("`{}` needs an argument `{}`", name.name, p.name.name),
                            name.span,
                        ));
                    }
                },
            };
            let v = self.check_param(&name.name, p, v, &arg)?;
            if let PKind::Choice(options) = &p.kind {
                if let ExprKind::Name(held) = &arg.kind {
                    self.chosen.insert(p.name.name.clone(), held.clone());
                }
                for (i, o) in options.iter().enumerate() {
                    self.scope
                        .insert(o.clone(), Datum::Sig(Sig::constant(Unit::Num, i as f64)));
                }
            }
            self.scope.insert(p.name.name.clone(), v);
        }
        let body: Vec<&Stmt> = def.body.iter().collect();
        let named = matches!(def.result, FnResult::Named(_));
        let tail = if named {
            for stmt in &body {
                self.stmt(stmt, false)?;
            }
            None
        } else {
            self.run_body_value(&body)?
        };
        if let FnResult::Named(outputs) = &def.result {
            let mut results = Vec::with_capacity(outputs.len());
            for out in outputs {
                let Some(v) = self.scope.get(&out.name).cloned() else {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` is a result of `{}`, but the body never defines it",
                            out.name, def.name.name
                        ),
                        out.span,
                    )
                    .with_help(format!("add a line `{} = ...`", out.name)));
                };
                results.push((out.name.clone(), v));
            }
            return Ok(Datum::Record(results));
        }
        let (value, span) = match (tail, self.out.take()) {
            (Some(_), Some(_)) => {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` sets `out`, so its last line can't also be a value",
                        def.name.name
                    ),
                    def.name.span,
                )
                .with_help("assign it to `out`, or remove the `out` lines"));
            }
            (Some(t), None) => t,
            (None, Some(out)) => (Datum::Sig(out), def.name.span),
            (None, None) if self.sends > 0 && !self.in_fn => {
                (Datum::Sig(Sig::constant(Unit::Num, 0.0)), def.name.span)
            }
            (None, None) => {
                return Err(Diagnostic::new(
                    format!("`{}` must end with the value it gives", def.name.name),
                    def.name.span,
                )
                .with_help("the last line of a function is its result"));
            }
        };
        if let Some(unit) = funcs::result_unit(&def.result)? {
            let found = self.signal_of(value.clone(), span)?.unit;
            if found != unit {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` gives {}, but its result is declared as {}",
                        def.name.name,
                        found.describe(),
                        unit.describe()
                    ),
                    span,
                ));
            }
        }
        Ok(value)
    }

    /// A function with state, compiled to a kernel for this call: one copy per channel of its widest
    /// signal argument, each delivering all of the function's results.
    pub(super) fn call_kernel(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
        span: Span,
    ) -> Res<Datum> {
        self.call_scoped_kernel(def, name, params, given, span, None)
    }

    pub(super) fn call_scoped_kernel(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
        span: Span,
        dsp_scope: Option<DspScope>,
    ) -> Res<Datum> {
        if self.mode == Mode::Plain {
            return Err(Diagnostic::new(
                format!(
                    "`{}` has state, so it runs while playing, not while compiling",
                    name.name
                ),
                name.span,
            ));
        }
        // A kernel can't see the file's `ctl`s, so the ones its body reads go in as inputs, which includes
        // the connection's tempo for a body that reads `clock.beat`.
        let mut def = def.clone();
        let mut params = params.to_vec();
        let mut given = given;
        let mut injected = 0;
        for control in self.names.controls.clone() {
            let shadowed = def.params.iter().any(|p| p.name.name == control.name)
                || self.scope.contains_key(&control.name);
            if shadowed
                || funcs::declares(&def, &control.name)
                || !(funcs::reads(&def, &control.name) || funcs::defaults_read(&def, &control.name))
            {
                continue;
            }
            let id = Ident {
                name: control.name.clone(),
                span,
            };
            // first, so a parameter default that reads the control can see it
            def.params.insert(
                injected,
                ParamDef {
                    name: id.clone(),
                    rate: None,
                    ty: None,
                    default: None,
                },
            );
            params.insert(
                injected,
                PParam {
                    name: id.clone(),
                    rate: None,
                    kind: PKind::Any,
                    default: None,
                },
            );
            let value = Datum::Sig(self.name(&control.name, span)?);
            given.insert(
                injected,
                Some((
                    value,
                    Expr {
                        kind: ExprKind::Name(control.name.clone()),
                        span,
                    },
                )),
            );
            injected += 1;
        }
        let (def, params) = (&def, &params[..]);
        let mut kargs = Vec::with_capacity(params.len());
        let mut wires: Vec<(String, Sig)> = Vec::new();
        let mut width = 1;
        let mut layout = Layout::Mono;
        for (p, g) in params.iter().zip(given) {
            let Some((v, arg)) = g else {
                kargs.push(KArg::Missing);
                continue;
            };
            let at = arg.span;
            let v = self.check_param(&name.name, p, v, &arg)?;
            kargs.push(self.kernel_arg(
                v,
                &p.name.name,
                &mut wires,
                &mut width,
                &mut layout,
                at,
            )?);
        }
        if dsp_scope.is_some() {
            // Shared reads hidden in a subvoice still participate in cross-node ordering.
            let mut pending = vec![def.clone()];
            let mut visited = std::collections::HashSet::new();
            while let Some(body) = pending.pop() {
                if !visited.insert(body.name.name.clone()) {
                    continue;
                }
                for (name, (id, _)) in &self.names.buffers {
                    if funcs::reads(&body, name) {
                        self.g.touch_buffer_read(*id);
                    }
                }
                for (name, nested) in &self.names.fns {
                    if funcs::reads(&body, name) {
                        pending.push(nested.clone());
                    }
                }
                for (name, nested) in &self.names.instrs {
                    if funcs::reads(&body, name) {
                        pending.push(FnDef {
                            name: nested.name.clone(),
                            params: nested.params.clone(),
                            body: nested.body.clone(),
                            result: FnResult::Inferred,
                            span: nested.span,
                        });
                    }
                }
            }
        }
        let (kernel, shape) = if dsp_scope.is_some() {
            crate::kernel::compile_scoped_kernel(
                def,
                &kargs,
                self.registry,
                self.names,
                self.tempo,
                true,
                matches!(dsp_scope, Some(DspScope::Control)),
            )?
        } else {
            crate::kernel::compile_kernel(def, &kargs, self.registry, self.names, self.tempo)?
        };
        let kernel = Arc::new(kernel);
        let outputs = kernel.outputs.len();
        let mut per_output: Vec<Vec<Src>> = vec![Vec::with_capacity(width); outputs];
        for channel in 0..width {
            let wired: Vec<(&str, Src)> = wires
                .iter()
                .map(|(n, s)| (n.as_str(), s.pick(channel)))
                .collect();
            let mut wired = wired;
            let op: Box<dyn niminal_engine::Opcode> = match &dsp_scope {
                Some(DspScope::Spawn {
                    max,
                    trigger,
                    duration,
                }) => {
                    wired.push(("__trigger", trigger.srcs[0]));
                    wired.push(("__duration", duration.srcs[0]));
                    Box::new(niminal_engine::ops::Spawn::new(
                        Box::new(Custom::new(kernel.clone())),
                        *max,
                    ))
                }
                Some(DspScope::Oversample(factor)) => {
                    Box::new(niminal_engine::ops::Oversample::new(
                        Box::new(Custom::new(kernel.clone())),
                        *factor,
                    ))
                }
                Some(DspScope::Control) => Box::new(niminal_engine::ops::ControlRate::new(
                    Box::new(Custom::new(kernel.clone())),
                )),
                None => Box::new(Custom::new(kernel.clone())),
            };
            let results = self
                .g
                .add_outputs(op, &wired)
                .map_err(|e| Diagnostic::new(e.to_string(), span))?;
            for (k, src) in results.into_iter().take(outputs).enumerate() {
                if matches!(dsp_scope, Some(DspScope::Control)) {
                    self.note_rate(src, Rate::Control);
                }
                per_output[k].push(src);
            }
        }
        Ok(Self::kernel_result(shape, &per_output, layout))
    }

    pub(super) fn kernel_arg(
        &mut self,
        value: Datum,
        path: &str,
        wires: &mut Vec<(String, Sig)>,
        width: &mut usize,
        layout: &mut Layout,
        span: Span,
    ) -> Res<KArg> {
        match value {
            Datum::BufferRef { selector, choices } => {
                let selector =
                    self.kernel_arg(Datum::Sig(selector), path, wires, width, layout, span)?;
                for (name, _) in choices.iter() {
                    self.g.touch_buffer_read(self.names.buffers[name].0);
                }
                Ok(KArg::Buffer {
                    selector: Box::new(selector),
                    choices,
                })
            }
            Datum::Function { def, captures } => {
                let mut entries: Vec<_> = captures.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                let captures = entries
                    .into_iter()
                    .map(|(n, v)| {
                        let arg = self.kernel_arg(
                            v,
                            &format!("{path}::{n}"),
                            wires,
                            width,
                            layout,
                            span,
                        )?;
                        Ok((n, arg))
                    })
                    .collect::<Res<_>>()?;
                Ok(KArg::Function { def, captures })
            }
            Datum::Sig(s) => {
                if let Some(value) = s.konst {
                    return Ok(KArg::Const {
                        unit: s.unit,
                        value,
                    });
                }
                if s.count() > 1 {
                    if *width > 1 && s.count() != *width {
                        return Err(Diagnostic::new(
                            "function arguments have different channel counts",
                            span,
                        ));
                    }
                    *width = s.count();
                    *layout = s.layout;
                }
                let arg = KArg::Port {
                    unit: s.unit,
                    rate: self.sig_rate(&s),
                };
                wires.push((path.to_string(), s));
                Ok(arg)
            }
            Datum::Table(table) => Ok(KArg::Table {
                unit: Unit::Num,
                values: table,
            }),
            Datum::Buffer { id, data } => {
                self.g.touch_buffer_read(id);
                Ok(KArg::Buffer {
                    selector: Box::new(KArg::Const {
                        unit: Unit::Num,
                        value: 0.0,
                    }),
                    choices: Choices::new(vec![(path.to_string(), data)]),
                })
            }
            Datum::Array(items) => {
                // Constant numeric arrays retain shared table storage.
                let numbers: Option<Vec<_>> = items
                    .iter()
                    .map(|v| match v {
                        Datum::Sig(s) => s.konst.map(|k| (s.unit, k)),
                        _ => None,
                    })
                    .collect();
                if let Some(values) = numbers
                    && !values.is_empty()
                    && values.iter().all(|(u, _)| *u == values[0].0)
                {
                    return Ok(KArg::Table {
                        unit: values[0].0,
                        values: Arc::new(Table::from_f64(values.into_iter().map(|(_, v)| v), None)),
                    });
                }
                Ok(KArg::Array(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            self.kernel_arg(v, &format!("{path}[{i}]"), wires, width, layout, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
            Datum::Range { lo, hi, inclusive } => match (lo.konst, hi.konst) {
                (Some(lo_value), Some(hi_value)) => Ok(KArg::Range {
                    unit: lo.unit,
                    lo: lo_value,
                    hi: hi_value,
                    inclusive,
                }),
                _ => Err(Diagnostic::new(
                    "a range given to a function must be known while compiling",
                    span,
                )),
            },
            Datum::Record(fields) => {
                self.kernel_arg(fields[0].1.clone(), path, wires, width, layout, span)
            }
        }
    }

    pub(super) fn kernel_result(
        shape: KernelResult,
        sources: &[Vec<Src>],
        layout: Layout,
    ) -> Datum {
        match shape {
            KernelResult::Scalar { index, unit } => Datum::Sig(Sig {
                srcs: sources[index].clone().into(),
                layout: if sources[index].len() == 1 {
                    Layout::Mono
                } else {
                    layout
                },
                unit,
                konst: None,
            }),
            KernelResult::Array(items) => Datum::Array(
                items
                    .into_iter()
                    .map(|v| Self::kernel_result(v, sources, layout))
                    .collect(),
            ),
            KernelResult::Record(fields) => Datum::Record(
                fields
                    .into_iter()
                    .map(|(name, v)| (name, Self::kernel_result(v, sources, layout)))
                    .collect(),
            ),
        }
    }

    /// An instrument played inside another: its body runs as part of this voice. `gain:` is the
    /// note's gain, applied after it, as for any note.
    pub(super) fn call_instr(
        &mut self,
        def: &InstrDef,
        name: &Ident,
        args: &[Arg],
        span: Span,
    ) -> Res<Datum> {
        if self.mode != Mode::Instrument || self.in_fn {
            return Err(Diagnostic::new(
                format!(
                    "`{}` is an instrument, so it can only be played inside another instrument",
                    name.name
                ),
                name.span,
            ));
        }
        let declares_gain = def.params.iter().any(|p| p.name.name == GAIN_PARAM);
        let (gain, rest): (Vec<&Arg>, Vec<&Arg>) = args
            .iter()
            .partition(|a| !declares_gain && a.name.as_ref().is_some_and(|n| n.name == GAIN_PARAM));
        let rest: Vec<Arg> = rest.into_iter().cloned().collect();
        let as_fn = FnDef {
            name: def.name.clone(),
            params: def.params.clone(),
            result: FnResult::Inferred,
            body: def.body.clone(),
            span: def.span,
        };
        let result = self.call_fn(&as_fn, name, &rest, span, true)?;
        let sig = self.signal_of(result, span)?;
        match gain.first() {
            Some(g) => {
                let gain = self.expr(&g.value)?;
                if !matches!(gain.unit, Unit::Db | Unit::Num) {
                    return Err(Diagnostic::new(
                        format!(
                            "`gain` expects db or a plain factor, found {}",
                            gain.unit.describe()
                        ),
                        g.value.span,
                    ));
                }
                self.mul(&sig, &gain, Unit::Num, span).map(Datum::Sig)
            }
            None => Ok(Datum::Sig(sig)),
        }
    }

    // ---- built-in functions ----------------------------------------------
}
