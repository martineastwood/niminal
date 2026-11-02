//! Parsing expressions: operators, `if`/`match`/`for` values, postfix calls and primaries.

use super::*;

impl Parser {
    // ---- expressions -------------------------------------------------

    /// Run `f` one level deeper, refusing to go too deep.
    pub(super) fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
        if self.depth >= MAX_DEPTH {
            return Err(Diagnostic::new(
                format!("this is nested more than {MAX_DEPTH} levels deep"),
                self.span(),
            ));
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    pub(super) fn expr(&mut self) -> Res<Expr> {
        self.nested(Self::or_expr)
    }

    pub(super) fn or_expr(&mut self) -> Res<Expr> {
        let mut lhs = self.and_expr()?;
        while self.is_keyword("or") {
            self.bump();
            let rhs = self.and_expr()?;
            lhs = logic(LogicOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    pub(super) fn and_expr(&mut self) -> Res<Expr> {
        let mut lhs = self.comparison()?;
        while self.is_keyword("and") {
            self.bump();
            let rhs = self.comparison()?;
            lhs = logic(LogicOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `a < b`. Comparisons don't chain: `a < b < c` is an error.
    pub(super) fn comparison(&mut self) -> Res<Expr> {
        let lhs = self.range()?;
        let op = match self.peek() {
            Tok::Lt => CmpOp::Lt,
            Tok::Gt => CmpOp::Gt,
            Tok::Le => CmpOp::Le,
            Tok::Ge => CmpOp::Ge,
            Tok::EqEq => CmpOp::Eq,
            Tok::NotEq => CmpOp::Ne,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.range()?;
        if matches!(
            self.peek(),
            Tok::Lt | Tok::Gt | Tok::Le | Tok::Ge | Tok::EqEq | Tok::NotEq
        ) {
            return Err(Diagnostic::new("comparisons don't chain", self.span())
                .with_help("combine two with `and`: `a < b and b < c`"));
        }
        let span = lhs.span.to(rhs.span);
        Ok(Expr {
            kind: ExprKind::Compare {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            span,
        })
    }

    /// `lo..hi` or `lo..<hi`.
    pub(super) fn range(&mut self) -> Res<Expr> {
        let lo = self.sum()?;
        let inclusive = match self.peek() {
            Tok::DotDot => true,
            Tok::DotDotLt => false,
            _ => return Ok(lo),
        };
        self.bump();
        let hi = self.sum()?;
        let span = lo.span.to(hi.span);
        Ok(Expr {
            kind: ExprKind::Range {
                lo: Box::new(lo),
                hi: Box::new(hi),
                inclusive,
            },
            span,
        })
    }

    /// Is the next token, past any newlines, `else`?
    pub(super) fn else_follows(&self) -> bool {
        let mut i = 0;
        while self.peek_at(i) == &Tok::Newline {
            i += 1;
        }
        matches!(self.peek_at(i), Tok::Ident(k) if k == "else")
    }

    /// `if cond { ... } else { ... }` (or `else if ...`), after the `if`. The `else` is optional;
    /// without one the `if` can only be a statement. `if cond then a else b` is the one-line form
    /// for a single value, and its `else` is required.
    pub(super) fn if_expr(&mut self) -> Res<Expr> {
        let start = self.bump().span; // `if`
        let cond = self.expr()?;
        if self.is_keyword("then") {
            self.bump();
            let then = self.expr()?;
            if !self.else_follows() {
                return Err(self.unexpected("`else`, as `if ... then` needs both values"));
            }
            self.skip_newlines();
            self.bump(); // `else`
            let otherwise = self.expr()?;
            return Ok(Expr {
                span: start.to(otherwise.span),
                kind: ExprKind::If {
                    cond: Box::new(cond),
                    then: vec![Stmt::Expr(then)],
                    otherwise: Some(vec![Stmt::Expr(otherwise)]),
                },
            });
        }
        let (then, mut end) = self.block()?;
        let otherwise = if self.else_follows() {
            self.skip_newlines();
            self.bump(); // `else`
            if self.is_keyword("if") {
                let inner = self.nested(Self::if_expr)?;
                end = inner.span;
                Some(vec![Stmt::Expr(inner)])
            } else {
                let (block, e) = self.block()?;
                end = e;
                Some(block)
            }
        } else {
            None
        };
        Ok(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                otherwise,
            },
            span: start.to(end),
        })
    }

    /// `match mode { low: a, band | high: b, else: c }`, which is an `if`/`else if` chain on
    /// `mode == pattern`. An arm's body is one expression or a `{ block }`, and `else` comes last.
    /// The subject is a name so that testing it once per arm costs nothing.
    pub(super) fn match_expr(&mut self) -> Res<Expr> {
        let start = self.bump().span; // `match`
        let subject = self.ident("a name to match on")?;
        let subject_span = self.prev_span();
        self.expect(&Tok::LBrace)?;
        let mut arms: Vec<(Expr, Vec<Stmt>)> = Vec::new();
        let mut otherwise = None;
        let end = loop {
            while matches!(self.peek(), Tok::Newline | Tok::Comma) {
                self.bump();
            }
            if self.peek() == &Tok::RBrace {
                break self.bump().span;
            }
            if otherwise.is_some() {
                return Err(self.unexpected("`}`, as `else` is the last arm"));
            }
            let mut cond: Option<Expr> = None;
            if self.is_keyword("else") {
                self.bump();
            } else {
                // `a | b` matches either
                loop {
                    let pattern = self.sum()?;
                    let name = Expr {
                        kind: ExprKind::Name(subject.name.clone()),
                        span: subject_span,
                    };
                    let test = Expr {
                        span: pattern.span,
                        kind: ExprKind::Compare {
                            op: CmpOp::Eq,
                            lhs: Box::new(name),
                            rhs: Box::new(pattern),
                        },
                    };
                    cond = Some(match cond {
                        None => test,
                        Some(prior) => Expr {
                            span: prior.span.to(test.span),
                            kind: ExprKind::Logic {
                                op: LogicOp::Or,
                                lhs: Box::new(prior),
                                rhs: Box::new(test),
                            },
                        },
                    });
                    if !self.eat(&Tok::Bar) {
                        break;
                    }
                }
            }
            self.expect(&Tok::Colon)?;
            let body = if self.peek() == &Tok::LBrace {
                self.block()?.0
            } else {
                vec![Stmt::Expr(self.expr()?)]
            };
            match cond {
                Some(cond) => arms.push((cond, body)),
                None => otherwise = Some(body),
            }
        };
        let span = start.to(end);
        let mut arms = arms.into_iter().rev();
        let Some((cond, then)) = arms.next() else {
            return Err(Diagnostic::new("a `match` needs an arm to test", span)
                .with_help("list the choices to match, such as `match mode { low: 1, high: 2 }`"));
        };
        let last = Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                otherwise,
            },
            span,
        };
        Ok(arms.fold(last, |rest, (cond, then)| Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                otherwise: Some(vec![Stmt::Expr(rest)]),
            },
            span,
        }))
    }

    /// `for i in 0..8 { ... }`, after seeing `for i in`.
    pub(super) fn for_expr(&mut self) -> Res<Expr> {
        let start = self.bump().span; // `for`
        let var = self.ident("a name for each entry")?;
        self.bump(); // `in`
        let iter = self.expr()?;
        let (body, end) = self.block()?;
        Ok(Expr {
            kind: ExprKind::For {
                var,
                iter: Box::new(iter),
                body,
            },
            span: start.to(end),
        })
    }

    pub(super) fn sum(&mut self) -> Res<Expr> {
        let mut lhs = self.term()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.term()?;
            lhs = binary(op, lhs, rhs);
        }
    }

    pub(super) fn term(&mut self) -> Res<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => {
                    return Err(Diagnostic::new("`%` only means percent", self.span())
                        .with_help("for the remainder write `x.mod(y)`"));
                }
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.unary()?;
            lhs = binary(op, lhs, rhs);
        }
    }

