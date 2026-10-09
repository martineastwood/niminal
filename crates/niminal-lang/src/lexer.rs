use crate::diag::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    /// A number with an optional unit suffix attached directly (`800hz`,
    /// `1/8beat`). Rationals only lex as one literal when a unit follows.
    Num { value: f64, unit: Option<String> },
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    DotDot,
    Eq,
    Plus,
    Minus,
    Star,
    Slash,
    Bar,
    Tilde,
    At,
    /// The raw text of a mini-notation pattern, between `[` and `]`. Outside
    /// instruments, opcodes and tracks a bracket is a pattern, not a channel list.
    Pattern(String),
    /// The raw text of `grid[...]`.
    Grid(String),
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn lex(src: &str) -> Result<Vec<Token>, Diagnostic> {
    let bytes = src.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    // Newlines separate statements, except inside ( ) and [ ].
    let mut brackets: Vec<u8> = Vec::new();
    // Whether each open brace is a signal context (an instrument, opcode or
    // track body), where a bracket is a channel list or envelope.
    let mut braces: Vec<bool> = Vec::new();
    let mut statement_start = true;
    let mut statement_keyword: Option<String> = None;
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        let mut start = i;
        let tok = match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'\n' => {
                i += 1;
                if matches!(brackets.last(), Some(b'(' | b'[')) {
                    continue;
                }
                Tok::Newline
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'0'..=b'9' => {
                let (mut tok, mut end) = lex_number(src, i);
                // `4 bars`: a space before a unit word is fine outside brackets,
                // where a space separates steps instead.
                if let Tok::Num { unit: unit @ None, .. } = &mut tok
                    && brackets.last() != Some(&b'[')
                    && let Some((word, after)) = spaced_unit(src, end)
                {
                    *unit = Some(word.to_string());
                    end = after;
                }
                i = end;
                tok
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let mut end = i;
                while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                    end += 1;
                }
                // Sharp note names such as `c#4`.
                if end - i == 1
                    && (b'a'..=b'g').contains(&c)
                    && bytes.get(end) == Some(&b'#')
                    && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
                {
                    end += 1;
                    while end < bytes.len() && bytes[end].is_ascii_digit() {
                        end += 1;
                    }
                }
                let tok = Tok::Ident(src[i..end].to_string());
                i = end;
                tok
            }
            b'[' if is_raw_bracket(&tokens, braces.last().copied().unwrap_or(false)) => {
                let is_grid = matches!(tokens.last(), Some(Token { tok: Tok::Ident(n), .. }) if n == "grid");
                let Some(close) = matching_bracket(bytes, i) else {
                    return Err(Diagnostic::new("this `[` is never closed", Span::new(i, i + 1)));
                };
                let text = src[i + 1..close].to_string();
                i = close + 1;
                if is_grid {
                    // the `grid` keyword is part of the token
                    start = tokens.pop().map_or(start, |t| t.span.start);
                    Tok::Grid(text)
                } else {
                    Tok::Pattern(text)
                }
            }
            b'.' => {
                if bytes.get(i + 1) == Some(&b'.') {
                    i += 2;
                    Tok::DotDot
                } else {
                    i += 1;
                    Tok::Dot
                }
            }
            _ => {
                i += 1;
                match c {
                    b'(' | b'[' | b'{' => {
                        brackets.push(c);
                        if c == b'{' {
                            let inherited = braces.last().copied().unwrap_or(false);
                            let signal = matches!(statement_keyword.as_deref(), Some("instr" | "opcode" | "track"));
                            braces.push(inherited || signal);
                        }
                        match c {
                            b'(' => Tok::LParen,
                            b'[' => Tok::LBracket,
                            _ => Tok::LBrace,
                        }
                    }
                    b')' | b']' | b'}' => {
                        brackets.pop();
                        if c == b'}' {
                            braces.pop();
                        }
                        match c {
                            b')' => Tok::RParen,
                            b']' => Tok::RBracket,
                            _ => Tok::RBrace,
                        }
                    }
                    b'~' => Tok::Tilde,
                    b'@' => Tok::At,
                    b',' => Tok::Comma,
                    b':' => Tok::Colon,
                    b'=' => Tok::Eq,
                    b'+' => Tok::Plus,
                    b'-' => Tok::Minus,
                    b'*' => Tok::Star,
                    b'/' => Tok::Slash,
                    b'|' => Tok::Bar,
                    _ => {
                        let ch = src[start..].chars().next().unwrap();
                        return Err(Diagnostic::new(
                            format!("unexpected character `{ch}`"),
                            Span::new(start, start + ch.len_utf8()),
                        ));
                    }
                }
            }
        };
        match &tok {
            Tok::Newline | Tok::LBrace | Tok::RBrace => {
                statement_start = true;
                statement_keyword = None;
            }
            Tok::Ident(name) if statement_start => {
                statement_keyword = Some(name.clone());
                statement_start = false;
            }
            _ => statement_start = false,
        }
        tokens.push(Token { tok, span: Span::new(start, i) });
    }

    tokens.push(Token { tok: Tok::Eof, span: Span::new(src.len(), src.len()) });
    Ok(tidy_newlines(tokens))
}

