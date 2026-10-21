//! Rename, signature help and semantic tokens. They read the compiler's own tokens, so what they
//! see as a name is what the compiler sees.

use niminal_compose::{RESERVED_WORDS, Symbol, SymbolKind, Tok, Token, lex, pattern_ops, symbols};
use niminal_lang::{OpInfo, builtin_ops, waves};
use serde_json::{Value, json};

use crate::{Document, text};

/// The token types semantic highlighting uses, in the order the legend lists them.
pub const TOKEN_TYPES: [&str; 6] = [
    "function",
    "class",
    "variable",
    "parameter",
    "enumMember",
    "namespace",
];
const FUNCTION: u32 = 0;
const CLASS: u32 = 1;
const VARIABLE: u32 = 2;
const PARAMETER: u32 = 3;
const ENUM_MEMBER: u32 = 4;
const NAMESPACE: u32 = 5;

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---- reading a file that doesn't quite parse ------------------------------------------------------

/// The tokens of `text`, skipping over whatever the lexer can't read: from the bad spot to the end of
/// its line is blanked out (keeping every other offset where it was) and the rest is read as usual.
/// The flag says whether anything had to be skipped.
fn lex_lenient(text: &str) -> (Vec<Token>, bool) {
    let mut working = text.to_string();
    for attempt in 0..32 {
        match lex(&working) {
            Ok(tokens) => return (tokens, attempt > 0),
            Err(d) => {
                let start = d.span.start.min(working.len());
                let end = working[start..]
                    .find('\n')
                    .map_or(working.len(), |i| start + i);
                let blank: String = working[start..end]
                    .chars()
                    .map(|c| " ".repeat(c.len_utf8()))
                    .collect();
                working.replace_range(start..end, &blank);
            }
        }
    }
    (Vec::new(), true)
}

/// Blank out the rest of the line from `at`, or the whole line if that leaves the text as it was.
fn blank_from(text: &mut String, at: usize) {
    let at = at.min(text.len());
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let blank = |s: &str| {
        s.chars()
            .map(|c| " ".repeat(c.len_utf8()))
            .collect::<String>()
    };
    let rest = blank(&text[at..line_end]);
    if rest == text[at..line_end] {
        let whole = blank(&text[line_start..line_end]);
        text.replace_range(line_start..line_end, &whole);
    } else {
        text.replace_range(at..line_end, &rest);
    }
}

/// The names `text` (a file in `dir`) defines, with its `use` lines resolved. Lines that don't read
/// are skipped, so a half-typed line doesn't take every other name away.
pub fn symbols_lenient(text: &str, dir: &std::path::Path) -> Vec<Symbol> {
    let mut working = text.to_string();
    for _ in 0..16 {
        let error = match niminal_compose::expand(&working, dir) {
            Ok(expanded) => match symbols(&expanded.text) {
                Ok(all) => {
                    return all
                        .into_iter()
                        .filter(|s| s.span.end <= expanded.root_len)
                        .collect();
                }
                Err(d) if d.span.start < expanded.root_len => d,
                Err(_) => return Vec::new(),
            },
            Err(d) => d,
        };
        blank_from(&mut working, error.span.start);
    }
    Vec::new()
}

// ---- rename -------------------------------------------------------------------------------------

/// What brackets a token sits inside: the innermost one, and for braces the word the statement began with.
#[derive(Clone, PartialEq)]
enum Within {
    Nothing,
    Paren,
    Bracket,
    Brace(String),
}

fn contexts(tokens: &[Token]) -> Vec<Within> {
    let mut stack: Vec<Within> = Vec::new();
    let mut head: Option<String> = None;
    let mut at_start = true;
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        out.push(stack.last().cloned().unwrap_or(Within::Nothing));
        match &t.tok {
            Tok::LParen => stack.push(Within::Paren),
            Tok::LBracket => stack.push(Within::Bracket),
            Tok::LBrace => stack.push(Within::Brace(head.take().unwrap_or_default())),
            Tok::RParen | Tok::RBracket | Tok::RBrace => {
                stack.pop();
            }
            _ => {}
        }
        match &t.tok {
            Tok::Newline | Tok::LBrace | Tok::RBrace => at_start = true,
            Tok::Ident(name) if at_start => {
                head = Some(name.clone());
                at_start = false;
            }
            _ => at_start = false,
        }
    }
    out
}