    pub(super) fn unary(&mut self) -> Res<Expr> {
        if self.is_keyword("not") {
            let start = self.bump().span;
            let inner = self.nested(Self::unary)?;
            let span = start.to(inner.span);
            return Ok(Expr {
                kind: ExprKind::Not(Box::new(inner)),
                span,
            });
        }
        // a leading `+`, as in `+5st`, changes nothing
        if self.peek() == &Tok::Plus {
            self.bump();
            return self.nested(Self::unary);
        }
        if self.peek() == &Tok::Minus {
            let start = self.bump().span;
            let inner = self.nested(Self::unary)?;
            let span = start.to(inner.span);
            return Ok(Expr {
                kind: ExprKind::Neg(Box::new(inner)),
                span,
            });
        }
        self.power()
    }

    /// `base ^ exponent` is `pow(base, exponent)`. It binds tighter than `*` and a leading `-`, and
    /// `a ^ b ^ c` is `a ^ (b ^ c)`, so `-2 ^ 2` is -4 and `2 ^ -1` is a half.
    pub(super) fn power(&mut self) -> Res<Expr> {
        let base = self.postfix()?;
        if self.peek() != &Tok::Caret {
            return Ok(base);
        }
        let op = self.bump().span;
        let exponent = self.nested(Self::unary)?;
        let span = base.span.to(exponent.span);
        let arg = |value| Arg {
            name: None,
            receiver: false,
            value,
        };
        Ok(Expr {
            kind: ExprKind::Call {
                name: Ident {
                    name: "pow".into(),
                    span: op,
                },
                args: vec![arg(base), arg(exponent)],
            },
            span,
        })
    }

