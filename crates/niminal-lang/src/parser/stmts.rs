//! Parsing function bodies and statements, notes, commands and `quantize`.

use super::*;

impl Parser {
    pub(super) fn function(&mut self) -> Res<FnDef> {
        self.function_from("fn")
    }

    pub(super) fn function_from(&mut self, keyword: &str) -> Res<FnDef> {
        let start = self.bump().span; // `fn` or `def`
        let name = self.ident(if keyword == "def" {
            "a score function name"
        } else {
            "a function name"
        })?;
        let params = self.params()?;
        let mut result = FnResult::Inferred;
        if self.peek() == &Tok::Arrow {
            let arrow = self.bump().span;
            if self.peek() == &Tok::LParen {
                self.bump();
                let mut outputs: Vec<Ident> = Vec::new();
                loop {
                    let out = self.ident("the name of a result")?;
                    if outputs.iter().any(|o| o.name == out.name) {
                        return Err(Diagnostic::new(
                            format!("result `{}` is named twice", out.name),
                            out.span,
                        ));
                    }
                    outputs.push(out);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RParen)?;
                if outputs.len() < 2 {
                    return Err(Diagnostic::new("`->` lists at least two results", arrow)
                        .with_help("a function with one result writes its unit, such as `-> hz`, or nothing"));
                }
                result = FnResult::Named(outputs);
            } else {
                result = FnResult::Unit(
                    self.ident("a unit such as `hz`, or `(a, b)` for several results")?,
                );
            }
        }
        let (body, end) = self.block()?;
        Ok(FnDef {
            name,
            params,
            result,
            body,
            span: start.to(end),
        })
    }

    /// `{ statements }`, returning the statements and the span of the braces.
    pub(super) fn block(&mut self) -> Res<(Vec<Stmt>, Span)> {
        let start = self.expect(&Tok::LBrace)?;
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            body.push(self.stmt()?);
            if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the statement"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((body, start.to(end)))
    }

    pub(super) fn type_spec(&mut self) -> Res<TypeSpec> {
        let start = self.span();
        match self.peek().clone() {
            Tok::LBracket => {
                self.bump();
                let inner = self.type_spec()?;
                let end = self.expect(&Tok::RBracket)?;
                Ok(TypeSpec::Array(Box::new(inner), start.to(end)))
            }
            Tok::LParen => {
                self.bump();
                let mut names = vec![self.ident("a name for a choice")?];
                while self.eat(&Tok::Bar) {
                    names.push(self.ident("a name for a choice")?);
                }
                self.expect(&Tok::RParen)?;
                if names.len() < 2 {
                    return Err(Diagnostic::new("a choice lists at least two names", start)
                        .with_help("for example `mode: (low | band | high) = low`"));
                }
                Ok(TypeSpec::Choice(names))
            }
            Tok::Num { .. } | Tok::Minus => {
                let lo = self.sum()?;
                if !matches!(self.peek(), Tok::DotDot) {
                    return Err(self.unexpected("`..` and the upper bound, as in `0..1`"));
                }
                self.bump();
                let hi = self.sum()?;
                let span = lo.span.to(hi.span);
                Ok(TypeSpec::Range { lo, hi, span })
            }
            _ => Ok(TypeSpec::Unit(self.ident(
                "a unit such as `hz`, a range such as `0..1`, a choice or a list type",
            )?)),
        }
    }

    /// Is the next thing `[name, name] =`?
    pub(super) fn at_destructure(&self) -> bool {
        if self.peek() != &Tok::LBracket {
            return false;
        }
        let mut i = 1;
        loop {
            if !matches!(self.peek_at(i), Tok::Ident(_)) {
                return false;
            }
            match self.peek_at(i + 1) {
                Tok::Comma => i += 2,
                Tok::RBracket => return self.peek_at(i + 2) == &Tok::Eq,
                _ => return false,
            }
        }
    }