/// The names a definition makes up for itself: its parameters, what its body binds, its states.
fn locals(tokens: &[Token], within: &[Within], first: usize, last: usize) -> Vec<String> {
    let mut names = Vec::new();
    let mut at_start = true;
    for i in first..last {
        match &tokens[i].tok {
            Tok::Newline | Tok::LBrace | Tok::RBrace => at_start = true,
            Tok::Ident(n)
                if within[i] == Within::Paren
                    && tokens.get(i + 1).map(|t| &t.tok) == Some(&Tok::Colon) =>
            {
                // `(freq: hz = 440hz)` in the header names a parameter; in a call it names an argument
                names.push(n.clone());
            }
            Tok::Ident(n) if at_start => {
                if n == "state" {
                    if let Some(Tok::Ident(state)) = tokens.get(i + 1).map(|t| &t.tok) {
                        names.push(state.clone());
                    }
                } else if tokens.get(i + 1).map(|t| &t.tok) == Some(&Tok::Eq) {
                    names.push(n.clone());
                }
                at_start = false;
            }
            Tok::LBracket if at_start => {
                // `[l, r] = src`
                let mut j = i + 1;
                let mut found = Vec::new();
                while let Some(Token {
                    tok: Tok::Ident(n), ..
                }) = tokens.get(j)
                {
                    found.push(n.clone());
                    j += 1;
                    if tokens.get(j).map(|t| &t.tok) == Some(&Tok::Comma) {
                        j += 1;
                    }
                }
                if tokens.get(j).map(|t| &t.tok) == Some(&Tok::RBracket)
                    && tokens.get(j + 1).map(|t| &t.tok) == Some(&Tok::Eq)
                {
                    names.extend(found);
                }
                at_start = false;
            }
            _ => at_start = false,
        }
    }
    names
}

/// Where `name`, a top-level definition of this `kind`, is written in `tokens`: leaving out what only
/// shares its spelling, such as a parameter or argument of that name, a lane key, or a name a
/// definition makes up for itself.
fn references(
    tokens: &[Token],
    name: &str,
    kind: SymbolKind,
    defs: &[Symbol],
) -> Vec<(usize, usize)> {
    let within = contexts(tokens);
    let index_at = |offset: usize| {
        tokens
            .iter()
            .position(|t| t.span.start >= offset)
            .unwrap_or(tokens.len())
    };

    // the stretches where `name` is something else: inside a definition that has a local of that name
    let mut shadowed: Vec<(usize, usize)> = Vec::new();
    for def in defs.iter().filter(|d| {
        matches!(
            d.kind,
            SymbolKind::Instrument
                | SymbolKind::Function
                | SymbolKind::Clip
                | SymbolKind::Fx
                | SymbolKind::Bus
        )
    }) {
        let (first, last) = (index_at(def.name_span.end), index_at(def.span.end));
        if locals(tokens, &within, first, last)
            .iter()
            .any(|n| n == name)
        {
            shadowed.push((def.name_span.end, def.span.end));
        }
    }

    let mut found = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let Tok::Ident(n) = &t.tok else { continue };
        if n != name
            || shadowed
                .iter()
                .any(|&(a, b)| t.span.start >= a && t.span.start < b)
        {
            continue;
        }
        let before = i.checked_sub(1).map(|i| &tokens[i].tok);
        let after = tokens.get(i + 1).map(|t| &t.tok);
        // `x.name` is a use of an opcode, and otherwise a word that only follows a dot
        if before == Some(&Tok::Dot) && kind != SymbolKind::Function {
            continue;
        }
        if after == Some(&Tok::Colon) {
            let is_definition = defs.iter().any(|d| d.name_span == t.span);
            let names_a_part = matches!(&within[i], Within::Brace(head) if head == "scene");
            if !is_definition && !names_a_part && within[i] != Within::Nothing {
                continue;
            }
        }
        found.push((t.span.start, t.span.end));
    }
    found
}

/// A file a rename may change: how to send an edit for it, and what it says.
struct Source {
    uri: String,
    path: Option<std::path::PathBuf>,
    text: String,
}

fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn check_new_name(old: &str, new: &str, symbols: &[Symbol]) -> Result<(), String> {
    if !is_name(new) {
        return Err(format!(
            "`{new}` isn't a name: use letters, digits and `_`, not starting with a digit"
        ));
    }
    let taken = symbols.iter().any(|s| s.name == new)
        || builtin_ops().iter().any(|o| o.name == new)
        || pattern_ops().iter().any(|o| o.name == new)
        || waves().any(|w| w == new)
        || RESERVED_WORDS.contains(&new);
    if taken && new != old {
        return Err(format!("`{new}` is already a name in use"));
    }
    Ok(())
}

/// The range and name under the cursor, if it is something defined in this file or in a module it uses.
pub fn prepare_rename(doc: &Document, offset: usize) -> Option<(usize, usize, String)> {
    let (word, start, end) = text::word_at(&doc.text, offset)?;
    let (symbol, ..) = doc.find(word)?;
    // for `alias::name` it is the name after the `::` that changes
    let from = end - symbol.name.len();
    Some((from.max(start), end, symbol.name.clone()))
}

impl crate::Server {
    /// The edit that gives the name at `offset` in the document `uri` the name `new`: in the file that
    /// defines it, and in every file next to it that uses it as a module.
    pub(crate) fn rename_edit(&self, uri: &str, offset: usize, new: &str) -> Result<Value, String> {
        let doc = self.documents.get(uri).ok_or("that file isn't open")?;
        let (word, ..) = text::word_at(&doc.text, offset).ok_or("there is no name here")?;
        let (symbol, _, module) = doc
            .find(word)
            .ok_or("only a name defined in this file, or in a module it uses, can be renamed")?;
        let (name, kind) = (symbol.name.clone(), symbol.kind);
        let here = text::path_of(uri).map(|p| canonical(&p));

        // the file that defines it
        let defined_in =
            match module {
                None => Source {
                    uri: uri.to_string(),
                    path: here.clone(),
                    text: doc.text.clone(),
                },
                Some(path) => {
                    let path = canonical(path);
                    match self.documents.iter().find(|(u, _)| {
                        text::path_of(u).map(|p| canonical(&p)).as_ref() == Some(&path)
                    }) {
                        Some((u, d)) => Source {
                            uri: u.clone(),
                            path: Some(path),
                            text: d.text.clone(),
                        },
                        None => Source {
                            uri: text::file_uri(&path),
                            text: std::fs::read_to_string(&path)
                                .map_err(|e| format!("can't read {}: {e}", path.display()))?,
                            path: Some(path),
                        },
                    }
                }
            };
        let tokens = lex(&defined_in.text)
            .map_err(|d| format!("{} doesn't read yet: {}", file_name(&defined_in), d.message))?;
        let own_symbols = strict_symbols(&defined_in)?;
        check_new_name(&name, new, &own_symbols)?;
        let mut edits: Vec<(Source, Vec<(usize, usize)>)> = Vec::new();
        let at = references(&tokens, &name, kind, &own_symbols);
        edits.push((defined_in, at));

        // the files that use it as a module: the ones open, and the ones beside it
        let defined_path = edits[0].0.path.clone();
        if let Some(defined_path) = defined_path.filter(|_| !name.starts_with('_')) {
            let mut candidates: Vec<Source> = self
                .documents
                .iter()
                .map(|(u, d)| Source {
                    uri: u.clone(),
                    path: text::path_of(u).map(|p| canonical(&p)),
                    text: d.text.clone(),
                })
                .collect();
            let dirs = [
                defined_path.parent(),
                defined_path.parent().and_then(std::path::Path::parent),
            ];
            for dir in dirs.into_iter().flatten().take(2) {
                let Ok(listing) = std::fs::read_dir(dir) else {
                    continue;
                };
                for entry in listing.flatten().take(200) {
                    let path = canonical(&entry.path());
                    if path.extension().is_some_and(|e| e == "nml")
                        && !candidates.iter().any(|c| c.path.as_ref() == Some(&path))
                        && let Ok(text) = std::fs::read_to_string(&path)
                    {
                        candidates.push(Source {
                            uri: text::file_uri(&path),
                            path: Some(path),
                            text,
                        });
                    }
                }
            }
            for candidate in candidates
                .into_iter()
                .filter(|c| c.path != Some(defined_path.clone()))
            {
                let dir = candidate
                    .path
                    .as_deref()
                    .and_then(std::path::Path::parent)
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| ".".into());
                let Ok(expanded) = niminal_compose::expand(&candidate.text, &dir) else {
                    continue;
                };
                let imports: Vec<Option<String>> = expanded
                    .imports()
                    .filter(|i| canonical(i.path) == defined_path)
                    .map(|i| i.alias.map(str::to_string))
                    .collect();
                if imports.is_empty() {
                    continue;
                }
                let theirs = strict_symbols(&candidate).unwrap_or_default();
                if theirs.iter().any(|s| s.name == new) {
                    return Err(format!(
                        "`{new}` is already a name in {}, which uses this module",
                        file_name(&candidate)
                    ));
                }
                let Ok(tokens) = lex(&candidate.text) else {
                    return Err(format!(
                        "{} uses this module but doesn't read yet",
                        file_name(&candidate)
                    ));
                };
                let mut at = Vec::new();
                for alias in imports {
                    match alias {
                        // `r::name`: only the part after the `::` changes
                        Some(alias) => {
                            let qualified = format!("{alias}::{name}");
                            at.extend(
                                tokens
                                    .iter()
                                    .filter(|t| matches!(&t.tok, Tok::Ident(n) if *n == qualified))
                                    .map(|t| (t.span.end - name.len(), t.span.end)),
                            );
                        }
                        None if theirs.iter().any(|s| s.name == name) => {}
                        None => at.extend(references(&tokens, &name, kind, &theirs)),
                    }
                }
                if !at.is_empty() {
                    edits.push((candidate, at));
                }
            }
        }

