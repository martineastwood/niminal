//! Kernel builder: inputs, outputs and the emitting helpers (rates, constants, arithmetic, guards).

use super::*;

impl Kb<'_> {
    pub(super) fn input_arg(&mut self, name: &str, arg: &KArg) -> KVal {
        match arg {
            KArg::Buffer { selector, choices } => {
                let KVal::Num(selector) = self.input_arg(name, selector) else {
                    unreachable!()
                };
                KVal::Buffer {
                    selector,
                    choices: choices.clone(),
                }
            }
            KArg::Function { def, captures } => KVal::Function {
                def: def.clone(),
                captures: captures
                    .iter()
                    .map(|(n, v)| (n.clone(), self.input_arg(&format!("{name}::{n}"), v)))
                    .collect(),
            },
            KArg::Port { unit, rate } => {
                let port = self.ports.len();
                self.ports.push(Port::named(name.to_string(), None));
                let reg = self.emit(*rate, |dst| Instr::Input { dst, port });
                KVal::Num(Value {
                    reg,
                    unit: *unit,
                    rate: *rate,
                    port: Some(port),
                })
            }
            KArg::Const { unit, value } => KVal::Num(self.constant(*unit, *value)),
            KArg::Table { unit, values } => self.table(*unit, values.clone()),
            KArg::Array(items) => KVal::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| self.input_arg(&format!("{name}[{i}]"), item))
                    .collect(),
            ),
            KArg::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => KVal::Range {
                unit: *unit,
                lo: *lo,
                hi: *hi,
                inclusive: *inclusive,
            },
            KArg::Missing => unreachable!("defaults are evaluated in the callee"),
        }
    }

    pub(super) fn output_shape(
        &mut self,
        value: KVal,
        name: &str,
        outputs: &mut Vec<(String, usize)>,
        span: Span,
    ) -> Res<KernelResult> {
        match value {
            KVal::Num(v) => {
                let index = outputs.len();
                outputs.push((name.to_string(), v.reg));
                Ok(KernelResult::Scalar {
                    index,
                    unit: v.unit,
                })
            }
            KVal::Record(fields) => Ok(KernelResult::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| {
                        let shape = self.output_shape(value, &name, outputs, span)?;
                        Ok((name, shape))
                    })
                    .collect::<Res<_>>()?,
            )),
            other if Self::is_array(&other) => {
                let items = self.array_elements(other, span)?;
                Ok(KernelResult::Array(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, value)| {
                            self.output_shape(value, &format!("{name}[{i}]"), outputs, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "a function returns a number or an array",
                span,
            )),
        }
    }

    pub(super) fn snapshot(&mut self, value: KVal, span: Span) -> Res<KVal> {
        match value {
            state @ KVal::State { .. } => self.lane_copy(state, span),
            KVal::Array(items) => Ok(KVal::Array(
                items
                    .into_iter()
                    .map(|v| self.snapshot(v, span))
                    .collect::<Res<_>>()?,
            )),
            KVal::Record(fields) => Ok(KVal::Record(
                fields
                    .into_iter()
                    .map(|(name, v)| Ok((name, self.snapshot(v, span)?)))
                    .collect::<Res<_>>()?,
            )),
            other => Ok(other),
        }
    }

    // ---- emitting ----------------------------------------------------

    pub(super) fn section(&self, rate: Rate) -> Section {
        self.force.unwrap_or(section_of(rate))
    }

    pub(super) fn rate(&self, rate: Rate) -> Rate {
        match self.force {
            Some(s) => rate.min(rate_of(s)),
            None => rate,
        }
    }

    pub(super) fn push(&mut self, rate: Rate, instr: Instr) {
        match self.section(rate) {
            Section::Init => self.init.push(instr),
            Section::Block => self.block.push(instr),
            Section::Sample => self.code.push(instr),
        }
    }

    pub(super) fn emit(&mut self, rate: Rate, make: impl FnOnce(usize) -> Instr) -> usize {
        let dst = self.registers;
        self.registers += 1;
        self.push(rate, make(dst));
        dst
    }

    pub(super) fn constant(&mut self, unit: Unit, value: f64) -> Value {
        let reg = self.emit(Rate::Const, |dst| Instr::Const { dst, value });
        self.consts.insert(reg, value);
        Value {
            reg,
            unit,
            rate: Rate::Const,
            port: None,
        }
    }

    pub(super) fn konst(&self, v: &Value) -> Option<f64> {
        self.consts.get(&v.reg).copied()
    }

    pub(super) fn table(&mut self, unit: Unit, values: Arc<Table>) -> KVal {
        let id = self.tables.len();
        self.tables.push(values.clone());
        KVal::Table { id, unit, values }
    }

    /// A value read where the rate is forced: a port is read again there; anything faster than the
    /// forced rate can't be.
    pub(super) fn read(&mut self, v: Value, span: Span) -> Res<Value> {
        let Some(forced) = self.force else {
            return Ok(v);
        };
        if v.rate <= rate_of(forced) {
            return Ok(v);
        }
        match v.port {
            Some(port) => {
                let rate = rate_of(forced);
                let reg = self.emit(rate, |dst| Instr::Input { dst, port });
                Ok(Value { reg, rate, ..v })
            }
            None => Err(Diagnostic::new(
                match forced {
                    Section::Init => {
                        "this changes while the note plays, so it isn't known at note start"
                    }
                    _ => "this changes every sample, so it can't be read once per block",
                },
                span,
            )
            .with_help("use the parameters and constants here")),
        }
    }

    pub(super) fn bin(&mut self, op: Op, a: Value, b: Value, unit: Unit) -> Value {
        if let (Some(x), Some(y)) = (self.konst(&a), self.konst(&b)) {
            return self.constant(unit, op.apply(x, y));
        }
        let rate = self.rate(a.rate.max(b.rate));
        let reg = self.emit(rate, |dst| Instr::Bin {
            dst,
            op,
            a: a.reg,
            b: b.reg,
        });
        Value {
            reg,
            unit,
            rate,
            port: None,
        }
    }

    pub(super) fn cmp(&mut self, op: Cmp, a: Value, b: Value) -> Value {
        if let (Some(x), Some(y)) = (self.konst(&a), self.konst(&b)) {
            return self.constant(Unit::Num, if op.holds(x, y) { 1.0 } else { 0.0 });
        }
        let rate = self.rate(a.rate.max(b.rate));
        let reg = self.emit(rate, |dst| Instr::Cmp {
            dst,
            op,
            a: a.reg,
            b: b.reg,
        });
        Value {
            reg,
            unit: Unit::Num,
            rate,
            port: None,
        }
    }

    pub(super) fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        if let Some(k) = self.konst(&c) {
            return if k != 0.0 { a } else { b };
        }
        if a.reg == b.reg {
            return a;
        }
        let rate = self.rate(c.rate.max(a.rate).max(b.rate));
        let reg = self.emit(rate, |dst| Instr::Select {
            dst,
            cond: c.reg,
            a: a.reg,
            b: b.reg,
        });
        Value {
            reg,
            unit: a.unit,
            rate,
            port: None,
        }
    }

    pub(super) fn call1(&mut self, f: Fn1, a: Value) -> Value {
        if let Some(x) = self.konst(&a) {
            return self.constant(Unit::Num, f.apply(x));
        }
        let rate = self.rate(a.rate);
        let reg = self.emit(rate, |dst| Instr::Call { dst, f, a: a.reg });
        Value {
            reg,
            unit: Unit::Num,
            rate,
            port: None,
        }
    }

    /// 1 where `v` is not 0.
    pub(super) fn truthy(&mut self, v: Value) -> Value {
        let zero = self.constant(v.unit, 0.0);
        self.cmp(Cmp::Ne, v, zero)
    }

    /// The guard for a branch: the current one, and `c`.
    pub(super) fn guard_with(&mut self, c: Value) -> Value {
        let c = self.truthy(c);
        match self.guard {
            None => c,
            Some(g) => {
                let g = Value {
                    reg: g,
                    unit: Unit::Num,
                    rate: Rate::Audio,
                    port: None,
                };
                self.bin(Op::Mul, g, c, Unit::Num)
            }
        }
    }

    /// `v` as it was at note start: a port is read in the note-start section, anything else that
    /// changes is caught on its first sample and held.
    pub(super) fn at_init(&mut self, v: Value) -> Value {
        if v.rate <= Rate::Init {
            return v;
        }
        if let Some(port) = v.port {
            let reg = self.emit(Rate::Init, |dst| Instr::Input { dst, port });
            return Value {
                reg,
                rate: Rate::Init,
                ..v
            };
        }
        let (flag, held) = (self.state_init.len(), self.state_init.len() + 1);
        self.state_init.extend([0.0, 0.0]);
        let started = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot: flag });
        let stored = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot: held });
        let reg = self.emit(Rate::Audio, |dst| Instr::Select {
            dst,
            cond: started,
            a: stored,
            b: v.reg,
        });
        let one = self.emit(Rate::Audio, |dst| Instr::Const { dst, value: 1.0 });
        self.code.push(Instr::StoreState {
            slot: held,
            src: reg,
        });
        self.code.push(Instr::StoreState {
            slot: flag,
            src: one,
        });
        Value {
            reg,
            unit: v.unit,
            rate: Rate::Audio,
            port: None,
        }
    }

    /// `v` read once per block.
    pub(super) fn at_control(&mut self, v: Value, span: Span) -> Res<Value> {
        if v.rate <= Rate::Control {
            return Ok(v);
        }
        match v.port {
            Some(port) => {
                let reg = self.emit(Rate::Control, |dst| Instr::Input { dst, port });
                Ok(Value {
                    reg,
                    rate: Rate::Control,
                    ..v
                })
            }
            None => Err(Diagnostic::new(
                "inside a function with state, only a parameter can be read once per block",
                span,
            )
            .with_help("apply `.control` where the function is called")),
        }
    }
}
