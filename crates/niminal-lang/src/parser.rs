use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Tok, Token, lex};

pub fn parse(src: &str) -> Result<Vec<Item>, Diagnostic> {
    parse_spanned(src).map(|items| items.into_iter().map(|(item, _)| item).collect())
}

/// Like [`parse`], with the stretch of source each top-level item covers.
pub fn parse_spanned(src: &str) -> Result<Vec<(Item, Span)>, Diagnostic> {
    let mut p = Parser { tokens: lex(src)?, pos: 0 };
    let mut items = Vec::new();
    loop {
        p.skip_newlines();
        if p.peek() == &Tok::Eof {
            return Ok(items);
        }
        let start = p.span();
        let item = p.item()?;
        items.push((item, start.to(p.prev_span())));
        if !matches!(p.peek(), Tok::Newline | Tok::Eof) {
            return Err(p.unexpected("end of statement"));
        }
    }
}

type Res<T> = Result<T, Diagnostic>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Num { .. } => "a number".into(),
        Tok::LParen => "`(`".into(),
        Tok::RParen => "`)`".into(),
        Tok::LBrace => "`{`".into(),
        Tok::RBrace => "`}`".into(),
        Tok::LBracket => "`[`".into(),
        Tok::RBracket => "`]`".into(),
        Tok::Comma => "`,`".into(),
        Tok::Colon => "`:`".into(),
        Tok::Dot => "`.`".into(),
        Tok::DotDot => "`..`".into(),
        Tok::Eq => "`=`".into(),
        Tok::Plus => "`+`".into(),
        Tok::Minus => "`-`".into(),
        Tok::Star => "`*`".into(),
        Tok::Slash => "`/`".into(),
        Tok::Bar => "`|`".into(),
        Tok::Tilde => "`~`".into(),
        Tok::At => "`@`".into(),
        Tok::Pattern(_) => "a pattern".into(),
        Tok::Grid(_) => "a grid".into(),
        Tok::Str(_) => "a string".into(),
        Tok::Newline => "the end of the line".into(),
        Tok::Eof => "the end of the file".into(),
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.tokens[(self.pos + n).min(self.tokens.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == tok {
            self.bump();
            true
        } else {
            false
        }
    }

    fn skip_newlines(&mut self) {
        while self.peek() == &Tok::Newline {
            self.bump();
        }
    }

    fn unexpected(&self, expected: &str) -> Diagnostic {
        Diagnostic::new(format!("expected {expected}, found {}", describe(self.peek())), self.span())
    }

    fn expect(&mut self, tok: &Tok) -> Res<Span> {
        if self.peek() == tok {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&describe(tok)))
        }
    }

    fn ident(&mut self, what: &str) -> Res<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => Ok(Ident { name, span: self.bump().span }),
            _ => Err(self.unexpected(what)),
        }
    }

    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    // ---- items -------------------------------------------------------

    fn item(&mut self) -> Res<Item> {
        if self.is_keyword("instr") {
            let (name, params, body, span) = self.definition()?;
            Ok(Item::Instr(InstrDef { name, params, body, span }))
        } else if self.is_keyword("bus") {
            self.bump();
            let name = self.ident("a bus name")?;
            let layout = self.eat(&Tok::Colon).then(|| self.expr()).transpose()?;
            Ok(Item::Bus(BusDecl { name, layout }))
        } else if self.is_keyword("sample") || self.is_keyword("kit") {
            let is_kit = self.is_keyword("kit");
            let start = self.bump().span;
            let name = self.ident(if is_kit { "a kit name" } else { "a sample name" })?;
            self.expect(&Tok::Eq)?;
            let source_span = self.span();
            let source = match self.peek().clone() {
                Tok::Str(path) => {
                    self.bump();
                    SampleSource::Path(path)
                }
                Tok::Ident(_) if is_kit && self.peek_at(1) == &Tok::Dot => {
                    let sample = self.ident("a sample name")?;
                    self.bump();
                    let method = self.ident("`slices`")?;
                    if method.name != "slices" {
                        return Err(Diagnostic::new(format!("a sample has no `{}`", method.name), method.span)
                            .with_help("the only thing to make from a sample is `.slices(n)`"));
                    }
                    self.expect(&Tok::LParen)?;
                    let Tok::Num { value: count, unit: None } = self.peek().clone() else {
                        return Err(self.unexpected("how many slices, such as `16`"));
                    };
                    self.bump();
                    self.expect(&Tok::RParen)?;
                    SampleSource::Slices { sample, count }
                }
                _ => {
                    return Err(self.unexpected("a file path in quotes").with_help(if is_kit {
                        "for example `kit drums = \"drums/808\"` or `kit chops = amen.slices(16)`"
                    } else {
                        "for example `sample kick = \"drums/kick.wav\"`"
                    }));
                }
            };
            let mut options = Vec::new();
            if self.is_keyword("with") {
                self.bump();
                options = self.args()?;
            }
            Ok(Item::Sample(SampleDecl { is_kit, name, source, source_span, options, span: start.to(self.prev_span()) }))
        } else if self.is_keyword("config") {
            let start = self.bump().span;
            let (entries, end) = self.entries()?;
            Ok(Item::Config(ConfigDecl { entries, span: start.to(end) }))
        } else if self.is_keyword("track") {
            let start = self.bump().span;
            let name = self.ident("a track name")?;
            let (body, end) = self.block()?;
            Ok(Item::Track(TrackDecl { name, body, span: start.to(end) }))
        } else if self.is_keyword("opcode") {
            let (name, params, body, span) = self.definition()?;
            Ok(Item::Opcode(OpcodeDef { name, params, body, span }))
        } else if self.is_keyword("tempo") {
            let start = self.bump().span;
            let value = self.expr()?;
            let quantize = self.quantize()?;
            let over = if self.is_keyword("over") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            Ok(Item::Tempo(TempoStmt { value, quantize, over, span: start.to(self.prev_span()) }))
        } else if self.is_keyword("meter") {
            self.bump();
            Ok(Item::Meter(self.expr()?))
        } else if self.is_keyword("clip") || self.is_keyword("scene") {
            let is_clip = self.is_keyword("clip");
            let start = self.bump().span;
            let name = self.ident(if is_clip { "a clip name" } else { "a scene name" })?;
            let (entries, end) = self.entries()?;
            let block = NamedBlock { name, entries, span: start.to(end) };
            Ok(if is_clip { Item::Clip(block) } else { Item::Scene(block) })
        } else if self.is_keyword("at") {
            self.bump();
            let at = self.at_pos()?;
            if self.at_command() {
                self.command(Some(at)).map(Item::Command)
            } else {
                self.note(Some(at)).map(Item::Note)
            }
        } else if self.at_command() {
            self.command(None).map(Item::Command)
        } else if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            Ok(Item::Bind { name, value: self.expr()? })
        } else {
            self.note(None).map(Item::Note)
        }
    }

    /// `instr|opcode name(params) { body }`
    fn definition(&mut self) -> Res<(Ident, Vec<ParamDef>, Vec<Stmt>, Span)> {
        let keyword = self.bump();
        let start = keyword.span;
        let what = if keyword.tok == Tok::Ident("instr".into()) { "an instrument name" } else { "an opcode name" };
        let name = self.ident(what)?;
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Tok::RParen {
            let pname = self.ident("a parameter name")?;
            let ty = self.eat(&Tok::Colon).then(|| self.type_spec()).transpose()?;
            let default = self.eat(&Tok::Eq).then(|| self.expr()).transpose()?;
            params.push(ParamDef { name: pname, ty, default });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        let (body, end) = self.block()?;
        Ok((name, params, body, start.to(end)))
    }

    /// `{ statements }`, returning the statements and the span of the braces.
    fn block(&mut self) -> Res<(Vec<Stmt>, Span)> {
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

    fn type_spec(&mut self) -> Res<TypeSpec> {
        if let Tok::Num { value: lo, unit: None } = self.peek().clone() {
            let start = self.bump().span;
            self.expect(&Tok::DotDot)?;
            match self.peek().clone() {
                Tok::Num { value: hi, unit: None } => {
                    let end = self.bump().span;
                    Ok(TypeSpec::Range { lo, hi, span: start.to(end) })
                }
                _ => Err(self.unexpected("the upper bound of the range")),
            }
        } else {
            Ok(TypeSpec::Unit(self.ident("a unit such as `hz`, or a range such as `0..1`")?))
        }
    }

    /// Is the next thing `[name, name] =`?
    fn at_destructure(&self) -> bool {
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

    fn stmt(&mut self) -> Res<Stmt> {
        if self.at_destructure() {
            self.bump();
            let mut names = vec![self.ident("a name")?];
            while self.eat(&Tok::Comma) {
                names.push(self.ident("a name")?);
            }
            self.expect(&Tok::RBracket)?;
            self.expect(&Tok::Eq)?;
            return Ok(Stmt::Destructure { names, value: self.expr()? });
        }
        if self.is_keyword("state") && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.bump();
            let name = self.ident("a name for the state")?;
            self.expect(&Tok::Eq)?;
            return Ok(Stmt::State { name, init: self.expr()? });
        }
        if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Plus && self.peek_at(2) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            self.bump();
            return Ok(Stmt::AddAssign { name, value: self.expr()? });
        }
        if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            Ok(Stmt::Bind { name, value: self.expr()? })
        } else {
            Ok(Stmt::Expr(self.expr()?))
        }
    }

    /// `name(args) for <duration>`, after any `at <position>`.
    fn note(&mut self, at: Option<AtPos>) -> Res<NoteStmt> {
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
        Ok(NoteStmt { at, target, args, dur, span })
    }

    /// `{ key: value, key: value }`, separated by commas or lines.
    fn entries(&mut self) -> Res<(Vec<(Ident, Expr)>, Span)> {
        self.expect(&Tok::LBrace)?;
        let mut entries = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            let key = self.ident("a name")?;
            self.expect(&Tok::Colon)?;
            entries.push((key, self.expr()?));
            if !self.eat(&Tok::Comma) && !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the entry"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((entries, end))
    }

    /// A time, or `bar 5 beat 3`.
    fn at_pos(&mut self) -> Res<AtPos> {
        if !self.is_keyword("bar") {
            return Ok(AtPos::Time(self.expr()?));
        }
        self.bump();
        let number = |p: &mut Parser| -> Res<Expr> {
            let span = p.span();
            match p.peek().clone() {
                Tok::Num { value, unit: None } => {
                    p.bump();
                    Ok(Expr { kind: ExprKind::Num { value, unit: None }, span })
                }
                _ => Err(p.unexpected("a number")),
            }
        };
        let span = self.span();
        match self.peek().clone() {
            // `bar 5 beat 3` lexes as `bar`, `5 beat`, `3`.
            Tok::Num { value, unit: Some(u) } if u == "beat" || u == "beats" => {
                self.bump();
                let bar = Expr { kind: ExprKind::Num { value, unit: None }, span };
                Ok(AtPos::Bar { bar, beat: Some(number(self)?) })
            }
            Tok::Num { value, unit: None } => {
                self.bump();
                let bar = Expr { kind: ExprKind::Num { value, unit: None }, span };
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

    fn at_command(&self) -> bool {
        ["play", "stop", "mute", "unmute", "solo", "unsolo", "hush", "panic", "launch"]
            .iter()
            .any(|kw| self.is_keyword(kw))
    }

    fn command(&mut self, at: Option<AtPos>) -> Res<CommandStmt> {
        let start = self.span();
        let Tok::Ident(keyword) = self.bump().tok else { unreachable!("checked by at_command") };
        let command = match keyword.as_str() {
            "play" => {
                let track = self.ident("a track to play on")?;
                self.expect(&Tok::Eq)?;
                Command::Play { track, what: self.expr()? }
            }
            "stop" => Command::Stop(self.ident("a track to stop")?),
            "mute" => Command::Mute(self.ident("a track to mute")?),
            "unmute" => Command::Unmute(self.ident("a track to unmute")?),
            "solo" => Command::Solo(self.ident("a track to solo")?),
            "unsolo" => Command::Unsolo(if matches!(self.peek(), Tok::Ident(_)) && !self.is_keyword("at") {
                Some(self.ident("a track")?)
            } else {
                None
            }),
            "hush" => Command::Hush,
            "panic" => Command::Panic,
            "launch" => Command::Launch(self.ident("a scene to launch")?),
            _ => unreachable!("checked by at_command"),
        };
        let quantize = self.quantize()?;
        Ok(CommandStmt { at, command, quantize, span: start.to(self.prev_span()) })
    }

    /// `@ next 4 bars`, `@ in 2 beats`, `@ bar`, `@ now`: when a change lands.
    fn quantize(&mut self) -> Res<Option<QuantizeSpec>> {
        if self.peek() != &Tok::At {
            return Ok(None);
        }
        let start = self.bump().span;
        let relation = if self.is_keyword("next") {
            self.bump();
            Relation::Next
        } else if self.is_keyword("in") {
            self.bump();
            Relation::In
        } else {
            Relation::OnGrid
        };
        let (count, unit) = match self.peek().clone() {
            Tok::Num { value, unit: Some(u) } => {
                let unit = match u.as_str() {
                    "beat" | "beats" => QuantUnit::Beat,
                    "bar" | "bars" => QuantUnit::Bar,
                    _ => return Err(self.unexpected("`beats` or `bars`")),
                };
                self.bump();
                (value, unit)
            }
            Tok::Ident(word) => {
                let unit = match word.as_str() {
                    "now" => QuantUnit::Now,
                    "beat" => QuantUnit::Beat,
                    "bar" => QuantUnit::Bar,
                    "cycle" => QuantUnit::Cycle,
                    _ => return Err(self.unexpected("`now`, `beat`, `bar`, `cycle`, or a count such as `4 bars`")),
                };
                self.bump();
                (1.0, unit)
            }
            _ => return Err(self.unexpected("when it should land, such as `bar` or `4 bars`")),
        };
        Ok(Some(QuantizeSpec { relation, count, unit, span: start.to(self.prev_span()) }))
    }

    // ---- expressions -------------------------------------------------

    fn expr(&mut self) -> Res<Expr> {
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

    fn term(&mut self) -> Res<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.unary()?;
            lhs = binary(op, lhs, rhs);
        }
    }

    fn unary(&mut self) -> Res<Expr> {
        // a leading `+`, as in `+5st`, changes nothing
        if self.peek() == &Tok::Plus {
            self.bump();
            return self.unary();
        }
        if self.peek() == &Tok::Minus {
            let start = self.bump().span;
            let inner = self.unary()?;
            let span = start.to(inner.span);
            return Ok(Expr { kind: ExprKind::Neg(Box::new(inner)), span });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Res<Expr> {
        let mut e = self.primary()?;
        while self.peek() == &Tok::Dot {
            self.bump();
            let name = self.ident("a method name after `.`")?;
            let mut args = vec![Arg { name: None, value: e.clone(), receiver: true }];
            if self.peek() == &Tok::LParen {
                args.extend(self.args()?);
            }
            let span = e.span.to(self.prev_span());
            e = Expr { kind: ExprKind::Call { name, args }, span };
        }
        Ok(e)
    }

    fn primary(&mut self) -> Res<Expr> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Num { value, unit } => {
                self.bump();
                Ok(Expr { kind: ExprKind::Num { value, unit }, span })
            }
            Tok::Pattern(raw) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Pattern(raw), span })
            }
            Tok::Grid(raw) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Grid(raw), span })
            }
            Tok::Tilde => {
                self.bump();
                Ok(Expr { kind: ExprKind::Rest, span })
            }
            Tok::LBracket => {
                self.bump();
                let mut items = vec![self.expr()?];
                while self.eat(&Tok::Comma) {
                    items.push(self.expr()?);
                }
                let end = self.expect(&Tok::RBracket)?;
                Ok(Expr { kind: ExprKind::Channels(items), span: span.to(end) })
            }
            Tok::LParen => {
                self.bump();
                let inner = self.expr()?;
                let end = self.expect(&Tok::RParen)?;
                Ok(Expr { span: span.to(end), ..inner })
            }
            Tok::Ident(name) if name == "env" && self.peek_at(1) == &Tok::LBracket => self.env(),
            Tok::Ident(name) => {
                let ident = self.ident("a name")?;
                if self.peek() == &Tok::LParen {
                    let args = self.args()?;
                    let span = span.to(self.prev_span());
                    Ok(Expr { kind: ExprKind::Call { name: ident, args }, span })
                } else {
                    Ok(Expr { kind: ExprKind::Name(name), span })
                }
            }
            _ => Err(self.unexpected("a value")),
        }
    }

    fn args(&mut self) -> Res<Vec<Arg>> {
        self.expect(&Tok::LParen)?;
        let mut args = Vec::new();
        while self.peek() != &Tok::RParen {
            let name = if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Colon {
                let n = self.ident("an argument name")?;
                self.bump();
                Some(n)
            } else {
                None
            };
            args.push(Arg { name, value: self.expr()?, receiver: false });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(args)
    }

    // ---- env literal -------------------------------------------------

    fn env(&mut self) -> Res<Expr> {
        let start = self.bump().span; // `env`
        self.expect(&Tok::LBracket)?;

        let start_level = self.env_level()?;
        let mut segments = Vec::new();
        let mut sustain_at = None;

        loop {
            if self.peek() == &Tok::Bar {
                let bar = self.bump().span;
                if sustain_at.is_some() {
                    return Err(Diagnostic::new("an envelope can have only one sustain point `|`", bar));
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
        let end = self.expect(&Tok::RBracket)?;

        Ok(Expr {
            kind: ExprKind::Env(EnvLit { start: start_level, segments, sustain_at }),
            span: start.to(end),
        })
    }

    /// A signed number, with an optional unit.
    fn env_number(&mut self) -> Res<EnvNum> {
        let start = self.span();
        let negative = self.peek() == &Tok::Minus;
        if negative {
            self.bump();
        }
        match self.peek().clone() {
            Tok::Num { value, unit } => {
                let end = self.bump().span;
                Ok(EnvNum { value: if negative { -value } else { value }, unit, span: start.to(end) })
            }
            _ => Err(self.unexpected("a number")),
        }
    }

    fn env_level(&mut self) -> Res<EnvNum> {
        let n = self.env_number().map_err(|_| self.env_expected("a level, such as `0` or `-6db`"))?;
        if n.unit.as_deref().is_some_and(is_time_unit) {
            return Err(Diagnostic::new("expected a level here, found a duration", n.span)
                .with_help("levels and durations alternate: two durations in a row is an error"));
        }
        Ok(n)
    }

    fn env_duration(&mut self) -> Res<(EnvNum, Option<CurveKind>)> {
        let n = self.env_number().map_err(|_| self.env_expected("a duration, such as `5ms`"))?;
        if !n.unit.as_deref().is_some_and(is_time_unit) {
            let help = match &n.unit {
                None => "levels and durations alternate: two levels in a row is an error; durations need a time unit such as `ms`, `sec` or `beats`".to_string(),
                Some(u) => format!("`{u}` is not a time unit; durations use `ms`, `sec` or `beats`"),
            };
            return Err(Diagnostic::new("expected a duration here", n.span).with_help(help));
        }
        let curve = if self.peek() == &Tok::Dot { Some(self.curve()?) } else { None };
        Ok((n, curve))
    }

    fn env_expected(&self, what: &str) -> Diagnostic {
        self.unexpected(what)
    }

    fn curve(&mut self) -> Res<CurveKind> {
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
            other => Err(Diagnostic::new(format!("unknown curve `{other}`"), name.span)
                .with_help("curves are `exp`, `log` and `curve(k)`")),
        }
    }
}

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Vec<Item> {
        parse(src).unwrap_or_else(|e| panic!("{}", e.render("t", src)))
    }

    fn err(src: &str) -> String {
        parse(src).unwrap_err().message
    }

    fn only_instr(src: &str) -> InstrDef {
        match parse_ok(src).remove(0) {
            Item::Instr(i) => i,
            other => panic!("{other:?}"),
        }
    }

    const SAW_LEAD: &str = "
instr saw_lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq)
    .lpf(cutoff: 2khz, res: 0.2)
    .gain(amp) * level
}