        let mut changes = serde_json::Map::new();
        for (source, places) in edits {
            if places.is_empty() {
                continue;
            }
            let list: Vec<Value> = places
                .iter()
                .map(|&(s, e)| json!({ "range": text::range(&source.text, s, e), "newText": new }))
                .collect();
            changes.insert(source.uri, Value::Array(list));
        }
        Ok(json!({ "changes": changes }))
    }
}

/// The names a file defines, if it reads as a whole.
fn strict_symbols(source: &Source) -> Result<Vec<Symbol>, String> {
    let dir = source
        .path
        .as_deref()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| ".".into());
    let reads = |d: niminal_lang::Diagnostic| {
        format!("{} doesn't read yet: {}", file_name(source), d.message)
    };
    let expanded = niminal_compose::expand(&source.text, &dir).map_err(reads)?;
    let all = symbols(&expanded.text)
        .map_err(|d| format!("{} doesn't read yet: {}", file_name(source), d.message))?;
    Ok(all
        .into_iter()
        .filter(|s| s.span.end <= expanded.root_len)
        .collect())
}

fn file_name(source: &Source) -> String {
    source
        .path
        .as_deref()
        .and_then(|p| p.file_name())
        .map_or_else(|| source.uri.clone(), |n| n.to_string_lossy().into_owned())
}

// ---- signature help -----------------------------------------------------------------------------

struct Signature {
    label: String,
    documentation: String,
    parameters: Vec<String>,
    /// How many of the parameters a method's receiver supplies before the first argument.
    receiver: usize,
}

fn op_signature(op: &OpInfo, receiver: usize) -> Signature {
    Signature {
        label: op.signature(),
        documentation: op.doc.clone(),
        parameters: op
            .params
            .iter()
            .map(|p| format!("{}{}: {}", p.name, if p.required { "" } else { "?" }, p.ty))
            .collect(),
        receiver,
    }
}

fn symbol_signature(doc: &Document, symbol: &Symbol, method: bool) -> Signature {
    Signature {
        label: symbol.detail.clone(),
        documentation: doc.comment_of(symbol),
        parameters: symbol.params.iter().map(|p| p.label.clone()).collect(),
        receiver: usize::from(method && symbol.kind == SymbolKind::Function),
    }
}

