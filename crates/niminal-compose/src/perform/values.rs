//! Evaluating expressions to performance values: patterns, rows, drum grids and numbers.

use super::*;

impl Perform<'_> {
    pub(super) fn define(&mut self, name: &Ident, value: PerfValue) -> Res<()> {
        // Score functions are also registered in the project name table.
        if let Some(kind) = self.cx.names.kind_of(&name.name).filter(|k| {
            let own_instrument = matches!(*k, "instrument" | "sample" | "kit")
                && matches!(value, PerfValue::Clip(_))
                && self.cx.nodes.iter().any(|node| {
                    node.name == name.name
                        && node
                            .instrument
                            .is_some_and(|i| self.cx.instruments[i].name == name.name)
                });
            // a function (such as one of the library's) is only reached by calling it, so a clip or scene can share its name
            !own_instrument
                && *k != "function"
                && !(*k == "score function" && matches!(value, PerfValue::Def(_)))
        }) {
            return Err(Diagnostic::new(
                format!("`{}` is already the name of a {kind}", name.name),
                name.span,
            ));
        }
        if matches!(name.name.as_str(), "in" | "it" | "out") {
            return Err(Diagnostic::new(
                format!("`{}` is a reserved word", name.name),
                name.span,
            ));
        }
        if self.bindings.contains_key(&name.name) {
            return Err(Diagnostic::new(
                format!("`{}` is defined twice", name.name),
                name.span,
            ));
        }
        self.bindings.insert(name.name.clone(), value);
        Ok(())
    }

    // ---- expressions -----------------------------------------------------

    pub(super) fn eval(&self, e: &Expr, it: Option<&PerfValue>) -> Res<PerfValue> {
        match &e.kind {
            ExprKind::Pattern(raw) => self.pattern(raw, e.span).map(PerfValue::Pattern),
            ExprKind::Grid(raw) => grid(raw)
                .map(PerfValue::Grid)
                .map_err(|err| at_offset(err.message, e.span, err.offset)),
            ExprKind::Rows(raw) => self.rows(raw, e.span).map(PerfValue::Pattern),
            ExprKind::Steps(raw) => steps(raw)
                .map(PerfValue::Pattern)
                .map_err(|message| Diagnostic::new(message, e.span)),
            ExprKind::FxLanes(_) => Err(Diagnostic::new(
                "a block of lanes belongs in a clip, named after an effect",
                e.span,
            )
            .with_help("for example `clip c { trance_gate { open: x.x. xx.x } }`")),
            ExprKind::Rest => Ok(PerfValue::Rest),
            // inside a change, `same` is the pattern being changed
            ExprKind::Name(name) if name == "same" && it.is_some() => {
                Ok(it.cloned().expect("checked"))
            }
            ExprKind::Name(name) if name == "true" => Ok(PerfValue::Num(Value::Num(1.0))),
            ExprKind::Name(name) if name == "false" => Ok(PerfValue::Num(Value::Num(0.0))),
            ExprKind::Name(name) => match self.bindings.get(name) {
                Some(v) => Ok(v.clone()),
                None if signal(name).is_some() => Ok(PerfValue::Pattern(
                    signal(name)
                        .expect("checked")
                        .map(|x| Step::from(Value::Num(x))),
                )),
                // a control is a value that changes while the pattern plays
                None if self.cx.bank.has(name) => {
                    let (bank, name) = (self.cx.bank.clone(), name.clone());
                    Ok(PerfValue::Pattern(Pattern::signal(move |_| {
                        Step::from(Value::Num(bank.get(&name).unwrap_or(0.0)))
                    })))
                }
                // a lone note or value is a one-step pattern (and a number for arithmetic)
                None => match atom_value(name, self.cx.sample_names) {
                    Ok(v) => Ok(PerfValue::Pattern(Pattern::pure(Step::from(v)))),
                    Err(_) => {
                        let mut d = Diagnostic::new(format!("`{name}` is not defined"), e.span);
                        if let Some(c) = closest(name, self.bindings.keys().map(String::as_str)) {
                            d = d.with_help(format!("did you mean `{c}`?"));
                        }
                        Err(d)
                    }
                },
            },
            ExprKind::Num { value, unit } => {
                Ok(PerfValue::Num(value_of(*value, unit.as_deref(), e.span)?))
            }
            ExprKind::Neg(inner) => match self.eval(inner, it)? {
                PerfValue::Num(v) => Ok(PerfValue::Num(negate(v))),
                PerfValue::Pattern(p) => Ok(PerfValue::Pattern(p.map(|s| s.with(negate(s.value))))),
                other => Err(Diagnostic::new(
                    format!("can't negate {}", other.describe()),
                    e.span,
                )),
            },
            ExprKind::Call { name, args } if name.name == "load" => self.load(args, e.span),
            ExprKind::Call { name, args }
                if name.name == "if_else" && !args.first().is_some_and(|a| a.receiver) =>
            {
                // `if_else(grid[x.x.], a, b)`: a where the grid hits, b where it rests
                let [cond, yes, no] = args.as_slice() else {
                    return Err(Diagnostic::new(
                        "`if_else` takes a grid and two patterns, as in `if_else(grid[x.x.], a, b)`",
                        e.span,
                    ));
                };
                let PerfValue::Grid(g) = self.eval(&cond.value, it)? else {
                    return Err(Diagnostic::new(
                        "`if_else` starts with a grid, such as `grid[x.x. xxx.]`",
                        cond.value.span,
                    ));
                };
                let pattern = |a: &Arg| match self.eval(&a.value, it)? {
                    PerfValue::Pattern(p) => Ok(p),
                    other => Err(Diagnostic::new(
                        format!("`if_else` needs patterns, found {}", other.describe()),
                        a.value.span,
                    )),
                };
                let (yes, no) = (pattern(yes)?, pattern(no)?);
                Ok(PerfValue::Pattern(Pattern::stack(vec![
                    yes.mask(&g.map(|_| true)),
                    no.mask(&g.invert(Hit::new(Strength::Normal)).map(|_| true)),
                ])))
            }
            ExprKind::Call { name, args }
                if matches!(name.name.as_str(), "count_to" | "random_int")
                    && !args.first().is_some_and(|a| a.receiver) =>
            {
                let [arg] = args.as_slice() else {
                    return Err(Diagnostic::new(
                        format!("`{}` takes one number", name.name),
                        e.span,
                    ));
                };
                let n = self.whole(&arg.value, &name.name)?;
                let number = |x: f64| Step::from(Value::Num(x));
                Ok(PerfValue::Pattern(if name.name == "count_to" {
                    Pattern::fastcat((0..n).map(|i| Pattern::pure(number(i as f64))).collect())
                } else {
                    Pattern::<f64>::rand().map(move |x| number((x * n as f64).floor()))
                }))
            }
            ExprKind::Call { name, args }
                if matches!(name.name.as_str(), "stack" | "cat" | "seq" | "take_turns")
                    && !args.first().is_some_and(|a| a.receiver) =>
            {
                self.combine(name, args, e.span, it)
            }
            ExprKind::Call { name, args } => {
                if let Some(PerfValue::Def(def)) = self.bindings.get(&name.name) {
                    return self.call_def(def.clone(), args, e.span, it);
                }
                if name.name == "drum_grid" && !args.first().is_some_and(|a| a.receiver) {
                    return self.drum_grid(args, e.span).map(PerfValue::Pattern);
                }
                self.op(name, args, e.span, it)
            }
            ExprKind::ClipBlock(block) => self.clip(block).map(PerfValue::Clip),
            ExprKind::SceneBlock(block) => self.scene(block).map(PerfValue::Scene),
            ExprKind::Env(lit) => env_lane(lit).map(PerfValue::Env),
            ExprKind::Events(_) => Err(Diagnostic::new(
                "`events { }` belongs inside a clip, as a lane",
                e.span,
            )),
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can only be an argument that asks for it",
                e.span,
            )),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span, it),
            ExprKind::Compare { op, lhs, rhs } => {
                let (l, r) = (self.eval(lhs, it)?, self.eval(rhs, it)?);
                let (lv, rv) = (l.as_num(), r.as_num());
                match (lv, rv) {
                    (Some(a), Some(b)) => {
                        let ok = cmp_values(*op, a, b);
                        Ok(PerfValue::Num(Value::Num(if ok { 1.0 } else { 0.0 })))
                    }
                    _ => Err(Diagnostic::new("comparisons need numbers or notes", e.span)),
                }
            }
            ExprKind::Logic { op, lhs, rhs } => {
                let l = self.eval(lhs, it)?.as_num().map(|v| match v {
                    Value::Num(n) => n != 0.0,
                    _ => true,
                });
                let r = self.eval(rhs, it)?.as_num().map(|v| match v {
                    Value::Num(n) => n != 0.0,
                    _ => true,
                });
                match (l, r, op) {
                    (Some(a), Some(b), LogicOp::And) => {
                        Ok(PerfValue::Num(Value::Num(if a && b { 1.0 } else { 0.0 })))
                    }
                    (Some(a), Some(b), LogicOp::Or) => {
                        Ok(PerfValue::Num(Value::Num(if a || b { 1.0 } else { 0.0 })))
                    }
                    _ => Err(Diagnostic::new("logic needs numbers", e.span)),
                }
            }
            ExprKind::Not(inner) => {
                let v = self.eval(inner, it)?;
                match v.as_num() {
                    Some(Value::Num(n)) => {
                        Ok(PerfValue::Num(Value::Num(if n == 0.0 { 1.0 } else { 0.0 })))
                    }
                    _ => Err(Diagnostic::new("`not` needs a number", e.span)),
                }
            }
            ExprKind::If {
                cond,
                then,
                otherwise,
            } => {
                let c = self.eval(cond, it)?;
                let truthy = match c.as_num() {
                    Some(Value::Num(n)) => n != 0.0,
                    Some(_) => true,
                    None => {
                        return Err(Diagnostic::new(
                            "`if` needs a condition that is a number",
                            cond.span,
                        ));
                    }
                };
                let body = if truthy {
                    then.as_slice()
                } else {
                    otherwise.as_deref().unwrap_or(&[])
                };
                self.block_value(body, it)
            }
            ExprKind::For { var, iter, body } => {
                let items = self.iterate(iter, it)?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let mut child = Perform {
                        cx: self.cx,
                        bindings: self.bindings.clone(),
                        arrangements: HashMap::new(),
                    };
                    child.bindings.insert(var.name.clone(), item);
                    out.push(child.block_value(body, None)?);
                }
                Ok(PerfValue::List(out))
            }
            ExprKind::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.eval(item, it)?);
                }
                Ok(PerfValue::List(out))
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let lo = self
                    .eval(lo, it)?
                    .as_num()
                    .and_then(|v| v.as_number().ok())
                    .ok_or_else(|| Diagnostic::new("range bounds are numbers", e.span))?;
                let hi = self
                    .eval(hi, it)?
                    .as_num()
                    .and_then(|v| v.as_number().ok())
                    .ok_or_else(|| Diagnostic::new("range bounds are numbers", e.span))?;
                let mut out = Vec::new();
                let mut i = lo.floor() as i64;
                let end = hi.floor() as i64;
                while if *inclusive { i <= end } else { i < end } {
                    out.push(PerfValue::Num(Value::Num(i as f64)));
                    i += 1;
                    if out.len() > 10_000 {
                        return Err(Diagnostic::new("range is too long", e.span));
                    }
                }
                Ok(PerfValue::List(out))
            }
            ExprKind::Index { list, index } => {
                let list = self.eval(list, it)?;
                let i = self
                    .eval(index, it)?
                    .as_num()
                    .and_then(|v| v.as_number().ok())
                    .ok_or_else(|| Diagnostic::new("an index is a whole number", index.span))?
                    as i64;
                match list {
                    PerfValue::List(items) => {
                        let i = if i < 0 { items.len() as i64 + i } else { i };
                        items.get(i as usize).cloned().ok_or_else(|| {
                            Diagnostic::new(format!("index {i} is out of range"), index.span)
                        })
                    }
                    _ => Err(Diagnostic::new("only a list can be indexed here", e.span)),
                }
            }
            ExprKind::Lambda { .. } => Err(Diagnostic::new(
                "lambdas belong in instruments and functions, not in clips and scenes",
                e.span,
            )),
        }
    }

    /// A range of the source as the client that sent it knows it: which evaluation, and where in it.
    pub(super) fn sent(&self, start: usize, end: usize) -> Option<(u64, usize, usize)> {
        let Some(origins) = self.cx.origins else {
            return Some((0, start, end));
        };
        origins
            .iter()
            .find(|(from, to, ..)| start >= *from && end <= *to)
            .map(|&(from, _, eval, offset)| (eval, offset + start - from, offset + end - from))
    }

    pub(super) fn pattern(&self, raw: &str, span: Span) -> Res<Pattern<Step>> {
        let pattern = parse_with(raw, |atom, at| {
            // the token's text starts after its opening bracket
            let start = span.start + 1 + at;
            let written = |v: Value| Step {
                src: self.sent(start, start + atom.len()),
                ..Step::from(v)
            };
            if let Ok(v) = atom_value(atom, self.cx.sample_names) {
                return Ok(vec![written(v)]);
            }
            if let Some(notes) = chord_notes(atom) {
                return Ok(notes.into_iter().map(written).collect());
            }
            // Score `def` params and lets: `[root ~]` resolves `root` from bindings.
            // A lone note name is stored as a one-step pattern; take its value.
            match self.bindings.get(atom).and_then(|v| v.as_num()) {
                Some(v) => Ok(vec![written(v)]),
                None => match self.bindings.get(atom) {
                    Some(other) => Err(format!(
                        "`{atom}` is {}, not a note or value for a pattern",
                        other.describe()
                    )),
                    None => atom_value(atom, self.cx.sample_names).map(|v| vec![written(v)]),
                },
            }
        })
        .map_err(|e| at_offset(e.message, span, e.offset))?;
        Ok(Pattern::new(move |span| {
            pattern
                .query(span)
                .into_iter()
                .flat_map(|hit| {
                    hit.value
                        .clone()
                        .into_iter()
                        .map(move |step| hit.clone().map(|_| step))
                })
                .collect()
        }))
    }

    /// A list of notes, rests or patterns as one pattern in sequence.
    pub(super) fn list_pattern(&self, items: Vec<PerfValue>, span: Span) -> Res<Pattern<Step>> {
        if items.is_empty() {
            return Ok(Pattern::silence());
        }
        let mut parts = Vec::with_capacity(items.len());
        for item in items {
            parts.push(match item {
                PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                PerfValue::Rest => Pattern::silence(),
                PerfValue::Pattern(p) => p,
                PerfValue::List(inner) => self.list_pattern(inner, span)?,
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "a pattern list holds notes, rests or patterns, not {}",
                            other.describe()
                        ),
                        span,
                    ));
                }
            });
        }
        Ok(Pattern::fastcat(parts))
    }

    /// Free functions `stack`, `cat`, `seq` and `take_turns` over patterns.
    pub(super) fn combine(
        &self,
        name: &Ident,
        args: &[Arg],
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        let mut patterns = Vec::with_capacity(args.len());
        for arg in args {
            if arg.receiver {
                continue;
            }
            let v = self.eval(&arg.value, it)?;
            // `take_turns(for c in 0..<8 { .. })`: each item of a list is one turn
            if name.name == "take_turns"
                && let PerfValue::List(items) = v
            {
                for item in items {
                    patterns.push(self.list_pattern(vec![item], arg.value.span)?);
                }
                continue;
            }
            patterns.push(match v {
                PerfValue::Pattern(p) => p,
                PerfValue::Num(n) => Pattern::pure(Step::from(n)),
                PerfValue::Rest => Pattern::silence(),
                PerfValue::List(items) => self.list_pattern(items, arg.value.span)?,
                PerfValue::Clip(c) => c
                    .lanes
                    .iter()
                    .find(|(n, _)| n == "notes")
                    .map(|(_, p)| p.clone())
                    .ok_or_else(|| {
                        Diagnostic::new(
                            format!("`{}` needs patterns or notes", name.name),
                            arg.value.span,
                        )
                    })?,
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` needs patterns or notes, found {}",
                            name.name,
                            other.describe()
                        ),
                        arg.value.span,
                    ));
                }
            });
        }
        if patterns.is_empty() {
            return Err(Diagnostic::new(
                format!("`{}` needs at least one pattern", name.name),
                span,
            ));
        }
        Ok(PerfValue::Pattern(match name.name.as_str() {
            "stack" => Pattern::stack(patterns),
            "cat" | "seq" => Pattern::fastcat(patterns),
            "take_turns" => Pattern::slowcat(patterns),
            _ => unreachable!(),
        }))
    }

    /// `grid { kick: x... snare: .x.. }`: each row plays the sample it names on its hits,
    /// and the rows play together. A row can name a part to send its hits there, take
    /// parameters (`snare(pan: 10deg): ...`), and use mini-notation (`hat: [x*2 x]`).
    pub(super) fn rows(&self, raw: &str, span: Span) -> Res<Pattern<Step>> {
        self.rows_from(raw, span.end - 1 - raw.len(), span, true)
    }

    /// `drum_grid(theme: house, density: 0.8, chaos: 0.3, seed: 4, hat: 0.6)`: a grid made to order, the
    /// same each time for the same choices. A drum named as an argument is made busier or sparser than the
    /// rest. Written out by hand, the same choices are a `grid { ... }` to edit.
    pub(super) fn drum_grid(&self, args: &[Arg], span: Span) -> Res<Pattern<Step>> {
        let number = |e: &Expr| self.plain(e).map(|r| r.to_f64());
        let mut request = crate::drums::Request::default();
        let mut theme = None;
        let mut per_drum = Vec::new();
        for a in args {
            match a.name.as_ref().map(|n| n.name.as_str()) {
                None | Some("theme") => {
                    let ExprKind::Name(word) = &a.value.kind else {
                        return Err(
                            Diagnostic::new("expected the name of a theme", a.value.span)
                                .with_help(format!(
                                    "the themes are {}",
                                    crate::drums::themes().join(", ")
                                )),
                        );
                    };
                    theme = Some(word.clone());
                }
                Some("density") => request.density = number(&a.value)?,
                Some("chaos") => request.chaos = number(&a.value)?,
                Some("seed") => request.seed = number(&a.value)?.max(0.0) as u64,
                Some(drum) => per_drum.push((drum.to_string(), number(&a.value)?)),
            }
        }
        if let Some(theme) = &theme {
            request.theme = theme;
        }
        request.per_drum = &per_drum;
        let rows = crate::drums::generate(&request).map_err(|m| Diagnostic::new(m, span))?;
        let text = crate::drums::grid_text(&rows);
        let inner = text
            .strip_prefix("grid {\n")
            .and_then(|t| t.strip_suffix("\n}"))
            .unwrap_or(&text);
        self.rows_from(inner, 0, span, false)
    }

    /// The rows of a grid written at `base` in the source. Rows that were made rather than written have
    /// no place in the source: they report problems at the `span` of what made them, and highlight nothing.
    pub(super) fn rows_from(&self, raw: &str, base: usize, span: Span, written: bool) -> Res<Pattern<Step>> {
        let sent = |from: usize, to: usize| if written { self.sent(from, to) } else { None };
        let mut layers = Vec::new();
        let mut line_start = 0;
        for line in raw.split('\n') {
            let at = base + line_start;
            line_start += line.len() + 1;
            let code = line.split("//").next().unwrap_or("");
            if code.trim().is_empty() {
                continue;
            }
            let here = |offset: usize, message: String| {
                if !written {
                    return Diagnostic::new(message, span);
                }
                Diagnostic::new(
                    message,
                    Span::new(at + offset, (at + offset + 1).min(span.end)),
                )
            };
            let row = parse_grid_row(code).map_err(|(offset, message)| here(offset, message))?;
            let member = row.name.rsplit_once('.').map_or(row.name, |(_, m)| m);
            let node = self
                .cx
                .nodes
                .iter()
                .position(|t| t.name == row.name || t.name == member);
            let id = self
                .cx
                .sample_names
                .iter()
                .position(|n| n == member)
                .or_else(|| {
                    // A row that only names a part takes the node's default sample.
                    node.and_then(|t| {
                        self.cx.nodes[t]
                            .defaults
                            .get("sample")
                            .and_then(|v| match *v {
                                Value::Sample(id) => Some(id as usize),
                                Value::Num(n) if n >= 0.0 => {
                                    let local = n as usize;
                                    self.cx.nodes[t]
                                        .instrument
                                        .and_then(|i| self.cx.instruments[i].members.get(local))
                                        .and_then(|name| {
                                            self.cx.sample_names.iter().position(|n| n == name)
                                        })
                                }
                                _ => None,
                            })
                    })
                });
            if id.is_none() && node.is_none() && self.cx.lenient {
                self.cx.skipped.borrow_mut().push(crate::compile::Skipped {
                    what: format!("drum `{}`", row.name),
                    name: row.name.to_string(),
                    span: if written {
                        Span::new(at + row.name_offset, at + row.name_offset + 1)
                    } else {
                        span
                    },
                    typo_when_new: true,
                });
                continue;
            }
            let Some(id) = id else {
                let mut message = format!("`{}` isn't a sample in a kit", row.name);
                if node.is_some() {
                    message = format!("`{}` is a node, but it has no kit sample to play", row.name);
                } else if let Some(c) =
                    closest(member, self.cx.sample_names.iter().map(String::as_str))
                {
                    message.push_str(&format!(" (did you mean `{c}`?)"));
                }
                return Err(here(row.name_offset, message));
            };
            let hits = if row.mini {
                parse_with(row.body, |atom, offset| {
                    let strength = match atom {
                        "x" => Strength::Normal,
                        "X" => Strength::Accent,
                        "o" => Strength::Ghost,
                        other => return Err(format!("`{other}` isn't a hit: use x, X, o, or ~")),
                    };
                    let start = at + row.body_offset + offset;
                    Ok(Hit {
                        strength,
                        src: sent(start, start + atom.len()),
                    })
                })
                .map_err(|e| here(row.body_offset + e.offset, e.message))?
            } else {
                grid_at(row.body, &|from, to| {
                    let base = at + row.body_offset;
                    sent(base + from, base + to)
                })
                .map_err(|e| here(row.body_offset + e.offset, e.message))?
            };
            let id = id as u32;
            let gain = row.gain;
            let pan = row.pan;
            let begin = row.begin;
            let pitch = row.pitch;
            layers.push(hits.map(move |hit| {
                let mut step = Step::from(Value::Sample(id));
                step.node = node;
                step.pan = pan;
                step.begin = begin;
                step.pitch = pitch;
                step.src = hit.src;
                step.gain = match hit.strength {
                    Strength::Normal => gain,
                    Strength::Accent => Some(gain.unwrap_or(0.0) + ACCENT_DB),
                    Strength::Ghost => Some(gain.unwrap_or(0.0) + GHOST_DB),
                };
                step
            }));
        }
        if layers.is_empty() {
            return Err(Diagnostic::new("a grid needs at least one row", span)
                .with_help("for example `grid { kick: x... }`"));
        }
        Ok(Pattern::stack(layers))
    }

    // ---- numbers -----------------------------------------------------------

    /// A constant number with an optional unit, evaluated exactly: `1/8beat`, `-5st`.
    pub(super) fn quantity(&self, e: &Expr) -> Res<(Rational, Option<String>)> {
        match &e.kind {
            ExprKind::Num { value, unit } => Ok((Rational::from_f64(*value), unit.clone())),
            ExprKind::Name(name) => match self.bindings.get(name).and_then(|v| v.as_num()) {
                Some(Value::Num(n)) => Ok((Rational::from_f64(n), None)),
                Some(Value::Bars(b)) => Ok((Rational::from_f64(b), Some("bars".into()))),
                Some(Value::Beats(b)) => Ok((Rational::from_f64(b), Some("beats".into()))),
                Some(Value::Seconds(s)) => Ok((Rational::from_f64(s), Some("sec".into()))),
                _ => Err(Diagnostic::new("expected a number", e.span)
                    .with_help("for example `2`, `3/2` or `1/8beat`")),
            },
            ExprKind::Neg(inner) => self.quantity(inner).map(|(r, u)| (-r, u)),
            ExprKind::Binary { op, lhs, rhs } => {
                let ((l, lu), (r, ru)) = (self.quantity(lhs)?, self.quantity(rhs)?);
                let bad = || Diagnostic::new("can't combine these units", e.span);
                match op {
                    BinOp::Add | BinOp::Sub if lu == ru => {
                        Ok((if *op == BinOp::Add { l + r } else { l - r }, lu))
                    }
                    BinOp::Mul if ru.is_none() => Ok((l * r, lu)),
                    BinOp::Mul if lu.is_none() => Ok((l * r, ru)),
                    BinOp::Div if ru.is_none() && !r.is_zero() => Ok((l / r, lu)),
                    BinOp::Div if lu == ru && !r.is_zero() => Ok((l / r, None)),
                    _ => Err(bad()),
                }
            }
            _ => Err(Diagnostic::new("expected a number", e.span)
                .with_help("for example `2`, `3/2` or `1/8beat`")),
        }
    }

    /// A plain number (no unit).
    pub(super) fn plain(&self, e: &Expr) -> Res<Rational> {
        match self.quantity(e)? {
            (r, None) => Ok(r),
            (_, Some(u)) => Err(Diagnostic::new(
                format!("expected a plain number, found a value in `{u}`"),
                e.span,
            )),
        }
    }

    /// A length of time as a number of cycles (bars). Beats are a share of a
    /// bar; seconds would depend on the tempo, so they are not accepted.
    pub(super) fn cycles(&self, e: &Expr) -> Res<Rational> {
        self.cycles_in(e, self.cx.tempo.beats_per_bar)
    }

    /// Like [`Perform::cycles`], for bars that are `beats_per_bar` long.
    pub(super) fn cycles_in(&self, e: &Expr, beats_per_bar: f64) -> Res<Rational> {
        let (r, unit) = self.quantity(e)?;
        match unit.as_deref() {
            None | Some("bar" | "bars") => Ok(r),
            Some("beat" | "beats") => Ok(r / Rational::from_f64(beats_per_bar)),
            Some(u) => Err(Diagnostic::new(
                format!("expected a length in beats or bars, found `{u}`"),
                e.span,
            )),
        }
    }
}
