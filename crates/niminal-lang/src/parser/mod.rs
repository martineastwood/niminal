mod env;
mod exprs;
mod helpers;
mod input;
mod items;
mod stmts;
#[cfg(test)]
mod tests;
pub(crate) use helpers::*;

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::lexer::{Tok, Token, lex, lex_dsp};

/// A name, its parameters, its settings, its body, and where it all is.
type Definition = (Ident, Vec<ParamDef>, Vec<Arg>, Vec<Stmt>, Span);

/// Sound-server definitions only: a composition's clips, scenes and commands are errors.
pub fn parse(src: &str) -> Result<Vec<Item>, Diagnostic> {
    parse_spanned(src).map(|items| items.into_iter().map(|(item, _)| item).collect())
}

/// Like [`parse`], with the stretch of source each top-level item covers.
pub fn parse_spanned(src: &str) -> Result<Vec<(Item, Span)>, Diagnostic> {
    parse_with(src, true)
}

/// A whole composition: definitions, clips, scenes, commands and notes.
pub fn parse_composition(src: &str) -> Result<Vec<Item>, Diagnostic> {
    parse_composition_spanned(src).map(|items| items.into_iter().map(|(item, _)| item).collect())
}

/// Like [`parse_composition`], with the stretch of source each top-level item covers.
pub fn parse_composition_spanned(src: &str) -> Result<Vec<(Item, Span)>, Diagnostic> {
    parse_with(src, false)
}

fn parse_with(src: &str, dsp: bool) -> Result<Vec<(Item, Span)>, Diagnostic> {
    let src = &*crate::version::check(src)?;
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
    let tokens = if dsp { lex_dsp(src)? } else { lex(src)? };
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
        dsp,
    };
    let mut items = Vec::new();
    loop {
        p.skip_newlines();
        if p.peek() == &Tok::Eof {
            return Ok(items);
        }
        let start = p.span();
        let item = p.item()?;
        check_defined_name(&item)?;
        items.push((item, start.to(p.prev_span())));
        if !matches!(p.peek(), Tok::Newline | Tok::Eof) {
            return Err(p.unexpected("end of statement"));
        }
    }
}

/// The settings of a `config`, the `quantize` table within it, and where it ends.
type ConfigEntries = (
    Vec<(Ident, Expr)>,
    Vec<(Ident, Expr)>,
    Vec<(Ident, Expr)>,
    Span,
);

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
    /// Parsing for the sound server, which takes definitions only.
    dsp: bool,
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
        Tok::Caret => "`^`".into(),
        Tok::Bar => "`|`".into(),
        Tok::Tilde => "`~`".into(),
        Tok::At => "`@`".into(),
        Tok::Pattern(_) => "a pattern".into(),
        Tok::Rows(_) => "a grid".into(),
        Tok::Grid(_) => "a grid".into(),
        Tok::Steps(_) => "a row of steps".into(),
        Tok::Events(_) => "an events block".into(),
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

    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }
}

/// The name an item defines, if it must not be a reserved word.
fn check_defined_name(item: &Item) -> Result<(), Diagnostic> {
    let name = match item {
        Item::Instr(d) => &d.name,
        Item::Control { name, .. } | Item::Const { name, .. } | Item::Bind { name, .. } => name,
        Item::Fn(d) | Item::Def(d) => &d.name,
        Item::Bus(d) => &d.name,
        Item::Buffer(d) => &d.name,
        Item::Node(d) | Item::Fx(d) => &d.name,
        Item::Sample(d) => &d.name,
        Item::MidiOut(d) => &d.name,
        Item::Arrangement(d) => &d.name,
        Item::Clip(d) | Item::Scene(d) | Item::Macro(d) => &d.name,
        Item::For { var, .. } => var,
        _ => return Ok(()),
    };
    if crate::names::is_reserved(&name.name) {
        return Err(Diagnostic::new(
            format!("`{}` is a reserved word", name.name),
            name.span,
        ));
    }
    Ok(())
}
