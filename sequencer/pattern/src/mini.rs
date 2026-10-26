//! Mini-notation: `c2 ~ [e2 g2] <a b>*2` and the grids used for drums.
//!
//! Steps divide a cycle evenly. `~` is a rest, a lone `.` holds the previous
//! step for another step's worth of time, `[a b]` squeezes a sequence into one
//! step, a comma plays sequences at once (chords, parallel parts), `<a b>`
//! plays one alternative per cycle, `x*2` plays a step twice as fast and `x/2`
//! half as fast, `x!3` repeats it, `x@3` makes it three steps long, `x(3,8)`
//! spreads it Euclidean-style, `x?` drops it at random, `[a | b]` picks one
//! each cycle and `{a b c}%4` reads lists of different lengths at four steps.

use std::fmt;

use crate::{Param, Pattern, Rational};

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

/// How deeply brackets may nest.
const MAX_DEPTH: usize = 48;

use crate::MAX_REPEATS as MAX_REPEAT;

/// Parse mini-notation into a pattern of atom text. What an atom means (a note,
/// a number with a unit) is up to the caller.
pub fn parse(src: &str) -> Res<Pattern<String>> {
    parse_with(src, |atom, _| Ok(atom.to_string()))
}

/// Parse mini-notation, turning each atom into a value as it is read; `convert` is given the
/// atom and where it starts in `src`. An atom
/// that `convert` rejects is an error at that atom's position, so a typo is
/// reported when the pattern is written rather than when it is first played.
pub fn parse_with<T: Clone + Send + Sync + 'static>(
    src: &str,
    convert: impl Fn(&str, usize) -> Result<T, String>,
) -> Res<Pattern<T>> {
    let convert: &dyn Fn(&str, usize) -> Result<T, String> = &convert;
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut p = Parser {
        src,
        chars: &chars,
        i: 0,
        depth: 0,
        seed: 0,
        convert,
    };
    let pattern = p.stack(None)?;
    p.skip_space();
    match p.peek() {
        None => Ok(pattern),
        Some(c) => Err(p.error(format!("unexpected `{c}`"))),
    }
}

struct Parser<'a, T> {
    src: &'a str,
    chars: &'a [(usize, char)],
    i: usize,
    /// How many brackets deep the parser is.
    depth: usize,
    /// Counts the random choices (`?`, `|`) so each one chooses differently.
    seed: u64,
    convert: &'a dyn Fn(&str, usize) -> Result<T, String>,
}

/// Characters that end an atom.
const ATOM_END: [char; 15] = [
    '[', ']', '<', '>', '{', '}', '(', ')', ',', '*', '?', '!', '@', '|', '/',
];

impl<T: Clone + Send + Sync + 'static> Parser<'_, T> {
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
        MiniError {
            message: message.into(),
            offset: self.offset(),
        }
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

    fn next_seed(&mut self) -> u64 {
        self.seed += 1;
        0x4D49_4E49_0000 + self.seed
    }

    /// Sequences separated by commas (played together) or bars (one chosen at
    /// random each cycle), up to `close`.
    fn stack(&mut self, close: Option<char>) -> Res<Pattern<T>> {
        let first = self.sequence(close)?;
        match self.peek() {
            Some(',') => {
                let mut layers = vec![first];
                while self.peek() == Some(',') {
                    self.i += 1;
                    layers.push(self.sequence(close)?);
                }
                if self.peek() == Some('|') {
                    return Err(
                        self.error("`,` and `|` can't be mixed here; put one inside brackets")
                    );
                }
                Ok(Pattern::stack(layers))
            }
            Some('|') => {
                let mut options = vec![first];
                while self.peek() == Some('|') {
                    self.i += 1;
                    options.push(self.sequence(close)?);
                }
                if self.peek() == Some(',') {
                    return Err(
                        self.error("`,` and `|` can't be mixed here; put one inside brackets")
                    );
                }
                let seed = self.next_seed();
                Ok(Pattern::randcat(options, seed))
            }
            _ => Ok(first),
        }
    }

