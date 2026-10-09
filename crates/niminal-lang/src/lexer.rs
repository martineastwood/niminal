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
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
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
                let (tok, end) = lex_number(src, i);
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
                        match c {
                            b'(' => Tok::LParen,
                            b'[' => Tok::LBracket,
                            _ => Tok::LBrace,
                        }
                    }
                    b')' | b']' | b'}' => {
                        brackets.pop();
                        match c {
                            b')' => Tok::RParen,
                            b']' => Tok::RBracket,
                            _ => Tok::RBrace,
                        }
                    }
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
    fn comments_and_errors() {
        assert_eq!(toks("a // hi\nb").len(), 4);
        assert_eq!(lex("a $").unwrap_err().message, "unexpected character `$`");
    }
}
