use niminal_lang::{Diagnostic, Span};

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
    Tilde,
    At,
    /// The raw text of a mini-notation pattern, between `[` and `]`. Outside
    /// instruments, opcodes and nodes a bracket is a pattern, not a channel list.
    Pattern(String),
    /// The raw text of `grid[...]`.
    Grid(String),
    /// The raw text of `grid { ... }`: one row per sample.
    Rows(String),
    /// A row of steps for a lane, `.x.x xx..`: drawn with `.`, `x`, `X` and `o`, only where a clip gives an
    /// effect's lanes (`fx_name { open: x.x. }`, or `fx_name.open: x.x.`).
    Steps(String),
    /// The raw text of `events { ... }` inside a clip.
    Events(String),
    /// A quoted string such as a file path.
    Str(String),
    Newline,
    Eof,
}

/// What an open `{` holds.
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    /// A clip's body.
    Clip,
    /// An effect's lanes inside a clip, `fx_name { open: x.x. }`.
    FxLanes,
    Other,
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
    // Whether each open brace is a signal context (an instrument, function or
    // part body), where a bracket is a channel list or envelope.
    let mut braces: Vec<bool> = Vec::new();
    // What each open brace holds, so a clip's body and an effect's block of lanes inside it can be told apart.
    let mut scopes: Vec<Scope> = Vec::new();
    let mut statement_start = true;
    let mut statement_keyword: Option<String> = None;
    // the macros, which a clip's lane can be named for, wherever in the file they are declared
    let macros: Vec<&str> = src
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("macro "))
        .filter_map(|rest| rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).find(|n| !n.is_empty()))
        .collect();
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
            b'.' | b'x' | b'X' | b'o' | b'<' if takes_steps(&tokens, scopes.last(), &macros) && steps_end(src, i).is_some() => {
                let end = steps_end(src, i).unwrap_or(i);
                let text = src[i..end].to_string();
                i = end;
                Tok::Steps(text)
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
            b'[' if is_raw_bracket(
                &tokens,
                braces.last().copied().unwrap_or(false)
                    || matches!(
                        statement_keyword.as_deref(),
                        Some("const" | "fn" | "instr" | "mono" | "fx")
                    ),
            ) =>
            {
                let is_grid =
                    matches!(tokens.last(), Some(Token { tok: Tok::Ident(n), .. }) if n == "grid");
                let Some(close) = matching_bracket(bytes, i) else {
                    return Err(Diagnostic::new(
                        "this `[` is never closed",
                        Span::new(i, i + 1),
                    ));
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
            b'{' if matches!(tokens.last(), Some(Token { tok: Tok::Ident(n), .. }) if n == "grid" || n == "events") =>
            {
                let Some(len) = matching_brace(src, i) else {
                    return Err(Diagnostic::new(
                        "this `{` is never closed",
                        Span::new(i, i + 1),
                    ));
                };
                let text = src[i + 1..i + len].to_string();
                let events = matches!(
                    tokens.last(),
                    Some(Token { tok: Tok::Ident(n), .. }) if n == "events"
                );
                // the keyword is part of the token
                start = tokens.pop().map_or(start, |t| t.span.start);
                i += len + 1;
                if events { Tok::Events(text) } else { Tok::Rows(text) }
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
                        if c == b'{' {
                            let inherited = braces.last().copied().unwrap_or(false);
                            let signal = matches!(
                                statement_keyword.as_deref(),
                                Some(
                                    "instr"
                                        | "mono"
                                        | "fn"
                                        | "const"
                                        | "fx"
                                        | "arrangement"
                                        | "bus"
                                )
                            );
                            braces.push(inherited || signal);
                            let after = tokens.len().checked_sub(2).map(|n| &tokens[n].tok);
                            scopes.push(if statement_keyword.as_deref() == Some("clip") {
                                Scope::Clip
                            } else if scopes.last() == Some(&Scope::Clip)
                                && matches!(tokens.last(), Some(Token { tok: Tok::Ident(_), .. }))
                                && matches!(after, None | Some(Tok::Newline | Tok::LBrace | Tok::Comma))
                            {
                                Scope::FxLanes
                            } else {
                                Scope::Other
                            });
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
                            scopes.pop();
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

/// Is a `[` here a raw pattern (or grid) rather than ordinary tokens?
fn is_raw_bracket(tokens: &[Token], in_signal: bool) -> bool {
    match tokens.last() {
        Some(Token {
            tok: Tok::Ident(name),
            ..
        }) if name == "env" => false,
        Some(Token {
            tok: Tok::Ident(name),
            ..
        }) if name == "grid" => true,
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

/// Whether a row of steps can start here: after the colon of an effect's lane in a clip, either inside the
/// effect's block or written `fx_name.param:`, or after the name of a macro.
fn takes_steps(tokens: &[Token], scope: Option<&Scope>, macros: &[&str]) -> bool {
    let [.., before, Token { tok: Tok::Colon, .. }] = tokens else {
        return false;
    };
    match scope {
        Some(Scope::FxLanes) => true,
        Some(Scope::Clip) => {
            matches!(&before.tok, Tok::Ident(name) if macros.contains(&name.as_str()))
                || matches!(before.tok, Tok::Ident(_))
                && matches!(
                    tokens.len().checked_sub(4).map(|n| (&tokens[n].tok, &tokens[n + 1].tok)),
                    Some((Tok::Ident(_), Tok::Dot))
                )
        }
        _ => false,
    }
}

/// Where a row of steps that starts at `from` ends: the end of the line, or its `//` comment, if all it holds
/// is `.`, `x`, `X`, `o` and spaces, with the mini-notation around them (`...<x X .>`, `x?`, `x(3,8)`).
fn steps_end(src: &str, from: usize) -> Option<usize> {
    let line = src[from..].split('\n').next().unwrap_or("");
    let line = line.split("//").next().unwrap_or("").trim_end();
    (!line.is_empty()
        && line.chars().all(|c| {
            c.is_ascii_digit() || matches!(c, '.' | 'x' | 'X' | 'o' | ' ' | '\t' | '<' | '>' | '[' | ']' | '(' | ')' | '*' | '/' | '!' | '?' | '@' | ',' | '~' | '%' | '|')
        }))
    .then_some(from + line.len())
}

/// Index of the matching `}`, or `None`. `open` is the index of `{`. Returns the
/// length from `{` to `}` inclusive so `src[open + 1..open + len]` is the body.
fn matching_brace(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i - open);
                }
            }
            _ => {}
        }
    }
    None
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
    fn brackets_are_patterns_outside_signal_bodies_and_tokens_inside_them() {
        // top level: raw text, including characters the lexer would reject
        assert_eq!(
            toks("riff = [c2 ~ <a b>*2 . x]")[2],
            Tok::Pattern("c2 ~ <a b>*2 . x".into())
        );
        // nested brackets stay inside one pattern
        assert_eq!(toks("x = [a [b c] d]")[2], Tok::Pattern("a [b c] d".into()));
        // clip and scene bodies are not signal contexts
        assert_eq!(
            toks("clip c { notes: [a b] }")[5],
            Tok::Pattern("a b".into())
        );
        // instruments, functions and nodes are
        for head in ["instr a(f: hz)", "fn o(x)", "fx t"] {
            let t = toks(&format!("{head} {{ [x, y] }}"));
            assert!(t.contains(&Tok::LBracket), "{head}: {t:?}");
            assert!(!t.iter().any(|t| matches!(t, Tok::Pattern(_))), "{head}");
        }
        // env literals are always tokens
        assert!(toks("x = env[0 1sec 1]").contains(&Tok::LBracket));
        // grids are raw
        assert_eq!(
            toks("g = grid[x.x. xXo.]")[2],
            Tok::Grid("x.x. xXo.".into())
        );
        assert_eq!(
            lex("x = [a b").unwrap_err().message,
            "this `[` is never closed"
        );
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
