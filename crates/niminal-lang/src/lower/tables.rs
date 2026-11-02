//! Table and wave constructors, `combine`/`clamp` helpers and argument parsing.

use super::*;

impl Lower<'_> {
    /// The values of a call such as `min(a, b)` or `a.mod(b)`, without names.
    pub(super) fn plain_args(&mut self, name: &Ident, args: &[Arg], count: usize) -> Res<Vec<(Datum, Span)>> {
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
    pub(super) fn combine(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let mut values = self.plain_args(name, args, 2)?;
        let (b, b_span) = values.pop().unwrap();
        let (a, _) = values.pop().unwrap();
        self.zip_values(a, b, b_span, |this, a, b| {
            this.combine_scalar(name, a, b, b_span)
        })
    }

    pub(super) fn combine_scalar(&mut self, name: &Ident, a: Sig, b: Sig, b_span: Span) -> Res<Sig> {
        match name.name.as_str() {
            "mod" => self.rem(a, b, b_span),
            "atan2" => {
                self.same_unit(&a, &b, "atan2", b_span)?;
                if let (Some(x), Some(y)) = (a.konst, b.konst) {
                    return Ok(Sig::constant(Unit::Num, x.atan2(y)));
                }
                self.elementwise(&a, &b, Unit::Num, b_span, |g, x, y| {
                    g.add(Atan2, &[("a", x), ("b", y)]).expect("atan2 ports")
                })
            }
            "pow" => {
                if a.unit != Unit::Num || b.unit != Unit::Num {
                    return Err(Diagnostic::new("`pow` expects plain numbers", b_span));
                }
                if let (Some(x), Some(y)) = (a.konst, b.konst) {
                    return Ok(Sig::constant(Unit::Num, niminal_engine::ops::pow(x, y)));
                }
                self.elementwise(&a, &b, Unit::Num, b_span, |g, x, y| {
                    g.add(Pow, &[("a", x), ("b", y)]).expect("pow ports")
                })
            }
            m => {
                self.same_unit(&a, &b, m, b_span)?;
                if m == "min" {
                    self.min_sig(&a, &b, b_span)
                } else {
                    self.max_sig(&a, &b, b_span)
                }
            }
        }
    }

    /// `x.clamp(lo..hi)`
    pub(super) fn clamp(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`clamp` takes a range", name.span)
                .with_help("for example `cutoff.clamp(20hz..18khz)`"));
        };
        let v = self.value(&x.value)?;
        let Datum::Range { lo, hi, .. } = self.value(&range.value)? else {
            return Err(Diagnostic::new(
                "`clamp` takes a range, such as `20hz..18khz`",
                range.value.span,
            ));
        };
        let up = self.extreme_value(v, Datum::Sig(lo), false, name.span)?;
        self.extreme_value(up, Datum::Sig(hi), true, name.span)
    }

    /// `zeros(n)`: a list of `n` zeros.
    pub(super) fn zeros(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let [count] = args else {
            return Err(Diagnostic::new("`zeros` takes a count", name.span));
        };
        let n = self.whole_number(&count.value, 0, 1 << 22)?;
        Ok(Datum::array(vec![
            Datum::Sig(Sig::constant(Unit::Num, 0.0));
            n
        ]))
    }

    /// A whole number from 1 to `max`, known while compiling.
    pub(super) fn whole_number(&mut self, e: &Expr, min: usize, max: usize) -> Res<usize> {
        match self.expr(e)? {
            Sig {
                konst: Some(k),
                unit: Unit::Num,
                ..
            } if k.fract() == 0.0 && k >= min as f64 && k <= max as f64 => Ok(k as usize),
            _ => Err(Diagnostic::new(
                format!("expected a whole number from {min} to {max}, known while compiling"),
                e.span,
            )),
        }
    }

    /// `true` or `false` (or a constant 1 or 0), known while compiling; `default` when not given.
    pub(super) fn flag(&mut self, arg: Option<&Arg>, default: bool) -> Res<bool> {
        let Some(arg) = arg else { return Ok(default) };
        match self.expr(&arg.value)? {
            Sig { konst: Some(k), .. } if k == 0.0 || k == 1.0 => Ok(k == 1.0),
            _ => Err(Diagnostic::new(
                "expected `true` or `false`",
                arg.value.span,
            )),
        }
    }

    pub(super) fn const_numbers(&mut self, value: Datum, span: Span, what: &str) -> Res<Vec<f64>> {
        let mut out = Vec::new();
        for item in self.entries(value, span)? {
            match item {
                Datum::Sig(Sig {
                    konst: Some(k),
                    unit: Unit::Num,
                    ..
                }) => out.push(k),
                _ => {
                    return Err(Diagnostic::new(
                        format!("{what} are plain numbers known while compiling"),
                        span,
                    ));
                }
            }
        }
        Ok(out)
    }

    /// `harmonics([1, -1/2, 1/3], size: 4096, phases: [...])`
    pub(super) fn harmonics(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["amps", "size", "phases", "normalize"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(amps) = bound[0] else {
            return Err(Diagnostic::new(
                "`harmonics` takes an array of amplitudes",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 2048,
        };
        let weights = {
            let __v = self.value(&amps.value)?;
            self.const_numbers(__v, amps.value.span, "the amplitudes")
        }?;
        let phases = match bound[2] {
            Some(a) => Some({
                let __v = self.value(&a.value)?;
                self.const_numbers(__v, a.value.span, "the phases")
            }?),
            None => None,
        };
        let normalize = self.flag(bound[3], true)?;
        Ok(Datum::Table(Arc::new(crate::tables::harmonics(
            &weights,
            phases.as_deref(),
            size,
            normalize,
        ))))
    }

    pub(super) fn partials(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["partials", "size", "normalize"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(list) = bound[0] else {
            return Err(Diagnostic::new(
                "`partials` takes an array of `[ratio, amp]` or `[ratio, amp, phase]`",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 2048,
        };
        let mut rows = Vec::new();
        for item in {
            let __v = self.value(&list.value)?;
            self.entries(__v, list.value.span)
        }? {
            let nums = self.const_numbers(item, list.value.span, "each partial")?;
            let row = match nums.as_slice() {
                [ratio, amp] => [*ratio, *amp, 0.0],
                [ratio, amp, phase] => [*ratio, *amp, *phase],
                _ => {
                    return Err(Diagnostic::new(
                        "each partial is `[ratio, amp]` or `[ratio, amp, phase]`",
                        list.value.span,
                    ));
                }
            };
            rows.push(row);
        }
        let normalize = self.flag(bound[2], true)?;
        Ok(Datum::Table(Arc::new(crate::tables::partials(
            &rows, size, normalize,
        ))))
    }

    pub(super) fn cosines(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["amps", "size", "normalize"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(amps) = bound[0] else {
            return Err(Diagnostic::new(
                "`cosines` takes an array of amplitudes",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 2048,
        };
        let weights = {
            let __v = self.value(&amps.value)?;
            self.const_numbers(__v, amps.value.span, "the amplitudes")
        }?;
        let normalize = self.flag(bound[2], true)?;
        Ok(Datum::Table(Arc::new(crate::tables::cosines(
            &weights, size, normalize,
        ))))
    }

    pub(super) fn curve(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["points", "size", "shape", "bend"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(points) = bound[0] else {
            return Err(Diagnostic::new(
                "`curve` takes an array of `[x, y]` points",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let shape = match bound[2] {
            Some(a) => match &a.value.kind {
                ExprKind::Name(n) if n == "linear" => crate::tables::CurveShape::Linear,
                ExprKind::Name(n) if n == "exp" => crate::tables::CurveShape::Exp,
                ExprKind::Name(n) if n == "cubic" => crate::tables::CurveShape::Cubic,
                ExprKind::Name(n) if n == "spline" => crate::tables::CurveShape::Spline,
                _ => {
                    return Err(Diagnostic::new(
                        "`shape` is `linear`, `exp`, `cubic` or `spline`",
                        a.value.span,
                    ));
                }
            },
            None => crate::tables::CurveShape::Linear,
        };
        let bend = match bound[3] {
            Some(a) => match self.expr(&a.value)?.konst {
                Some(k) if k.is_finite() && k.abs() <= 20.0 => k,
                _ => {
                    return Err(Diagnostic::new(
                        "`bend` is a number from -20 to 20, known while compiling",
                        a.value.span,
                    ));
                }
            },
            None => 0.0,
        };
        let mut rows = Vec::new();
        for item in {
            let __v = self.value(&points.value)?;
            self.entries(__v, points.value.span)
        }? {
            let nums = self.const_numbers(item, points.value.span, "each point")?;
            let [x, y] = nums.as_slice() else {
                return Err(Diagnostic::new("each point is `[x, y]`", points.value.span));
            };
            rows.push([*x, *y]);
        }
        crate::tables::curve(&rows, size, shape, bend)
            .map(|t| Datum::Table(Arc::new(t)))
            .map_err(|e| Diagnostic::new(e, name.span))
    }

    pub(super) fn steps(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["xs", "ys", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 2, args)?;
        let (Some(xs), Some(ys)) = (bound[0], bound[1]) else {
            return Err(Diagnostic::new(
                "`steps` takes `xs` and `ys` arrays",
                name.span,
            ));
        };
        let size = match bound[2] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let x = {
            let __v = self.value(&xs.value)?;
            self.const_numbers(__v, xs.value.span, "the x positions")
        }?;
        let y = {
            let __v = self.value(&ys.value)?;
            self.const_numbers(__v, ys.value.span, "the y values")
        }?;
        crate::tables::steps(&x, &y, size)
            .map(|t| Datum::Table(Arc::new(t)))
            .map_err(|e| Diagnostic::new(e, name.span))
    }

    pub(super) fn poly(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["coeffs", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(coeffs) = bound[0] else {
            return Err(Diagnostic::new(
                "`poly` takes an array of coefficients",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let c = {
            let __v = self.value(&coeffs.value)?;
            self.const_numbers(__v, coeffs.value.span, "the coefficients")
        }?;
        Ok(Datum::Table(Arc::new(crate::tables::poly(&c, size))))
    }

    pub(super) fn ramp(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["start", "end", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 2, args)?;
        let (Some(start), Some(end)) = (bound[0], bound[1]) else {
            return Err(Diagnostic::new("`ramp` takes `start` and `end`", name.span));
        };
        let size = match bound[2] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let s = self.expr(&start.value)?;
        let e = self.expr(&end.value)?;
        let (Some(start_v), Some(end_v)) = (s.konst, e.konst) else {
            return Err(Diagnostic::new(
                "`ramp` ends must be known while compiling",
                name.span,
            ));
        };
        Ok(Datum::Table(Arc::new(crate::tables::ramp(
            start_v, end_v, size,
        ))))
    }

    pub(super) fn cheby(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["amps", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(amps) = bound[0] else {
            return Err(Diagnostic::new(
                "`cheby` takes an array of amplitudes",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let weights = {
            let __v = self.value(&amps.value)?;
            self.const_numbers(__v, amps.value.span, "the amplitudes")
        }?;
        Ok(Datum::Table(Arc::new(crate::tables::cheby(&weights, size))))
    }

    pub(super) fn window(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["kind", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(kind_arg) = bound[0] else {
            return Err(Diagnostic::new(
                "`window` takes a kind such as `hann`",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let kind = match &kind_arg.value.kind {
            ExprKind::Name(n) if n == "hann" => crate::tables::WindowKind::Hann,
            ExprKind::Name(n) if n == "hamming" => crate::tables::WindowKind::Hamming,
            ExprKind::Name(n) if n == "blackman" => crate::tables::WindowKind::Blackman,
            ExprKind::Name(n) if n == "sinc" => crate::tables::WindowKind::Sinc,
            ExprKind::Name(n) if n == "tri" => crate::tables::WindowKind::Tri,
            ExprKind::Call { name: n, args } if n.name == "kaiser" => {
                let beta = match args.as_slice() {
                    [a] => self.expr(&a.value)?.konst.unwrap_or(8.0),
                    _ => 8.0,
                };
                crate::tables::WindowKind::Kaiser { beta }
            }
            ExprKind::Call { name: n, args } if n.name == "gauss" => {
                let alpha = match args.as_slice() {
                    [a] => self.expr(&a.value)?.konst.unwrap_or(2.5),
                    _ => 2.5,
                };
                crate::tables::WindowKind::Gauss { alpha }
            }
            ExprKind::Call { name: n, args } if n.name == "tukey" => {
                let alpha = match args.as_slice() {
                    [a] => self.expr(&a.value)?.konst.unwrap_or(0.5),
                    _ => 0.5,
                };
                crate::tables::WindowKind::Tukey { alpha }
            }
            _ => {
                return Err(Diagnostic::new(
                    "`window` kind is hann, hamming, blackman, kaiser(beta), gauss, tukey, sinc or tri",
                    kind_arg.value.span,
                ));
            }
        };
        Ok(Datum::Table(Arc::new(crate::tables::window(kind, size))))
    }

    pub(super) fn random_table(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["size", "dist", "seed"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(size_arg) = bound[0] else {
            return Err(Diagnostic::new("`random` takes a size", name.span));
        };
        let size = self.whole_number(&size_arg.value, 1, 1 << 20)?;
        let dist = match bound[1] {
            Some(a) => match &a.value.kind {
                ExprKind::Name(n) if n == "uniform" => crate::tables::Dist::Uniform,
                ExprKind::Name(n) if n == "linear" => crate::tables::Dist::Linear,
                ExprKind::Name(n) if n == "triangle" => crate::tables::Dist::Triangle,
                ExprKind::Name(n) if n == "exponential" => crate::tables::Dist::Exponential,
                ExprKind::Name(n) if n == "bilateral" => crate::tables::Dist::Bilateral,
                ExprKind::Name(n) if n == "cauchy" => crate::tables::Dist::Cauchy,
                ExprKind::Name(n) if n == "poisson" => crate::tables::Dist::Poisson,
                ExprKind::Name(n) if n == "gauss" => crate::tables::Dist::Gauss,
                _ => {
                    return Err(Diagnostic::new(
                        "`dist` is uniform, linear, triangle, exponential, bilateral, cauchy, poisson or gauss",
                        a.value.span,
                    ));
                }
            },
            None => crate::tables::Dist::Uniform,
        };
        let Some(seed_arg) = bound[2] else {
            return Err(Diagnostic::new(
                "`random` needs `seed:` so renders stay identical",
                name.span,
            ));
        };
        let seed = self.whole_number(&seed_arg.value, 0, usize::MAX)? as u64;
        Ok(Datum::Table(Arc::new(crate::tables::random(
            size, dist, seed,
        ))))
    }

    pub(super) fn file_table(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["path", "channel", "start", "end"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(path_arg) = bound[0] else {
            return Err(Diagnostic::new("`file` takes a path string", name.span));
        };
        let path = self.path_text(&path_arg.value)?;
        let path = path.as_str();
        let channel = match bound[1] {
            Some(a) => self.whole_number(&a.value, 0, 64)?,
            None => 0,
        };
        let start = match bound[2] {
            Some(a) => Some(self.whole_number(&a.value, 0, 1 << 30)?),
            None => None,
        };
        let end = match bound[3] {
            Some(a) => Some(self.whole_number(&a.value, 0, 1 << 30)?),
            None => None,
        };
        self.names
            .samples
            .load_channel(path, channel, start, end)
            .map(Datum::Table)
            .map_err(|e| Diagnostic::new(e, path_arg.value.span))
    }

    /// A path in quotes. A `{name}` in it is the whole number the constant `name` holds, and
    /// `{name:3}` pads it with zeros to 3 digits: `"kick{i:2}.wav"` is `kick07.wav`.
    pub(super) fn path_text(&mut self, e: &Expr) -> Res<String> {
        let ExprKind::Str(text) = &e.kind else {
            return Err(Diagnostic::new("expected a path in quotes", e.span));
        };
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('}') else {
                return Err(Diagnostic::new(
                    "a `{` in a path needs a closing `}`",
                    e.span,
                ));
            };
            let inner = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            let (name, width) = match inner.split_once(':') {
                Some((n, w)) => (n.trim(), w.trim().parse::<usize>().ok()),
                None => (inner.trim(), Some(0)),
            };
            let (Some(width), true) = (
                width,
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'),
            ) else {
                return Err(Diagnostic::new(
                    format!(
                        "`{{{inner}}}` in a path is a constant's name, as `{{i}}` or `{{i:2}}`"
                    ),
                    e.span,
                ));
            };
            let value = self.expr(&Expr {
                kind: ExprKind::Name(name.to_string()),
                span: e.span,
            })?;
            match value {
                Sig {
                    konst: Some(k),
                    unit: Unit::Num,
                    ..
                } if k.fract() == 0.0 && k >= 0.0 => {
                    out.push_str(&format!("{:0width$}", k as u64));
                }
                _ => {
                    return Err(Diagnostic::new(
                        format!("`{name}` in a path is a whole number known while compiling"),
                        e.span,
                    ));
                }
            }
        }
        out.push_str(rest);
        Ok(out)
    }

    pub(super) fn bank(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let [tables] = args else {
            return Err(Diagnostic::new(
                "`bank` takes an array of tables",
                name.span,
            ));
        };
        let list = self.value(&tables.value)?;
        let mut out = Vec::new();
        for item in self.entries(list, tables.value.span)? {
            out.push(Datum::Table(self.as_table(item, tables.value.span)?));
        }
        if out.len() < 2 {
            return Err(Diagnostic::new(
                "a wavetable bank needs at least two tables",
                name.span,
            ));
        }
        Ok(Datum::Array(out.into()))
    }

}
