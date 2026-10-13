use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Tok, Token, lex};
use crate::performance::{Quantize, QuantizeRelation, QuantizeUnit};

/// A name, its parameters, its `with(...)` options, its body, and where it all is.
type Definition = (Ident, Vec<ParamDef>, Vec<Arg>, Vec<Stmt>, Span);

pub fn parse(src: &str) -> Result<Vec<Item>, Diagnostic> {
    parse_spanned(src).map(|items| items.into_iter().map(|(item, _)| item).collect())
}

/// Like [`parse`], with the stretch of source each top-level item covers.
pub fn parse_spanned(src: &str) -> Result<Vec<(Item, Span)>, Diagnostic> {
    if src.len() > MAX_SOURCE {
        return Err(Diagnostic::new(
            format!(
                "this is {} KB of code; niminal takes at most {} KB at a time",
                src.len() / 1024,
                MAX_SOURCE / 1024
            ),
            Span::new(0, 0),
        ));
    }
    let tokens = lex(src)?;
    if tokens.len() > MAX_TOKENS {
        return Err(Diagnostic::new(
            format!(
                "this has {} tokens; niminal takes at most {MAX_TOKENS} at a time",
                tokens.len()
            ),
            tokens[MAX_TOKENS].span,
        ));
    }
    let mut p = Parser {
        tokens,
        pos: 0,
        depth: 0,
    };
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

/// The settings of a `config`, the `quantize` table within it, and where it ends.
type ConfigEntries = (Vec<(Ident, Expr)>, Vec<(Ident, Expr)>, Span);

type Res<T> = Result<T, Diagnostic>;

/// The most source and tokens one evaluation may hold, and how deeply
/// expressions may nest. Past these the parser and everything after it would
/// recurse far enough to overflow the stack.
const MAX_SOURCE: usize = 1 << 20;
const MAX_TOKENS: usize = 50_000;
const MAX_DEPTH: usize = 64;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
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
        Tok::DotDotLt => "`..<`".into(),
        Tok::Eq => "`=`".into(),
        Tok::Arrow => "`->`".into(),
        Tok::EqEq => "`==`".into(),
        Tok::NotEq => "`!=`".into(),
        Tok::Lt => "`<`".into(),
        Tok::Gt => "`>`".into(),
        Tok::Le => "`<=`".into(),
        Tok::Ge => "`>=`".into(),
        Tok::Plus => "`+`".into(),
        Tok::Minus => "`-`".into(),
        Tok::Star => "`*`".into(),
        Tok::Slash => "`/`".into(),
        Tok::Percent => "`%`".into(),
        Tok::Bar => "`|`".into(),
        Tok::Tilde => "`~`".into(),
        Tok::At => "`@`".into(),
        Tok::Pattern(_) => "a pattern".into(),
        Tok::Rows(_) => "a grid".into(),
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
        Diagnostic::new(
            format!("expected {expected}, found {}", describe(self.peek())),
            self.span(),
        )
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
            Tok::Ident(name) => Ok(Ident {
                name,
                span: self.bump().span,
            }),
            _ => Err(self.unexpected(what)),
        }
    }

    /// The rest of `ctl name = midi.cc(21).range(200hz..4khz).smooth(10ms)` or
    /// `ctl name = osc_in("/fx/width").range(0..1)`.
    fn input_control(&mut self, name: Ident) -> Res<Item> {
        let start = self.span();
        let source = if self.is_keyword("midi") {
            self.bump();
            self.expect(&Tok::Dot)?;
            let method = self.ident("`cc` or `learn`")?;
            match method.name.as_str() {
                "learn" => InputSource::MidiLearn,
                "cc" => {
                    self.expect(&Tok::LParen)?;
                    let cc = self.small_number("a CC number from 0 to 127", 127)?;
                    let channel = if self.eat(&Tok::Comma) {
                        Some(self.small_number("a channel from 1 to 16", 16)?)
                    } else {
                        None
                    };
                    if channel == Some(0) {
                        return Err(Diagnostic::new(
                            "MIDI channels count from 1",
                            start.to(self.prev_span()),
                        ));
                    }
                    self.expect(&Tok::RParen)?;
                    InputSource::MidiCc { cc, channel }
                }
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "`midi.{other}` isn't supported; use `midi.cc(number)` or `midi.learn`"
                        ),
                        method.span,
                    ));
                }
            }
        } else {
            self.bump();
            self.expect(&Tok::LParen)?;
            let Tok::Str(address) = self.peek().clone() else {
                return Err(self.unexpected("an OSC address such as \"/fx/width\""));
            };
            self.bump();
            if !address.starts_with('/') {
                return Err(Diagnostic::new(
                    "an OSC address starts with `/`",
                    start.to(self.prev_span()),
                ));
            }
            self.expect(&Tok::RParen)?;
            InputSource::Osc(address)
        };
        let number = |value: f64, span| Expr {
            kind: ExprKind::Num { value, unit: None },
            span,
        };
        let (mut lo, mut hi, mut smooth) = (number(0.0, start), number(1.0, start), None);
        while self.eat(&Tok::Dot) {
            let method = self.ident("`range` or `smooth`")?;
            self.expect(&Tok::LParen)?;
            match method.name.as_str() {
                "range" => {
                    let range = self.expr()?;
                    let ExprKind::Range {
                        lo: l,
                        hi: h,
                        inclusive: true,
                    } = range.kind
                    else {
                        return Err(Diagnostic::new(
                            "`range` takes a range, such as `200hz..4khz`",
                            range.span,
                        ));
                    };
                    lo = *l;
                    hi = *h;
                }
                "smooth" => smooth = Some(self.expr()?),
                other => {
                    return Err(Diagnostic::new(
                        format!("a control input has no `.{other}`; use `.range` or `.smooth`"),
                        method.span,
                    ));
                }
            }
            self.expect(&Tok::RParen)?;
        }
        let value = match smooth {
            None => lo,
            Some(duration) => {
                let span = lo.span.to(duration.span);
                let args = vec![
                    Arg {
                        name: None,
                        value: lo,
                        receiver: true,
                    },
                    Arg {
                        name: None,
                        value: duration,
                        receiver: false,
                    },
                ];
                Expr {
                    kind: ExprKind::Call {
                        name: Ident {
                            name: "smooth".into(),
                            span,
                        },
                        args,
                    },
                    span,
                }
            }
        };
        Ok(Item::Control {
            name,
            value,
            input: Some(ControlInput { source, hi }),
        })
    }

    fn small_number(&mut self, what: &str, max: u8) -> Res<u8> {
        match self.peek().clone() {
            Tok::Num { value, unit: None }
                if value.fract() == 0.0 && (0.0..=f64::from(max)).contains(&value) =>
            {
                self.bump();
                Ok(value as u8)
            }
            _ => Err(self.unexpected(what)),
        }
    }

    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    // ---- items -------------------------------------------------------

    fn item(&mut self) -> Res<Item> {
        if self.is_keyword("ctl") {
            self.bump();
            let name = self.ident("a control name")?;
            self.expect(&Tok::Eq)?;
            if self.is_keyword("midi") || self.is_keyword("osc_in") {
                return self.input_control(name);
            }
            Ok(Item::Control {
                name,
                value: self.expr()?,
                input: None,
            })
        } else if self.is_keyword("instr") || self.is_keyword("mono") {
            let mono = self.is_keyword("mono");
            let (name, params, options, body, span) = self.definition()?;
            Ok(Item::Instr(InstrDef {
                name,
                params,
                body,
                span,
                mono,
                options,
            }))
        } else if self.is_keyword("bus") {
            self.bump();
            let name = self.ident("a bus name")?;
            let layout = self.eat(&Tok::Colon).then(|| self.expr()).transpose()?;
            let body = if self.peek() == &Tok::LBrace {
                Some(self.block()?.0)
            } else {
                None
            };
            Ok(Item::Bus(BusDecl { name, layout, body }))
        } else if self.is_keyword("midi_out") {
            let start = self.bump().span;
            let name = self.ident("a name for the MIDI output")?;
            let args = if self.peek() == &Tok::LParen {
                self.args()?
            } else {
                Vec::new()
            };
            Ok(Item::MidiOut(MidiOutDecl {
                name,
                args,
                span: start.to(self.prev_span()),
            }))
        } else if self.is_keyword("sample") || self.is_keyword("kit") {
            let is_kit = self.is_keyword("kit");
            let start = self.bump().span;
            let name = self.ident(if is_kit {
                "a kit name"
            } else {
                "a sample name"
            })?;
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
                        return Err(Diagnostic::new(
                            format!("a sample has no `{}`", method.name),
                            method.span,
                        )
                        .with_help("the only thing to make from a sample is `.slices(n)`"));
                    }
                    self.expect(&Tok::LParen)?;
                    let Tok::Num {
                        value: count,
                        unit: None,
                    } = self.peek().clone()
                    else {
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
            Ok(Item::Sample(SampleDecl {
                is_kit,
                name,
                source,
                source_span,
                options,
                span: start.to(self.prev_span()),
            }))
        } else if self.is_keyword("arrangement") {
            let start = self.bump().span;
            let name = self.ident("an arrangement name")?;
            self.arrangement(name, start).map(Item::Arrangement)
        } else if self.is_keyword("config") {
            let start = self.bump().span;
            let (entries, quantize, end) = self.config_entries()?;
            Ok(Item::Config(ConfigDecl {
                entries,
                quantize,
                span: start.to(end),
            }))
        } else if self.is_keyword("track") {
            let start = self.bump().span;
            let name = self.ident("a track name")?;
            // `track bass = pluck(bright: 0.4)` is `track bass { instrument = pluck(bright: 0.4) }`
            if self.eat(&Tok::Eq) {
                let value = self.expr()?;
                let end = self.prev_span();
                let key = Ident {
                    name: "instrument".into(),
                    span: start,
                };
                return Ok(Item::Track(TrackDecl {
                    name,
                    params: Vec::new(),
                    body: vec![Stmt::Bind { name: key, value }],
                    options: Vec::new(),
                    span: start.to(end),
                }));
            }
            let params = if self.peek() == &Tok::LParen {
                self.params()?
            } else {
                Vec::new()
            };
            if let Some(p) = params.iter().find(|p| p.default.is_none()) {
                return Err(Diagnostic::new(
                    format!("track parameter `{}` needs a starting value", p.name.name),
                    p.name.span,
                )
                .with_help("for example `track bass(tone: hz = 2khz)`"));
            }
            let (body, end) = self.block()?;
            let options = self.with_options()?;
            Ok(Item::Track(TrackDecl {
                name,
                params,
                body,
                options,
                span: start.to(end),
            }))
        } else if self.is_keyword("fn") {
            if !matches!(self.peek_at(1), Tok::Ident(_)) {
                self.bump();
                return Err(self.unexpected("a function name"));
            }
            self.function().map(Item::Fn)
        } else if self.is_keyword("opcode") {
            Err(Diagnostic::new("`opcode` is now `fn`", self.span())
                .with_help("write `fn name(x, ...) { ... }`; a function with `state` is a unit generator"))
        } else if self.is_keyword("const") {
            self.bump();
            let name = self.ident("a name for the constant")?;
            self.expect(&Tok::Eq)?;
            Ok(Item::Const {
                name,
                value: self.expr()?,
            })
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
            Ok(Item::Tempo(TempoStmt {
                value,
                quantize,
                over,
                span: start.to(self.prev_span()),
            }))
        } else if self.is_keyword("meter") {
            self.bump();
            Ok(Item::Meter(self.expr()?))
        } else if self.is_keyword("clip") || self.is_keyword("scene") {
            let is_clip = self.is_keyword("clip");
            let start = self.bump().span;
            let name = self.ident(if is_clip {
                "a clip name"
            } else {
                "a scene name"
            })?;
            let (entries, end) = self.entries(!is_clip)?;
            let options = self.with_options()?;
            let block = NamedBlock {
                name,
                entries,
                options,
                span: start.to(end),
            };
            Ok(if is_clip {
                Item::Clip(block)
            } else {
                Item::Scene(block)
            })
        } else if self.is_keyword("at") {
            self.bump();
            let at = self.at_pos()?;
            if self.at_command() {
                self.command(Some(at)).map(Item::Command)
            } else if self.is_edit() {
                self.edit(Some(at)).map(Item::Command)
            } else {
                self.note(Some(at)).map(Item::Note)
            }
        } else if self.at_command() {
            self.command(None).map(Item::Command)
        } else if self.is_edit() {
            self.edit(None).map(Item::Command)
        } else if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            Ok(Item::Bind {
                name,
                value: self.expr()?,
            })
        } else {
            self.note(None).map(Item::Note)
        }
    }

    /// The body of `arrangement name { sections: [scene.over(8bars) ...] }`.
    fn arrangement(&mut self, name: Ident, start: Span) -> Res<ArrangementDecl> {
        self.expect(&Tok::LBrace)?;
        let mut sections = None;
        let (mut tempos, mut meters) = (None, None);
        let mut tempo_env: Option<EnvLit> = None;
        let mut lanes: Vec<ArrangementLane> = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            let key = self.ident("`sections`")?;
            if self.peek() == &Tok::Dot {
                self.bump();
                let param = self.ident("a parameter name")?;
                self.expect(&Tok::Colon)?;
                let value = self.expr()?;
                let ExprKind::Env(lit) = value.kind else {
                    return Err(
                        Diagnostic::new("an arrangement lane is an envelope", value.span)
                            .with_help(format!(
                                "for example `{}.{}: env[0.2 8bars 1.0]`",
                                key.name, param.name
                            )),
                    );
                };
                if lanes
                    .iter()
                    .any(|l| l.track.name == key.name && l.param.name == param.name)
                {
                    return Err(Diagnostic::new(
                        format!("lane `{}.{}` is given twice", key.name, param.name),
                        key.span,
                    ));
                }
                lanes.push(ArrangementLane {
                    track: key,
                    param,
                    value: lit,
                });
                if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                    return Err(self.unexpected("the end of the line"));
                }
                continue;
            }
            if key.name == "tempo" || key.name == "meter" {
                self.expect(&Tok::Colon)?;
                let slot = if key.name == "tempo" {
                    &mut tempos
                } else {
                    &mut meters
                };
                if slot.is_some() || (key.name == "tempo" && tempo_env.is_some()) {
                    return Err(Diagnostic::new(
                        format!("`{}` is given twice", key.name),
                        key.span,
                    ));
                }
                if key.name == "tempo" && self.is_keyword("env") {
                    let value = self.expr()?;
                    let ExprKind::Env(lit) = value.kind else {
                        unreachable!("`env` starts an envelope")
                    };
                    tempo_env = Some(lit);
                    if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                        return Err(self.unexpected("the end of the line"));
                    }
                    continue;
                }
                self.expect(&Tok::LBracket)?;
                let mut list = Vec::new();
                while self.peek() != &Tok::RBracket {
                    list.push(self.expr()?);
                    self.eat(&Tok::Comma);
                }
                self.expect(&Tok::RBracket)?;
                *slot = Some(list);
                if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                    return Err(self.unexpected("the end of the line"));
                }
                continue;
            }
            if key.name != "sections" {
                return Err(Diagnostic::new(format!("`{}` isn't supported in an arrangement yet", key.name), key.span)
                    .with_help("an arrangement has `sections`, `tempo` and `meter` for each of them, and envelope lanes such as `bass.tone: env[...]`"));
            }
            self.expect(&Tok::Colon)?;
            if sections.is_some() {
                return Err(Diagnostic::new("`sections` is given twice", key.span));
            }
            self.expect(&Tok::LBracket)?;
            let mut list = Vec::new();
            while self.peek() != &Tok::RBracket {
                let scene = self.ident("a scene name")?;
                if !(self.eat(&Tok::Dot) && self.is_keyword("over")) {
                    return Err(Diagnostic::new(
                        format!("`{}` needs a length", scene.name),
                        scene.span,
                    )
                    .with_help(format!("for example `{}.over(8bars)`", scene.name)));
                }
                self.bump();
                self.expect(&Tok::LParen)?;
                let length = self.expr()?;
                self.expect(&Tok::RParen)?;
                list.push(SectionRef { scene, length });
                self.eat(&Tok::Comma);
            }
            self.expect(&Tok::RBracket)?;
            sections = Some(list);
            if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the line"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        let sections = sections.ok_or_else(|| {
            Diagnostic::new(
                format!("arrangement `{}` has no sections", name.name),
                name.span,
            )
            .with_help("add `sections: [scene.over(8bars) ...]`")
        })?;
        Ok(ArrangementDecl {
            name,
            sections,
            tempos,
            tempo_env,
            meters,
            lanes,
            span: start.to(end),
        })
    }

    /// `(a: hz = 1khz, b, size: const int = 4096)`
    fn params(&mut self) -> Res<Vec<ParamDef>> {
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Tok::RParen {
            let pname = self.ident("a parameter name")?;
            let (mut rate, mut ty) = (None, None);
            if self.eat(&Tok::Colon) {
                if let Tok::Ident(word) = self.peek().clone() {
                    let r = match word.as_str() {
                        "const" => Some(Rate::Const),
                        "init" => Some(Rate::Init),
                        "control" => Some(Rate::Control),
                        _ => None,
                    };
                    if let Some(r) = r {
                        rate = Some((r, self.bump().span));
                    }
                }
                if rate.is_none() || !matches!(self.peek(), Tok::Eq | Tok::Comma | Tok::RParen) {
                    ty = Some(self.type_spec()?);
                }
            }
            let default = self.eat(&Tok::Eq).then(|| self.expr()).transpose()?;
            params.push(ParamDef {
                name: pname,
                rate,
                ty,
                default,
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(params)
    }

    /// `instr name(params) with(options) { body }`
    fn definition(&mut self) -> Res<Definition> {
        let start = self.bump().span;
        let name = self.ident("an instrument name")?;
        let params = self.params()?;
        if self.peek() == &Tok::Arrow {
            return Err(Diagnostic::new("an instrument's result is a signal, so it has no `->`", self.span())
                .with_help("functions name their results with `->`"));
        }
        let options = self.with_options()?;
        let (body, end) = self.block()?;
        Ok((name, params, options, body, start.to(end)))
    }

    /// `fn name(params) -> result { body }`
    fn function(&mut self) -> Res<FnDef> {
        let start = self.bump().span; // `fn`
        let name = self.ident("a function name")?;
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
                result = FnResult::Unit(self.ident("a unit such as `hz`, or `(a, b)` for several results")?);
            }
        }
        if self.is_keyword("with") {
            return Err(Diagnostic::new("a function has no options", self.span()));
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
        let start = self.span();
        match self.peek().clone() {
            Tok::LBracket => {
                self.bump();
                let inner = self.type_spec()?;
                let end = self.expect(&Tok::RBracket)?;
                Ok(TypeSpec::List(Box::new(inner), start.to(end)))
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

    /// Is the next thing `name[...] =`? The position of the `]` if so.
    fn at_index_assign(&self) -> Option<usize> {
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
                        return (self.tokens.get(i + 1).map(|t| &t.tok) == Some(&Tok::Eq)).then_some(i);
                    }
                }
                Tok::Newline | Tok::Eof | Tok::LBrace | Tok::RBrace => return None,
                _ => {}
            }
            i += 1;
        }
        None
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
    fn entries(&mut self, bare: bool) -> Res<(Vec<(Ident, Expr)>, Span)> {
        self.expect(&Tok::LBrace)?;
        let mut entries = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            let key = self.ident("a name")?;
            if bare && matches!(self.peek(), Tok::Comma | Tok::Newline | Tok::RBrace) {
                let value = Expr {
                    kind: ExprKind::Name(key.name.clone()),
                    span: key.span,
                };
                entries.push((key, value));
            } else {
                self.expect(&Tok::Colon)?;
                entries.push((key, self.expr()?));
            }
            if !self.eat(&Tok::Comma) && !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the entry"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((entries, end))
    }

    /// `with(name: value, ...)` after a definition, if there is one.
    fn with_options(&mut self) -> Res<Vec<Arg>> {
        if self.is_keyword("with") {
            self.bump();
            self.args()
        } else {
            Ok(Vec::new())
        }
    }

    /// `{ channels: stereo  quantize: { default: 1bar  play: cycle } }`
    fn config_entries(&mut self) -> Res<ConfigEntries> {
        self.expect(&Tok::LBrace)?;
        let (mut entries, mut quantize) = (Vec::new(), Vec::new());
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
            } else {
                entries.push((key, self.expr()?));
            }
            if !self.eat(&Tok::Comma) && !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the entry"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((entries, quantize, end))
    }

    /// A time, or `bar 5 beat 3`.
    fn at_pos(&mut self) -> Res<AtPos> {
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

    fn is_edit(&self) -> bool {
        matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Dot
    }

    /// `bass.reverse`, `lead.transpose(+5st).thin(10%)`, or a command such as `bass.mute` or `verse.start`.
    fn edit(&mut self, at: Option<AtPos>) -> Res<CommandStmt> {
        let start = self.span();
        let track = self.ident("a track")?;
        self.expect(&Tok::Dot)?;
        let first = self.span();
        // `bass.mute`, `verse.start`...: the commands, written after the thing they act on
        if let Tok::Ident(verb) = self.peek().clone()
            && !matches!(self.peek_at(1), Tok::LParen | Tok::Dot)
        {
            let command = match verb.as_str() {
                "start" => Some(Command::Start {
                    name: track.clone(),
                    verb: first,
                }),
                "stop" => Some(Command::Stop(track.clone())),
                "mute" => Some(Command::Mute(track.clone())),
                "unmute" => Some(Command::Unmute(track.clone())),
                "solo" => Some(Command::Solo(track.clone())),
                "unsolo" => Some(Command::Unsolo(Some(track.clone()))),
                _ => None,
            };
            if let Some(command) = command {
                self.bump();
                let quantize = self.quantize()?;
                return Ok(CommandStmt {
                    at,
                    command,
                    quantize,
                    span: start.to(self.prev_span()),
                });
            }
        }
        // `lead.play(answer)`: put this on the part
        if let Tok::Ident(verb) = self.peek().clone()
            && verb == "play"
            && self.peek_at(1) == &Tok::LParen
        {
            self.bump();
            self.bump();
            let what = self.expr()?;
            self.expect(&Tok::RParen)?;
            let quantize = self.quantize()?;
            return Ok(CommandStmt {
                at,
                command: Command::Play { track, what },
                quantize,
                span: start.to(self.prev_span()),
            });
        }
        let change = self.expr()?;
        let quantize = self.quantize()?;
        Ok(CommandStmt {
            at,
            command: Command::Edit {
                track,
                change: first.to(change.span),
            },
            quantize,
            span: start.to(self.prev_span()),
        })
    }

    fn at_command(&self) -> bool {
        ["hush", "panic", "reset", "new", "unsolo"]
            .iter()
            .any(|kw| self.is_keyword(kw))
    }

    fn command(&mut self, at: Option<AtPos>) -> Res<CommandStmt> {
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

    /// `@ next 4bars`, `@ in 2beats`, `@ bar`, `@ now`: when a change lands.
    fn quantize(&mut self) -> Res<Option<Quantize>> {
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

    // ---- expressions -------------------------------------------------

    /// Run `f` one level deeper, refusing to go too deep.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
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

    fn expr(&mut self) -> Res<Expr> {
        self.nested(Self::or_expr)
    }

    fn or_expr(&mut self) -> Res<Expr> {
        let mut lhs = self.and_expr()?;
        while self.is_keyword("or") {
            self.bump();
            let rhs = self.and_expr()?;
            lhs = logic(LogicOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> Res<Expr> {
        let mut lhs = self.comparison()?;
        while self.is_keyword("and") {
            self.bump();
            let rhs = self.comparison()?;
            lhs = logic(LogicOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `a < b`. Comparisons don't chain: `a < b < c` is an error.
    fn comparison(&mut self) -> Res<Expr> {
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
    fn range(&mut self) -> Res<Expr> {
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
    fn else_follows(&self) -> bool {
        let mut i = 0;
        while self.peek_at(i) == &Tok::Newline {
            i += 1;
        }
        matches!(self.peek_at(i), Tok::Ident(k) if k == "else")
    }

    /// `if cond { ... } else { ... }` (or `else if ...`), after the `if`. The `else` is optional;
    /// without one the `if` can only be a statement.
    fn if_expr(&mut self) -> Res<Expr> {
        let start = self.bump().span; // `if`
        let cond = self.expr()?;
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

    /// `for i in 0..8 { ... }`, after seeing `for i in`.
    fn for_expr(&mut self) -> Res<Expr> {
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

    fn sum(&mut self) -> Res<Expr> {
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

    fn unary(&mut self) -> Res<Expr> {
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
        self.postfix()
    }

    fn postfix(&mut self) -> Res<Expr> {
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

    fn primary(&mut self) -> Res<Expr> {
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
            Tok::Tilde => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Rest,
                    span,
                })
            }
            Tok::LBracket => {
                self.bump();
                let mut items = vec![self.expr()?];
                while self.eat(&Tok::Comma) {
                    items.push(self.expr()?);
                }
                let end = self.expect(&Tok::RBracket)?;
                Ok(Expr {
                    kind: ExprKind::Channels(items),
                    span: span.to(end),
                })
            }
            Tok::LParen => {
                self.bump();
                let inner = self.expr()?;
                let end = self.expect(&Tok::RParen)?;
                Ok(Expr {
                    span: span.to(end),
                    ..inner
                })
            }
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
    fn eat_loop(&mut self, end: &mut Span) -> bool {
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
    fn env_number(&mut self) -> Res<EnvNum> {
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
    fn env_value(&mut self, what: &str) -> Res<EnvValue> {
        match self.peek() {
            Tok::Ident(_) => {
                let id = self.ident(what)?;
                Ok(EnvValue::Expr(Box::new(Expr {
                    kind: ExprKind::Name(id.name),
                    span: id.span,
                })))
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

    fn env_level(&mut self) -> Res<EnvValue> {
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

    fn env_duration(&mut self) -> Res<(EnvValue, Option<CurveKind>)> {
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
            other => Err(
                Diagnostic::new(format!("unknown curve `{other}`"), name.span)
                    .with_help("curves are `exp`, `log` and `curve(k)`"),
            ),
        }
    }
}

fn logic(op: LogicOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    Expr {
        kind: ExprKind::Logic {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    }
}

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    Expr {
        kind: ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    }
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
        let ExprKind::Call { name, args } = &e.kind else {
            panic!()
        };
        assert_eq!(name.name, "lpf");
        assert!(args[0].receiver);
        assert!(matches!(&args[0].value.kind, ExprKind::Call { name, .. } if name.name == "osc"));
        assert_eq!(args[1].name.as_ref().unwrap().name, "cutoff");
    }

    #[test]
    fn paren_less_method_calls_work() {
        let i = only_instr("instr a(f: hz) { f.neg }");
        let Stmt::Expr(e) = &i.body[0] else { panic!() };
        let ExprKind::Call { args, .. } = &e.kind else {
            panic!()
        };
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn operator_precedence() {
        let i = only_instr("instr a(f: hz) { 1 + 2 * 3 - 4 }");
        let Stmt::Expr(e) = &i.body[0] else { panic!() };
        // (1 + (2*3)) - 4
        let ExprKind::Binary {
            op: BinOp::Sub,
            lhs,
            ..
        } = &e.kind
        else {
            panic!("{e:?}")
        };
        let ExprKind::Binary {
            op: BinOp::Add,
            rhs,
            ..
        } = &lhs.kind
        else {
            panic!()
        };
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn range_typed_parameters() {
        let i = only_instr("instr a(bright: 0..1 = 0.5, cut: 20hz..20khz) { bright }");
        assert!(matches!(i.params[0].ty, Some(TypeSpec::Range { .. })));
        assert!(matches!(i.params[1].ty, Some(TypeSpec::Range { .. })));
    }

    #[test]
    fn functions_with_state_untyped_parameters_and_results() {
        let items = parse_ok(
            "
fn one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}",
        );
        let Item::Fn(f) = &items[0] else { panic!() };
        assert_eq!(f.name.name, "one_pole");
        assert!(f.params[0].ty.is_none());
        assert!(f.params[1].ty.is_some());
        assert!(matches!(f.body[0], Stmt::State { .. }));
        assert_eq!(f.body.len(), 4);
        assert_eq!(f.result, FnResult::Inferred);
        let Item::Fn(f) = &parse_ok("fn mtof(n: num) -> hz { 440hz }")[0] else { panic!() };
        assert!(matches!(&f.result, FnResult::Unit(u) if u.name == "hz"));
        let Item::Fn(f) = &parse_ok("fn s(x) -> (low, high) { low = x\n high = x }")[0] else { panic!() };
        assert!(matches!(&f.result, FnResult::Named(n) if n.len() == 2));
        assert_eq!(err("fn (x) {}"), "expected a function name, found `(`");
        assert_eq!(err("opcode o(x) { x }"), "`opcode` is now `fn`");
    }

    #[test]
    fn rates_choices_and_list_types() {
        let Item::Fn(f) = &parse_ok(
            "fn f(a: init hz, b: control = 1, size: const int = 8, mode: (low | high) = low, taps: [num]) { a }",
        )[0] else {
            panic!()
        };
        assert_eq!(f.params[0].rate.map(|r| r.0), Some(Rate::Init));
        assert!(matches!(&f.params[0].ty, Some(TypeSpec::Unit(u)) if u.name == "hz"));
        assert_eq!(f.params[1].rate.map(|r| r.0), Some(Rate::Control));
        assert!(f.params[1].ty.is_none());
        assert_eq!(f.params[2].rate.map(|r| r.0), Some(Rate::Const));
        assert!(matches!(&f.params[3].ty, Some(TypeSpec::Choice(n)) if n.len() == 2));
        assert!(matches!(&f.params[4].ty, Some(TypeSpec::List(..))));
    }

    #[test]
    fn ranges_logic_and_remainder() {
        let i = only_instr("instr a() { x = 0..<8\n y = 1..8\n z = a > 1 and not b or c\n z }");
        assert!(matches!(&i.body[0], Stmt::Bind { value: Expr { kind: ExprKind::Range { inclusive: false, .. }, .. }, .. }));
        assert!(matches!(&i.body[1], Stmt::Bind { value: Expr { kind: ExprKind::Range { inclusive: true, .. }, .. }, .. }));
        // `or` binds loosest
        assert!(matches!(&i.body[2], Stmt::Bind { value: Expr { kind: ExprKind::Logic { op: LogicOp::Or, .. }, .. }, .. }));
        assert_eq!(err("instr a() { 3 % 2 }"), "`%` only means percent");
    }

    #[test]
    fn blocks_in_if_and_for_and_index_writes() {
        let i = only_instr(
            "instr a() {
  if x > 0 {
    y = 1
    z = 2
  }
  for n in 1..8 { out += n }
  buf[i + 1] = 3
  v = if c { 1 } else if d { 2 } else { 3 }
  v
}",
        );
        assert!(matches!(&i.body[0], Stmt::Expr(Expr { kind: ExprKind::If { otherwise: None, then, .. }, .. }) if then.len() == 2));
        assert!(matches!(&i.body[1], Stmt::Expr(Expr { kind: ExprKind::For { .. }, .. })));
        assert!(matches!(&i.body[2], Stmt::IndexAssign { .. }));
        assert!(matches!(&i.body[3], Stmt::Bind { value: Expr { kind: ExprKind::If { otherwise: Some(o), .. }, .. }, .. } if o.len() == 1));
    }

    #[test]
    fn constants_are_lists_not_patterns() {
        let items = parse_ok("const taps = [0.25, 0.5, 0.25]");
        assert!(matches!(&items[0], Item::Const { value: Expr { kind: ExprKind::Channels(v), .. }, .. } if v.len() == 3));
    }

    fn env_of(src: &str) -> EnvLit {
        let i = only_instr(&format!("instr a() {{ {src} }}"));
        match &i.body[0] {
            Stmt::Expr(Expr {
                kind: ExprKind::Env(e),
                ..
            }) => e.clone(),
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
        assert_eq!(e.segments[2].level.lit().unwrap().value, 0.0);
    }

    #[test]
    fn env_without_sustain_and_with_units_and_fractions() {
        let e = env_of("env[200hz 10ms 6khz 1/8beat.log 900hz]");
        assert_eq!(e.sustain_at, None);
        assert_eq!(e.segments[1].dur.lit().unwrap().value, 0.125);
        assert_eq!(e.start.lit().unwrap().unit.as_deref(), Some("hz"));
        let e = env_of("env[-18db 2sec -6db]");
        assert_eq!(e.start.lit().unwrap().value, -18.0);
    }

    #[test]
    fn env_steps_from_names_and_expressions() {
        let e = env_of("env[0 attack 1 | (release * 2).exp 0]");
        assert!(matches!(&e.segments[0].dur, EnvValue::Expr(n) if matches!(n.kind, ExprKind::Name(_))));
        assert!(matches!(&e.segments[1].dur, EnvValue::Expr(n) if matches!(n.kind, ExprKind::Binary { .. })));
        assert_eq!(e.segments[1].curve, Some(CurveKind::Exp));
    }

    #[test]
    fn env_can_loop_but_not_with_a_sustain_point() {
        assert!(!env_of("env[0 5ms 1 5ms 0]").looped);
        let e = env_of("env[0 5ms 1 5ms 0].loop");
        assert!(e.looped);
        assert_eq!(e.segments.len(), 2);
        assert!(
            err("instr a() { env[0 5ms 1 | 5ms 0].loop }").contains("can't have a sustain point")
        );
    }

    #[test]
    fn env_alternation_errors() {
        assert_eq!(err("instr a() { env[0 1 1] }"), "expected a duration here");
        assert_eq!(
            err("instr a() { env[0 5ms 10ms 1] }"),
            "expected a level here, found a duration"
        );
        assert!(err("instr a() { env[0 5ms 1 | 5ms 0 | 5ms 0] }").contains("only one sustain"));
        assert!(err("instr a() { env[0 5ms] }").starts_with("expected a level"));
        assert_eq!(
            err("instr a() { env[0 5ms.wobble 1] }"),
            "unknown curve `wobble`"
        );
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
        let items = parse_ok(
            "
bus space
bus hall: stereo
track lead {
  instrument = pluck(bright: 0.6)
  out = it.lpf(cutoff: 1khz)
  space += out.gain(-12db)
}",
        );
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
        let items = parse_ok(
            "
config { channels: surround(5.1) }
instr a(f: hz) {
  [l, r] = osc(saw, f).pan(azimuth: 20deg)
  mid = (l + r) * 0.5
  [mid + l, mid - r]
}",
        );
        let Item::Config(c) = &items[0] else { panic!() };
        assert_eq!(c.entries[0].0.name, "channels");
        assert!(
            matches!(&c.entries[0].1.kind, ExprKind::Call { name, .. } if name.name == "surround")
        );
        let Item::Instr(i) = &items[1] else { panic!() };
        assert!(matches!(&i.body[0], Stmt::Destructure { names, .. } if names.len() == 2));
        let Stmt::Expr(e) = &i.body[2] else { panic!() };
        assert!(matches!(&e.kind, ExprKind::Channels(items) if items.len() == 2));
        // a bracketed expression that is not followed by `=` is a value, not a pattern
        assert!(matches!(
            parse_ok("config { a: 1, b: 2 }")[0],
            Item::Config(_)
        ));
    }

    #[test]
    fn tempo_statement() {
        assert!(matches!(parse_ok("tempo 120bpm")[0], Item::Tempo(_)));
    }

    #[test]
    fn syntax_errors_say_what_was_expected() {
        assert_eq!(
            err("instr (a) {}"),
            "expected an instrument name, found `(`"
        );
        assert_eq!(
            err("lead(freq: c4)"),
            "expected `for` and a duration, such as `for 1beat`, found the end of the file"
        );
        assert_eq!(err("instr a() { 1 + }"), "expected a value, found `}`");
        assert_eq!(err("instr a() { f(1 2) }"), "expected `)`, found a number");
    }
}
