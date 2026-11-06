//! Operation arguments, arithmetic, blocks, calls to definitions and `load`.

use super::*;

impl Perform<'_> {
    // ---- arguments -----------------------------------------------------------

    /// The parameter names of each operation, in order. Arguments may be given
    /// by name; the rest fill the remaining parameters in order.
    pub(super) fn op_params(op: &str) -> &'static [&'static str] {
        match op {
            "fast" | "slow" | "shift" | "transpose" => &["by"],
            "over" => &["length"],
            "repeat_each" => &["times"],
            "drop" => &["amount", "seed"],
            "sometimes" => &["chance", "change", "per", "seed"],
            "every" => &["n", "change"],
            "overlay" => &["change", "after"],
            "only_between" => &["from", "to", "change"],
            "swing" => &["amount", "every"],
            "start_later" | "shuffle" | "pick_random" => &["parts", "seed"],
            "step_through" => &["parts", "change"],
            "loop_start" => &["length"],
            "to_steps" => &["steps"],
            "range" => &["from", "to"],
            "where" => &["when", "change"],
            "gain" => &["level"],
            "pan" => &["angle"],
            "begin" => &["share"],
            "stereo_copy" => &["change", "angle"],
            "chop" | "interleave_chops" => &["pieces"],
            "slice" => &["parts", "order"],
            "rotate" => &["by"],
            "window" => &["start", "size", "mode", "steps"],
            "phase" => &["by", "steps"],
            "density" => &["amount", "seed"],
            "in_scale" | "snap_to" => &["scale", "root"],
            "chord" => &["kind"],
            "arpeggiate" => &["order"],
            "invert_melody" => &["around"],
            "markov" => &["seed"],
            "random_walk" => &["step", "seed"],
            "spread_evenly" => &["hits", "steps", "rotation"],
            "with_rhythm_of" => &["pattern"],
            "keep_where" => &["pattern"],
            "add" | "sub" | "mul" | "div" => &["other", "timing"],
            "take" => &["length"],
            "mod" => &["by"],
            "times" => &["count"],
            "crop" | "squeeze" => &["from", "to"],
            "strum" => &["amount"],
            "in_cycles" => &["cycles", "from", "change"],
            "often" | "rarely" => &["change", "seed"],
            "echo" => &["times", "time", "fade"],
            _ => &[],
        }
    }

    /// The argument for parameter `param` of `op`, if given.
    pub(super) fn find<'e>(&self, op: &Ident, args: &'e [Arg], param: &str) -> Option<&'e Expr> {
        let params = Self::op_params(&op.name);
        let mut slots: Vec<Option<&Expr>> = vec![None; params.len()];
        for a in args {
            if let Some(n) = &a.name
                && let Some(i) = params.iter().position(|p| *p == n.name)
            {
                slots[i] = Some(&a.value);
            }
        }
        let mut next = 0;
        for a in args.iter().filter(|a| a.name.is_none()) {
            while next < slots.len() && slots[next].is_some() {
                next += 1;
            }
            if next < slots.len() {
                slots[next] = Some(&a.value);
                next += 1;
            }
        }
        params
            .iter()
            .position(|p| *p == param)
            .and_then(|i| slots[i])
    }

    pub(super) fn arg<'e>(&self, op: &Ident, args: &'e [Arg], param: &str, span: Span) -> Res<&'e Expr> {
        self.find(op, args, param).ok_or_else(|| {
            Diagnostic::new(format!("`{}` needs an argument `{param}`", op.name), span)
        })
    }

    pub(super) fn binary(
        &self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        let (l, r) = (self.eval(lhs, it)?, self.eval(rhs, it)?);
        if matches!(l, PerfValue::Pattern(_)) || matches!(r, PerfValue::Pattern(_)) {
            return self.arithmetic(op, Structure::Left, l, r, span);
        }
        match (l.as_num(), r.as_num(), op) {
            (Some(a), Some(b), BinOp::Add) => Ok(PerfValue::Num(add_values(a, b, span)?)),
            (Some(a), Some(b), BinOp::Sub) => Ok(PerfValue::Num(sub_values(a, b, span)?)),
            (Some(a), Some(b), BinOp::Mul) => Ok(PerfValue::Num(mul_values(a, b, span)?)),
            (Some(a), Some(b), BinOp::Div) => Ok(PerfValue::Num(div_values(a, b, span)?)),
            _ => Err(Diagnostic::new(
                format!("can't {} these values in a clip or scene", op_name(op)),
                span,
            )),
        }
    }

    /// `riff + 7st`, `riff * [1 2]`, `a.add(b, structure: both)`: do the sum for every
    /// note. A pattern on both sides is joined by `structure`; a plain number on one
    /// side changes each note of the other.
    pub(super) fn arithmetic(
        &self,
        op: BinOp,
        structure: Structure,
        l: PerfValue,
        r: PerfValue,
        span: Span,
    ) -> Res<PerfValue> {
        let sum = move |a: Value, b: Value| step_arithmetic(op, a, b);
        let out = match (l, r) {
            (PerfValue::Pattern(a), PerfValue::Pattern(b)) => {
                a.combine(&b, structure, move |x, y| match structure {
                    Structure::Right => y.with(sum(x.value, y.value).unwrap_or(y.value)),
                    _ => x.with(sum(x.value, y.value).unwrap_or(x.value)),
                })
            }
            (PerfValue::Pattern(a), other) if other.as_num().is_some() => {
                let n = other.as_num().expect("checked");
                a.map(move |x| x.with(sum(x.value, n).unwrap_or(x.value)))
            }
            (other, PerfValue::Pattern(b)) if other.as_num().is_some() => {
                let n = other.as_num().expect("checked");
                b.map(move |y| y.with(sum(n, y.value).unwrap_or(y.value)))
            }
            (l, r) => {
                return Err(Diagnostic::new(
                    format!(
                        "can't {} {} and {}",
                        op_name(op),
                        l.describe(),
                        r.describe()
                    ),
                    span,
                )
                .with_help("one side must be a pattern, the other a pattern or a number"));
            }
        };
        Ok(PerfValue::Pattern(out))
    }

    pub(super) fn block_value(&self, body: &[Stmt], it: Option<&PerfValue>) -> Res<PerfValue> {
        let mut local = Perform {
            cx: self.cx,
            bindings: self.bindings.clone(),
            arrangements: HashMap::new(),
        };
        let mut last = PerfValue::Rest;
        for stmt in body {
            match stmt {
                Stmt::Bind { name, value } => {
                    let v = local.eval(value, it)?;
                    local.bindings.insert(name.name.clone(), v.clone());
                    last = v;
                }
                Stmt::Expr(e) => last = local.eval(e, it)?,
                Stmt::State { name, .. }
                | Stmt::AddAssign { name, .. }
                | Stmt::IndexAssign { name, .. } => {
                    return Err(Diagnostic::new(
                        "only bindings and expressions belong in a score `def`, `for` or `if`",
                        name.span,
                    ));
                }
                Stmt::Destructure { names, .. } => {
                    return Err(Diagnostic::new(
                        "only bindings and expressions belong in a score `def`, `for` or `if`",
                        names.first().map(|n| n.span).unwrap_or_default(),
                    ));
                }
            }
        }
        Ok(last)
    }

    pub(super) fn iterate(&self, iter: &Expr, it: Option<&PerfValue>) -> Res<Vec<PerfValue>> {
        match self.eval(iter, it)? {
            PerfValue::List(items) => Ok(items),
            other => Err(Diagnostic::new(
                format!("`for` needs a list or range, found {}", other.describe()),
                iter.span,
            )),
        }
    }

    pub(super) fn call_def(
        &self,
        def: DefFn,
        args: &[Arg],
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        if self.bindings.values().any(|v| {
            matches!(v, PerfValue::Def(d) if d.name == def.name) && false // recursion recorded below
        }) {
            // fall through
        }
        let mut child = Perform {
            cx: self.cx,
            bindings: HashMap::new(),
            arrangements: HashMap::new(),
        };
        // copy defs and outer bindings except the active call stack marker
        for (k, v) in &self.bindings {
            if !matches!(v, PerfValue::Def(_)) {
                continue;
            }
            child.bindings.insert(k.clone(), v.clone());
        }
        // mark this call to catch recursion
        let marker = format!("__calling:{}", def.name);
        if self.bindings.contains_key(&marker) {
            return Err(Diagnostic::new(
                format!("score function `{}` calls itself", def.name),
                span,
            ));
        }
        child
            .bindings
            .insert(marker.clone(), PerfValue::Num(Value::Num(1.0)));

        let mut positional = args.iter().filter(|a| a.name.is_none() && !a.receiver);
        for param in &def.params {
            let value = if let Some(arg) = args
                .iter()
                .find(|a| a.name.as_ref().is_some_and(|n| n.name == param.name.name))
            {
                self.eval(&arg.value, it)?
            } else if let Some(arg) = positional.next() {
                self.eval(&arg.value, it)?
            } else if let Some(default) = &param.default {
                self.eval(default, it)?
            } else {
                return Err(Diagnostic::new(
                    format!("`{}` needs an argument `{}`", def.name, param.name.name),
                    span,
                ));
            };
            child.bindings.insert(param.name.name.clone(), value);
        }
        child.block_value(&def.body, None)
    }

    /// `load("score/riff.nms")`: a section or single-clip project as a clip.
    pub(super) fn load(&self, args: &[Arg], span: Span) -> Res<PerfValue> {
        let path = match args {
            [
                Arg {
                    name: None,
                    value:
                        Expr {
                            kind: ExprKind::Str(path),
                            ..
                        },
                    ..
                },
            ] => path.as_str(),
            _ => {
                return Err(Diagnostic::new(
                    "`load` needs a file path, as in `load(\"score/riff.nms\")`",
                    span,
                ));
            }
        };
        let resolved = self.cx.samples.resolve_path(path);
        let text = std::fs::read_to_string(&resolved)
            .map_err(|e| Diagnostic::new(format!("couldn't read `{path}`: {e}"), span))?;
        let section = crate::Section::from_json(&text)
            .map_err(|e| Diagnostic::new(format!("couldn't load `{path}`: {e}"), span))?;
        let clip = section_to_clip(section, self.cx.tempo)?;
        Ok(PerfValue::Clip(clip))
    }
}
