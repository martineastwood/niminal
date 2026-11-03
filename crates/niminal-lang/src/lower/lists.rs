//! Lists as values: element-wise maths, zipping, mapping, stepping, table transforms, buffers.

use super::*;

impl Lower<'_> {
    pub(super) fn extreme_value(
        &mut self,
        a: Datum,
        b: Datum,
        minimum: bool,
        span: Span,
    ) -> Res<Datum> {
        self.zip_values(a, b, span, |this, a, b| {
            this.same_unit(&a, &b, if minimum { "min" } else { "max" }, span)?;
            if minimum {
                this.min_sig(&a, &b, span)
            } else {
                this.max_sig(&a, &b, span)
            }
        })
    }

    pub(super) fn math_value(&mut self, f: Fn1, value: Datum, span: Span) -> Res<Datum> {
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
            Ok(this.func(f, &value))
        })
    }

    pub(super) fn negate_value(&mut self, value: Datum, span: Span) -> Res<Datum> {
        self.map_value(value, span, |this, value| this.negate(value, span))
    }

    /// Arithmetic on lists goes entry by entry. A single value applies to every entry, and two lists must be the same length.
    pub(super) fn binary_value(&mut self, op: BinOp, l: Datum, r: Datum, span: Span) -> Res<Datum> {
        let tables = matches!(l, Datum::Table(_)) || matches!(r, Datum::Table(_));
        let result = self.zip_values(l, r, span, |this, a, b| this.binary(op, a, b, span))?;
        Ok(if tables { self.tabled(result) } else { result })
    }

    /// An array of numbers known while compiling, as the table it came from; anything else as it is.
    pub(super) fn tabled(&mut self, value: Datum) -> Datum {
        match self.as_table(value.clone(), Span::new(0, 0)) {
            Ok(table) => Datum::Table(table),
            Err(_) => value,
        }
    }

    pub(super) fn zip_values(
        &mut self,
        a: Datum,
        b: Datum,
        span: Span,
        operation: impl Fn(&mut Self, Sig, Sig) -> Res<Sig> + Copy,
    ) -> Res<Datum> {
        match (a, b) {
            (Datum::Record(fields), b) => self.zip_values(fields[0].1.clone(), b, span, operation),
            (a, Datum::Record(fields)) => self.zip_values(a, fields[0].1.clone(), span, operation),
            (Datum::Table(t), b) => {
                self.zip_values(Datum::array(table_entries(&t)), b, span, operation)
            }
            (a, Datum::Table(t)) => {
                self.zip_values(a, Datum::array(table_entries(&t)), span, operation)
            }
            (Datum::Sig(a), Datum::Sig(b)) => operation(self, a, b).map(Datum::Sig),
            (Datum::Array(a), Datum::Array(b)) => {
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
                Ok(Datum::Array(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| self.zip_values(a, b, span, operation))
                        .collect::<Res<_>>()?,
                ))
            }
            (Datum::Array(a), Datum::Sig(b)) if b.count() > 1 => {
                let layout = b.layout;
                let b = (0..b.count())
                    .map(|i| Datum::Sig(Self::element(&b, i)))
                    .collect();
                let result = self.zip_values(Datum::Array(a), Datum::Array(b), span, operation)?;
                Self::signal_array(result, layout, span)
            }
            (Datum::Sig(a), Datum::Array(b)) if a.count() > 1 => {
                let layout = a.layout;
                let a = (0..a.count())
                    .map(|i| Datum::Sig(Self::element(&a, i)))
                    .collect();
                let result = self.zip_values(Datum::Array(a), Datum::Array(b), span, operation)?;
                Self::signal_array(result, layout, span)
            }
            (Datum::Array(a), b @ Datum::Sig(_)) => Ok(Datum::Array(
                a.into_iter()
                    .map(|a| self.zip_values(a, b.clone(), span, operation))
                    .collect::<Res<_>>()?,
            )),
            (a @ Datum::Sig(_), Datum::Array(b)) => Ok(Datum::Array(
                b.into_iter()
                    .map(|b| self.zip_values(a.clone(), b, span, operation))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new("arithmetic doesn't work on ranges", span)),
        }
    }

    pub(super) fn signal_array(value: Datum, layout: Layout, span: Span) -> Res<Datum> {
        if let Datum::Array(items) = &value
            && items.len() == layout.channels()
            && items
                .iter()
                .all(|v| matches!(v, Datum::Sig(sig) if sig.count() == 1))
        {
            let entries = items
                .iter()
                .map(|v| match v {
                    Datum::Sig(sig) => (sig.clone(), span),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            let mut signal = join_channels(&entries, span)?;
            signal.layout = layout;
            return Ok(Datum::Sig(signal));
        }
        Ok(value)
    }

    pub(super) fn map_value(
        &mut self,
        value: Datum,
        span: Span,
        operation: impl Fn(&mut Self, Sig) -> Res<Sig> + Copy,
    ) -> Res<Datum> {
        match value {
            Datum::Sig(value) => operation(self, value).map(Datum::Sig),
            Datum::Array(items) => Ok(Datum::Array(
                items
                    .into_iter()
                    .map(|v| self.map_value(v, span, operation))
                    .collect::<Res<_>>()?,
            )),
            Datum::Record(fields) => self.map_value(fields[0].1.clone(), span, operation),
            Datum::Table(table) => {
                let mapped =
                    self.map_value(Datum::array(table_entries(&table)), span, operation)?;
                Ok(self.tabled(mapped))
            }
            _ => Err(Diagnostic::new(
                "this operation needs a number or an array",
                span,
            )),
        }
    }

    /// `list.map(f)`, `list.sum`, `list.len`, `list.indices`, `list.min`, `list.max`, `list.at(pos)`
    /// and `signal.channels`.
    pub(super) fn array_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                format!("`{}` is called on an array", name.name),
                name.span,
            )
            .with_help(format!("for example `[1, 2, 3].{}`", name.name)));
        };
        let value = self.value(&receiver.value)?;
        let layout = match &value {
            Datum::Sig(signal) => Some(signal.layout),
            _ => None,
        };
        let no_args = |what: &str| -> Res<()> {
            if args.len() > 1 {
                return Err(Diagnostic::new(
                    format!("`{what}` takes no arguments"),
                    name.span,
                ));
            }
            Ok(())
        };
        if name.name == "channels" {
            no_args("channels")?;
            return match value {
                Datum::Function { .. } => Err(Diagnostic::new(
                    "a function has no channels",
                    receiver.value.span,
                )),
                list @ (Datum::Array(_) | Datum::Table(_) | Datum::Buffer { .. }) => Ok(list),
                Datum::Sig(sig) => Ok(Datum::Array(
                    (0..sig.count())
                        .map(|k| Datum::Sig(Self::element(&sig, k)))
                        .collect(),
                )),
                Datum::Record(fields) => Ok(Datum::array(self.entries(fields[0].1.clone(), span)?)),
                Datum::BufferRef { .. } => Err(Diagnostic::new(
                    "a buffer reference has no channel array",
                    receiver.value.span,
                )),
                Datum::Range { .. } => Err(Diagnostic::new(
                    "a range has no channels",
                    receiver.value.span,
                )),
            };
        }
        if name.name == "at" {
            return self
                .at(name, args, value, receiver.value.span)
                .map(Datum::Sig);
        }
        if name.name == "phase" {
            return self
                .phase(name, args, value, receiver.value.span)
                .map(Datum::Sig);
        }
        if let Some(result) = self.table_transform(name, args, &value, span)? {
            return Ok(result);
        }
        if name.name == "step" {
            return self.step(name, args, value, receiver.value.span);
        }
        let items = self.entries(value, receiver.value.span)?;
        if name.name == "active" {
            return self.active(name, args, items, span);
        }
        match name.name.as_str() {
            "len" => {
                no_args("len")?;
                Ok(Datum::Sig(Sig::constant(Unit::Num, items.len() as f64)))
            }
            "indices" => {
                no_args("indices")?;
                Ok(Datum::Range {
                    lo: Sig::constant(Unit::Num, 0.0),
                    hi: Sig::constant(Unit::Num, items.len() as f64),
                    inclusive: false,
                })
            }
            "sum" | "min" | "max" => {
                no_args(&name.name)?;
                let mut total: Option<Datum> = None;
                for item in items {
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
            _ => {
                let result = self.map(name, args, items)?;
                match layout {
                    Some(layout) => Self::signal_array(result, layout, span),
                    None => Ok(result),
                }
            }
        }
    }

    /// `(0..1).step(0.25)`: the values of a range in steps.
    pub(super) fn step(
        &mut self,
        name: &Ident,
        args: &[Arg],
        range: Datum,
        span: Span,
    ) -> Res<Datum> {
        let Datum::Range { lo, hi, inclusive } = range else {
            return Err(Diagnostic::new("`step` is called on a range", span)
                .with_help("for example `(0..1).step(0.25)`"));
        };
        let [_, size] = args else {
            return Err(Diagnostic::new(
                "`step` takes the size of a step",
                name.span,
            ));
        };
        let size_sig = self.expr(&size.value)?;
        let (Some(a), Some(b), Some(size_k)) = (lo.konst, hi.konst, size_sig.konst) else {
            return Err(Diagnostic::new(
                "the ends of a range and its step must be known while compiling",
                span,
            ));
        };
        if size_sig.unit != lo.unit || lo.unit == Unit::Db {
            return Err(Diagnostic::new(
                "a step has the same unit as the range, and a db range can't be stepped",
                size.value.span,
            ));
        }
        let values = crate::kernel::step_numbers(a, b, inclusive, size_k, span)?;
        Ok(Datum::array(
            values
                .into_iter()
                .map(|v| Datum::Sig(Sig::constant(lo.unit, v)))
                .collect(),
        ))
    }

    /// `xs.active(count)`: the first `count` entries sound and the rest are silent. `count` can change
    /// while playing, which a loop's length can't, so build the most there can be and switch the rest off.
    pub(super) fn active(
        &mut self,
        name: &Ident,
        args: &[Arg],
        items: Vec<Datum>,
        span: Span,
    ) -> Res<Datum> {
        let [_, count] = args else {
            return Err(Diagnostic::new(
                "`active` takes how many entries are on",
                name.span,
            ));
        };
        let count = self.expr(&count.value)?;
        if count.unit != Unit::Num {
            return Err(Diagnostic::new(
                "`active` takes a plain number of entries",
                span,
            ));
        }
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let on = self.compare(
                CmpOp::Gt,
                count.clone(),
                Sig::constant(Unit::Num, i as f64),
                span,
            )?;
            out.push(self.silence_unless(&on, item, span)?);
        }
        Ok(Datum::array(out))
    }

    /// `item` where `on` is not 0, and zero (of the same unit) elsewhere.
    pub(super) fn silence_unless(&mut self, on: &Sig, item: Datum, span: Span) -> Res<Datum> {
        match item {
            Datum::Sig(sig) => {
                let off = Sig::constant(sig.unit, 0.0);
                self.select_sig(on, sig, off, span).map(Datum::Sig)
            }
            Datum::Array(inner) => Ok(Datum::Array(
                inner
                    .into_iter()
                    .map(|d| self.silence_unless(on, d, span))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new(
                "`active` works on arrays of numbers and signals",
                span,
            )),
        }
    }

    /// `t.phase(p)`: read at normalized position 0..1.
    pub(super) fn phase(
        &mut self,
        name: &Ident,
        args: &[Arg],
        list: Datum,
        span: Span,
    ) -> Res<Sig> {
        let mut pos = None;
        let mut interp = Interp::Linear;
        for a in &args[1..] {
            match a.name.as_ref().map(|n| n.name.as_str()) {
                None if pos.is_none() => pos = Some(self.expr(&a.value)?),
                Some("interp") => interp = interp_of(&a.value)?,
                _ => {
                    return Err(Diagnostic::new(
                        "`phase` takes a position in 0..1 and `interp:`",
                        a.value.span,
                    ));
                }
            }
        }
        let Some(pos) = pos else {
            return Err(Diagnostic::new(
                "`phase` takes a position in 0..1",
                name.span,
            ));
        };
        if let Datum::Buffer { id, data: buffer } = &list {
            self.g.touch_buffer_read(*id);
            let len = Sig::constant(Unit::Num, buffer.len() as f64);
            let index = self.mul(&pos, &len, Unit::Num, span)?;
            let rate = self.sig_rate(&index);
            let srcs: Array<Src> = index
                .srcs
                .iter()
                .map(|&p| {
                    let src = self
                        .g
                        .add(BufferRead::new(buffer.clone(), interp), &[("pos", p)])
                        .expect("buffer ports");
                    self.note_rate(src, rate);
                    src
                })
                .collect();
            return Ok(Sig {
                srcs,
                layout: index.layout,
                unit: Unit::Num,
                konst: None,
            });
        }
        let table = self.as_table(list, span)?;
        if table.is_empty() {
            return Err(Diagnostic::new("this array is empty", span));
        }
        let len = Sig::constant(Unit::Num, table.len() as f64);
        let index = self.mul(&pos, &len, Unit::Num, span)?;
        self.table_read(table, Unit::Num, &index, interp, Edge::Wrap, span)
    }

    pub(super) fn table_transform(
        &mut self,
        name: &Ident,
        args: &[Arg],
        value: &Datum,
        span: Span,
    ) -> Res<Option<Datum>> {
        let table = match value {
            Datum::Table(t) => t.clone(),
            Datum::Array(_)
                if matches!(
                    name.name.as_str(),
                    "normalize"
                        | "reverse"
                        | "rotate"
                        | "resample"
                        | "concat"
                        | "integrate"
                        | "mix"
                        | "slice"
                        | "frames"
                        | "spectrum"
                        | "phases"
                        | "mean"
                        | "rms"
                        | "remove_dc"
                        | "moving_average"
                        | "fade"
                ) =>
            {
                self.as_table(value.clone(), span)?
            }
            _ => return Ok(None),
        };
        Ok(Some(match name.name.as_str() {
            "normalize" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`normalize` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.normalize()))
            }
            "reverse" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`reverse` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.reverse()))
            }
            "integrate" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`integrate` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.integrate()))
            }
            "rotate" => {
                let amount = match args.get(1) {
                    Some(a) => self.expr(&a.value)?.konst.unwrap_or(0.0) as isize,
                    None => {
                        return Err(Diagnostic::new("`rotate` takes an amount", name.span));
                    }
                };
                Datum::Table(Arc::new(table.rotate(amount)))
            }
            "resample" => {
                let size = match args.get(1) {
                    Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
                    None => {
                        return Err(Diagnostic::new("`resample` takes a size", name.span));
                    }
                };
                Datum::Table(Arc::new(table.resample(size)))
            }
            "concat" => {
                let other = match args.get(1) {
                    Some(a) => {
                        let v = self.value(&a.value)?;
                        self.as_table(v, a.value.span)?
                    }
                    None => {
                        return Err(Diagnostic::new("`concat` takes another table", name.span));
                    }
                };
                Datum::Table(Arc::new(table.concat(&other)))
            }
            "slice" => {
                let bound = funcs::bind("slice", name.span, &["start", "end"], 2, &args[1..])?;
                let len = table.len();
                let start = self.whole_number(&bound[0].expect("required").value, 0, len)?;
                let end = match bound[1] {
                    Some(a) => self.whole_number(&a.value, 0, len)?,
                    None => len,
                };
                if end <= start {
                    return Err(Diagnostic::new(
                        "a slice of a table needs `end` after `start`",
                        name.span,
                    ));
                }
                Datum::Table(Arc::new(table.slice(start, end)))
            }
            "frames" => {
                let size = self.whole_number(&args[1].value, 1, table.len())?;
                Datum::array(
                    table
                        .frames(size)
                        .into_iter()
                        .map(|t| Datum::Table(Arc::new(t)))
                        .collect(),
                )
            }
            "spectrum" | "phases" => {
                let count = match args.get(1) {
                    Some(a) => self.whole_number(&a.value, 1, 4096)?,
                    None => {
                        return Err(Diagnostic::new(
                            format!("`{}` takes how many harmonics to measure", name.name),
                            name.span,
                        ));
                    }
                };
                let (amps, phases) = crate::tables::spectrum(&table, count);
                let found = if name.name == "spectrum" {
                    amps
                } else {
                    phases
                };
                Datum::array(
                    found
                        .into_iter()
                        .map(|v| Datum::Sig(Sig::constant(Unit::Num, v)))
                        .collect(),
                )
            }
            "mean" | "rms" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new(
                        format!("`{}` takes no arguments", name.name),
                        name.span,
                    ));
                }
                let v = if name.name == "mean" {
                    table.mean()
                } else {
                    table.rms()
                };
                Datum::Sig(Sig::constant(Unit::Num, v))
            }
            "remove_dc" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`remove_dc` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.remove_dc()))
            }
            "moving_average" | "fade" => {
                let what = if name.name == "moving_average" {
                    "a width"
                } else {
                    "a length"
                };
                let Some(a) = args.get(1) else {
                    return Err(Diagnostic::new(
                        format!("`{}` takes {what} in entries", name.name),
                        name.span,
                    ));
                };
                let n = self.whole_number(&a.value, 1, table.len())?;
                Datum::Table(Arc::new(if name.name == "moving_average" {
                    table.moving_average(n)
                } else {
                    table.fade(n)
                }))
            }
            "mix" => {
                let names = ["other", "amount"];
                let bound = funcs::bind("mix", name.span, &names, 1, &args[1..])?;
                let Some(other_arg) = bound[0] else {
                    return Err(Diagnostic::new("`mix` takes another table", name.span));
                };
                let other_v = self.value(&other_arg.value)?;
                let other = self.as_table(other_v, other_arg.value.span)?;
                let amount = match bound[1] {
                    Some(a) => self.expr(&a.value)?.konst.unwrap_or(0.5),
                    None => 0.5,
                };
                Datum::Table(Arc::new(table.mix(&other, amount)))
            }
            _ => return Ok(None),
        }))
    }

    /// `take.record(sig, loop: true)`: write consecutive samples into a shared buffer.
    pub(super) fn buffer_record(&mut self, name: &Ident, args: &[Arg], _span: Span) -> Res<Datum> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new("`record` is called on a buffer", name.span));
        };
        let Datum::Buffer { id, data: buffer } = self.value(&receiver.value)? else {
            return Err(Diagnostic::new(
                "`record` needs a shared buffer",
                receiver.value.span,
            ));
        };
        if self.in_fn {
            return Err(Diagnostic::new(
                "a shared buffer can't be written inside a function",
                name.span,
            ));
        }
        let names = ["x", "loop", "channel"];
        let bound = funcs::bind("record", name.span, &names, 1, &args[1..])?;
        let Some(x_arg) = bound[0] else {
            return Err(Diagnostic::new(
                "`record` takes a signal to write",
                name.span,
            ));
        };
        let looped = match bound[1] {
            Some(a) => match &a.value.kind {
                ExprKind::Name(n) if n == "true" || n == "forever" => true,
                ExprKind::Name(n) if n == "false" || n == "once" => false,
                _ => {
                    return Err(Diagnostic::new(
                        "`loop` is `true`/`forever` or `false`/`once`",
                        a.value.span,
                    ));
                }
            },
            None => true,
        };
        self.g.touch_buffer_write(id);
        let x = self.expr(&x_arg.value)?;
        let channel = if let Some(arg) = bound[2] {
            let value = self.value(&arg.value)?;
            let value = self.signal_of(value, arg.value.span)?;
            let Some(n) = value.konst else {
                return Err(Diagnostic::new(
                    "record channel is a constant 1-based integer",
                    arg.value.span,
                ));
            };
            if n < 1. || n.fract() != 0. {
                return Err(Diagnostic::new(
                    "record channel is a constant 1-based integer",
                    arg.value.span,
                ));
            }
            n as usize - 1
        } else {
            0
        };
        if channel + x.srcs.len() > buffer.channels() {
            return Err(Diagnostic::new(
                "record signal exceeds the buffer channel layout",
                x_arg.value.span,
            ));
        }
        let rate = self.sig_rate(&x);
        let srcs: Array<Src> = x
            .srcs
            .iter()
            .enumerate()
            .map(|(c, &s)| {
                let mut ports = vec![("x", s)];
                if let Some(gate) = self.write_gate {
                    ports.push(("gate", gate));
                }
                let src = self
                    .g
                    .add(
                        BufferRecord::new(buffer.clone(), looped).with_channel(channel + c),
                        &ports,
                    )
                    .expect("record ports");
                self.note_rate(src, rate);
                src
            })
            .collect();
        Ok(Datum::Sig(Sig {
            srcs,
            layout: x.layout,
            unit: Unit::Num,
            konst: None,
        }))
    }

    /// `kick.data`: the sample's underlying table(s).
    pub(super) fn sample_data(&mut self, name: &Ident, receiver: &Expr) -> Res<Datum> {
        let ExprKind::Name(sample) = &receiver.kind else {
            return Err(Diagnostic::new(
                "`.data` is used on a sample name",
                receiver.span,
            ));
        };
        let Some(channels) = self.names.sample_data.get(sample) else {
            let mut d = Diagnostic::new(format!("no sample named `{sample}`"), receiver.span);
            if let Some(c) = closest(sample, self.names.sample_data.keys().map(String::as_str)) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        match channels.as_slice() {
            [] => Err(Diagnostic::new("this sample has no audio", name.span)),
            [one] => Ok(Datum::Table(one.clone())),
            many => Ok(Datum::Array(
                many.iter().cloned().map(Datum::Table).collect(),
            )),
        }
    }

    pub(super) fn same_unit(&self, a: &Sig, b: &Sig, what: &str, span: Span) -> Res<()> {
        if a.unit != b.unit {
            return Err(Diagnostic::new(
                format!(
                    "`{what}` needs values with the same unit: {} and {}",
                    a.unit.describe(),
                    b.unit.describe()
                ),
                span,
            ));
        }
        Ok(())
    }

    pub(super) fn read_buffer_ref(
        &mut self,
        selector: &Sig,
        choices: &BufferChoices,
        pos: &Sig,
        interp: Interp,
        span: Span,
    ) -> Res<Sig> {
        self.read_buffer_channel(selector, choices, pos, interp, 0, span)
    }
    pub(super) fn read_buffer_channel(
        &mut self,
        selector: &Sig,
        choices: &BufferChoices,
        pos: &Sig,
        interp: Interp,
        channel: usize,
        span: Span,
    ) -> Res<Sig> {
        if pos.unit != Unit::Num || pos.count() != 1 {
            return Err(Diagnostic::new(
                "a buffer position is one plain number",
                span,
            ));
        }
        for (name, _) in choices.iter() {
            self.g.touch_buffer_read(self.names.buffers[name].0);
        }
        let buffers = Arc::new(choices.iter().map(|(_, b)| b.clone()).collect());
        let src = self
            .g
            .add(
                niminal_engine::ops::BufferReadBank::new(buffers, interp, choices.key)
                    .channel(channel),
                &[("source", selector.srcs[0]), ("pos", pos.srcs[0])],
            )
            .expect("buffer reader ports");
        Ok(Sig::signal(src, Unit::Num))
    }

    /// `table.at(pos, interp: linear, edge: wrap)`: a list of numbers read at a fractional position.
    pub(super) fn at(&mut self, name: &Ident, args: &[Arg], list: Datum, span: Span) -> Res<Sig> {
        let mut pos = None;
        let mut interp = Interp::Linear;
        let mut edge = Edge::Wrap;
        let mut channel = 0usize;
        for a in &args[1..] {
            match a.name.as_ref().map(|n| n.name.as_str()) {
                None if pos.is_none() => pos = Some(self.expr(&a.value)?),
                Some("interp") => interp = interp_of(&a.value)?,
                Some("edge") => edge = edge_of(&a.value)?,
                Some("channel") => {
                    let value = self
                        .expr(&a.value)?
                        .konst()
                        .ok_or_else(|| Diagnostic::new("channel is constant", a.value.span))?;
                    if value.fract() != 0. || !(1.0..=16.).contains(&value) {
                        return Err(Diagnostic::new("channel is 1–16", a.value.span));
                    }
                    channel = value as usize - 1;
                }
                _ => {
                    return Err(Diagnostic::new(
                        "`at` takes a position, `interp:` and `edge:`",
                        a.value.span,
                    ));
                }
            }
        }
        let Some(pos) = pos else {
            return Err(Diagnostic::new("`at` takes a position", name.span));
        };
        if pos.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "a position is a plain number, found {}",
                    pos.unit.describe()
                ),
                name.span,
            ));
        }
        if let Datum::Table(table) = list {
            if table.is_empty() {
                return Err(Diagnostic::new("this array is empty", span));
            }
            return self.table_read(table, Unit::Num, &pos, interp, edge, span);
        }
        if let Datum::BufferRef { selector, choices } = &list {
            return self.read_buffer_channel(selector, choices, &pos, interp, channel, span);
        }
        if let Datum::Buffer { id, data: buffer } = list {
            self.g.touch_buffer_read(id);
            let rate = self.sig_rate(&pos);
            let srcs: Array<Src> = pos
                .srcs
                .iter()
                .map(|&p| {
                    let src = self
                        .g
                        .add(
                            BufferRead::new(buffer.clone(), interp).channel(channel),
                            &[("pos", p)],
                        )
                        .expect("buffer ports");
                    self.note_rate(src, rate);
                    src
                })
                .collect();
            return Ok(Sig {
                srcs,
                layout: pos.layout,
                unit: Unit::Num,
                konst: None,
            });
        }
        let items = self.entries(list, span)?;
        let sigs = items
            .into_iter()
            .map(|item| match item {
                Datum::Sig(sig) if sig.count() == 1 => Ok(sig),
                _ => Err(Diagnostic::new(
                    "`at` reads a one-dimensional numeric array",
                    span,
                )),
            })
            .collect::<Res<Vec<_>>>()?;
        if edge != Edge::Wrap {
            let data = Arc::new(Table::from_f64(
                sigs.iter()
                    .map(|s| {
                        s.konst.ok_or_else(|| {
                            Diagnostic::new("`edge:` needs a table known while compiling", span)
                        })
                    })
                    .collect::<Res<Vec<_>>>()?,
                None,
            ));
            return self.table_read(data, sigs[0].unit, &pos, interp, edge, span);
        }
        self.read_values(sigs, &pos, interp, span)
    }

    /// The function runs once for each entry, while compiling, and the results are the new list.
    pub(super) fn map(&mut self, name: &Ident, args: &[Arg], items: Vec<Datum>) -> Res<Datum> {
        let [_, f] = args else {
            return Err(Diagnostic::new("`map` takes a function", name.span)
                .with_help("for example `[1, 2, 3].map(fn(x) { x * 2 })` or `.map(it * 2)`"));
        };
        let mapped = match &f.value.kind {
            ExprKind::Name(n)
                if self.names.fns.contains_key(n)
                    || matches!(self.scope.get(n), Some(Datum::Function { .. })) =>
            {
                Some(funcs::mapped_call(n, f.value.span))
            }
            _ => None,
        };
        let (param, body) = lambda_of(mapped.as_ref().unwrap_or(&f.value))?;
        if param != "it" {
            self.names.check_local_in(
                &Ident {
                    name: param.clone(),
                    span: f.value.span,
                },
                self.in_fn,
            )?;
        }
        if self.scope.contains_key(&param) {
            return Err(Diagnostic::new(
                format!("`{param}` is already in use, so it can't be this function's parameter"),
                f.value.span,
            ));
        }
        let mut results = Vec::with_capacity(items.len());
        for item in items {
            self.scope.insert(param.clone(), item);
            let v = match &body {
                Body::Block(stmts) => self
                    .block(stmts, true, f.value.span)
                    .map(|v| v.expect("a value")),
                Body::Expr(e) => self.value(e),
            };
            self.scope.remove(&param);
            results.push(v?);
        }
        Ok(Datum::array(results))
    }
}
