use crate::diag::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    /// A number with an optional unit suffix attached directly (`800hz`,
    /// `1/8beat`). Rationals only lex as one literal when a unit follows.
    Num {
        value: f64,
        unit: Option<String>,
    },
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
    /// `..<`: a range that stops before its end.
    DotDotLt,
    Eq,
    /// `->`
    Arrow,
    /// `==`, `!=`, `<`, `>`, `<=`, `>=`
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    /// `%` between two values: the remainder
    Percent,
    Bar,
    /// A quoted string such as a file path.
    Str(String),
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
            // Non-breaking spaces commonly arrive through editor/clipboard input.
            0xc2 if bytes.get(i + 1) == Some(&0xa0) => {
                i += 2;
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
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
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
                // `alias::name` is one identifier.
                if src[end..].starts_with("::")
                    && bytes
                        .get(end + 2)
                        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
                {
                    end += 2;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                }
                let tok = Tok::Ident(src[i..end].to_string());
                i = end;
                tok
            }
            b'"' => {
                let Some(len) = src[i + 1..]
                    .find(['"', '\n'])
                    .filter(|&n| bytes[i + 1 + n] == b'"')
                else {
                    return Err(Diagnostic::new(
                        "this string is never closed",
                        Span::new(i, i + 1),
                    ));
                };
                let text = src[i + 1..i + 1 + len].to_string();
                i += len + 2;
                Tok::Str(text)
            }
            b'.' => {
                if bytes.get(i + 1) == Some(&b'.') && bytes.get(i + 2) == Some(&b'<') {
                    i += 3;
                    Tok::DotDotLt
                } else if bytes.get(i + 1) == Some(&b'.') {
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
                    b'=' if bytes.get(i) == Some(&b'=') => {
                        i += 1;
                        Tok::EqEq
                    }
                    b'=' => Tok::Eq,
                    b'!' if bytes.get(i) == Some(&b'=') => {
                        i += 1;
                        Tok::NotEq
                    }
                    b'<' if bytes.get(i) == Some(&b'=') => {
                        i += 1;
                        Tok::Le
                    }
                    b'<' => Tok::Lt,
                    b'>' if bytes.get(i) == Some(&b'=') => {
                        i += 1;
                        Tok::Ge
                    }
                    b'>' => Tok::Gt,
                    b'+' => Tok::Plus,
                    b'-' if bytes.get(i) == Some(&b'>') => {
                        i += 1;
                        Tok::Arrow
                    }
                    b'-' => Tok::Minus,
                    b'*' => Tok::Star,
                    b'/' => Tok::Slash,
                    b'%' => Tok::Percent,
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
        tokens.push(Token {
            tok,
            span: Span::new(start, i),
        });
    }

    tokens.push(Token {
        tok: Tok::Eof,
        span: Span::new(src.len(), src.len()),
    });
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
    if bytes.get(end) == Some(&b'%') {
        return (
            Tok::Num {
                value,
                unit: Some("%".into()),
            },
            end + 1,
        );
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
        Tok::Num {
            value,
            unit: unit.map(String::from),
        }
    }

    #[test]
    fn numbers_and_units() {
        assert_eq!(toks("800hz")[0], num(800.0, Some("hz")));
        assert_eq!(toks("0.5")[0], num(0.5, None));
        assert_eq!(toks("1/8beat")[0], num(0.125, Some("beat")));
        assert_eq!(
            toks("600ms.exp")[..3],
            [num(600.0, Some("ms")), Tok::Dot, Tok::Ident("exp".into())]
        );
    }

    #[test]
    fn rational_without_unit_stays_a_division() {
        assert_eq!(
            toks("3/4")[..3],
            [num(3.0, None), Tok::Slash, num(4.0, None)]
        );
    }

    #[test]
    fn ranges_do_not_eat_the_dot() {
        assert_eq!(
            toks("0..1")[..3],
            [num(0.0, None), Tok::DotDot, num(1.0, None)]
        );
    }

    #[test]
    fn note_names_are_identifiers() {
        assert_eq!(
            toks("a3 c#4 bb2")[..3],
            [
                Tok::Ident("a3".into()),
                Tok::Ident("c#4".into()),
                Tok::Ident("bb2".into())
            ]
        );
    }

    #[test]
    fn newlines_separate_statements_but_not_inside_brackets_or_before_dots() {
        let t = toks("a\n\n\nb(\n1,\n2\n)\n  .c\nd");
        // a NL b ( 1 , 2 ) .c NL d: newlines only after `a` and after `.c`
        assert_eq!(t.iter().filter(|t| **t == Tok::Newline).count(), 2);
    }

    #[test]
    fn percent_and_units_that_never_follow_a_space() {
        assert_eq!(toks("30%")[0], num(30.0, Some("%")));
        assert_eq!(toks("2bars")[0], num(2.0, Some("bars")));
        assert_eq!(
            toks("2 bars")[0],
            num(2.0, None),
            "a unit is attached or it is a name"
        );
        assert_eq!(toks("2 bars")[1], Tok::Ident("bars".into()));
        assert_eq!(
            toks("1/2 beat")[0],
            Tok::Num {
                value: 1.0,
                unit: None
            },
            "a lone fraction isn't a literal"
        );
        assert_eq!(toks("env[0 5ms 1 2beats]")[3], num(5.0, Some("ms")));
    }

    #[test]
    fn ranges_that_stop_before_their_end() {
        assert_eq!(
            toks("0..<8")[..3],
            [num(0.0, None), Tok::DotDotLt, num(8.0, None)]
        );
    }

    #[test]
    fn brackets_after_const_are_lists() {
        let t = toks("const taps = [0.25, 0.5]");
        assert!(t.contains(&Tok::LBracket), "{t:?}");
    }

    #[test]
    fn comments_and_errors() {
        assert_eq!(toks("a // hi\nb").len(), 4);
        assert_eq!(lex("a $").unwrap_err().message, "unexpected character `$`");
    }
}

#[cfg(test)]
mod pasted_whitespace_tests {
    use super::*;

    #[test]
    fn non_breaking_spaces_are_whitespace_and_keep_byte_spans() {
        let tokens = lex("a\u{a0}+\u{a0}b").unwrap();
        assert_eq!(tokens[1].tok, Tok::Plus);
        assert_eq!(tokens[1].span, Span::new(3, 4));
        assert_eq!(tokens[2].span, Span::new(6, 7));
        assert_eq!(
            lex("\"a\u{a0}b\"").unwrap()[0].tok,
            Tok::Str("a\u{a0}b".into())
        );
    }
}
