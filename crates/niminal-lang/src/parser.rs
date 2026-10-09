use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Tok, Token, lex};

pub fn parse(src: &str) -> Result<Vec<Item>, Diagnostic> {
    let mut p = Parser { tokens: lex(src)?, pos: 0 };
    let mut items = Vec::new();
    loop {
        p.skip_newlines();
        if p.peek() == &Tok::Eof {
            return Ok(items);
        }
        items.push(p.item()?);
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
            self.instr().map(Item::Instr)
        } else if self.is_keyword("tempo") {
            self.bump();
            Ok(Item::Tempo(self.expr()?))
        } else {
            self.note().map(Item::Note)
        }
    }

    fn instr(&mut self) -> Res<InstrDef> {
        let start = self.bump().span;
        let name = self.ident("an instrument name")?;
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Tok::RParen {
            let pname = self.ident("a parameter name")?;
            self.expect(&Tok::Colon)?;
            let ty = self.type_spec()?;
            let default = self.eat(&Tok::Eq).then(|| self.expr()).transpose()?;
            params.push(ParamDef { name: pname, ty, default });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::LBrace)?;

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
        Ok(InstrDef { name, params, body, span: start.to(end) })
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

    fn stmt(&mut self) -> Res<Stmt> {
        if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            Ok(Stmt::Bind { name, value: self.expr()? })
        } else {
            Ok(Stmt::Expr(self.expr()?))
        }
    }

    /// `[at <time>] name(args) for <duration>`
    fn note(&mut self) -> Res<NoteStmt> {
        let start = self.span();
        let at = self.is_keyword("at").then(|| {
            self.bump();
            self.expr()
        });
        let at = at.transpose()?;

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
        assert!(matches!(i.params[0].ty, TypeSpec::Range { lo, hi, .. } if lo == 0.0 && hi == 1.0));
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
        assert!(n.at.is_some());
        assert_eq!(n.args.len(), 2);
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