/// Collapse runs of newlines, and drop a newline that precedes a line starting
/// with `.`, which continues the previous expression.
fn tidy_newlines(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        if t.tok == Tok::Newline {
            if out.last().is_some_and(|p| p.tok == Tok::Newline) {
                continue;
            }
            let next = tokens[i + 1..].iter().find(|t| t.tok != Tok::Newline);
            if next.is_some_and(|n| n.tok == Tok::Dot) {
                continue;
            }
        }
        out.push(t.clone());
    }
    out
}

/// Is a `[` here a raw pattern (or grid) rather than ordinary tokens?
fn is_raw_bracket(tokens: &[Token], in_signal: bool) -> bool {
    match tokens.last() {
        Some(Token { tok: Tok::Ident(name), .. }) if name == "env" => false,
        Some(Token { tok: Tok::Ident(name), .. }) if name == "grid" => true,
        _ => !in_signal,
    }
}

/// The index of the `]` matching the `[` at `open`.
fn matching_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

const UNIT_WORDS: [&str; 12] = ["hz", "khz", "db", "sec", "ms", "beat", "beats", "bar", "bars", "bpm", "deg", "st"];

/// A unit word after some spaces (not a newline): its text and where it ends.
fn spaced_unit(src: &str, from: usize) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    let mut i = from;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i == from {
        return None; // no space: not this case
    }
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    let word = &src[start..i];
    let ends_word = bytes.get(i).is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
    (ends_word && UNIT_WORDS.contains(&word)).then_some((word, i))
}

