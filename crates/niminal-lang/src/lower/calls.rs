//! Calls: ranges, `spawn` scopes and built-in opcode dispatch.

use super::*;

impl Lower<'_> {
    /// `x.range(200hz..4khz)`: a plain signal from 0 to 1 (a control, a knob) mapped to a range. Decibels
    /// move evenly in db, everything else evenly in its own unit.
    pub(super) fn range(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`range` takes a range", name.span));
        };
        if range.name.is_some() {
            return Err(Diagnostic::new(
                "`range` takes its range without a name",
                range.value.span,
            ));
        }
        let value = self.value(&x.value)?;
        let Datum::Range { lo, hi, .. } = self.value(&range.value)? else {
            return Err(Diagnostic::new(
                "`range` takes a range, such as `200hz..4khz`",
                range.value.span,
            ));
        };
        self.map_value(value, span, |this, x| {
            if x.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "`range` maps a plain signal from 0 to 1, but this is {}",
                        x.unit.describe()
                    ),
                    span,
                ));
            }
            if lo.unit == Unit::Db {
                let (Some(a), Some(b)) = (lo.konst, hi.konst) else {
                    return Err(Diagnostic::new(
                        "the ends of a db range must be constants",
                        span,
                    ));
                };
                let exponent =
                    this.mul(&x, &Sig::constant(Unit::Num, (b / a).ln()), Unit::Num, span)?;
                let grown = this.func(Fn1::Exp, &exponent);
                return this.mul(&grown, &Sig::constant(Unit::Num, a), Unit::Db, span);
            }
            let width = this.binary(BinOp::Sub, hi.clone(), lo.clone(), span)?;
            let scaled = this.binary(BinOp::Mul, x, width, span)?;
            this.binary(BinOp::Add, scaled, lo.clone(), span)
        })
    }

    pub(super) fn spawn_scope(&mut self, args: &[Arg], span: Span) -> Res<Datum> {
        let usage = || Diagnostic::new("write `spawn(trigger, dur: 100ms, max: 8) { ... }`", span);
        let Some((body, options)) = args.split_last() else {
            return Err(usage());
        };
        let ExprKind::Lambda { params, body } = &body.value.kind else {
            return Err(usage());
        };
        if !params.is_empty() {
            return Err(usage());
        }
        let Some((trigger, options)) = options.split_first() else {
            return Err(usage());
        };
        if trigger.name.is_some() {
            return Err(usage());
        }
        let trigger = self.expr(&trigger.value)?;
        if trigger.unit != Unit::Num || trigger.count() != 1 {
            return Err(usage());
        }
        let mut duration = None;
        let mut max = None;
        for option in options {
            match option.name.as_ref().map(|n| n.name.as_str()) {
                Some("dur") if duration.is_none() => {
                    let value = self.expr(&option.value)?;
                    if value.unit != Unit::Time || value.count() != 1 {
                        return Err(usage());
                    }
                    if value
                        .konst
                        .is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 600.0)
                    {
                        return Err(Diagnostic::new(
                            "spawn duration is above 0 and at most 600sec",
                            option.value.span,
                        ));
                    }
                    duration = Some(value);
                }
                Some("max") if max.is_none() => {
                    let value = self.expr(&option.value)?;
                    let Some(v) = value.konst.filter(|v| {
                        value.unit == Unit::Num && v.fract() == 0.0 && (1.0..=64.0).contains(v)
                    }) else {
                        return Err(Diagnostic::new(
                            "spawn max is a constant whole number from 1 to 64",
                            option.value.span,
                        ));
                    };
                    max = Some(v as usize);
                }
                _ => return Err(usage()),
            }
        }
        let duration = duration.ok_or_else(usage)?;
        let (def, params, given) = self.scope_kernel(body, span);
        self.call_scoped_kernel(
            &def,
            &def.name,
            &params,
            given,
            span,
            Some(DspScope::Spawn {
                max: max.unwrap_or(8),
                trigger,
                duration,
            }),
        )
    }

    /// Make free variables explicit ports; each fx decides when to read them.
    pub(super) fn scope_kernel(
        &mut self,
        body: &[Stmt],
        span: Span,
    ) -> (FnDef, Vec<PParam>, BoundFnArgs) {
        let mut def = funcs::lambda(&[], body, span);
        let capture_in =
            self.mode == Mode::Node && !self.scope.contains_key("in") && funcs::reads(&def, "in");
        if capture_in {
            let input = self.node_input();
            self.scope.insert("in".into(), Datum::Sig(input));
        }
        let mut captures: Vec<_> = self
            .scope
            .iter()
            .filter(|(n, _)| funcs::reads(&def, n))
            .collect();
        captures.sort_by(|a, b| a.0.cmp(b.0));
        let mut params = Vec::new();
        let mut given = Vec::new();
        for (name, value) in captures {
            let id = Ident {
                name: name.clone(),
                span,
            };
            def.params.push(ParamDef {
                name: id.clone(),
                rate: None,
                ty: None,
                default: None,
            });
            params.push(PParam {
                name: id,
                rate: None,
                kind: PKind::Any,
                default: None,
            });
            given.push(Some((
                value.clone(),
                Expr {
                    kind: ExprKind::Name(name.clone()),
                    span,
                },
            )));
        }
        if capture_in {
            self.scope.remove("in");
        }
        (def, params, given)
    }

    pub(super) fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        let n = name.name.as_str();
        if let Some(receiver) = args.first().filter(|a| a.receiver)
            && let ExprKind::Name(kit) = &receiver.value.kind
            && let Some(sampler) = self.names.samplers.get(kit)
            && sampler.instrument.members.iter().any(|member| member == n)
        {
            let mut selected = args[1..].to_vec();
            selected.push(Arg {
                name: Some(Ident {
                    name: "sample".into(),
                    span: name.span,
                }),
                value: Expr {
                    kind: ExprKind::Name(n.into()),
                    span: name.span,
                },
                receiver: false,
            });
            return self.call_sampler(sampler, name, &selected);
        }
        if let Some(sampler) = self.names.samplers.get(n) {
            return self.call_sampler(sampler, name, args);
        }
        if n == "control"
            && let [body] = args
            && !body.receiver
            && let ExprKind::Lambda { params, body } = &body.value.kind
        {
            if !params.is_empty() {
                return Err(Diagnostic::new("a control body takes no arguments", span));
            }
            let (def, params, given) = self.scope_kernel(body, span);
            return self.call_scoped_kernel(
                &def,
                &def.name,
                &params,
                given,
                span,
                Some(DspScope::Control),
            );
        }
        if n == "oversample" {
            let [factor, body] = args else {
                return Err(Diagnostic::new("write `oversample(4) { ... }`", span));
            };
            let value = self.expr(&factor.value)?;
            let Some(factor) = value
                .konst
                .filter(|f| value.unit == Unit::Num && [2.0, 4.0, 8.0].contains(f))
            else {
                return Err(Diagnostic::new(
                    "oversample factor is 2, 4 or 8",
                    factor.value.span,
                ));
            };
            let ExprKind::Lambda { params, body } = &body.value.kind else {
                return Err(Diagnostic::new("oversample needs a body", span));
            };
            if !params.is_empty() {
                return Err(Diagnostic::new("oversample body takes no arguments", span));
            }
            let (def, params, given) = self.scope_kernel(body, span);
            return self.call_scoped_kernel(
                &def,
                &def.name,
                &params,
                given,
                span,
                Some(DspScope::Oversample(factor as usize)),
            );
        }
        if n == "spawn" {
            return self.spawn_scope(args, span);
        }
        if n == "reset_on" {
            let (trigger, body) = funcs::reset_args(args, span)?;
            if self.mode == Mode::Plain {
                return Err(Diagnostic::new("reset scopes run while playing", span));
            }
            let trig = self.expr(trigger)?;
            if trig.unit != Unit::Num || trig.count() != 1 {
                return Err(Diagnostic::new(
                    "a reset trigger is one plain signal",
                    trigger.span,
                ));
            }
            let first = self.g.node_count();
            let def = funcs::lambda(&[], body, span);
            let id = def.name.clone();
            if !funcs::has_state(&def, &self.names.fns) {
                let saved_scope = self.scope.clone();
                let out = self.out.take();
                let result = self.fn_body(&def, &id, &[], Vec::new());
                self.scope = saved_scope;
                self.out = out;
                self.g.reset_since(first, trig.srcs[0]);
                return result;
            }
            let captures = self
                .scope
                .iter()
                .filter(|(n, _)| funcs::reads(&def, n))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            self.scope
                .insert(id.name.clone(), Datum::Function { def, captures });
            let result = self.call(&id, &[], span);
            self.scope.remove(&id.name);
            let result = result?;
            self.g.reset_since(first, trig.srcs[0]);
            return Ok(result);
        }
        // `f.band`: one result of a function that has several
        if let [receiver] = args
            && receiver.receiver
            && !self.names.fns.contains_key(n)
            && let Some(record) = self.record(&receiver.value)?
        {
            return record
                .iter()
                .find(|(o, _)| o == n)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| {
                    let names: Vec<&str> = record.iter().map(|(o, _)| o.as_str()).collect();
                    Diagnostic::new(format!("this has no result `{n}`"), name.span)
                        .with_help(format!("its results are {}", names.join(", ")))
                });
        }
        // `note.time`: the running voice
        if let [receiver] = args
            && receiver.receiver
            && matches!(&receiver.value.kind, ExprKind::Name(r) if r == "note" && !self.scope.contains_key("note"))
        {
            return self.note_context(name).map(Datum::Sig);
        }
        if let [receiver] = args
            && receiver.receiver
            && matches!(&receiver.value.kind, ExprKind::Name(r) if r == "clock" && !self.scope.contains_key("clock"))
        {
            return self.clock_context(name).map(Datum::Sig);
        }
        if let Some(Datum::Function { mut def, captures }) = self.scope.get(n).cloned() {
            let params = funcs::signature(&def.params, self.tempo)?;
            let mut given = self.fn_args(name, &params, args)?;
            let mut params = params;
            let mut captures: Vec<_> = captures.into_iter().collect();
            captures.sort_by(|a, b| a.0.cmp(&b.0));
            for (n, v) in captures {
                let id = Ident { name: n, span };
                def.params.push(ParamDef {
                    name: id.clone(),
                    rate: None,
                    ty: None,
                    default: None,
                });
                params.push(PParam {
                    name: id.clone(),
                    rate: None,
                    kind: PKind::Any,
                    default: None,
                });
                given.push(Some((
                    v,
                    Expr {
                        kind: ExprKind::Name(id.name),
                        span,
                    },
                )));
            }
            if self.fn_stack.contains(&def.name.name) {
                return Err(Diagnostic::new(
                    "functions can't call themselves",
                    name.span,
                ));
            }
            if funcs::has_state(&def, &self.names.fns) {
                return self.call_kernel(&def, name, &params, given, span);
            }
            let saved = std::mem::take(&mut self.scope);
            let out = self.out.take();
            let in_fn = std::mem::replace(&mut self.in_fn, true);
            self.fn_stack.push(def.name.name.clone());
            let result = self.fn_body(&def, name, &params, given);
            self.fn_stack.pop();
            self.scope = saved;
            self.out = out;
            self.in_fn = in_fn;
            return result;
        }
        if self.names.failed_fns.iter().any(|f| f == n) {
            // already reported where it is defined; this stops the caller without a second report
            return Err(Diagnostic::quiet(name.span));
        }
        if let Some(def) = self.names.fns.get(n) {
            let def = def.clone();
            return self.call_fn(&def, name, args, span, false);
        }
        if let Some(def) = self.names.instrs.get(n) {
            let def = def.clone();
            return self.call_instr(&def, name, args, span);
        }
        if let Some(f) = opcodes::math_fn(n) {
            let [arg] = args else {
                return Err(Diagnostic::new(format!("`{n}` takes one value"), name.span));
            };
            if arg.name.is_some() {
                return Err(Diagnostic::new(
                    format!("`{n}` takes its value without a name"),
                    arg.value.span,
                ));
            }
            let value = self.value(&arg.value)?;
            return self.math_value(f, value, arg.value.span);
        }
        // `x.lag(time)` and `x.smooth(time)` keep the unit of what they smooth: `cutoff.smooth(5ms)` is still hz
        if n == "lag"
            && args.len() == 2
            && args[0].receiver
            && let Datum::Sig(sig) = self.value(&args[0].value)?
            && sig.unit != Unit::Num
        {
            let unit = sig.unit;
            let hidden = "__lag_input";
            self.scope.insert(
                hidden.into(),
                Datum::Sig(Sig {
                    unit: Unit::Num,
                    ..sig
                }),
            );
            let mut plain = args.to_vec();
            plain[0].value = Expr {
                kind: ExprKind::Name(hidden.into()),
                span: args[0].value.span,
            };
            let smoothed = self.call(name, &plain, span);
            self.scope.remove(hidden);
            return self.map_value(smoothed?, span, move |_, s| Ok(Sig { unit, ..s }));
        }
        match n {
            "map" | "sum" | "len" | "channels" | "indices" | "at" | "phase" | "step" | "active"
            | "normalize" | "reverse" | "rotate" | "resample" | "concat" | "integrate" | "mix"
            | "slice" | "spectrum" | "phases" | "mean" | "rms" | "remove_dc" | "moving_average"
            | "fade" => {
                return self.array_call(name, args, span);
            }
            // `x.frames(size, fn(w) { ... })` belongs to signals
            "frames" if args.len() == 2 && args[0].receiver => {
                return self.array_call(name, args, span);
            }
            "record" => return self.buffer_record(name, args, span),
            "data" if args.len() == 1 && args[0].receiver => {
                return self.sample_data(name, &args[0].value);
            }
            "min" | "max" if args.len() == 1 && args[0].receiver => {
                return self.array_call(name, args, span);
            }
            "split" if args.len() == 1 && args[0].receiver => {
                return Err(Diagnostic::new(
                    "`split` without arguments is now `channels`",
                    name.span,
                )
                .with_help("write `signal.channels` for an array of its channels"));
            }
            "min" | "max" | "pow" | "mod" | "atan2" => return self.combine(name, args),
            "clamp" => return self.clamp(name, args),
            "hold" | "smooth" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                let smooth = n == "smooth";
                return self.map_value(value, span, move |this, v| {
                    if this.sig_rate(&v) > Rate::Control {
                        return Err(Diagnostic::new(
                            format!("`{n}` is for values that change at most once per block"),
                            span,
                        )
                        .with_help(
                            "compute it in a `control { ... }` scope, or read it with `.control`",
                        ));
                    }
                    if !smooth || v.konst.is_some() {
                        return Ok(v);
                    }
                    let srcs: Array<Src> = v
                        .srcs
                        .iter()
                        .map(|&x| {
                            this.g
                                .add(niminal_engine::ops::Ramp::default(), &[("x", x)])
                                .expect("ramp ports")
                        })
                        .collect();
                    Ok(Sig { srcs, ..v })
                });
            }
            "smooth" if args.len() == 2 && args[0].receiver => {
                // `x.smooth(5ms)` is a one-pole lag
                let mut lag = args.to_vec();
                if lag[1].name.is_none() {
                    lag[1].name = Some(Ident {
                        name: "time".into(),
                        span: name.span,
                    });
                }
                let lag_name = Ident {
                    name: "lag".into(),
                    span: name.span,
                };
                return self.call(&lag_name, &lag, span);
            }
            "init" | "control" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                let rate = if n == "init" {
                    Rate::Init
                } else {
                    Rate::Control
                };
                return self.map_value(value, span, |this, v| Ok(this.at_rate(v, Some(rate))));
            }
            "zeros" => return self.zeros(name, args),
            "harmonics" => return self.harmonics(name, args),
            "partials" => return self.partials(name, args),
            "cosines" => return self.cosines(name, args),
            "curve" => return self.curve(name, args),
            "steps" => return self.steps(name, args),
            "poly" => return self.poly(name, args),
            "ramp" => return self.ramp(name, args),
            "cheby" => return self.cheby(name, args),
            "window" => return self.window(name, args),
            "random" => return self.random_table(name, args),
            "file" => return self.file_table(name, args),
            "bank" => return self.bank(name, args),
            _ => {}
        }
        if let Some(v) = self.unit_method(name, args)? {
            return Ok(v);
        }
        self.call_builtin(name, args, span)
    }

    pub(super) fn call_builtin(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        if name.name == "prev" {
            return self.prev(name, args).map(Datum::Sig);
        }
        if name.name == "late" {
            return self.late(name, args).map(Datum::Sig);
        }
        if name.name == "range" && self.registry.find("range").is_none() {
            return self.range(name, args, span);
        }
        if name.name == "input" && self.registry.find("input").is_none() {
            return self.audio_input(name, args, span).map(Datum::Sig);
        }
        if name.name == "report" {
            return self.report(name, args);
        }
        if name.name == "osc_out" && self.registry.find("osc_out").is_none() {
            return self.osc_out(name, args).map(Datum::Sig);
        }
        if name.name == "to" {
            return Err(Diagnostic::new(
                "`to` can only end the line that sets a node's `out`",
                name.span,
            )
            .with_help("for example `out = in.lpf(cutoff: 1khz).to(space)`"));
        }
        let registry = self.registry;
        let Some(spec) = registry.find(&name.name) else {
            let known: Vec<&str> = registry
                .names()
                .chain(self.names.fns.keys().map(String::as_str))
                .chain(self.names.instrs.keys().map(String::as_str))
                .chain([
                    "min",
                    "max",
                    "pow",
                    "atan2",
                    "mod",
                    "clamp",
                    "prev",
                    "late",
                    "range",
                    "zeros",
                    "harmonics",
                    "partials",
                    "cosines",
                    "curve",
                    "steps",
                    "poly",
                    "ramp",
                    "cheby",
                    "window",
                    "random",
                    "file",
                    "bank",
                ])
                .collect();
            let mut d = Diagnostic::new(format!("unknown function `{}`", name.name), name.span);
            if let Some(c) = closest(&name.name, known) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let op = spec.name.as_str();

        // Match arguments to parameters.
        let names: Vec<&str> = spec.params.iter().map(|p| p.name.as_str()).collect();
        let bound = funcs::bind(op, name.span, &names, spec.positional, args)?;

        // Evaluate each argument according to what the parameter wants.
        let mut vals: Vec<Option<Val>> = Vec::with_capacity(spec.params.len());
        for (param, arg) in spec.params.iter().zip(bound) {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(
                        format!("`{op}` needs an argument `{}`", param.name),
                        span,
                    ));
                }
                vals.push(None);
                continue;
            };
            vals.push(Some(match param.kind {
                Kind::Wave => Val::Wave(self.wave_arg(&arg.value)?),
                Kind::Layout => Val::Layout(layout_from_expr(&arg.value)?),
                Kind::Table => Val::Tables(self.table_arg(&arg.value, true)?),
                Kind::Signal(_) | Kind::Array(_) | Kind::Gain => {
                    let value = self.value(&arg.value)?;
                    self.builtin_value(value, param, &arg.value)?
                }
            }));
        }

        self.dispatch_arrays(spec, &vals, span)
    }

    /// One table, or an array of them (one per channel), known while compiling.
    pub(super) fn table_arg(&mut self, e: &Expr, nested: bool) -> Res<Vec<Arc<Table>>> {
        let tables = match self.value(e)? {
            Datum::Array(items)
                if !items.is_empty()
                    && items.iter().all(|i| {
                        matches!(i, Datum::Table(_)) || (nested && matches!(i, Datum::Array(_)))
                    }) =>
            {
                items
                    .into_iter()
                    .map(|item| self.as_table(item, e.span))
                    .collect::<Res<Vec<_>>>()?
            }
            other => vec![self.as_table(other, e.span)?],
        };
        if tables.iter().any(|t| t.is_empty()) {
            return Err(Diagnostic::new("a table can't be empty", e.span));
        }
        Ok(tables)
    }

    /// A waveform name, or a table of plain numbers known while compiling that holds one cycle.
    pub(super) fn wave_arg(&mut self, e: &Expr) -> Res<opcodes::WaveArg> {
        if let ExprKind::Name(n) = &e.kind
            && let Some(w) = opcodes::wave(n)
        {
            return Ok(opcodes::WaveArg::Builtin(w));
        }
        let not_a_wave = || {
            let names: Vec<_> = opcodes::WAVES.iter().map(|(n, _)| *n).collect();
            Diagnostic::new("expected a waveform or a table", e.span).with_help(format!(
                "one of {}, or an array of numbers",
                names.join(", ")
            ))
        };
        let tables = match self.table_arg(e, false) {
            Err(_) if matches!(e.kind, ExprKind::Name(_)) => return Err(not_a_wave()),
            tables => tables?,
        };
        opcodes::WaveArg::from_tables(&tables).map_err(|m| Diagnostic::new(m, e.span))
    }

    pub(super) fn builtin_value(
        &self,
        value: Datum,
        param: &opcodes::ParamSpec,
        arg: &Expr,
    ) -> Res<Val> {
        match value {
            Datum::Function { .. } => Err(Diagnostic::new(
                "this argument needs a numeric value",
                arg.span,
            )),
            Datum::Table(_) | Datum::Buffer { .. } | Datum::BufferRef { .. } => {
                Err(Diagnostic::new(
                    "pass a table as the wave of `osc`, not as a signal argument",
                    arg.span,
                ))
            }
            Datum::Array(items) => Ok(Val::Array(
                items
                    .into_iter()
                    .map(|v| self.builtin_value(v, param, arg))
                    .collect::<Res<_>>()?,
            )),
            Datum::Record(fields) => self.builtin_value(fields[0].1.clone(), param, arg),
            Datum::Sig(sig) => {
                if let Some(expected) = call::unit_expected(param.kind, sig.unit) {
                    if param.kind == Kind::Gain {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` expects {}, found {}",
                                param.name,
                                call::expectation(param.kind, expected),
                                sig.unit.describe()
                            ),
                            arg.span,
                        ));
                    }
                    return Err(self.unit_mismatch(&param.name, expected, sig.unit, arg));
                }
                Ok(Val::Sig(sig))
            }
            Datum::Range { .. } => Err(Diagnostic::new("a range isn't a numeric array", arg.span)),
        }
    }

    pub(super) fn dispatch_arrays(
        &mut self,
        spec: &opcodes::OpSpec,
        vals: &[Option<Val>],
        span: Span,
    ) -> Res<Datum> {
        if spec.params.iter().any(|p| matches!(p.kind, Kind::Array(_))) {
            let vals: Vec<_> = spec
                .params
                .iter()
                .zip(vals)
                .map(|(p, v)| match v {
                    Some(Val::Sig(sig)) if matches!(p.kind, Kind::Array(_)) && sig.count() > 1 => {
                        Some(Val::Array(
                            (0..sig.count())
                                .map(|i| Val::Sig(Self::element(sig, i)))
                                .collect(),
                        ))
                    }
                    other => other.clone(),
                })
                .collect();
            let (packed, vals) = call::packed(spec, &vals)
                .map_err(|m| Diagnostic::new(m, span))?
                .expect("array parameters are packed");
            return self.dispatch_arrays(&packed, &vals, span);
        }
        if spec.name == "mverb" {
            let vals: Vec<_> = vals
                .iter()
                .map(|v| match v {
                    Some(Val::Sig(sig)) if sig.count() > 1 => Some(Val::Array(
                        (0..sig.count())
                            .map(|i| Val::Sig(Self::element(sig, i)))
                            .collect(),
                    )),
                    other => other.clone(),
                })
                .collect();
            let vals = call::mverb_args(&vals).map_err(|m| Diagnostic::new(m, span))?;
            return self.expand(spec, &vals, span).map(Datum::Sig);
        }
        if !vals.iter().any(|v| matches!(v, Some(Val::Array(_)))) {
            let sig = self.dispatch(spec, vals, span)?;
            let monitor = spec.name == "granular"
                && spec.params.iter().zip(vals).any(|(p, v)| {
                    p.name == "monitor" && matches!(v, Some(Val::Sig(s)) if s.konst == Some(1.0))
                });
            if monitor {
                let fields = niminal_engine::ops::GRANULAR_MONITOR_FIELDS;
                let channels = sig.count() - fields.len();
                let audio = if channels == 1 {
                    Self::element(&sig, 0)
                } else {
                    Sig::channels(sig.srcs[..channels].to_vec(), Layout::from_count(channels))
                };
                let mut result = vec![("audio".into(), Datum::Sig(audio))];
                result.extend(fields.iter().enumerate().map(|(i, name)| {
                    (
                        name.to_string(),
                        Datum::Sig(Self::element(&sig, channels + i)),
                    )
                }));
                return Ok(Datum::Record(result));
            }
            return Ok(Datum::Sig(sig));
        }
        // These operations consume audio channel layouts, rather than mapping
        // a scalar DSP operation over the array.
        if matches!(spec.build, Build::Pan | Build::ToLayout) {
            let vals = vals
                .iter()
                .map(|v| match v {
                    Some(Val::Array(items)) => {
                        let entries = items
                            .iter()
                            .map(|item| match item {
                                Val::Sig(sig) => Ok((sig.clone(), span)),
                                _ => Err(Diagnostic::new(
                                    "audio channels must be a one-dimensional numeric array",
                                    span,
                                )),
                            })
                            .collect::<Res<Vec<_>>>()?;
                        Ok(Some(Val::Sig(join_channels(&entries, span)?)))
                    }
                    other => Ok(other.clone()),
                })
                .collect::<Res<Vec<_>>>()?;
            return self.dispatch(spec, &vals, span).map(Datum::Sig);
        }
        let layout = vals.iter().find_map(|v| match v {
            Some(Val::Sig(signal)) if signal.count() > 1 => Some(signal.layout),
            _ => None,
        });
        let vals: Vec<_> = vals
            .iter()
            .map(|v| match v {
                Some(Val::Sig(sig)) if sig.count() > 1 => Some(Val::Array(
                    (0..sig.count())
                        .map(|i| Val::Sig(Self::element(sig, i)))
                        .collect(),
                )),
                other => other.clone(),
            })
            .collect();
        let rows = call::rows(&vals)
            .map_err(|m| Diagnostic::new(m, span))?
            .expect("a call with an array argument has rows");
        let items = rows
            .iter()
            .map(|row| self.dispatch_arrays(spec, row, span))
            .collect::<Res<_>>()?;
        let result = Datum::Array(items);
        match layout {
            Some(layout) => Self::signal_array(result, layout, span),
            None => Ok(result),
        }
    }

    pub(super) fn dispatch(
        &mut self,
        spec: &opcodes::OpSpec,
        vals: &[Option<Val>],
        span: Span,
    ) -> Res<Sig> {
        match spec.build {
            Build::Pan => self.pan(spec, vals, span),
            Build::ToLayout => {
                let (Some(Val::Sig(x)), Some(Val::Layout(layout))) = (&vals[0], &vals[1]) else {
                    unreachable!("to_layout has a signal and a layout")
                };
                self.convert(x, *layout, span)
            }
            _ => self.expand(spec, vals, span),
        }
    }

    /// Build an opcode once per channel of its widest argument.
    pub(super) fn expand(
        &mut self,
        spec: &opcodes::OpSpec,
        vals: &[Option<Val>],
        span: Span,
    ) -> Res<Sig> {
        let mut width = 1;
        let mut layout = Layout::Mono;
        for (param, val) in spec.params.iter().zip(vals) {
            let Some(Val::Sig(sig)) = val else { continue };
            if param.wired && sig.count() > 1 {
                if width > 1 && sig.count() != width {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}`'s arguments have {width} and {} channels, which can't be combined",
                            spec.name,
                            sig.count()
                        ),
                        span,
                    ));
                }
                width = sig.count();
                layout = sig.layout;
            }
        }

        let mut srcs = Vec::with_capacity(width);
        for channel in 0..width {
            let salt = (self.g.node_count() + channel) as u32;
            let build = call::build_args(spec, vals, |s| s.konst, channel, salt)
                .map_err(|m| Diagnostic::new(m, span))?;
            let opcode = spec
                .instantiate(&build)
                .map_err(|m| Diagnostic::new(m, span))?
                .expect("only built-in opcodes are expanded");
            let wired: Vec<(&str, Src)> = spec
                .params
                .iter()
                .zip(vals)
                .filter(|(p, _)| p.wired)
                .filter_map(|(p, v)| match v {
                    Some(Val::Sig(sig)) => Some((p.name.as_str(), sig.pick(channel))),
                    _ => None,
                })
                .collect();
            let outputs = opcode.outputs();
            if outputs > 1 && width > 1 {
                return Err(Diagnostic::new(
                    "an opcode with multiple outputs needs scalar controls; map over control arrays to make separate clouds",
                    span,
                ));
            }
            srcs.extend(
                self.g
                    .add_outputs(opcode, &wired)
                    .expect("ports match the opcode table"),
            );
            if outputs > 1 {
                layout = Layout::from_count(outputs);
            }
        }
        Ok(if srcs.len() == 1 {
            Sig::signal(srcs[0], Unit::Num)
        } else {
            Sig::channels(srcs, layout)
        })
    }

    /// `pan`: place a mono signal in the master's layout.
    pub(super) fn pan(
        &mut self,
        spec: &opcodes::OpSpec,
        vals: &[Option<Val>],
        span: Span,
    ) -> Res<Sig> {
        let mut wired: Vec<(&str, Src)> = Vec::new();
        for (param, val) in spec.params.iter().zip(vals) {
            let Some(Val::Sig(sig)) = val else { continue };
            if sig.count() != 1 {
                return Err(Diagnostic::new(
                    format!(
                        "`pan` takes a one-channel signal, but `{}` has {}",
                        param.name,
                        sig.count()
                    ),
                    span,
                )
                .with_help("pan each channel separately, or convert to mono first"));
            }
            wired.push((param.name.as_str(), sig.srcs[0]));
        }

        let master = self.names.master;
        let angles: Arc<[Option<f32>]> = master.speaker_angles().into();
        let srcs: Vec<Src> = (0..master.channels())
            .map(|k| {
                self.g
                    .add(PanChannel::new(angles.clone(), k), &wired)
                    .expect("ports match the opcode table")
            })
            .collect::<Vec<_>>();
        Ok(if srcs.len() == 1 {
            Sig::signal(srcs[0], Unit::Num)
        } else {
            Sig::channels(srcs, master)
        })
    }

    pub(super) fn unit_mismatch(
        &self,
        param: &str,
        expected: Unit,
        found: Unit,
        arg: &Expr,
    ) -> Diagnostic {
        let mut d = Diagnostic::new(
            format!(
                "`{param}` expects {}, found {}",
                expected.describe(),
                found.describe()
            ),
            arg.span,
        );
        if found == Unit::Num {
            let literal = match &arg.kind {
                ExprKind::Num { value, .. } => Some(*value),
                ExprKind::Neg(inner) => match inner.kind {
                    ExprKind::Num { value, .. } => Some(-value),
                    _ => None,
                },
                _ => None,
            };
            d = match (literal, expected.suffix()) {
                (Some(v), Some(suffix)) => d.with_help(format!("did you mean `{v}{suffix}`?")),
                (None, Some(suffix)) => d.with_help(format!(
                    "multiply by a value with a unit, such as `* 1{suffix}`"
                )),
                _ => d,
            };
        }
        d
    }

    // ---- envelopes -------------------------------------------------------
}