    /// Steps, one after another, until a comma, bar, `close` or the end.
    fn sequence(&mut self, close: Option<char>) -> Res<Pattern<T>> {
        Ok(Pattern::timecat(self.steps(close)?))
    }

    /// The steps of a sequence with the weight of each.
    fn steps(&mut self, close: Option<char>) -> Res<Vec<(Rational, Pattern<T>)>> {
        let mut steps: Vec<(Rational, Pattern<T>)> = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                None | Some(',' | '|') => break,
                Some(c) if Some(c) == close => break,
                Some('.') if self.ends_token(1) => {
                    let Some(last) = steps.last_mut() else {
                        return Err(
                            self.error("a `.` holds the step before it, but there isn't one")
                        );
                    };
                    last.0 = last.0 + Rational::ONE;
                    self.i += 1;
                }
                Some('!') if self.ends_token(1) => {
                    let Some(last) = steps.last().cloned() else {
                        return Err(
                            self.error("a `!` repeats the step before it, but there isn't one")
                        );
                    };
                    steps.push(last);
                    self.i += 1;
                }
                Some(_) => {
                    let term = self.term()?;
                    let mut weight = Rational::ONE;
                    let mut copies = 1;
                    loop {
                        match self.peek() {
                            Some('@') => {
                                self.i += 1;
                                let at = self.offset();
                                let text = self.literal();
                                weight = parse_factor(&text)
                                    .filter(|w| *w > Rational::ZERO)
                                    .ok_or(MiniError {
                                        message: format!(
                                            "`@` needs a number above 0, found `{text}`"
                                        ),
                                        offset: at,
                                    })?;
                            }
                            Some('!') => {
                                self.i += 1;
                                let at = self.offset();
                                let text = self.literal();
                                copies = if text.is_empty() {
                                    copies + 1
                                } else {
                                    text.parse::<i64>().ok().filter(|n| (1..=MAX_REPEAT).contains(n)).ok_or(MiniError {
                                        message: format!("`!` needs a whole number from 1 to {MAX_REPEAT}, found `{text}`"),
                                        offset: at,
                                    })?
                                };
                            }
                            _ => break,
                        }
                    }
                    for _ in 0..copies {
                        steps.push((weight, term.clone()));
                    }
                }
            }
        }
        Ok(steps)
    }

    /// Does the token starting `n` characters ago end here?
    fn ends_token(&self, n: usize) -> bool {
        self.peek_at(n)
            .is_none_or(|c| c.is_whitespace() || ATOM_END.contains(&c))
    }

    fn term(&mut self) -> Res<Pattern<T>> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error(format!("brackets are nested more than {MAX_DEPTH} deep")));
        }
        self.depth += 1;
        let result = self.term_inner();
        self.depth -= 1;
        result
    }

    fn term_inner(&mut self) -> Res<Pattern<T>> {
        let mut term = match self.peek() {
            Some('[') => {
                self.i += 1;
                let inner = self.stack(Some(']'))?;
                self.expect(']')
                    .map_err(|_| self.error("this `[` is never closed"))?;
                inner
            }
            Some('<') => {
                self.i += 1;
                self.alternatives()?
            }
            Some('{') => {
                self.i += 1;
                self.polymeter()?
            }
            Some('~') if self.ends_token(1) => {
                self.i += 1;
                Pattern::silence()
            }
            Some('(') => self.grouped_atom()?,
            Some(c @ (']' | '>' | '}' | ')')) => {
                return Err(self.error(format!("unexpected `{c}`")));
            }
            _ => self.atom()?,
        };

        loop {
            match self.peek() {
                Some(op @ ('*' | '/')) => {
                    self.i += 1;
                    let factor = self.number(op)?;
                    term = factor.apply(
                        &term,
                        move |p, f| if op == '*' { p.fast(f) } else { p.slow(f) },
                    );
                }
                Some('(') => {
                    self.i += 1;
                    let mut args = Vec::new();
                    loop {
                        self.skip_space();
                        args.push(self.number('(')?);
                        self.skip_space();
                        if self.peek() == Some(',') {
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                    self.expect(')')
                        .map_err(|_| self.error("this `(` is never closed"))?;
                    if !(2..=3).contains(&args.len()) {
                        return Err(self.error(
                            "a Euclidean rhythm is `(hits, steps)` or `(hits, steps, rotation)`",
                        ));
                    }
                    let rotation = args.get(2).cloned().unwrap_or(Param::Const(Rational::ZERO));
                    term =
                        term.rhythm(&Pattern::<bool>::euclid_with(&args[0], &args[1], &rotation));
                }
                Some('?') => {
                    self.i += 1;
                    let at = self.offset();
                    let text = self.literal();
                    let drop = if text.is_empty() {
                        0.5
                    } else {
                        parse_factor(&text)
                            .map(|r| r.to_f64())
                            .filter(|d| (0.0..=1.0).contains(d))
                            .ok_or(MiniError {
                                message: format!("`?` takes a chance from 0 to 1, found `{text}`"),
                                offset: at,
                            })?
                    };
                    let seed = self.next_seed();
                    term = term.thin(drop, seed);
                }
                _ => break,
            }
        }
        Ok(term)
    }

    /// The text of a number: digits, a point, a slash, a leading minus.
    fn literal(&mut self) -> String {
        let mut text = String::new();
        if self.peek() == Some('-') {
            text.push('-');
            self.i += 1;
        }
        while let Some(c) = self
            .peek()
            .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '/'))
        {
            text.push(c);
            self.i += 1;
        }
        text
    }

    /// A number after `op`: written out (`2`, `3/2`) or a pattern (`<2 4>`).
    fn number(&mut self, op: char) -> Res<Param<Rational>> {
        if matches!(self.peek(), Some('<' | '[')) {
            let convert =
                |a: &str, _: usize| parse_factor(a).ok_or_else(|| format!("`{a}` isn't a number"));
            let mut sub = Parser {
                src: self.src,
                chars: self.chars,
                i: self.i,
                depth: self.depth,
                seed: self.seed,
                convert: &convert,
            };
            let pattern = sub.term()?;
            (self.i, self.seed) = (sub.i, sub.seed);
            return Ok(Param::Pat(pattern));
        }
        let start = self.offset();
        let text = self.literal();
        let value = parse_factor(&text);
        match op {
            '*' | '/' => value
                .filter(|f| *f > Rational::ZERO && *f <= Rational::int(MAX_REPEAT))
                .map(Param::Const)
                .ok_or(MiniError {
                    message: format!(
                        "`{op}` needs a number from above 0 to {MAX_REPEAT}, found `{text}`"
                    ),
                    offset: start,
                }),
            _ => value.map(Param::Const).ok_or(MiniError {
                message: format!("expected a number, found `{text}`"),
                offset: start,
            }),
        }
    }

    /// `<a b c>`: one alternative per cycle; `<a b, c d e>` runs several at once.
    fn alternatives(&mut self) -> Res<Pattern<T>> {
        let mut layers = Vec::new();
        loop {
            let open = self.offset();
            let steps = self.steps(Some('>'))?;
            if steps.is_empty() {
                return Err(MiniError {
                    message: "`< >` needs at least one alternative".into(),
                    offset: open,
                });
            }
            let total = steps.iter().fold(Rational::ZERO, |t, (w, _)| t + *w);
            layers.push(if steps.iter().all(|(w, _)| *w == Rational::ONE) {
                Pattern::slowcat(steps.into_iter().map(|(_, p)| p).collect())
            } else {
                Pattern::timecat(steps).slow(total)
            });
            match self.peek() {
                Some(',') => self.i += 1,
                Some('>') => {
                    self.i += 1;
                    break;
                }
                Some('|') => return Err(self.error("`|` isn't supported inside `< >`")),
                _ => return Err(self.error("this `<` is never closed")),
            }
        }
        Ok(if layers.len() == 1 {
            layers.remove(0)
        } else {
            Pattern::stack(layers)
        })
    }

    /// `{a b c, d e}%4`: lists that each keep their own length, read at four
    /// steps a cycle (the first list's length if there is no `%`).
    fn polymeter(&mut self) -> Res<Pattern<T>> {
        let open = self.offset() - 1;
        let mut lists = Vec::new();
        loop {
            let steps = self.steps(Some('}'))?;
            let total = steps.iter().fold(Rational::ZERO, |t, (w, _)| t + *w);
            lists.push((total, Pattern::timecat(steps)));
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    break;
                }
                _ => {
                    return Err(MiniError {
                        message: "this `{` is never closed".into(),
                        offset: open,
                    });
                }
            }
        }
        let per_cycle = if self.peek() == Some('%') {
            self.i += 1;
            self.number('%')?
        } else if lists[0].0.is_zero() {
            return Err(MiniError {
                message: "`{ }` needs at least one step".into(),
                offset: open,
            });
        } else {
            Param::Const(lists[0].0)
        };
        let layers = lists
            .into_iter()
            .filter(|(total, _)| !total.is_zero())
            .map(|(total, p)| per_cycle.apply(&p, move |p, n| p.fast(n / total)))
            .collect();
        Ok(Pattern::stack(layers))
    }

    fn atom(&mut self) -> Res<Pattern<T>> {
        let start = self.offset();
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || ATOM_END.contains(&c) {
                // a fraction such as `1/8beat` is one atom; `bd/2` slows `bd`
                let number = text
                    .trim_start_matches(['+', '-'])
                    .starts_with(|d: char| d.is_ascii_digit() || d == '.');
                if c == '/' && number {
                    text.push(c);
                    self.i += 1;
                    continue;
                }
                break;
            }
            text.push(c);
            self.i += 1;
        }
        if text.is_empty() {
            return Err(self.error(format!("unexpected `{}`", self.peek().unwrap_or(' '))));
        }
        self.atom_value(text, start)
    }

    fn atom_value(&self, text: String, offset: usize) -> Res<Pattern<T>> {
        match (self.convert)(&text, offset) {
            Ok(value) => Ok(Pattern::pure(value)),
            Err(message) => Err(MiniError { message, offset }),
        }
    }

    /// `1/8beat`: parentheses make one atom out of text with spaces in it.
    fn grouped_atom(&mut self) -> Res<Pattern<T>> {
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
                None => {
                    return Err(MiniError {
                        message: "this `(` is never closed".into(),
                        offset: open,
                    });
                }
            }
        }
        if text.is_empty() {
            return Err(MiniError {
                message: "empty parentheses".into(),
                offset: open,
            });
        }
        self.atom_value(text, open)
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
pub enum Strength {
    Normal,
    Accent,
    Ghost,
}

