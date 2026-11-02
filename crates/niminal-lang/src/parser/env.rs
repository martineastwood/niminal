//! Parsing envelope literals and their curves.

use super::*;

impl Parser {
    // ---- env literal -------------------------------------------------

    pub(super) fn env(&mut self) -> Res<Expr> {
        let start = self.bump().span; // `env`
        self.expect(&Tok::LBracket)?;

        let start_level = self.env_level()?;
        let mut segments = Vec::new();
        let mut sustain_at = None;

        loop {
            if self.peek() == &Tok::Bar {
                let bar = self.bump().span;
                if sustain_at.is_some() {
                    return Err(Diagnostic::new(
                        "an envelope can have only one sustain point `|`",
                        bar,
                    ));
                }
                sustain_at = Some(segments.len());
            }
            if self.peek() == &Tok::RBracket {
                break;
            }
            let (dur, curve) = self.env_duration()?;
            let level = self.env_level()?;
            segments.push(EnvSegment { dur, curve, level });
        }
        let mut end = self.expect(&Tok::RBracket)?;
        let looped = self.eat_loop(&mut end);
        if looped && sustain_at.is_some() {
            return Err(Diagnostic::new(
                "a looping envelope can't have a sustain point `|`",
                start.to(end),
            )
            .with_help("a loop never waits for a release: take out the `|`"));
        }

        Ok(Expr {
            kind: ExprKind::Env(EnvLit {
                start: start_level,
                segments,
                sustain_at,
                looped,
            }),
            span: start.to(end),
        })
    }

    /// `.loop` straight after an envelope's `]`. `.loop(g: ..)` is the feedback opcode, so it is left alone.
    pub(super) fn eat_loop(&mut self, end: &mut Span) -> bool {
        if self.peek() == &Tok::Dot
            && matches!(self.peek_at(1), Tok::Ident(s) if s == "loop")
            && self.peek_at(2) != &Tok::LParen
        {
            self.bump();
            *end = self.bump().span;
            return true;
        }
        false
    }

    /// A signed number, with an optional unit.
    pub(super) fn env_number(&mut self) -> Res<EnvNum> {
        let start = self.span();
        let negative = self.peek() == &Tok::Minus;
        if negative {
            self.bump();
        }
        match self.peek().clone() {
            Tok::Num { value, unit } => {
                let end = self.bump().span;
                Ok(EnvNum {
                    value: if negative { -value } else { value },
                    unit,
                    span: start.to(end),
                })
            }
            _ => Err(self.unexpected("a number")),
        }
    }

    /// A literal, a name, or a `(expression)`.
    pub(super) fn env_value(&mut self, what: &str) -> Res<EnvValue> {
        match self.peek() {
            Tok::Ident(_) => {
                let id = self.ident(what)?;
                let mut value = Expr {
                    kind: ExprKind::Name(id.name),
                    span: id.span,
                };
                // `note.dur`: a field read, unless the name after the dot is a curve (`decay.exp`)
                if self.peek() == &Tok::Dot
                    && matches!(self.peek_at(1), Tok::Ident(s) if !matches!(s.as_str(), "exp" | "log" | "curve"))
                {
                    self.bump();
                    let name = self.ident("a field name after `.`")?;
                    value = Expr {
                        span: id.span.to(name.span),
                        kind: ExprKind::Call {
                            name,
                            args: vec![Arg {
                                name: None,
                                value,
                                receiver: true,
                            }],
                        },
                    };
                }
                Ok(EnvValue::Expr(Box::new(value)))
            }
            Tok::LParen => {
                let start = self.bump().span;
                let inner = self.expr()?;
                let end = self.expect(&Tok::RParen)?;
                Ok(EnvValue::Expr(Box::new(Expr {
                    span: start.to(end),
                    ..inner
                })))
            }
            _ => self
                .env_number()
                .map(EnvValue::Lit)
                .map_err(|_| self.env_expected(what)),
        }
    }

    pub(super) fn env_level(&mut self) -> Res<EnvValue> {
        let v = self.env_value("a level, such as `0` or `-6db`")?;
        if let EnvValue::Lit(n) = &v
            && n.unit.as_deref().is_some_and(is_time_unit)
        {
            return Err(
                Diagnostic::new("expected a level here, found a duration", n.span).with_help(
                    "levels and durations alternate: two durations in a row is an error",
                ),
            );
        }
        Ok(v)
    }

    pub(super) fn env_duration(&mut self) -> Res<(EnvValue, Option<CurveKind>)> {
        let v = self.env_value("a duration, such as `5ms`")?;
        if let EnvValue::Lit(n) = &v
            && !n.unit.as_deref().is_some_and(is_time_unit)
        {
            let help = match &n.unit {
                None => "levels and durations alternate: two levels in a row is an error; durations need a time unit such as `ms`, `sec` or `beats`".to_string(),
                Some(u) => format!("`{u}` is not a time unit; durations use `ms`, `sec` or `beats`"),
            };
            return Err(Diagnostic::new("expected a duration here", n.span).with_help(help));
        }
        let curve = if self.peek() == &Tok::Dot {
            Some(self.curve()?)
        } else {
            None
        };
        Ok((v, curve))
    }

    pub(super) fn env_expected(&self, what: &str) -> Diagnostic {
        self.unexpected(what)
    }

    pub(super) fn curve(&mut self) -> Res<CurveKind> {
        self.bump(); // `.`
        let name = self.ident("`exp`, `log` or `curve(k)`")?;
        match name.name.as_str() {
            "exp" => Ok(CurveKind::Exp),
            "log" => Ok(CurveKind::Log),
            "curve" => {
                self.expect(&Tok::LParen)?;
                let neg = self.eat(&Tok::Minus);
                let Tok::Num { value, unit: None } = self.peek().clone() else {
                    return Err(self.unexpected("a number"));
                };
                self.bump();
                self.expect(&Tok::RParen)?;
                Ok(CurveKind::Custom(if neg { -value } else { value }))
            }
            other => Err(
                Diagnostic::new(format!("unknown curve `{other}`"), name.span)
                    .with_help("curves are `exp`, `log` and `curve(k)`; to use a calculation as a duration or level, put it in parentheses, such as `(note.dur * 0.9)`"),
            ),
        }
    }
}
