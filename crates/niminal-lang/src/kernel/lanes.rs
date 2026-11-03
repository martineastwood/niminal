//! Lanes: vector instructions, `frames` scopes and FFT calls.

use super::*;

// ---- lanes ------------------------------------------------------------------------------------

/// What a vector instruction reads, and what is known about it.
#[derive(Clone, Copy)]
pub(super) struct LaneIn {
    pub(super) src: LaneSrc,
    pub(super) unit: Unit,
    /// `None` for a single value that stands for every entry.
    pub(super) len: Option<usize>,
    pub(super) rate: Rate,
}

impl Kb<'_> {
    pub(super) fn new_lane(&mut self, len: usize, rate: Rate) -> usize {
        let id = self.lanes.len();
        self.lanes.push(len);
        self.lane_rates.push(rate);
        id
    }

    /// What a vector instruction can read of `v`, if it is a number or an array it holds in memory.
    pub(super) fn lane_input(&self, v: &KVal) -> Option<LaneIn> {
        Some(match v {
            KVal::Num(n) => LaneIn {
                src: LaneSrc::Reg(n.reg),
                unit: n.unit,
                len: None,
                rate: n.rate,
            },
            KVal::State { id, unit, len } if *len > 0 => LaneIn {
                src: LaneSrc::State(*id),
                unit: *unit,
                len: Some(*len),
                rate: Rate::Audio,
            },
            KVal::Table { id, unit, values } if !values.is_empty() => LaneIn {
                src: LaneSrc::Table(*id),
                unit: *unit,
                len: Some(values.len()),
                rate: Rate::Const,
            },
            KVal::Lane { id, unit, len } if *len > 0 => LaneIn {
                src: LaneSrc::Lane(*id),
                unit: *unit,
                len: Some(*len),
                rate: self.lane_rates[*id],
            },
            _ => return None,
        })
    }

    /// An array (or a single value) as something a vector instruction reads, gathering computed
    /// elements into a lane.
    pub(super) fn lane_of(&mut self, v: KVal, span: Span) -> Res<LaneIn> {
        if let Some(input) = self.lane_input(&v) {
            return Ok(input);
        }
        let v = match v {
            KVal::Range { .. } => KVal::array(self.array_elements(v, span)?),
            other => other,
        };
        if !matches!(v, KVal::Array(_)) {
            return Err(Diagnostic::new("expected an array of numbers here", span));
        }
        let (source, unit) = self.numeric_array(v, span)?;
        let ArraySource::Values(values) = source else {
            unreachable!("computed elements are held as values")
        };
        let (len, rate) = (self.values[values].len(), self.value_rates[values]);
        let dst = self.new_lane(len, rate);
        self.push(rate, Instr::LaneFromRegs { dst, values });
        Ok(LaneIn {
            src: LaneSrc::Lane(dst),
            unit,
            len: Some(len),
            rate,
        })
    }

    /// Like [`Kb::lane_of`], but `None` for anything that is not a number or a flat array of them.
    pub(super) fn lane_operand(&mut self, v: &KVal, span: Span) -> Res<Option<LaneIn>> {
        if let Some(input) = self.lane_input(v) {
            return Ok(Some(input));
        }
        let flat = match v {
            KVal::Array(items) => {
                !items.is_empty() && items.iter().all(|i| matches!(i, KVal::Num(_)))
            }
            _ => false,
        };
        if flat {
            return self.lane_of(v.clone(), span).map(Some);
        }
        Ok(None)
    }

    /// A copy of a state array as it is now.
    pub(super) fn lane_copy(&mut self, v: KVal, span: Span) -> Res<KVal> {
        let KVal::State { unit, len, .. } = v else {
            return Ok(v);
        };
        if len == 0 || self.force.is_some() {
            return Ok(KVal::array(self.array_elements(v, span)?));
        }
        let input = self.lane_input(&v).expect("a state array");
        let one = self.constant(Unit::Num, 1.0);
        let dst = self.new_lane(len, Rate::Audio);
        self.push(
            Rate::Audio,
            Instr::LaneBin {
                dst,
                op: Op::Mul,
                a: input.src,
                b: LaneSrc::Reg(one.reg),
            },
        );
        Ok(KVal::Lane { id: dst, unit, len })
    }

    /// `a op b` as one vector instruction, when one of them is a state array or a lane and the
    /// units combine plainly. `None` means the caller should work element by element.
    pub(super) fn lane_binary(
        &mut self,
        op: Op,
        unit_of: impl Fn(Unit, Unit) -> Option<Unit>,
        a: &KVal,
        b: &KVal,
        span: Span,
    ) -> Res<Option<KVal>> {
        let held = |v: &KVal| matches!(v, KVal::State { .. } | KVal::Lane { .. });
        if self.force.is_some() || !(held(a) || held(b)) {
            return Ok(None);
        }
        let (Some(x), Some(y)) = (self.lane_operand(a, span)?, self.lane_operand(b, span)?) else {
            return Ok(None);
        };
        let len = match (x.len, y.len) {
            (Some(m), Some(n)) if m != n => {
                return Err(Diagnostic::new(
                    format!("can't combine an array of {m} with an array of {n}"),
                    span,
                ));
            }
            (Some(n), _) | (_, Some(n)) => n,
            (None, None) => return Ok(None),
        };
        let Some(unit) = unit_of(x.unit, y.unit) else {
            return Ok(None);
        };
        let rate = self.rate(x.rate.max(y.rate));
        let dst = self.new_lane(len, rate);
        self.push(
            rate,
            Instr::LaneBin {
                dst,
                op,
                a: x.src,
                b: y.src,
            },
        );
        Ok(Some(KVal::Lane { id: dst, unit, len }))
    }

    /// `x.frames(size, fn(w) { ... })`: the body runs every `size / overlap` samples on the last `size` samples
    /// of `x` (windowed, as an array `w`) and returns `size` samples, which are overlapped into the output.
    /// The output is `size` samples late.
    pub(super) fn frames_scope(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        const WINDOW: &str = "<frame window>";
        if self.in_frame || self.guard.is_some() || self.force.is_some() {
            return Err(Diagnostic::new(
                "`frames` can't be used inside another frame, an `if` or here",
                name.span,
            ));
        }
        let bound = funcs::bind(
            "frames",
            name.span,
            &["x", "size", "body", "other", "overlap"],
            3,
            args,
        )?;
        let need = |i: usize, what: &str| {
            bound[i].ok_or_else(|| Diagnostic::new(format!("`frames` needs {what}"), name.span))
        };
        let x = self.num(&need(0, "a signal")?.value)?;
        if x.unit != Unit::Num {
            return Err(Diagnostic::new(
                "`frames` takes a plain signal",
                need(0, "a signal")?.value.span,
            ));
        }
        let other = bound[3].map(|arg| self.num(&arg.value)).transpose()?;
        if other.as_ref().is_some_and(|v| v.unit != Unit::Num) {
            return Err(Diagnostic::new("`frames` takes a plain signal", span));
        }
        let size_arg = need(1, "a size")?;
        let size = self.num(&size_arg.value)?;
        let size = match self.konst(&size) {
            Some(k)
                if k.fract() == 0.0
                    && (4.0..=65536.0).contains(&k)
                    && (k as usize).is_power_of_two() =>
            {
                k as usize
            }
            _ => {
                return Err(Diagnostic::new(
                    "a frame size is a power of two from 4 to 65536, known while compiling",
                    size_arg.value.span,
                ));
            }
        };
        let overlap = if let Some(arg) = bound[4] {
            let value = self.num(&arg.value)?;
            match self.konst(&value) {
                Some(n)
                    if value.unit == Unit::Num
                        && [2.0, 4.0, 8.0, 16.0].contains(&n)
                        && n <= size as f64 =>
                {
                    n as usize
                }
                _ => {
                    return Err(Diagnostic::new(
                        "frame overlap is 2, 4, 8 or 16, no larger than size, known while compiling",
                        arg.value.span,
                    ));
                }
            }
        } else {
            2
        };
        let body_arg = need(2, "a function")?;
        let KVal::Function { def, captures } = self.value(&body_arg.value)? else {
            return Err(Diagnostic::new(
                "the `body` argument of `frames` is a function, as in `fn(w) { ... }`",
                body_arg.value.span,
            ));
        };
        let window = self.new_lane(size, Rate::Audio);
        self.frame.scope.insert(
            WINDOW.into(),
            KVal::Lane {
                id: window,
                unit: Unit::Num,
                len: size,
            },
        );
        let given = Arg {
            name: None,
            receiver: false,
            value: Expr {
                kind: ExprKind::Name(WINDOW.into()),
                span: body_arg.value.span,
            },
        };
        let mut given = vec![given];
        let window2 = other.as_ref().map(|_| {
            let lane = self.new_lane(size, Rate::Audio);
            self.frame.scope.insert(
                "<frame window 2>".into(),
                KVal::Lane {
                    id: lane,
                    unit: Unit::Num,
                    len: size,
                },
            );
            given.push(Arg {
                name: None,
                receiver: false,
                value: Expr {
                    kind: ExprKind::Name("<frame window 2>".into()),
                    span,
                },
            });
            lane
        });
        // the body's code is kept apart, to run when a frame is ready
        let outer = std::mem::take(&mut self.code);
        self.in_frame = true;
        let called = self.call_captured(&def, name, &given, captures);
        let result = called.and_then(|result| self.lane_of(result, span));
        self.in_frame = false;
        let code = std::mem::replace(&mut self.code, outer);
        self.frame.scope.remove(WINDOW);
        self.frame.scope.remove("<frame window 2>");
        let result = result?;
        if result.len != Some(size) || result.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("the function of `frames` returns {size} plain samples"),
                body_arg.value.span,
            ));
        }
        let scope = self.frames.len();
        self.frames.push(niminal_engine::ops::FrameScope {
            size,
            hop: size / overlap,
            window,
            window2,
            result: result.src,
            code,
        });
        let reg = self.emit(Rate::Audio, |dst| Instr::Frame {
            scope,
            input: x.reg,
            input2: other.map(|v| v.reg),
            dst,
        });
        Ok(KVal::Num(Value {
            reg,
            unit: Unit::Num,
            rate: Rate::Audio,
            port: None,
        }))
    }

    /// The FFT plan for `size`, made once.
    pub(super) fn fft_plan(&mut self, size: usize) -> usize {
        match self.ffts.iter().position(|&n| n == size) {
            Some(plan) => plan,
            None => {
                self.ffts.push(size);
                self.ffts.len() - 1
            }
        }
    }

    /// `fft(w)`: the spectrum of an array of `n` samples, as `[re, im]` with `n / 2 + 1` entries each.
    pub(super) fn fft_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let [arg] = args else {
            return Err(Diagnostic::new("`fft` takes an array", name.span));
        };
        let value = self.value(&arg.value)?;
        let input = self.lane_of(value, arg.value.span)?;
        let n = input
            .len
            .filter(|n| n.is_power_of_two() && *n >= 4)
            .ok_or_else(|| {
                Diagnostic::new(
                    "`fft` takes an array whose length is a power of two, at least 4",
                    span,
                )
            })?;
        if input.unit != Unit::Num {
            return Err(Diagnostic::new("`fft` takes plain numbers", arg.value.span));
        }
        let rate = self.rate(input.rate);
        let (re, im) = (
            self.new_lane(n / 2 + 1, rate),
            self.new_lane(n / 2 + 1, rate),
        );
        let plan = self.fft_plan(n);
        self.push(
            rate,
            Instr::Fft {
                re,
                im,
                src: input.src,
                plan,
            },
        );
        let lane = |id| KVal::Lane {
            id,
            unit: Unit::Num,
            len: n / 2 + 1,
        };
        Ok(KVal::array(vec![lane(re), lane(im)]))
    }

    /// `ifft(re, im)` (or `ifft([re, im])`): the `n` samples with that spectrum.
    pub(super) fn ifft_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let parts = match args {
            [spectrum] => match self.value(&spectrum.value)? {
                KVal::Array(items) if items.len() == 2 => items.into_vec(),
                _ => {
                    return Err(Diagnostic::new(
                        "`ifft` takes the real and imaginary parts, as `ifft(re, im)`",
                        name.span,
                    ));
                }
            },
            [re, im] => vec![self.value(&re.value)?, self.value(&im.value)?],
            _ => {
                return Err(Diagnostic::new(
                    "`ifft` takes the real and imaginary parts, as `ifft(re, im)`",
                    name.span,
                ));
            }
        };
        let mut lanes = Vec::new();
        for part in parts {
            lanes.push(self.lane_of(part, span)?);
        }
        let (re, im) = (lanes[0], lanes[1]);
        let bins = match (re.len, im.len) {
            (Some(a), Some(b)) if a == b && a >= 3 => a,
            _ => {
                return Err(Diagnostic::new(
                    "`ifft` takes two arrays of the same length, `n / 2 + 1` for a signal of `n`",
                    span,
                ));
            }
        };
        let n = (bins - 1) * 2;
        if !n.is_power_of_two() || re.unit != Unit::Num || im.unit != Unit::Num {
            return Err(Diagnostic::new(
                "`ifft` makes a signal whose length is a power of two, from plain numbers",
                span,
            ));
        }
        let rate = self.rate(re.rate.max(im.rate));
        let dst = self.new_lane(n, rate);
        let plan = self.fft_plan(n);
        self.push(
            rate,
            Instr::Ifft {
                dst,
                re: re.src,
                im: im.src,
                plan,
            },
        );
        Ok(KVal::Lane {
            id: dst,
            unit: Unit::Num,
            len: n,
        })
    }

    /// `dot`, `fir`, `taps` and `matvec`: arrays in, one instruction.
    pub(super) fn vector_call(
        &mut self,
        name: &Ident,
        args: &[Arg],
        l: KVal,
        span: Span,
    ) -> Res<KVal> {
        let op = name.name.as_str();
        let (params, positional): (&[&str], usize) = match op {
            "dot" => (&["a", "b"], 2),
            "fir" => (&["ring", "taps", "from"], 3),
            "taps" => (&["ring", "offsets", "from", "interp"], 3),
            _ => (&["matrix", "v"], 2),
        };
        let bound = funcs::bind(op, name.span, params, positional, args)?;
        let given = |i: usize| bound[i];
        let need = |i: usize| {
            given(i)
                .ok_or_else(|| Diagnostic::new(format!("`{op}` needs `{}`", params[i]), name.span))
        };
        let first = need(0)?.value.span;
        let rate_of = |this: &Self, rates: &[Rate]| {
            this.rate(rates.iter().copied().max().unwrap_or(Rate::Const))
        };
        let product = |a: Unit, b: Unit| {
            crate::unit::mul_unit(a, b).ok_or_else(|| {
                Diagnostic::new(
                    format!("can't multiply {} and {}", a.describe(), b.describe()),
                    span,
                )
            })
        };
        let scalar = |this: &mut Self, reg: usize, unit: Unit, rate: Rate| {
            KVal::Num(Value {
                reg: {
                    let _ = this;
                    reg
                },
                unit,
                rate,
                port: None,
            })
        };
        match op {
            "dot" => {
                let a = self.lane_of(l, first)?;
                let other = self.value(&need(1)?.value)?;
                let b = self.lane_of(other, need(1)?.value.span)?;
                if a.len.is_none() || a.len != b.len {
                    return Err(Diagnostic::new(
                        "`dot` takes two arrays of the same length",
                        span,
                    ));
                }
                let unit = product(a.unit, b.unit)?;
                let rate = rate_of(self, &[a.rate, b.rate]);
                let reg = self.emit(rate, |dst| Instr::Dot {
                    dst,
                    a: a.src,
                    b: b.src,
                });
                Ok(scalar(self, reg, unit, rate))
            }
            "fir" | "taps" => {
                let KVal::State {
                    id: ring,
                    unit,
                    len: ring_len,
                } = l
                else {
                    return Err(Diagnostic::new(
                        format!("`{op}` reads a state array, such as `state buf = zeros(1024)`"),
                        first,
                    ));
                };
                if ring_len == 0 {
                    return Err(Diagnostic::new("this array is empty", first));
                }
                let array = self.value(&need(1)?.value)?;
                let array = self.lane_of(array, need(1)?.value.span)?;
                let Some(count) = array.len else {
                    return Err(Diagnostic::new(
                        format!("`{op}` takes an array of {}", params[1]),
                        need(1)?.value.span,
                    ));
                };
                let from = self.num(&need(2)?.value)?;
                if from.unit != Unit::Num {
                    return Err(Diagnostic::new(
                        "a position is a plain number",
                        need(2)?.value.span,
                    ));
                }
                if op == "fir" {
                    if count > ring_len {
                        return Err(Diagnostic::new(
                            format!("{count} taps are more than the {ring_len} entries to read"),
                            need(1)?.value.span,
                        ));
                    }
                    let unit = product(array.unit, unit)?;
                    let rate = Rate::Audio;
                    let reg = self.emit(rate, |dst| Instr::Fir {
                        dst,
                        taps: array.src,
                        ring,
                        from: from.reg,
                    });
                    return Ok(scalar(self, reg, unit, rate));
                }
                if array.unit != Unit::Num {
                    return Err(Diagnostic::new(
                        "offsets are plain numbers of samples",
                        need(1)?.value.span,
                    ));
                }
                let interp = match given(3) {
                    Some(a) => interp_of(&a.value)?,
                    None => Interp::Linear,
                };
                let dst = self.new_lane(count, Rate::Audio);
                self.push(
                    Rate::Audio,
                    Instr::Taps {
                        dst,
                        ring,
                        offsets: array.src,
                        from: from.reg,
                        interp,
                    },
                );
                Ok(KVal::Lane {
                    id: dst,
                    unit,
                    len: count,
                })
            }
            _ => {
                let m = self.lane_of(l, first)?;
                let other = self.value(&need(1)?.value)?;
                let v = self.lane_of(other, need(1)?.value.span)?;
                let (Some(total), Some(cols)) = (m.len, v.len) else {
                    return Err(Diagnostic::new(
                        "`matvec` takes a matrix and a vector",
                        span,
                    ));
                };
                if total % cols != 0 {
                    return Err(Diagnostic::new(
                        format!("a matrix of {total} entries does not have rows of {cols}"),
                        span,
                    ));
                }
                let unit = product(m.unit, v.unit)?;
                let rate = rate_of(self, &[m.rate, v.rate]);
                let dst = self.new_lane(total / cols, rate);
                self.push(
                    rate,
                    Instr::MatVec {
                        dst,
                        matrix: m.src,
                        cols,
                        v: v.src,
                    },
                );
                Ok(KVal::Lane {
                    id: dst,
                    unit,
                    len: total / cols,
                })
            }
        }
    }
}