fn lex_number(src: &str, start: usize) -> (Tok, usize) {
    let bytes = src.as_bytes();
    let digits = |mut i: usize| {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        i
    };

    let mut end = digits(start);
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end = digits(end + 1);
    }
    let mut value: f64 = src[start..end].parse().unwrap();

    // `1/8beat`: a rational is one literal only when a unit follows it.
    if bytes.get(end) == Some(&b'/') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        let den_end = digits(end + 1);
        if bytes.get(den_end).is_some_and(u8::is_ascii_alphabetic) {
            value /= src[end + 1..den_end].parse::<f64>().unwrap();
            end = den_end;
        }
    }

    let unit_start = end;
    if bytes.get(end) == Some(&b'%') {
        return (Tok::Num { value, unit: Some("%".into()) }, end + 1);
    }
    while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
        end += 1;
    }
    let unit = (end > unit_start).then(|| src[unit_start..end].to_string());
    (Tok::Num { value, unit }, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn num(value: f64, unit: Option<&str>) -> Tok {
        Tok::Num { value, unit: unit.map(String::from) }
    }

    #[test]
    fn numbers_and_units() {
        assert_eq!(toks("800hz")[0], num(800.0, Some("hz")));
        assert_eq!(toks("0.5")[0], num(0.5, None));
        assert_eq!(toks("1/8beat")[0], num(0.125, Some("beat")));
        assert_eq!(toks("600ms.exp")[..3], [num(600.0, Some("ms")), Tok::Dot, Tok::Ident("exp".into())]);
    }

    #[test]
    fn rational_without_unit_stays_a_division() {
        assert_eq!(toks("3/4")[..3], [num(3.0, None), Tok::Slash, num(4.0, None)]);
    }

    #[test]
    fn ranges_do_not_eat_the_dot() {
        assert_eq!(toks("0..1")[..3], [num(0.0, None), Tok::DotDot, num(1.0, None)]);
    }

    #[test]
    fn note_names_are_identifiers() {
        assert_eq!(toks("a3 c#4 bb2")[..3], [Tok::Ident("a3".into()), Tok::Ident("c#4".into()), Tok::Ident("bb2".into())]);
    }

    #[test]
    fn newlines_separate_statements_but_not_inside_brackets_or_before_dots() {
        let t = toks("a\n\n\nb(\n1,\n2\n)\n  .c\nd");
        // a NL b ( 1 , 2 ) .c NL d: newlines only after `a` and after `.c`
        assert_eq!(t.iter().filter(|t| **t == Tok::Newline).count(), 2);
    }

    #[test]
    fn brackets_are_patterns_outside_signal_bodies_and_tokens_inside_them() {
        // top level: raw text, including characters the lexer would reject
        assert_eq!(toks("riff = [c2 ~ <a b>*2 . x]")[2], Tok::Pattern("c2 ~ <a b>*2 . x".into()));
        // nested brackets stay inside one pattern
        assert_eq!(toks("x = [a [b c] d]")[2], Tok::Pattern("a [b c] d".into()));
        // clip and scene bodies are not signal contexts
        assert_eq!(toks("clip c { notes: [a b] }")[5], Tok::Pattern("a b".into()));
        // instruments, opcodes and tracks are
        for head in ["instr a(f: hz)", "opcode o(x)", "track t"] {
            let t = toks(&format!("{head} {{ [x, y] }}"));
            assert!(t.contains(&Tok::LBracket), "{head}: {t:?}");
            assert!(!t.iter().any(|t| matches!(t, Tok::Pattern(_))), "{head}");
        }
        // env literals are always tokens
        assert!(toks("x = env[0 1sec 1]").contains(&Tok::LBracket));
        // grids are raw
        assert_eq!(toks("g = grid[x.x. xXo.]")[2], Tok::Grid("x.x. xXo.".into()));
        assert_eq!(lex("x = [a b").unwrap_err().message, "this `[` is never closed");
    }

    #[test]
    fn percent_at_tilde_and_spaced_units() {
        assert_eq!(toks("30%")[0], num(30.0, Some("%")));
        assert_eq!(toks("2 bars")[0], num(2.0, Some("bars")));
        assert_eq!(toks("120 bpm")[0], num(120.0, Some("bpm")));
        assert_eq!(toks("1/2 beat")[0], Tok::Num { value: 1.0, unit: None }, "a lone fraction isn't a literal");
        assert_eq!(toks("3 x")[0], num(3.0, None), "only unit words merge");
        assert_eq!(toks("3 barn")[0], num(3.0, None), "and only whole words");
        assert_eq!(toks("2\nbars")[0], num(2.0, None), "never across a line");
        // inside env brackets a space separates steps
        assert_eq!(toks("env[0 5ms 1 2 beats]")[3], num(5.0, Some("ms")));
        assert_eq!(&toks("@ next 4 bars ~")[..4], [Tok::At, Tok::Ident("next".into()), num(4.0, Some("bars")), Tok::Tilde]);
    }

    #[test]
    fn comments_and_errors() {
        assert_eq!(toks("a // hi\nb").len(), 4);
        assert_eq!(lex("a $").unwrap_err().message, "unexpected character `$`");
    }
}