/// What to show while the cursor is inside the brackets of a call.
pub fn signature_help(doc: &Document, offset: usize) -> Value {
    let before = &doc.text[..offset.min(doc.text.len())];
    let (tokens, _) = lex_lenient(before);
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t.tok, Tok::Eof | Tok::Newline))
        .collect();

    // the `(` the cursor is inside, and how many commas and which name came after it
    let mut depth = 0usize;
    let mut open = None;
    for (i, t) in tokens.iter().enumerate().rev() {
        match t.tok {
            Tok::RParen => depth += 1,
            Tok::LParen if depth > 0 => depth -= 1,
            Tok::LParen => {
                open = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(open) = open else { return Value::Null };
    let Some(Tok::Ident(callee)) = open.checked_sub(1).map(|i| &tokens[i].tok) else {
        return Value::Null;
    };
    let method = open >= 2 && tokens[open - 2].tok == Tok::Dot;

    let (mut commas, mut nested, mut segment_start) = (0usize, 0usize, open + 1);
    for (i, t) in tokens.iter().enumerate().skip(open + 1) {
        match t.tok {
            Tok::LParen | Tok::LBracket | Tok::LBrace => nested += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => nested = nested.saturating_sub(1),
            Tok::Comma if nested == 0 => {
                commas += 1;
                segment_start = i + 1;
            }
            _ => {}
        }
    }
    let named = match (tokens.get(segment_start), tokens.get(segment_start + 1)) {
        (
            Some(Token {
                tok: Tok::Ident(n), ..
            }),
            Some(Token {
                tok: Tok::Colon, ..
            }),
        ) => Some(n.as_str()),
        _ => None,
    };

    let signature = doc
        .find(callee)
        .filter(|(s, ..)| matches!(s.kind, SymbolKind::Instrument | SymbolKind::Function))
        .map(|(s, ..)| symbol_signature(doc, s, method))
        .or_else(|| {
            method
                .then(|| pattern_ops().into_iter().find(|o| o.name == *callee))
                .flatten()
                .map(|o| op_signature(&o, 0))
        })
        .or_else(|| {
            builtin_ops()
                .into_iter()
                .find(|o| o.name == *callee)
                .map(|o| op_signature(&o, usize::from(method)))
        });
    let Some(signature) = signature else {
        return Value::Null;
    };

    let active = match named {
        Some(name) => signature.parameters.iter().position(|p| {
            p.trim_end_matches('?')
                .split(':')
                .next()
                .is_some_and(|n| n.trim_end_matches('?') == name)
        }),
        None => Some(commas + signature.receiver),
    };
    json!({
        "signatures": [{
            "label": signature.label,
            "documentation": { "kind": "markdown", "value": signature.documentation },
            "parameters": signature.parameters.iter().map(|p| json!({ "label": p })).collect::<Vec<_>>(),
        }],
        "activeSignature": 0,
        "activeParameter": active,
    })
}

// ---- semantic tokens ----------------------------------------------------------------------------

/// For each token, whether it names a parameter or a local (or `out`) of the instrument, function or fx
/// it sits in. Parameters are the names in the header's parentheses; locals are the names bound
/// in the body with `=`, `+=`, `for` or `state`.
fn local_kinds(tokens: &[Token]) -> Vec<Option<u32>> {
    let mut kinds = vec![None; tokens.len()];
    let ident = |i: usize| match tokens.get(i).map(|t| &t.tok) {
        Some(Tok::Ident(n)) if n == "out" || !RESERVED_WORDS.contains(&n.as_str()) => {
            Some(n.as_str())
        }
        _ => None,
    };
    let is = |i: usize, tok: Tok| tokens.get(i).map(|t| &t.tok) == Some(&tok);
    let mut i = 0;
    while i < tokens.len() {
        let Tok::Ident(word) = &tokens[i].tok else {
            i += 1;
            continue;
        };
        if !matches!(word.as_str(), "instr" | "mono" | "fn" | "def" | "fx")
            || (i > 0 && is(i - 1, Tok::Dot))
        {
            i += 1;
            continue;
        }
        let Some(open) = (i..tokens.len()).find(|&j| is(j, Tok::LBrace)) else {
            break;
        };
        let close = {
            let mut depth = 0;
            (open..tokens.len())
                .find(|&j| {
                    depth += is(j, Tok::LBrace) as i32 - is(j, Tok::RBrace) as i32;
                    depth == 0
                })
                .unwrap_or(tokens.len() - 1)
        };
        let mut names: Vec<(&str, u32)> = Vec::new();
        let mut depth = 0;
        for j in i..open {
            depth += is(j, Tok::LParen) as i32 - is(j, Tok::RParen) as i32;
            if let Some(n) = ident(j)
                && depth == 1
                && (is(j - 1, Tok::LParen) || is(j - 1, Tok::Comma))
            {
                names.push((n, PARAMETER));
            }
        }
        for j in open..close {
            let Some(n) = ident(j) else { continue };
            let binds = is(j + 1, Tok::Eq)
                || (is(j + 1, Tok::Plus) && is(j + 2, Tok::Eq))
                || (j > 0
                    && matches!(&tokens[j - 1].tok, Tok::Ident(w) if w == "for" || w == "state"));
            if binds && !is(j - 1, Tok::Dot) && !names.iter().any(|(m, _)| *m == n) {
                names.push((n, PARAMETER));
            }
        }
        for (j, slot) in kinds.iter_mut().enumerate().take(close + 1).skip(i) {
            if let Some(n) = ident(j)
                && !is(j - 1, Tok::Dot)
                && let Some((_, kind)) = names.iter().find(|(m, _)| *m == n)
            {
                *slot = Some(*kind);
            }
        }
        i = close + 1;
    }
    kinds
}

/// The whole file's highlighted names, in the encoding LSP uses: for each, how far from the last
/// one it is (lines, then columns), its length, its type and its modifiers.
pub fn semantic_tokens(doc: &Document) -> Vec<u32> {
    let (tokens, _) = lex_lenient(&doc.text);
    let builtins: Vec<String> = builtin_ops()
        .into_iter()
        .chain(pattern_ops())
        .map(|o| o.name)
        .collect();
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(doc.text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    let locals = local_kinds(&tokens);
    let mut data = Vec::new();
    let (mut last_line, mut last_char) = (0usize, 0usize);
    for (i, t) in tokens.iter().enumerate() {
        let Tok::Ident(name) = &t.tok else { continue };
        let before = i.checked_sub(1).map(|i| &tokens[i].tok);
        let after = tokens.get(i + 1).map(|t| &t.tok);
        let kind = if RESERVED_WORDS.contains(&name.as_str()) {
            locals[i]
        } else if let Some((symbol, ..)) = doc.find(name) {
            Some(match symbol.kind {
                SymbolKind::Instrument | SymbolKind::Function => FUNCTION,
                SymbolKind::Fx
                | SymbolKind::Bus
                | SymbolKind::Sample
                | SymbolKind::Kit
                | SymbolKind::Pattern => CLASS,
                SymbolKind::Control => VARIABLE,
                SymbolKind::Clip | SymbolKind::Scene | SymbolKind::Arrangement => NAMESPACE,
            })
        } else if (after == Some(&Tok::LParen) || before == Some(&Tok::Dot))
            && builtins.contains(name)
        {
            Some(FUNCTION)
        } else if before == Some(&Tok::LParen)
            && after == Some(&Tok::Comma)
            && waves().any(|w| w == name)
        {
            Some(ENUM_MEMBER)
        } else if after == Some(&Tok::Colon) && before != Some(&Tok::Dot) {
            Some(PARAMETER)
        } else {
            locals[i]
        };
        let Some(kind) = kind else { continue };

        let line = line_starts.partition_point(|&s| s <= t.span.start) - 1;
        let character: usize = doc.text[line_starts[line]..t.span.start]
            .chars()
            .map(char::len_utf16)
            .sum();
        let length: usize = doc.text[t.span.start..t.span.end]
            .chars()
            .map(char::len_utf16)
            .sum();
        let (delta_line, delta_char) = (
            line - last_line,
            if line == last_line {
                character - last_char
            } else {
                character
            },
        );
        data.extend([delta_line as u32, delta_char as u32, length as u32, kind, 0]);
        (last_line, last_char) = (line, character);
    }
    data
}
