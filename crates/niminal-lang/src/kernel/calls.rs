//! Calls: unit methods, `prev`, array builders and the other built-in functions.

use super::*;

impl Kb<'_> {
    pub(super) fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let n = name.name.as_str();
        if matches!(n, "spawn" | "oversample")
            || (n == "control"
                && matches!(args, [a] if !a.receiver && matches!(a.value.kind, ExprKind::Lambda { .. })))
        {
            return self.dsp_scope(n, args, span);
        }
        if n == "reset_on" {
            return self.reset_scope(args, span);
        }
        match n {
            "frames" => return self.frames_scope(name, args, span),
            "fft" => return self.fft_call(name, args, span),
            "ifft" => return self.ifft_call(name, args, span),
            _ => {}
        }
        if let [receiver] = args
            && receiver.receiver
            && n == "time"
            && matches!(&receiver.value.kind, ExprKind::Name(c) if c == "clock")
        {
            use niminal_engine::ops::{Context, ReadContext};
            let op = Box::new(ReadContext::new(Context::ClockTime));
            return Ok(self.emit_native(op, Vec::new(), Unit::Time));
        }
        if self.instrument_scope
            && let [receiver] = args
            && receiver.receiver
        {
            use niminal_engine::ops::{Context, NoteStart, NoteTime, ReadContext, Released};
            if matches!(&receiver.value.kind, ExprKind::Name(n) if n == "note") {
                let (op, unit): (Box<dyn niminal_engine::Opcode>, Unit) = match n {
                    "time" => (Box::new(NoteTime::default()), Unit::Time),
                    "start" => (Box::new(NoteStart::default()), Unit::Num),
                    "released" => (Box::new(Released), Unit::Num),
                    "dur" => (Box::new(ReadContext::new(Context::NoteDur)), Unit::Time),
                    "index" => (Box::new(ReadContext::new(Context::NoteIndex)), Unit::Num),
                    _ => return Err(Diagnostic::new("unknown note field", name.span)),
                };
                return Ok(self.emit_native(op, Vec::new(), unit));
            }
        }
        if self.instrument_scope
            && let Some(def) = self.names.instrs.get(n).cloned()
        {
            let own_gain = def.params.iter().any(|p| p.name.name == "gain");
            let (gains, rest): (Vec<_>, Vec<_>) = args
                .iter()
                .cloned()
                .partition(|a| !own_gain && a.name.as_ref().is_some_and(|n| n.name == "gain"));
            if gains.len() > 1 {
                return Err(Diagnostic::new("gain given more than once", span));
            }
            let gain = if let Some(arg) = gains.first() {
                let mut value = self.num(&arg.value)?;
                if !matches!(value.unit, Unit::Num | Unit::Db) {
                    return Err(Diagnostic::new(
                        "gain needs db or a plain factor",
                        arg.value.span,
                    ));
                }
                value.unit = Unit::Num;
                Some(value)
            } else {
                None
            };
            let as_fn = FnDef {
                name: def.name,
                params: def.params,
                body: def.body,
                result: FnResult::Inferred,
                span: def.span,
            };
            let result = self.call_fn(&as_fn, name, &rest)?;
            return match gain {
                Some(gain) => self.binary_value(BinOp::Mul, result, KVal::Num(gain), span),
                None => Ok(result),
            };
        }
        if self.instrument_scope
            && n == "at"
            && let Some(receiver) = args.first().filter(|a| a.receiver)
            && let ExprKind::Name(buffer) = &receiver.value.kind
            && let Some((_, buffer)) = self.names.buffers.get(buffer).cloned()
        {
            let mut position = None;
            let mut interp = None;
            for arg in &args[1..] {
                match arg.name.as_ref().map(|n| n.name.as_str()) {
                    None | Some("pos") if position.is_none() => {
                        position = Some(self.num(&arg.value)?)
                    }
                    Some("interp") if interp.is_none() => interp = Some(interp_of(&arg.value)?),
                    _ => {
                        return Err(Diagnostic::new(
                            "write `buffer.at(pos, interp: linear)`",
                            arg.value.span,
                        ));
                    }
                }
            }
            let position =
                position.ok_or_else(|| Diagnostic::new("buffer.at needs a position", span))?;
            if position.unit != Unit::Num {
                return Err(Diagnostic::new("a buffer position is a plain number", span));
            }
            return Ok(self.emit_native(
                Box::new(niminal_engine::ops::BufferRead::new(
                    buffer,
                    interp.unwrap_or(Interp::Linear),
                )),
                vec![NativeInput::Reg(position.reg)],
                Unit::Num,
            ));
        }
        if n == "at"
            && let Some(receiver) = args.first().filter(|a| a.receiver)
            && let ExprKind::Name(id) = &receiver.value.kind
            && let Some(KVal::Buffer { selector, choices }) = self.frame.scope.get(id).cloned()
        {
            let mut pos = None;
            let mut interp = None;
            for arg in &args[1..] {
                match arg.name.as_ref().map(|n| n.name.as_str()) {
                    None if pos.is_none() => pos = Some(self.num(&arg.value)?),
                    Some("interp") if interp.is_none() => interp = Some(interp_of(&arg.value)?),
                    _ => {
                        return Err(Diagnostic::new(
                            "write `source.at(pos, interp: linear)`",
                            span,
                        ));
                    }
                }
            }
            return self.read_buffer_ref(
                selector,
                choices,
                pos.ok_or_else(|| Diagnostic::new("at needs a position", span))?,
                interp.unwrap_or(Interp::Linear),
                span,
            );
        }
        // Named result lookup must preserve an array field's shape.
        if let [receiver] = args
            && receiver.receiver
            && !self.names.fns.contains_key(n)
        {
            let record = match &receiver.value.kind {
                ExprKind::Name(name) => self.frame.scope.get(name).cloned(),
                ExprKind::Call { name, .. }
                    if self
                        .names
                        .fns
                        .get(&name.name)
                        .is_some_and(|def| matches!(def.result, FnResult::Named(_)))
                        || matches!(self.frame.scope.get(&name.name), Some(KVal::Function { def, .. }) if matches!(def.result, FnResult::Named(_))) =>
                {
                    Some(self.value(&receiver.value)?)
                }
                _ => None,
            };
            if let Some(KVal::Record(fields)) = record {
                return fields
                    .iter()
                    .find(|(field, _)| field == n)
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        Diagnostic::new(format!("this has no result `{n}`"), name.span)
                    });
            }
        }
        if let Some(KVal::Function { def, captures }) = self.frame.scope.get(n).cloned() {
            return self.call_captured(&def, name, args, captures);
        }
        if let Some(def) = self.names.fns.get(n) {
            let def = def.clone();
            return self.call_fn(&def, name, args);
        }
        match n {
            "prev" => return self.prev(name, args).map(KVal::Num),
            "min" | "max" if args.len() == 1 && args[0].receiver => {
                return self.array_call(name, args, span);
            }
            "min" | "max" | "pow" | "mod" | "atan2" => return self.combine(name, args),
            "clamp" => return self.clamp(name, args),
            "range" => return self.range(name, args, span),
            "hold" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                return self.map_value(value, span, |_, v| {
                    if v.rate > Rate::Control {
                        return Err(Diagnostic::new(
                            "`hold` is for values that change at most once per block",
                            span,
                        )
                        .with_help(
                            "compute it in a `control { ... }` scope, or read it with `.control`",
                        ));
                    }
                    Ok(v)
                });
            }
            "smooth" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                return self.map_value(value, span, |this, v| {
                    if v.rate > Rate::Control {
                        return Err(Diagnostic::new(
                            "`smooth` is for values that change at most once per block",
                            span,
                        )
                        .with_help(
                            "compute it in a `control { ... }` scope, or read it with `.control`",
                        ));
                    }
                    match this.emit_native(
                        Box::new(niminal_engine::ops::Ramp::default()),
                        vec![NativeInput::Reg(v.reg)],
                        v.unit,
                    ) {
                        KVal::Num(smoothed) => Ok(smoothed),
                        _ => unreachable!("a native gives one number"),
                    }
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
                return self.map_value(value, span, |this, v| {
                    if n == "init" {
                        Ok(this.at_init(v))
                    } else {
                        this.at_control(v, args[0].value.span)
                    }
                });
            }
            "len" | "sum" | "map" | "indices" | "at" | "channels" | "step" | "dot" | "fir"
            | "taps" | "matvec" => {
                return self.array_call(name, args, span);
            }
            "zeros" => {
                let [count] = args else {
                    return Err(Diagnostic::new("`zeros` takes a count", name.span));
                };
                let c = self.num(&count.value)?;
                let n = match self.konst(&c) {
                    Some(k) if k.fract() == 0.0 && (0.0..=(1 << 22) as f64).contains(&k) => {
                        k as usize
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            "`zeros` takes a whole number known while compiling",
                            count.value.span,
                        ));
                    }
                };
                if n == 0 {
                    return Ok(KVal::array(vec![]));
                }
                return Ok(self.table(
                    Unit::Num,
                    Arc::new(Table::from_f32(vec![0.0f32; n].into_boxed_slice(), None)),
                ));
            }
            "split" => {
                return Err(Diagnostic::new(
                    "a function with state works on one channel at a time",
                    name.span,
                ));
            }
            _ => {}
        }
        if let Some(v) = self.unit_method(name, args)? {
            return Ok(v);
        }
        if let Some(f) = math_fn(n) {
            let [arg] = args else {
                return Err(Diagnostic::new(format!("`{n}` takes one value"), name.span));
            };
            if let Some(a) = &arg.name {
                return Err(Diagnostic::new(
                    format!("`{n}` takes its value without a name"),
                    a.span,
                ));
            }
            let value = self.value(&arg.value)?;
            return self.math_value(f, value, arg.value.span);
        }

        let registry = self.registry;
        if let Some(spec) = registry.find(n) {
            if let Build::Builtin(_) = &spec.build {
                return self.native(name, spec, args);
            }
            return Err(Diagnostic::new(
                format!("`{n}` can't be used inside a function with state"),
                name.span,
            )
            .with_help("it works on channels; use it where the function is called"));
        }
        if matches!(n, "osc_out" | "input") {
            return Err(Diagnostic::new(
                format!("`{n}` can't be used inside a function"),
                name.span,
            ));
        }
        if self.names.kind_of(n) == Some("instrument") {
            return Err(Diagnostic::new(
                format!("`{n}` is an instrument; a function with state can't play one"),
                name.span,
            ));
        }
        let known: Vec<&str> = registry
            .names()
            .chain(self.names.fns.keys().map(String::as_str))
            .chain(Fn1::ALL.iter().map(|f| f.name()))
            .chain([
                "min", "max", "pow", "mod", "atan2", "clamp", "prev", "zeros",
            ])
            .collect();
        let mut d = Diagnostic::new(format!("unknown function `{n}`"), name.span);
        if let Some(c) = closest(n, known) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        Err(d)
    }

    /// `x.hz`, `n.ms`, `(-6db).num`...: give a plain value a unit, or take it away.
    pub(super) fn unit_method(&mut self, name: &Ident, args: &[Arg]) -> Res<Option<KVal>> {
        let scale = matches!(
            name.name.as_str(),
            "num"
                | "hz"
                | "khz"
                | "db"
                | "sec"
                | "ms"
                | "beat"
                | "beats"
                | "bar"
                | "bars"
                | "deg"
                | "st"
                | "ct"
        );
        if !scale || !(args.len() == 1 && args[0].receiver) {
            return Ok(None);
        }
        let value = self.value(&args[0].value)?;
        let span = args[0].value.span;
        self.map_value(value, span, |this, value| {
            this.unit_value(name, value, span)
        })
        .map(Some)
    }

    pub(super) fn unit_value(&mut self, name: &Ident, v: Value, span: Span) -> Res<Value> {
        if name.name == "num" {
            return Ok(match v.unit {
                Unit::Db => {
                    // a db value is held as a gain factor
                    let ln = self.call1(
                        Fn1::Log,
                        Value {
                            unit: Unit::Num,
                            ..v
                        },
                    );
                    let k = self.constant(Unit::Num, 20.0 / std::f64::consts::LN_10);
                    self.bin(Op::Mul, ln, k, Unit::Num)
                }
                _ => Value {
                    unit: Unit::Num,
                    ..v
                },
            });
        }
        if v.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "`.{}` gives a plain number a unit, but this is {}",
                    name.name,
                    v.unit.describe()
                ),
                span,
            )
            .with_help("take the unit away first with `.num`"));
        }
        let (unit, factor) = literal_sig(1.0, Some(&name.name), self.tempo, name.span)?;
        if unit == Unit::Db {
            let k = self.constant(Unit::Num, std::f64::consts::LN_10 / 20.0);
            let e = self.bin(Op::Mul, v, k, Unit::Num);
            let g = self.call1(Fn1::Exp, e);
            return Ok(Value {
                unit: Unit::Db,
                ..g
            });
        }
        let f = self.constant(Unit::Num, factor);
        Ok(self.bin(Op::Mul, v, f, unit))
    }

    /// `prev(x)`: what `x` was one sample ago. `y = x + prev(y) * 0.5` is a feedback loop.
    pub(super) fn prev(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let [arg] = args else {
            return Err(Diagnostic::new("`prev` takes one value", name.span));
        };
        if let Some(n) = &arg.name {
            return Err(Diagnostic::new(
                "`prev` takes its value without a name",
                n.span,
            ));
        }
        if self.force.is_some() {
            return Err(Diagnostic::new(
                "`prev` changes every sample, so it can't be used here",
                name.span,
            ));
        }
        if let ExprKind::Name(n) = &arg.value.kind
            && let Some(&(slot, _)) = self.frame.forward.get(n)
        {
            let reg = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot });
            return Ok(Value {
                reg,
                unit: Unit::Num,
                rate: Rate::Audio,
                port: None,
            });
        }
        let slot = self.state_init.len();
        self.state_init.push(0.0);
        let reg = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot });
        if let ExprKind::Name(n) = &arg.value.kind
            && !self.frame.scope.contains_key(n)
            && !matches!(n.as_str(), "pi" | "sample_rate")
        {
            // a name defined further down: this is feedback
            self.frame.forward.insert(n.clone(), (slot, arg.value.span));
            return Ok(Value {
                reg,
                unit: Unit::Num,
                rate: Rate::Audio,
                port: None,
            });
        }
        let v = self.num(&arg.value)?;
        self.frame.deferred.push((slot, v.reg));
        Ok(Value {
            reg,
            unit: v.unit,
            rate: Rate::Audio,
            port: None,
        })
    }

    /// The values of a call such as `min(a, b)` or `a.mod(b)`, without names.
    pub(super) fn plain_args(
        &mut self,
        name: &Ident,
        args: &[Arg],
        count: usize,
    ) -> Res<Vec<(KVal, Span)>> {
        if args.len() != count {
            return Err(Diagnostic::new(
                format!("`{}` takes {count} values", name.name),
                name.span,
            ));
        }
        if let Some(n) = args.iter().find_map(|a| a.name.as_ref()) {
            return Err(Diagnostic::new(
                format!("`{}` takes its values without names", name.name),
                n.span,
            ));
        }
        let mut values = Vec::with_capacity(count);
        for a in args {
            values.push((self.value(&a.value)?, a.value.span));
        }
        Ok(values)
    }

    /// `min(a, b)`, `max(a, b)`, `pow(x, y)` and `a.mod(b)`.
    pub(super) fn combine(&mut self, name: &Ident, args: &[Arg]) -> Res<KVal> {
        let mut values = self.plain_args(name, args, 2)?;
        let (b, b_span) = values.pop().unwrap();
        let (a, _) = values.pop().unwrap();
        self.zip_values(a, b, b_span, |this, a, b| {
            this.combine_scalar(name, a, b, b_span)
        })
    }

    pub(super) fn combine_scalar(
        &mut self,
        name: &Ident,
        a: Value,
        b: Value,
        b_span: Span,
    ) -> Res<Value> {
        match name.name.as_str() {
            "pow" => {
                if a.unit != Unit::Num || b.unit != Unit::Num {
                    return Err(Diagnostic::new("`pow` expects plain numbers", b_span));
                }
                Ok(self.bin(Op::Pow, a, b, Unit::Num))
            }
            "atan2" => {
                if a.unit != b.unit {
                    return Err(Diagnostic::new(
                        "`atan2` needs values with the same unit",
                        b_span,
                    ));
                }
                Ok(self.bin(Op::Atan2, a, b, Unit::Num))
            }
            "mod" => {
                let unit = crate::unit::rem_unit(a.unit, b.unit).ok_or_else(|| {
                    Diagnostic::new(
                        format!(
                            "can't take the remainder of {} and {}",
                            a.unit.describe(),
                            b.unit.describe()
                        ),
                        b_span,
                    )
                })?;
                Ok(self.bin(Op::Rem, a, b, unit))
            }
            m => {
                if a.unit != b.unit {
                    return Err(Diagnostic::new(
                        format!(
                            "`{m}` needs values with the same unit: this is {}, the first was {}",
                            b.unit.describe(),
                            a.unit.describe()
                        ),
                        b_span,
                    ));
                }
                Ok(self.bin(if m == "min" { Op::Min } else { Op::Max }, a, b, a.unit))
            }
        }
    }

    pub(super) fn range(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
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
        let KVal::Range { unit, lo, hi, .. } = self.value(&range.value)? else {
            return Err(Diagnostic::new("`range` takes a range", range.value.span));
        };
        self.map_value(value, span, |this, x| {
            if x.unit != Unit::Num {
                return Err(Diagnostic::new(
                    "`range` maps plain numbers from 0 to 1",
                    span,
                ));
            }
            if unit == Unit::Db {
                let scale = this.constant(Unit::Num, (hi / lo).ln());
                let exponent = this.bin(Op::Mul, x, scale, Unit::Num);
                let grown = this.call1(Fn1::Exp, exponent);
                let base = this.constant(Unit::Db, lo);
                return Ok(this.bin(Op::Mul, grown, base, Unit::Db));
            }
            let width = this.constant(unit, hi - lo);
            let scaled = this.bin(Op::Mul, x, width, unit);
            let base = this.constant(unit, lo);
            Ok(this.bin(Op::Add, scaled, base, unit))
        })
    }

    /// `x.clamp(lo..hi)`
    pub(super) fn clamp(&mut self, name: &Ident, args: &[Arg]) -> Res<KVal> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`clamp` takes a range", name.span)
                .with_help("for example `x.clamp(0..1)`"));
        };
        let v = self.value(&x.value)?;
        let ExprKind::Range { lo, hi, .. } = &range.value.kind else {
            return Err(Diagnostic::new(
                "`clamp` takes a range, such as `20hz..18khz`",
                range.value.span,
            ));
        };
        let (l, h) = (self.num(lo)?, self.num(hi)?);
        let up = self.extreme_value(v, KVal::Num(l), false, name.span)?;
        self.extreme_value(up, KVal::Num(h), true, name.span)
    }

    /// `list.len`, `list.sum`, `list.map(f)`, `list.indices` and `list.at(pos)`.
    pub(super) fn array_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let receiver = self.receiver(name, args)?;
        let l = self.value(&receiver.value)?;
        if name.name == "step" {
            let KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } = l
            else {
                return Err(
                    Diagnostic::new("`step` is called on a range", receiver.value.span)
                        .with_help("for example `(0..1).step(0.25)`"),
                );
            };
            let [_, step] = args else {
                return Err(Diagnostic::new(
                    "`step` takes the size of a step",
                    name.span,
                ));
            };
            let size = self.num(&step.value)?;
            let size_unit = size.unit;
            let Some(size) = self.konst(&size) else {
                return Err(Diagnostic::new(
                    "the size of a step must be known while compiling",
                    step.value.span,
                ));
            };
            if size_unit != unit || unit == Unit::Db {
                return Err(Diagnostic::new(
                    "a step has the same unit as the range, and a db range can't be stepped",
                    step.value.span,
                ));
            }
            let values = step_numbers(lo, hi, inclusive, size, span)?;
            return Ok(KVal::array(
                values
                    .into_iter()
                    .map(|v| KVal::Num(self.constant(unit, v)))
                    .collect(),
            ));
        }
        let l = if matches!(l, KVal::Range { .. }) {
            if name.name == "channels" {
                return Err(Diagnostic::new("a range has no channels", span));
            }
            KVal::array(self.array_elements(l, span)?)
        } else {
            l
        };
        let len = |l: &KVal| match l {
            KVal::Array(items) => Some(items.len()),
            KVal::Table { values, .. } => Some(values.len()),
            KVal::State { len, .. } | KVal::Lane { len, .. } => Some(*len),
            KVal::Num(_) => Some(1),
            _ => None,
        };
        let Some(count) = len(&l) else {
            return Err(Diagnostic::new(
                format!("`{}` works on an array", name.name),
                receiver.value.span,
            ));
        };
        if matches!(
            name.name.as_str(),
            "len" | "indices" | "sum" | "min" | "max" | "channels"
        ) && args.len() != 1
        {
            return Err(Diagnostic::new(
                format!("`{}` takes no arguments", name.name),
                name.span,
            ));
        }
        match name.name.as_str() {
            "channels" => Ok(KVal::array(self.array_elements(l, span)?)),
            "len" => Ok(KVal::Num(self.constant(Unit::Num, count as f64))),
            "indices" => Ok(KVal::Range {
                unit: Unit::Num,
                lo: 0.0,
                hi: count as f64,
                inclusive: false,
            }),
            "sum" | "min" | "max" if matches!(l, KVal::State { .. } | KVal::Lane { .. }) => {
                let kind = match name.name.as_str() {
                    "sum" => Reduce::Sum,
                    "min" => Reduce::Min,
                    _ => Reduce::Max,
                };
                let input = self.lane_of(l, receiver.value.span)?;
                let rate = self.rate(input.rate);
                let reg = self.emit(rate, |dst| Instr::Reduce {
                    dst,
                    kind,
                    a: input.src,
                });
                Ok(KVal::Num(Value {
                    reg,
                    unit: input.unit,
                    rate,
                    port: None,
                }))
            }
            "dot" | "fir" | "taps" | "matvec" => self.vector_call(name, args, l, span),
            "sum" | "min" | "max" => {
                let entries = self.array_elements(l, receiver.value.span)?;
                let mut total: Option<KVal> = None;
                for item in entries {
                    total = Some(match total {
                        None => item,
                        Some(t) if name.name == "sum" => {
                            self.binary_value(BinOp::Add, t, item, span)?
                        }
                        Some(t) => self.extreme_value(t, item, name.name == "min", span)?,
                    });
                }
                total.ok_or_else(|| Diagnostic::new("this array is empty", span))
            }
            "at" => {
                let (list, unit) = self.numeric_array(l, receiver.value.span)?;
                let mut pos = None;
                let mut interp = Interp::Linear;
                for a in &args[1..] {
                    match a.name.as_ref().map(|n| n.name.as_str()) {
                        None if pos.is_none() => pos = Some(self.num(&a.value)?),
                        Some("interp") => interp = interp_of(&a.value)?,
                        _ => {
                            return Err(Diagnostic::new(
                                "`at` takes a position and `interp:`",
                                a.value.span,
                            ));
                        }
                    }
                }
                let Some(pos) = pos else {
                    return Err(Diagnostic::new("`at` takes a position", name.span));
                };
                if pos.unit != Unit::Num {
                    return Err(Diagnostic::new("a position is a plain number", span));
                }
                Ok(KVal::Num(self.read_array(list, unit, pos, Some(interp))))
            }
            _ => {
                // map
                let [_, f] = args else {
                    return Err(Diagnostic::new("`map` takes a function", name.span));
                };
                let mapped = match &f.value.kind {
                    ExprKind::Name(n)
                        if self.names.fns.contains_key(n)
                            || matches!(self.frame.scope.get(n), Some(KVal::Function { .. })) =>
                    {
                        Some(funcs::mapped_call(n, f.value.span))
                    }
                    _ => None,
                };
                let (param, body) = lambda_of(mapped.as_ref().unwrap_or(&f.value))?;
                if param != "it" {
                    self.check_name(&Ident {
                        name: param.clone(),
                        span: f.value.span,
                    })?;
                }
                if self.frame.scope.contains_key(&param) {
                    return Err(Diagnostic::new(
                        format!(
                            "`{param}` is already in use, so it can't be this function's parameter"
                        ),
                        f.value.span,
                    ));
                }
                let entries = self.array_elements(l, receiver.value.span)?;
                let mut out = Vec::with_capacity(entries.len());
                for entry in entries {
                    self.frame.scope.insert(param.clone(), entry);
                    let v = match &body {
                        Body::Block(stmts) => self
                            .block(stmts, true, f.value.span)
                            .map(|v| v.expect("a value")),
                        Body::Expr(e) => self.value(e),
                    };
                    self.frame.scope.remove(&param);
                    out.push(self.snapshot(v?, f.value.span)?);
                }
                Ok(KVal::array(out))
            }
        }
    }
}