/// One hit of a grid: how hard, and where it was written when the grid says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub strength: Strength,
    /// Where the step was written, as (evaluation, start, end), so an editor can light it as it plays.
    pub src: Option<(u64, usize, usize)>,
}

impl Hit {
    pub fn new(strength: Strength) -> Hit {
        Hit {
            strength,
            src: None,
        }
    }
}

/// A grid: each character is one step. `x` is a hit, `X` an accent, `o` a
/// ghost note and `.` a rest; spaces only group steps for reading.
pub fn grid(src: &str) -> Res<Pattern<Hit>> {
    grid_at(src, &|_, _| None)
}

/// [`grid`], with each hit remembering where it is written: `place` turns its byte range in `src`
/// into the range the editor knows.
pub fn grid_at(
    src: &str,
    place: &dyn Fn(usize, usize) -> Option<(u64, usize, usize)>,
) -> Res<Pattern<Hit>> {
    let mut steps = Vec::new();
    for (offset, c) in src.char_indices() {
        let hit = |strength| {
            Pattern::pure(Hit {
                strength,
                src: place(offset, offset + 1),
            })
        };
        let step = match c {
            c if c.is_whitespace() => continue,
            'x' => hit(Strength::Normal),
            'X' => hit(Strength::Accent),
            'o' => hit(Strength::Ghost),
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