    /// Is the next thing `name[...] =`? The position of the `]` if so.
    pub(super) fn at_index_assign(&self) -> Option<usize> {
        if !matches!(self.peek(), Tok::Ident(_)) || self.peek_at(1) != &Tok::LBracket {
            return None;
        }
        let mut depth = 0;
        let mut i = self.pos + 1;
        while i < self.tokens.len() {
            match self.tokens[i].tok {
                Tok::LBracket | Tok::LParen => depth += 1,
                Tok::RBracket | Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return (self.tokens.get(i + 1).map(|t| &t.tok) == Some(&Tok::Eq))
                            .then_some(i);
                    }
                }
                Tok::Newline | Tok::Eof | Tok::LBrace | Tok::RBrace => return None,
                _ => {}
            }
            i += 1;
        }
        None
    }

    pub(super) fn stmt(&mut self) -> Res<Stmt> {
        if self.at_destructure() {
            self.bump();
            let mut names = vec![self.ident("a name")?];
            while self.eat(&Tok::Comma) {
                names.push(self.ident("a name")?);
            }
            self.expect(&Tok::RBracket)?;
            self.expect(&Tok::Eq)?;
            return Ok(Stmt::Destructure {
                names,
                value: self.expr()?,
            });
        }
        if let Some(close) = self.at_index_assign() {
            let name = self.ident("a name")?;
            self.bump(); // `[`
            let index = self.expr()?;
            if self.pos != close {
                return Err(self.unexpected("`]`"));
            }
            self.bump(); // `]`
            self.bump(); // `=`
            return Ok(Stmt::IndexAssign {
                name,
                index,
                value: self.expr()?,
            });
        }
        if self.is_keyword("state") && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.bump();
            let name = self.ident("a name for the state")?;
            self.expect(&Tok::Eq)?;
            return Ok(Stmt::State {
                name,
                init: self.expr()?,
            });
        }
        if matches!(self.peek(), Tok::Ident(_))
            && self.peek_at(1) == &Tok::Plus
            && self.peek_at(2) == &Tok::Eq
        {
            let name = self.ident("a name")?;
            self.bump();
            self.bump();
            return Ok(Stmt::AddAssign {
                name,
                value: self.expr()?,
            });
        }
        if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            Ok(Stmt::Bind {
                name,
                value: self.expr()?,
            })
        } else {
            Ok(Stmt::Expr(self.expr()?))
        }
    }

    /// `name(args) for <duration>`, after any `at <position>`.
    pub(super) fn note(&mut self, at: Option<AtPos>) -> Res<NoteStmt> {
        let start = at.as_ref().map_or(self.span(), |_| self.span());
        let target = self.ident("an instrument to play")?;
        if self.peek() != &Tok::LParen {
            return Err(self.unexpected("`(` after the instrument name"));
        }
        let args = self.args()?;
        if !self.is_keyword("for") {
            return Err(self.unexpected("`for` and a duration, such as `for 1beat`"));
        }
        self.bump();
        let dur = self.expr()?;
        let span = start.to(self.prev_span());
        Ok(NoteStmt {
            at,
            target,
            args,
            dur,
            span,
        })
    }

    /// `{ key: value, key: value }`, separated by commas or lines.
    /// `{ key: value, ... }`. With `bare`, a lone name is shorthand for `name: name`.
    /// `events { ... }` is accepted without a colon (the lexer folds it into one token).
    pub(super) fn entries(&mut self, bare: bool) -> Res<(Vec<(Ident, Expr)>, Span)> {
        self.expect(&Tok::LBrace)?;
        let mut entries = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            if let Tok::Events(raw) = self.peek().clone() {
                let span = self.span();
                self.bump();
                let key = Ident {
                    name: "events".into(),
                    span,
                };
                entries.push((
                    key,
                    Expr {
                        kind: ExprKind::Events(raw),
                        span,
                    },
                ));
            } else {
                let mut key = self.ident("a name")?;
                if self.eat(&Tok::Dot) {
                    let param = self.ident("an fx parameter")?;
                    key.name = format!("{}::{}", key.name, param.name);
                    key.span = key.span.to(param.span);
                }
                if !bare && self.peek() == &Tok::LBrace {
                    // `fx_name { open: x.x.  release: 300ms }`: lanes for one effect
                    let (rows, end) = self.entries(false)?;
                    let span = key.span.to(end);
                    let value = Expr {
                        kind: ExprKind::FxLanes(rows),
                        span,
                    };
                    entries.push((key, value));
                } else if bare && matches!(self.peek(), Tok::Comma | Tok::Newline | Tok::RBrace) {
                    let value = Expr {
                        kind: ExprKind::Name(key.name.clone()),
                        span: key.span,
                    };
                    entries.push((key, value));
                } else if !bare && key.name == "send" {
                    // `send: space -8db`: a bus, then an optional gain
                    self.expect(&Tok::Colon)?;
                    let bus = self.ident("the name of a bus")?;
                    let mut args = vec![Arg {
                        name: None,
                        receiver: false,
                        value: Expr {
                            kind: ExprKind::Name(bus.name),
                            span: bus.span,
                        },
                    }];
                    let mut span = bus.span;
                    if !matches!(self.peek(), Tok::Comma | Tok::Newline | Tok::RBrace) {
                        let gain = self.expr()?;
                        span = span.to(gain.span);
                        args.push(Arg {
                            name: None,
                            receiver: false,
                            value: gain,
                        });
                    }
                    let name = key.clone();
                    entries.push((
                        key,
                        Expr {
                            kind: ExprKind::Call { name, args },
                            span,
                        },
                    ));
                } else {
                    self.expect(&Tok::Colon)?;
                    entries.push((key, self.expr()?));
                }
            }
            if !self.eat(&Tok::Comma) && !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the entry"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((entries, end))
    }

    /// `{ channels: stereo  quantize: { default: 1bar  play: cycle }  tuning: { a4: 432hz } }`
    pub(super) fn config_entries(&mut self) -> Res<ConfigEntries> {
        self.expect(&Tok::LBrace)?;
        let (mut entries, mut quantize, mut tuning) = (Vec::new(), Vec::new(), Vec::new());
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            let key = self.ident("a name")?;
            self.expect(&Tok::Colon)?;
            if key.name == "quantize" && self.peek() == &Tok::LBrace {
                if !quantize.is_empty() {
                    return Err(Diagnostic::new("`quantize` is given twice", key.span));
                }
                quantize = self.entries(false)?.0;
            } else if key.name == "tuning" && self.peek() == &Tok::LBrace {
                if !tuning.is_empty() {
                    return Err(Diagnostic::new("`tuning` is given twice", key.span));
                }
                tuning = self.entries(false)?.0;
            } else {
                entries.push((key, self.expr()?));
            }
            if !self.eat(&Tok::Comma) && !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the entry"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((entries, quantize, tuning, end))
    }

    /// A time, or `bar 5 beat 3`.
    pub(super) fn at_pos(&mut self) -> Res<AtPos> {
        if self.peek() == &Tok::Plus {
            self.bump();
            return Ok(AtPos::After(self.expr()?));
        }
        if !self.is_keyword("bar") {
            return Ok(AtPos::Time(self.expr()?));
        }
        self.bump();
        let number = |p: &mut Parser| -> Res<Expr> {
            let span = p.span();
            match p.peek().clone() {
                Tok::Num { value, unit: None } => {
                    p.bump();
                    Ok(Expr {
                        kind: ExprKind::Num { value, unit: None },
                        span,
                    })
                }
                _ => Err(p.unexpected("a number")),
            }
        };
        let span = self.span();
        match self.peek().clone() {
            // `bar 5 beat 3` lexes as `bar`, `5beat`, `3`.
            Tok::Num {
                value,
                unit: Some(u),
            } if u == "beat" || u == "beats" => {
                self.bump();
                let bar = Expr {
                    kind: ExprKind::Num { value, unit: None },
                    span,
                };
                Ok(AtPos::Bar {
                    bar,
                    beat: Some(number(self)?),
                })
            }
            Tok::Num { value, unit: None } => {
                self.bump();
                let bar = Expr {
                    kind: ExprKind::Num { value, unit: None },
                    span,
                };
                let beat = if self.is_keyword("beat") {
                    self.bump();
                    Some(number(self)?)
                } else {
                    None
                };
                Ok(AtPos::Bar { bar, beat })
            }
            _ => Err(self.unexpected("a bar number, as in `bar 5`")),
        }
    }

    pub(super) fn is_edit(&self) -> bool {
        matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Dot
    }

    /// `bass.reverse`, `lead.transpose(+5st).drop(10%)`, or a command such as `bass.mute` or `verse.start`.
    pub(super) fn edit(&mut self, at: Option<AtPos>) -> Res<CommandStmt> {
        let start = self.span();
        let node = self.ident("a node")?;
        self.expect(&Tok::Dot)?;
        let first = self.span();
        // `bass.mute`, `verse.start`...: the commands, written after the thing they act on
        if let Tok::Ident(verb) = self.peek().clone()
            && !matches!(self.peek_at(1), Tok::LParen | Tok::Dot)
        {
            let command = match verb.as_str() {
                "start" => Some(Command::Start {
                    name: node.clone(),
                    verb: first,
                }),
                "stop" => Some(Command::Stop(node.clone())),
                "mute" => Some(Command::Mute(node.clone())),
                "unmute" => Some(Command::Unmute(node.clone())),
                "solo" => Some(Command::Solo(node.clone())),
                "unsolo" => Some(Command::Unsolo(Some(node.clone()))),
                _ => None,
            };
            if let Some(command) = command {
                self.bump();
                if matches!(&command, Command::Start { .. }) && self.is_keyword("then") {
                    return Err(Diagnostic::new(
                        "`then` is gone: time the next start with `at +4bars chorus.start`",
                        self.span(),
                    ));
                }
                let quantize = self.quantize()?;
                return Ok(CommandStmt {
                    at,
                    command,
                    quantize,
                    span: start.to(self.prev_span()),
                });
            }
        }
        if let Tok::Ident(verb) = self.peek().clone()
            && self.peek_at(1) == &Tok::LParen
        {
            if verb == "play" {
                return Err(Diagnostic::new(
                    "there is no `play`: edit the clip and evaluate it, or change it with `set`, `reverse` and the like",
                    self.span(),
                ));
            }
            if verb == "__update" {
                self.bump();
                self.bump();
                let what = self.expr()?;
                self.expect(&Tok::RParen)?;
                let quantize = self.quantize()?;
                return Ok(CommandStmt {
                    at,
                    command: Command::Update { node, what },
                    quantize,
                    span: start.to(self.prev_span()),
                });
            }
        }
        let change = self.expr()?;
        let quantize = self.quantize()?;
        Ok(CommandStmt {
            at,
            command: Command::Edit {
                node,
                change: first.to(change.span),
            },
            quantize,
            span: start.to(self.prev_span()),
        })
    }

    pub(super) fn at_command(&self) -> bool {
        ["hush", "panic", "reset", "new", "unsolo", "capture"]
            .iter()
            .any(|kw| self.is_keyword(kw))
    }

    pub(super) fn command(&mut self, at: Option<AtPos>) -> Res<CommandStmt> {
        let start = self.span();
        let Tok::Ident(keyword) = self.bump().tok else {
            unreachable!("checked by at_command")
        };
        let command = match keyword.as_str() {
            "hush" => Command::Hush,
            "panic" => Command::Panic,
            "reset" => Command::Reset,
            "new" => Command::New,
            "unsolo" => Command::Unsolo(None),
            "capture" => self.capture_command()?,
            _ => unreachable!("checked by at_command"),
        };
        let quantize = self.quantize()?;
        Ok(CommandStmt {
            at,
            command,
            quantize,
            span: start.to(self.prev_span()),
        })
    }

    /// `capture last 32bars as jam`
    pub(super) fn capture_command(&mut self) -> Res<Command> {
        if !self.is_keyword("last") {
            return Err(self
                .unexpected("`last`")
                .with_help("for example `capture last 32bars as jam`"));
        }
        self.bump();
        let length = self.expr()?;
        if !self.is_keyword("as") {
            return Err(self
                .unexpected("`as` and a name")
                .with_help("for example `capture last 32bars as jam`"));
        }
        self.bump();
        let name = self.ident("a name for the captured section")?;
        Ok(Command::Capture { length, name })
    }

    /// `@ next 4bars`, `@ in 2beats`, `@ bar`, `@ now`: when a change lands.
    pub(super) fn quantize(&mut self) -> Res<Option<Quantize>> {
        if self.peek() != &Tok::At {
            return Ok(None);
        }
        self.bump();
        let relation = if self.is_keyword("next") {
            self.bump();
            QuantizeRelation::Next
        } else if self.is_keyword("in") {
            self.bump();
            QuantizeRelation::In
        } else {
            QuantizeRelation::OnGrid
        };
        let (count, unit) = match self.peek().clone() {
            Tok::Num {
                value,
                unit: Some(u),
            } => {
                let unit = match u.as_str() {
                    "beat" | "beats" => QuantizeUnit::Beat,
                    "bar" | "bars" => QuantizeUnit::Bar,
                    _ => return Err(self.unexpected("`beats` or `bars`")),
                };
                self.bump();
                (value, unit)
            }
            Tok::Ident(word) => {
                let unit = match word.as_str() {
                    "now" => QuantizeUnit::Now,
                    "beat" => QuantizeUnit::Beat,
                    "bar" => QuantizeUnit::Bar,
                    "cycle" => QuantizeUnit::Cycle,
                    _ => {
                        return Err(self.unexpected(
                            "`now`, `beat`, `bar`, `cycle`, or a count such as `4bars`",
                        ));
                    }
                };
                self.bump();
                (1.0, unit)
            }
            _ => return Err(self.unexpected("when it should land, such as `bar` or `4bars`")),
        };
        Ok(Some(Quantize {
            relation,
            count,
            unit,
        }))
    }
}
