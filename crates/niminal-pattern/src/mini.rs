//! Mini-notation: `c2 ~ [e2 g2] <a b>*2` and the grids used for drums.
//!
//! Steps divide a cycle evenly. `~` is a rest, a lone `.` holds the previous
//! step for another step's worth of time, `[a b]` squeezes a sequence into one
//! step, a comma plays sequences at once (chords, parallel parts), `<a b>`
//! plays one alternative per cycle, and `x*2` plays a step twice as fast.

use std::fmt;

use crate::{Pattern, Rational};

#[derive(Debug, Clone, PartialEq)]
pub struct MiniError {
    pub message: String,
    /// Byte offset into the source where the problem is.
    pub offset: usize,
}

impl fmt::Display for MiniError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at {})", self.message, self.offset)
    }
}

impl std::error::Error for MiniError {}

type Res<T> = Result<T, MiniError>;

/// Parse mini-notation into a pattern of atoms. What an atom means (a note, a
/// number with a unit) is up to the caller.
pub fn parse(src: &str) -> Res<Pattern<String>> {
    let mut p = Parser { src, chars: src.char_indices().collect(), i: 0 };
    let pattern = p.stack(None)?;
    p.skip_space();
    match p.peek() {
        None => Ok(pattern),
        Some(c) => Err(p.error(format!("unexpected `{c}`"))),
    }
}

struct Parser<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).map(|(_, c)| *c)
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.i + n).map(|(_, c)| *c)
    }

    fn offset(&self) -> usize {
        self.chars.get(self.i).map_or(self.src.len(), |(o, _)| *o)
    }

    fn error(&self, message: impl Into<String>) -> MiniError {
        MiniError { message: message.into(), offset: self.offset() }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    fn expect(&mut self, c: char) -> Res<()> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected `{c}`")))
        }
    }

    /// Sequences separated by commas, played together, up to `close`.
    fn stack(&mut self, close: Option<char>) -> Res<Pattern<String>> {
        let mut layers = vec![self.sequence(close)?];
        while self.peek() == Some(',') {
            self.i += 1;
            layers.push(self.sequence(close)?);
        }
        Ok(if layers.len() == 1 { layers.remove(0) } else { Pattern::stack(layers) })
    }

    /// Steps, one after another, until a comma, `close` or the end.
    fn sequence(&mut self, close: Option<char>) -> Res<Pattern<String>> {
        let mut steps: Vec<(Rational, Pattern<String>)> = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                None | Some(',') => break,
                Some(c) if Some(c) == close => break,
                Some('~') if self.ends_token(1) => {
                    self.i += 1;
                    steps.push((Rational::ONE, Pattern::silence()));
                }
                Some('.') if self.ends_token(1) => {
                    let Some(last) = steps.last_mut() else {
                        return Err(self.error("a `.` holds the step before it, but there isn't one"));
                    };
                    last.0 = last.0 + Rational::ONE;
                    self.i += 1;
                }
                Some(_) => {
                    let term = self.term()?;
                    steps.push((Rational::ONE, term));
                }
            }
        }
        Ok(Pattern::timecat(steps))
    }

    /// Does the token starting `n` characters ago end here?
    fn ends_token(&self, n: usize) -> bool {
        self.peek_at(n).is_none_or(|c| c.is_whitespace() || matches!(c, ',' | ']' | '>'))
    }

    fn term(&mut self) -> Res<Pattern<String>> {
        let mut term = match self.peek() {
            Some('[') => {
                self.i += 1;
                let inner = self.stack(Some(']'))?;
                self.expect(']').map_err(|_| self.error("this `[` is never closed"))?;
                inner
            }
            Some('<') => {
                self.i += 1;
                self.alternatives()?
            }
            Some('(') => self.grouped_atom()?,
            Some(c @ (']' | '>')) => return Err(self.error(format!("unexpected `{c}`"))),
            _ => self.atom()?,
        };

        if self.peek() == Some('*') {
            self.i += 1;
            let start = self.offset();
            let mut text = String::new();
            while let Some(c) = self.peek().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '/')) {
                text.push(c);
                self.i += 1;
            }
            let factor = parse_factor(&text)
                .filter(|f| *f > Rational::ZERO)
                .ok_or(MiniError { message: format!("`*` needs a positive number, found `{text}`"), offset: start })?;
            term = term.fast(factor);
        }
        Ok(term)
    }

    /// `<a b c>`: one alternative per cycle.
    fn alternatives(&mut self) -> Res<Pattern<String>> {
        let mut options = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                Some('>') => {
                    self.i += 1;
                    break;
                }
                None => return Err(self.error("this `<` is never closed")),
                Some(',') => return Err(self.error("commas aren't supported inside `< >`")),
                Some('~') if self.ends_token(1) => {
                    self.i += 1;
                    options.push(Pattern::silence());
                }
                Some(_) => options.push(self.term()?),
            }
        }
        if options.is_empty() {
            return Err(self.error("`< >` needs at least one alternative"));
        }
        Ok(Pattern::slowcat(options))
    }

    fn atom(&mut self) -> Res<Pattern<String>> {
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '[' | ']' | '<' | '>' | ',' | '*') {
                break;
            }
            text.push(c);
            self.i += 1;
        }
        if text.is_empty() {
            return Err(self.error(format!("unexpected `{}`", self.peek().unwrap_or(' '))));
        }
        Ok(Pattern::pure(text))
    }

    /// `(1/8 beat)`: parentheses make one atom out of text with spaces in it.
    fn grouped_atom(&mut self) -> Res<Pattern<String>> {
        let open = self.offset();
        self.i += 1;
        let mut text = String::new();
        loop {
            match self.peek() {
                Some(')') => {
                    self.i += 1;
                    break;
                }
                Some(c) => {
                    if !c.is_whitespace() {
                        text.push(c);
                    }
                    self.i += 1;
                }
                None => return Err(MiniError { message: "this `(` is never closed".into(), offset: open }),
            }
        }
        if text.is_empty() {
            return Err(MiniError { message: "empty parentheses".into(), offset: open });
        }
        Ok(Pattern::pure(text))
    }
}

fn parse_factor(text: &str) -> Option<Rational> {
    match text.split_once('/') {
        Some((n, d)) => {
            let (n, d) = (n.parse::<i64>().ok()?, d.parse::<i64>().ok()?);
            (d != 0).then(|| Rational::new(n, d))
        }
        None if text.contains('.') => text.parse::<f64>().ok().map(Rational::from_f64),
        None => text.parse::<i64>().ok().map(Rational::int),
    }
}

/// How hard a grid step is hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Normal,
    Accent,
    Ghost,
}

/// A grid: each character is one step. `x` is a hit, `X` an accent, `o` a
/// ghost note and `.` a rest; spaces only group steps for reading.
pub fn grid(src: &str) -> Res<Pattern<Hit>> {
    let mut steps = Vec::new();
    for (offset, c) in src.char_indices() {
        let step = match c {
            c if c.is_whitespace() => continue,
            'x' => Pattern::pure(Hit::Normal),
            'X' => Pattern::pure(Hit::Accent),
            'o' => Pattern::pure(Hit::Ghost),
            '.' => Pattern::silence(),
            other => {
                return Err(MiniError {
                    message: format!("`{other}` isn't a grid step: use x, X, o or ."),
                    offset,
                });
            }
        };
        steps.push(step);
    }
    Ok(Pattern::fastcat(steps))
}
