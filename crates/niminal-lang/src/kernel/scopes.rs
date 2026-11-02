//! Scopes that run a nested kernel: `dsp`, block-start reads and `reset`.

use super::*;

impl Kb<'_> {
    // ---- calls -----------------------------------------------------------

    /// The receiver of a method call, if it is one.
    pub(super) fn receiver<'e>(&self, name: &Ident, args: &'e [Arg]) -> Res<&'e Arg> {
        args.first().filter(|a| a.receiver).ok_or_else(|| {
            Diagnostic::new(
                format!(
                    "`{}` is called on a value, as in `x.{}`",
                    name.name, name.name
                ),
                name.span,
            )
        })
    }

    pub(super) fn capture_arg(&mut self, value: KVal, wires: &mut Vec<NativeInput>, span: Span) -> Res<KArg> {
        Ok(match value {
            KVal::Buffer { selector, choices } => KArg::Buffer {
                selector: Box::new(self.capture_arg(KVal::Num(selector), wires, span)?),
                choices,
            },
            KVal::Num(v) => match self.konst(&v) {
                Some(value) => KArg::Const {
                    unit: v.unit,
                    value,
                },
                None => {
                    wires.push(NativeInput::Reg(v.reg));
                    KArg::Port {
                        unit: v.unit,
                        rate: v.rate,
                    }
                }
            },
            KVal::Function { def, captures } => {
                let mut captures: Vec<_> = captures.into_iter().collect();
                captures.sort_by(|a, b| a.0.cmp(&b.0));
                KArg::Function {
                    def,
                    captures: captures
                        .into_iter()
                        .map(|(n, v)| Ok((n, self.capture_arg(v, wires, span)?)))
                        .collect::<Res<_>>()?,
                }
            }
            KVal::Table { unit, values, .. } => KArg::Table { unit, values },
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => KArg::Range {
                unit,
                lo,
                hi,
                inclusive,
            },
            KVal::Record(fields) => return self.capture_arg(fields[0].1.clone(), wires, span),
            other => {
                let values = self.array_elements(other, span)?;
                KArg::Array(
                    values
                        .into_iter()
                        .map(|v| self.capture_arg(v, wires, span))
                        .collect::<Res<_>>()?,
                )
            }
        })
    }

    pub(super) fn read_buffer_ref(
        &mut self,
        selector: Value,
        choices: crate::lower::BufferChoices,
        pos: Value,
        interp: Interp,
        span: Span,
    ) -> Res<KVal> {
        if pos.unit != Unit::Num {
            return Err(Diagnostic::new("a buffer position is a plain number", span));
        }
        let buffers = Arc::new(choices.iter().map(|(_, b)| b.clone()).collect());
        Ok(self.emit_native(
            Box::new(niminal_engine::ops::BufferReadBank::new(
                buffers,
                interp,
                choices.key,
            )),
            vec![NativeInput::Reg(selector.reg), NativeInput::Reg(pos.reg)],
            Unit::Num,
        ))
    }

    pub(super) fn dsp_scope(&mut self, name: &str, args: &[Arg], span: Span) -> Res<KVal> {
        let usage = || {
            Diagnostic::new(
                match name {
                    "spawn" => "write `spawn(trigger, dur: 100ms, max: 8) { ... }`",
                    "control" => "write `control { ... }`",
                    _ => "write `oversample(4) { ... }`",
                },
                span,
            )
        };
        let Some((body, options)) = args.split_last() else {
            return Err(usage());
        };
        let ExprKind::Lambda { params, body } = &body.value.kind else {
            return Err(usage());
        };
        if !params.is_empty() {
            return Err(usage());
        }
        let control = name == "control";
        if control && self.in_control {
            // already once per step: the body runs here, with state of its own
            let def = funcs::lambda(&[], body, span);
            let captures = self
                .frame
                .scope
                .iter()
                .filter(|(n, _)| funcs::reads(&def, n))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            return self.call_captured(&def, &def.name, &[], captures);
        }
        if control && self.guard.is_some() {
            return Err(Diagnostic::new(
                "a `control` scope can't be inside an `if` or `match` branch",
                span,
            )
            .with_help("put the branch inside the scope instead"));
        }
        let (first, options) = if control {
            if !options.is_empty() {
                return Err(usage());
            }
            (None, options)
        } else {
            let Some((first, options)) = options.split_first() else {
                return Err(usage());
            };
            if first.name.is_some() {
                return Err(usage());
            }
            let first = self.num(&first.value)?;
            if first.unit != Unit::Num {
                return Err(usage());
            }
            (Some(first), options)
        };
        let mut duration = None;
        let mut max = None;
        for option in options {
            match option.name.as_ref().map(|n| n.name.as_str()) {
                Some("dur") if name == "spawn" && duration.is_none() => {
                    let value = self.num(&option.value)?;
                    if value.unit != Unit::Time
                        || self
                            .konst(&value)
                            .is_some_and(|v| !(0.0..=600.0).contains(&v) || v == 0.0)
                    {
                        return Err(usage());
                    }
                    duration = Some(value);
                }
                Some("max") if name == "spawn" && max.is_none() => {
                    let value = self.num(&option.value)?;
                    max = self
                        .konst(&value)
                        .filter(|v| {
                            value.unit == Unit::Num && v.fract() == 0.0 && (1.0..=64.0).contains(v)
                        })
                        .map(|v| v as usize);
                    if max.is_none() {
                        return Err(usage());
                    }
                }
                _ => return Err(usage()),
            }
        }
        let factor = if name == "oversample" {
            first
                .as_ref()
                .and_then(|first| self.konst(first))
                .filter(|f| [2.0, 4.0, 8.0].contains(f))
                .ok_or_else(usage)? as usize
        } else {
            1
        };
        if name == "spawn" && duration.is_none() {
            return Err(usage());
        }
        let mut def = funcs::lambda(&[], body, span);
        let mut captures: Vec<_> = self
            .frame
            .scope
            .iter()
            .filter(|(n, _)| funcs::reads(&def, n))
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        captures.sort_by(|a, b| a.0.cmp(&b.0));
        let mut wires = Vec::new();
        let mut kargs = Vec::new();
        for (n, v) in captures {
            let v = if control {
                self.at_block_start(&n, v, span)?
            } else {
                v
            };
            def.params.push(ParamDef {
                name: Ident { name: n, span },
                rate: None,
                ty: None,
                default: None,
            });
            kargs.push(self.capture_arg(v, &mut wires, span)?);
        }
        let (kernel, shape) = compile_scope_depth(
            &def,
            &kargs,
            self.registry,
            self.names,
            self.tempo,
            true,
            self.scope_depth + 1,
            control || self.in_control,
        )?;
        let custom = Box::new(niminal_engine::ops::Custom::new(Arc::new(kernel)));
        let op: Box<dyn niminal_engine::Opcode> = if name == "spawn" {
            wires.extend([
                NativeInput::Reg(first.expect("spawn has a trigger").reg),
                NativeInput::Reg(duration.unwrap().reg),
            ]);
            Box::new(niminal_engine::ops::Spawn::new(custom, max.unwrap_or(8)))
        } else if control {
            Box::new(niminal_engine::ops::ControlRate::new(custom))
        } else {
            Box::new(niminal_engine::ops::Oversample::new(custom, factor))
        };
        let outputs = op.outputs();
        let index = self.natives.len();
        self.natives.push(Native { op, inputs: wires });
        let first = self.registers;
        self.registers += outputs;
        let rate = if control { Rate::Control } else { Rate::Audio };
        self.push(
            rate,
            Instr::Native {
                dst: first,
                index,
                guard: self.guard,
            },
        );
        fn result(shape: KernelResult, first: usize, rate: Rate) -> KVal {
            match shape {
                KernelResult::Scalar { index, unit } => KVal::Num(Value {
                    reg: first + index,
                    unit,
                    rate,
                    port: None,
                }),
                KernelResult::Array(values) => {
                    KVal::Array(values.into_iter().map(|v| result(v, first, rate)).collect())
                }
                KernelResult::Record(values) => KVal::Record(
                    values
                        .into_iter()
                        .map(|(n, v)| (n, result(v, first, rate)))
                        .collect(),
                ),
            }
        }
        Ok(result(shape, first, rate))
    }

    /// A value a `control` scope captures, as it is at the start of the block: a parameter is read
    /// there, and anything that changes every sample can't be.
    pub(super) fn at_block_start(&mut self, name: &str, v: KVal, span: Span) -> Res<KVal> {
        match v {
            KVal::Num(x) => self.read_at_block_start(name, x, span).map(KVal::Num),
            KVal::Buffer { selector, choices } => Ok(KVal::Buffer {
                selector: self.read_at_block_start(name, selector, span)?,
                choices,
            }),
            KVal::Function { def, captures } => Ok(KVal::Function {
                def,
                captures: captures
                    .into_iter()
                    .map(|(n, c)| Ok((n, self.at_block_start(name, c, span)?)))
                    .collect::<Res<_>>()?,
            }),
            KVal::Record(fields) => Ok(KVal::Record(
                fields
                    .into_iter()
                    .map(|(n, f)| Ok((n, self.at_block_start(name, f, span)?)))
                    .collect::<Res<_>>()?,
            )),
            other if Self::is_array(&other) => {
                let items = self.array_elements(other, span)?;
                Ok(KVal::Array(
                    items
                        .into_iter()
                        .map(|v| self.at_block_start(name, v, span))
                        .collect::<Res<_>>()?,
                ))
            }
            other => Ok(other),
        }
    }

    pub(super) fn read_at_block_start(&mut self, name: &str, v: Value, span: Span) -> Res<Value> {
        self.at_control(v, span).map_err(|_| {
            Diagnostic::new(
                format!("`{name}` changes every sample, so a `control` scope can't read it"),
                span,
            )
            .with_help("pass a parameter in, or compute the value inside the scope")
        })
    }

    pub(super) fn reset_scope(&mut self, args: &[Arg], span: Span) -> Res<KVal> {
        let (trigger, body) = funcs::reset_args(args, span)?;
        let trigger = self.num(trigger)?;
        if trigger.unit != Unit::Num {
            return Err(Diagnostic::new("a reset trigger is one plain signal", span));
        }
        let previous = self.state_init.len();
        self.state_init.push(0.0);
        let (states, buffers, natives) = (
            self.state_init.len(),
            self.buffers.len(),
            self.natives.len(),
        );
        let (init, block, code) = (self.init.len(), self.block.len(), self.code.len());
        let registers = self.registers;
        let def = funcs::lambda(&[], body, span);
        let captures = self
            .frame
            .scope
            .iter()
            .filter(|(n, _)| funcs::reads(&def, n))
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        let result = self.call_captured(&def, &def.name, &[], captures)?;
        let scope = self.resets.len();
        self.resets.push(niminal_engine::ops::ResetScope {
            states: states..self.state_init.len(),
            buffers: buffers..self.buffers.len(),
            natives: (natives..self.natives.len()).collect(),
            registers: registers..self.registers,
            init: self.init[init..].to_vec(),
            block: self.block[block..].to_vec(),
        });
        self.code.insert(
            code,
            Instr::Reset {
                scope,
                trigger: trigger.reg,
                previous,
                guard: self.guard,
            },
        );
        Ok(result)
    }
}