saw_lead(freq: a3) for 1beat
";

    #[test]
    fn parses_the_basic_synth() {
        let items = parse_ok(SAW_LEAD);
        assert_eq!(items.len(), 2);
        let Item::Instr(i) = &items[0] else { panic!() };
        assert_eq!(i.name.name, "saw_lead");
        assert_eq!(i.params.len(), 2);
        assert!(i.params[0].default.is_none());
        assert!(i.params[0].ty.is_some());
        assert!(i.params[1].default.is_some());
        assert_eq!(i.body.len(), 2);
        let Item::Note(n) = &items[1] else { panic!() };
        assert_eq!(n.target.name, "saw_lead");
        assert_eq!(n.args[0].name.as_ref().unwrap().name, "freq");
    }

    #[test]
    fn ufcs_desugars_to_calls_with_a_receiver() {
        let i = only_instr("instr a(f: hz) { osc(saw, f).lpf(cutoff: 1khz) }");
        let Stmt::Expr(e) = &i.body[0] else { panic!() };
        let ExprKind::Call { name, args } = &e.kind else { panic!() };
        assert_eq!(name.name, "lpf");
        assert!(args[0].receiver);
        assert!(matches!(&args[0].value.kind, ExprKind::Call { name, .. } if name.name == "osc"));
        assert_eq!(args[1].name.as_ref().unwrap().name, "cutoff");
    }

    #[test]
    fn paren_less_method_calls_work() {
        let i = only_instr("instr a(f: hz) { f.neg }");
        let Stmt::Expr(e) = &i.body[0] else { panic!() };
        let ExprKind::Call { args, .. } = &e.kind else { panic!() };
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn operator_precedence() {
        let i = only_instr("instr a(f: hz) { 1 + 2 * 3 - 4 }");
        let Stmt::Expr(e) = &i.body[0] else { panic!() };
        // (1 + (2*3)) - 4
        let ExprKind::Binary { op: BinOp::Sub, lhs, .. } = &e.kind else { panic!("{e:?}") };
        let ExprKind::Binary { op: BinOp::Add, rhs, .. } = &lhs.kind else { panic!() };
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn range_typed_parameters() {
        let i = only_instr("instr a(bright: 0..1 = 0.5) { bright }");
        assert!(matches!(i.params[0].ty, Some(TypeSpec::Range { lo, hi, .. }) if lo == 0.0 && hi == 1.0));
    }

    #[test]
    fn opcode_definitions_with_state_and_untyped_parameters() {
        let items = parse_ok("
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}");
        let Item::Opcode(o) = &items[0] else { panic!() };
        assert_eq!(o.name.name, "one_pole");
        assert!(o.params[0].ty.is_none());
        assert!(o.params[1].ty.is_some());
        assert!(matches!(o.body[0], Stmt::State { .. }));
        assert_eq!(o.body.len(), 4);
        assert_eq!(err("opcode (x) {}"), "expected an opcode name, found `(`");
    }

    fn env_of(src: &str) -> EnvLit {
        let i = only_instr(&format!("instr a() {{ {src} }}"));
        match &i.body[0] {
            Stmt::Expr(Expr { kind: ExprKind::Env(e), .. }) => e.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn env_literal_with_sustain_and_curves() {
        let e = env_of("env[0 5ms 1 120ms.exp 0.4 | 600ms.curve(-4) 0]");
        assert_eq!(e.segments.len(), 3);
        assert_eq!(e.sustain_at, Some(2));
        assert_eq!(e.segments[1].curve, Some(CurveKind::Exp));
        assert_eq!(e.segments[2].curve, Some(CurveKind::Custom(-4.0)));
        assert_eq!(e.segments[2].level.value, 0.0);
    }

    #[test]
    fn env_without_sustain_and_with_units_and_fractions() {
        let e = env_of("env[200hz 10ms 6khz 1/8beat.log 900hz]");
        assert_eq!(e.sustain_at, None);
        assert_eq!(e.segments[1].dur.value, 0.125);
        assert_eq!(e.start.unit.as_deref(), Some("hz"));
        let e = env_of("env[-18db 2sec -6db]");
        assert_eq!(e.start.value, -18.0);
    }

    #[test]
    fn env_alternation_errors() {
        assert_eq!(err("instr a() { env[0 1 1] }"), "expected a duration here");
        assert_eq!(err("instr a() { env[0 5ms 10ms 1] }"), "expected a level here, found a duration");
        assert!(err("instr a() { env[0 5ms 1 | 5ms 0 | 5ms 0] }").contains("only one sustain"));
        assert!(err("instr a() { env[0 5ms] }").starts_with("expected a level"));
        assert_eq!(err("instr a() { env[0 5ms.wobble 1] }"), "unknown curve `wobble`");
    }

    #[test]
    fn note_statements() {
        let items = parse_ok("at 2beats lead(freq: c4, amp: -3db) for 1/2beat");
        let Item::Note(n) = &items[0] else { panic!() };
        assert!(matches!(n.at, Some(AtPos::Time(_))));
        assert_eq!(n.args.len(), 2);
    }

    #[test]
    fn buses_tracks_and_add_assign() {
        let items = parse_ok("
bus space
bus hall: stereo
track lead {
  instrument = pluck(bright: 0.6)
  out = it.lpf(cutoff: 1khz)
  space += out.gain(-12db)
}");
        let Item::Bus(b) = &items[0] else { panic!() };
        assert_eq!((b.name.name.as_str(), b.layout.is_none()), ("space", true));
        let Item::Bus(b) = &items[1] else { panic!() };
        assert!(matches!(&b.layout.as_ref().unwrap().kind, ExprKind::Name(n) if n == "stereo"));
        let Item::Track(t) = &items[2] else { panic!() };
        assert_eq!(t.name.name, "lead");
        assert_eq!(t.body.len(), 3);
        assert!(matches!(&t.body[2], Stmt::AddAssign { name, .. } if name.name == "space"));
    }

    #[test]
    fn channel_lists_destructuring_and_config() {
        let items = parse_ok("
config { channels: surround(5.1) }
instr a(f: hz) {
  [l, r] = osc(saw, f).pan(azimuth: 20deg)
  mid = (l + r) * 0.5
  [mid + l, mid - r]
}");
        let Item::Config(c) = &items[0] else { panic!() };
        assert_eq!(c.entries[0].0.name, "channels");
        assert!(matches!(&c.entries[0].1.kind, ExprKind::Call { name, .. } if name.name == "surround"));
        let Item::Instr(i) = &items[1] else { panic!() };
        assert!(matches!(&i.body[0], Stmt::Destructure { names, .. } if names.len() == 2));
        let Stmt::Expr(e) = &i.body[2] else { panic!() };
        assert!(matches!(&e.kind, ExprKind::Channels(items) if items.len() == 2));
        // a bracketed expression that is not followed by `=` is a value, not a pattern
        assert!(matches!(parse_ok("config { a: 1, b: 2 }")[0], Item::Config(_)));
    }

    #[test]
    fn tempo_statement() {
        assert!(matches!(parse_ok("tempo 120bpm")[0], Item::Tempo(_)));
    }

    #[test]
    fn syntax_errors_say_what_was_expected() {
        assert_eq!(err("instr (a) {}"), "expected an instrument name, found `(`");
        assert_eq!(err("lead(freq: c4)"), "expected `for` and a duration, such as `for 1beat`, found the end of the file");
        assert_eq!(err("instr a() { 1 + }"), "expected a value, found `}`");
        assert_eq!(err("instr a() { f(1 2) }"), "expected `)`, found a number");
    }
}