    pub(super) fn postfix(&mut self) -> Res<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.peek() == &Tok::LBracket {
                self.bump();
                let index = self.expr()?;
                let end = self.expect(&Tok::RBracket)?;
                let span = e.span.to(end);
                e = Expr {
                    kind: ExprKind::Index {
                        list: Box::new(e),
                        index: Box::new(index),
                    },
                    span,
                };
                continue;
            }
            if self.peek() == &Tok::LBrace && matches!(&e.kind, ExprKind::Name(n) if n == "control")
            {
                // `control { ... }` takes no arguments, so it has no parentheses to make it a call
                e = Expr {
                    kind: ExprKind::Call {
                        name: Ident {
                            name: "control".into(),
                            span: e.span,
                        },
                        args: Vec::new(),
                    },
                    span: e.span,
                };
            }
            if self.peek() == &Tok::LBrace
                && matches!(&e.kind, ExprKind::Call { name, .. } if matches!(name.name.as_str(), "reset_on" | "spawn" | "oversample" | "control"))
            {
                let (body, end) = self.block()?;
                let body_span = e.span.to(end);
                if let ExprKind::Call { args, .. } = &mut e.kind {
                    args.push(Arg {
                        name: None,
                        receiver: false,
                        value: Expr {
                            kind: ExprKind::Lambda {
                                params: Vec::new(),
                                body,
                            },
                            span: body_span,
                        },
                    });
                }
                e.span = body_span;
                continue;
            }
            if self.peek() == &Tok::LBrace
                && let ExprKind::Call { name, args } = &e.kind
                && name.name == "after"
                && let [
                    Arg {
                        name: None,
                        value: delay,
                        ..
                    },
                ] = args.as_slice()
            {
                e = self.after_block(e.span, name.span, delay.clone())?;
                continue;
            }
            if self.peek() != &Tok::Dot {
                break;
            }
            self.bump();
            let name = self.ident("a method name after `.`")?;
            let mut args = vec![Arg {
                name: None,
                value: e.clone(),
                receiver: true,
            }];
            if self.peek() == &Tok::LParen {
                args.extend(self.args()?);
            }
            let span = e.span.to(self.prev_span());
            e = Expr {
                kind: ExprKind::Call { name, args },
                span,
            };
        }
        Ok(e)
    }

    /// `after(delay) { body }`: the body starts when the note is `delay` old, from fresh state,
    /// and is silent until then. It is `reset_on(note.time >= delay) { body } * (note.time >= delay)`.
    pub(super) fn after_block(&mut self, call: Span, name: Span, delay: Expr) -> Res<Expr> {
        let (body, end) = self.block()?;
        let span = call.to(end);
        let reached = || Expr {
            span: delay.span,
            kind: ExprKind::Compare {
                op: CmpOp::Ge,
                lhs: Box::new(Expr {
                    span: name,
                    kind: ExprKind::Call {
                        name: Ident {
                            name: "time".into(),
                            span: name,
                        },
                        args: vec![Arg {
                            name: None,
                            receiver: true,
                            value: Expr {
                                kind: ExprKind::Name("note".into()),
                                span: name,
                            },
                        }],
                    },
                }),
                rhs: Box::new(delay.clone()),
            },
        };
        let scope = Expr {
            span,
            kind: ExprKind::Call {
                name: Ident {
                    name: "reset_on".into(),
                    span: name,
                },
                args: vec![
                    Arg {
                        name: None,
                        receiver: false,
                        value: reached(),
                    },
                    Arg {
                        name: None,
                        receiver: false,
                        value: Expr {
                            kind: ExprKind::Lambda {
                                params: Vec::new(),
                                body,
                            },
                            span,
                        },
                    },
                ],
            },
        };
        Ok(binary(BinOp::Mul, scope, reached()))
    }

    pub(super) fn primary(&mut self) -> Res<Expr> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Num { value, unit } => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Num { value, unit },
                    span,
                })
            }
            Tok::Str(text) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Str(text),
                    span,
                })
            }
            Tok::Pattern(raw) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Pattern(raw),
                    span,
                })
            }
            Tok::Grid(raw) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Grid(raw),
                    span,
                })
            }
            Tok::Rows(raw) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Rows(raw),
                    span,
                })
            }
            Tok::Steps(raw) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Steps(raw),
                    span,
                })
            }
            Tok::Events(raw) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Events(raw),
                    span,
                })
            }
            Tok::Tilde => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Rest,
                    span,
                })
            }
            Tok::LBracket => self.nested(|p| {
                p.bump();
                let mut items = Vec::new();
                if p.peek() != &Tok::RBracket {
                    items.push(p.expr()?);
                    while p.eat(&Tok::Comma) && p.peek() != &Tok::RBracket {
                        items.push(p.expr()?);
                    }
                }
                let end = p.expect(&Tok::RBracket)?;
                Ok(Expr {
                    kind: ExprKind::Array(items),
                    span: span.to(end),
                })
            }),
            Tok::LParen => self.nested(|p| {
                p.bump();
                let inner = p.expr()?;
                let end = p.expect(&Tok::RParen)?;
                Ok(Expr {
                    span: span.to(end),
                    ..inner
                })
            }),
            Tok::Ident(name)
                if name == "for"
                    && matches!(self.peek_at(1), Tok::Ident(_))
                    && matches!(self.peek_at(2), Tok::Ident(k) if k == "in") =>
            {
                self.nested(Self::for_expr)
            }
            Tok::Ident(name) if name == "fn" && self.peek_at(1) == &Tok::LParen => {
                self.bump();
                self.bump();
                let mut params = Vec::new();
                while self.peek() != &Tok::RParen {
                    params.push(self.ident("a parameter name")?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RParen)?;
                let (body, end) = self.block()?;
                Ok(Expr {
                    span: span.to(end),
                    kind: ExprKind::Lambda { params, body },
                })
            }
            Tok::Ident(name) if name == "env" && self.peek_at(1) == &Tok::LBracket => self.env(),
            Tok::Ident(name) if name == "if" => self.nested(Self::if_expr),
            Tok::Ident(name)
                if name == "match"
                    && matches!(self.peek_at(1), Tok::Ident(_))
                    && self.peek_at(2) == &Tok::LBrace =>
            {
                self.nested(Self::match_expr)
            }
            Tok::Ident(name)
                if (name == "clip" || name == "scene") && self.peek_at(1) == &Tok::LBrace =>
            {
                let is_clip = name == "clip";
                self.nested(|p| {
                    let start = p.bump().span;
                    let (mut entries, end) = p.entries(!is_clip)?;
                    let options = take_entry_options(&mut entries);
                    let block = NamedBlock {
                        name: Ident {
                            name: String::new(),
                            span: start,
                        },
                        base: None,
                        entries,
                        options,
                        span: start.to(end),
                    };
                    Ok(Expr {
                        kind: if is_clip {
                            ExprKind::ClipBlock(block)
                        } else {
                            ExprKind::SceneBlock(block)
                        },
                        span: start.to(p.prev_span()),
                    })
                })
            }
            Tok::Ident(name) => {
                let ident = self.ident("a name")?;
                if self.peek() == &Tok::LParen {
                    let args = self.args()?;
                    let span = span.to(self.prev_span());
                    Ok(Expr {
                        kind: ExprKind::Call { name: ident, args },
                        span,
                    })
                } else {
                    Ok(Expr {
                        kind: ExprKind::Name(name),
                        span,
                    })
                }
            }
            _ => Err(self.unexpected("a value")),
        }
    }

    pub(super) fn args(&mut self) -> Res<Vec<Arg>> {
        self.expect(&Tok::LParen)?;
        let mut args = Vec::new();
        while self.peek() != &Tok::RParen {
            let name = if matches!(self.peek(), Tok::Ident(_))
                && (self.peek_at(1) == &Tok::Colon
                    || (self.peek_at(1) == &Tok::Dot && self.peek_at(3) == &Tok::Colon))
            {
                let mut n = self.ident("an argument name")?;
                if self.eat(&Tok::Dot) {
                    let param = self.ident("an fx parameter")?;
                    n.name = format!("{}::{}", n.name, param.name);
                    n.span = n.span.to(param.span);
                }
                self.bump();
                Some(n)
            } else {
                None
            };
            let value = self.expr()?;
            args.push(Arg {
                name,
                value,
                receiver: false,
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(args)
    }
}
