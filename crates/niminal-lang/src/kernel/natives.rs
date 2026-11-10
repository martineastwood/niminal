//! Calling native opcodes from a kernel.

use super::*;

impl Kb<'_> {
    /// A call of a built-in opcode such as `lpf` or `osc`: it is run a sample at a time, with
    /// state of its own for each call.
    pub(super) fn native(
        &mut self,
        name: &Ident,
        spec: &opcodes::OpSpec,
        args: &[Arg],
    ) -> Res<KVal> {
        if self.force.is_some() {
            return Err(Diagnostic::new(
                format!(
                    "`{}` changes every sample, so it can't be used here",
                    name.name
                ),
                name.span,
            ));
        }
        let names: Vec<&str> = spec.params.iter().map(|p| p.name.as_str()).collect();
        let bound = funcs::bind(&name.name, name.span, &names, spec.positional, args)?;
        let mut vals: Vec<Option<ArgVal<Value>>> = vec![None; spec.params.len()];
        for (index, (param, arg)) in spec.params.iter().zip(bound).enumerate() {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(
                        format!("`{}` needs an argument `{}`", name.name, param.name),
                        name.span,
                    ));
                }
                continue;
            };
            vals[index] = Some(match param.kind {
                opcodes::Kind::Wave => ArgVal::Wave(self.wave_arg(&arg.value)?),
                opcodes::Kind::Table => ArgVal::Tables(self.table_arg(&arg.value, true)?),
                opcodes::Kind::Layout => {
                    return Err(Diagnostic::new(
                        format!("`{}` can't be used inside a function with state", name.name),
                        name.span,
                    ));
                }
                opcodes::Kind::Signal(_) | opcodes::Kind::Array(_) | opcodes::Kind::Gain => {
                    let value = self.value(&arg.value)?;
                    self.arg_val(value, arg.value.span)?
                }
            });
        }
        self.native_call(name, spec, &vals)
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

    /// An argument of a built-in opcode: a number, or an array of them.
    pub(super) fn arg_val(&mut self, value: KVal, span: Span) -> Res<ArgVal<Value>> {
        match value {
            KVal::Num(v) => Ok(ArgVal::Sig(v)),
            v if Self::is_array(&v) => Ok(ArgVal::Array(
                self.array_elements(v, span)?
                    .into_iter()
                    .map(|item| self.arg_val(item, span))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new(
                "opcode arguments must be numbers or arrays",
                span,
            )),
        }
    }

    /// A table known while compiling, or one per channel.
    pub(super) fn table_arg(&mut self, e: &Expr, nested: bool) -> Res<Vec<Arc<Table>>> {
        let value = self.value(e)?;
        let not_a_table = || {
            Diagnostic::new(
                "expected a one-dimensional table of numbers known while compiling",
                e.span,
            )
        };
        let constants = |this: &Self, items: &[KVal]| -> Option<Arc<Table>> {
            let numbers: Option<Vec<f64>> = items
                .iter()
                .map(|item| match item {
                    KVal::Num(v) => this.konst(v),
                    _ => None,
                })
                .collect();
            numbers
                .filter(|n| !n.is_empty())
                .map(|n| Arc::new(Table::from_f64(n, None)))
        };
        let tables = match value {
            KVal::Table { values, .. } => vec![values],
            KVal::Array(items)
                if !items.is_empty()
                    && items.iter().all(|i| {
                        matches!(i, KVal::Table { .. }) || (nested && matches!(i, KVal::Array(_)))
                    }) =>
            {
                items
                    .iter()
                    .map(|i| match i {
                        KVal::Table { values, .. } => Ok(values.clone()),
                        KVal::Array(items) => constants(self, items).ok_or_else(not_a_table),
                        _ => unreachable!(),
                    })
                    .collect::<Res<Vec<_>>>()?
            }
            KVal::Array(items) => vec![constants(self, &items).ok_or_else(not_a_table)?],
            _ => return Err(not_a_table()),
        };
        if tables.iter().any(|t| t.is_empty()) {
            return Err(Diagnostic::new("a table can't be empty", e.span));
        }
        Ok(tables)
    }

    /// One call of a built-in opcode, spread over the elements of any array argument.
    pub(super) fn native_call(
        &mut self,
        name: &Ident,
        spec: &opcodes::OpSpec,
        vals: &[Option<ArgVal<Value>>],
    ) -> Res<KVal> {
        if let Some((packed, vals)) =
            call::packed(spec, vals).map_err(|m| Diagnostic::new(m, name.span))?
        {
            return self.native_call(name, &packed, &vals);
        }
        let stereo_vals;
        let vals = if spec.name == "mverb" {
            stereo_vals = call::mverb_args(vals).map_err(|m| Diagnostic::new(m, name.span))?;
            &stereo_vals
        } else {
            vals
        };
        if let Some(rows) = call::rows(vals).map_err(|m| Diagnostic::new(m, name.span))? {
            return Ok(KVal::Array(
                rows.iter()
                    .map(|row| self.native_call(name, spec, row))
                    .collect::<Res<_>>()?,
            ));
        }
        for (param, val) in spec.params.iter().zip(vals) {
            if let Some(ArgVal::Sig(v)) = val
                && let Some(expected) = call::unit_expected(param.kind, v.unit)
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` expects {} for `{}`, found {}",
                        name.name,
                        call::expectation(param.kind, expected),
                        param.name,
                        v.unit.describe()
                    ),
                    name.span,
                ));
            }
        }
        let build = call::build_args(spec, vals, |v| self.konst(v), 0, self.natives.len() as u32)
            .map_err(|m| Diagnostic::new(m, name.span))?;
        let op = spec
            .instantiate(&build)
            .map_err(|e| Diagnostic::new(e, name.span))?
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("`{}` can't be used inside a function with state", name.name),
                    name.span,
                )
            })?;
        if !op.per_sample() {
            return Err(Diagnostic::new(
                format!(
                    "`{}` works on whole blocks, so it can't be used inside a function with state",
                    name.name
                ),
                name.span,
            ));
        }
        // one input for each of the opcode's ports, from an argument or the port's default
        let mut wired = Vec::with_capacity(op.ports().len());
        for port in op.ports() {
            let given = spec
                .params
                .iter()
                .position(|p| p.name == port.name)
                .and_then(|i| match &vals[i] {
                    Some(ArgVal::Sig(v)) => Some(NativeInput::Reg(v.reg)),
                    _ => None,
                });
            wired.push(match (given, port.default) {
                (Some(input), _) => input,
                (None, Some(default)) => NativeInput::Const(f64::from(default)),
                (None, None) => {
                    return Err(Diagnostic::new(
                        format!("`{}` needs an argument `{}`", name.name, port.name),
                        name.span,
                    ));
                }
            });
        }
        let outputs = op.outputs();
        let index = self.natives.len();
        self.natives.push(Native { op, inputs: wired });
        let first = self.registers;
        self.registers += outputs;
        self.push(
            Rate::Audio,
            Instr::Native {
                dst: first,
                index,
                guard: self.guard,
            },
        );
        let values: Vec<_> = (0..outputs)
            .map(|i| {
                KVal::Num(Value {
                    reg: first + i,
                    unit: Unit::Num,
                    rate: Rate::Audio,
                    port: None,
                })
            })
            .collect();
        if spec.name == "granular" && build.consts.get("monitor") == Some(&1.0) {
            let fields = niminal_engine::ops::GRANULAR_MONITOR_FIELDS;
            let channels = outputs - fields.len();
            let audio = if channels == 1 {
                values[0].clone()
            } else {
                KVal::Array(values[..channels].to_vec().into())
            };
            let mut result = vec![("audio".into(), audio)];
            result.extend(
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (name.to_string(), values[channels + i].clone())),
            );
            return Ok(KVal::Record(result));
        }
        Ok(if outputs == 1 {
            values
                .into_iter()
                .next()
                .expect("one output means one value")
        } else {
            KVal::Array(values.into())
        })
    }
}
