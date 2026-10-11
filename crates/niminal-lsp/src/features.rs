//! Rename, signature help and semantic tokens. They read the compiler's own tokens, so what they
//! see as a name is what the compiler sees.

use niminal_lang::{OpInfo, Symbol, SymbolKind, Tok, Token, builtin_ops, lex, pattern_ops, waves};
use serde_json::{Value, json};

use crate::{Document, text};

/// The token types semantic highlighting uses, in the order the legend lists them.
pub const TOKEN_TYPES: [&str; 6] = ["function", "class", "variable", "parameter", "enumMember", "namespace"];
const FUNCTION: u32 = 0;
const CLASS: u32 = 1;
const VARIABLE: u32 = 2;
const PARAMETER: u32 = 3;
const ENUM_MEMBER: u32 = 4;
const NAMESPACE: u32 = 5;

/// Names that mean something to the language itself, which no definition may take.
const KEYWORDS: [&str; 33] = [
    "instr", "mono", "opcode", "fn", "state", "bus", "buffer", "table", "sample", "kit", "ctl", "track", "clip", "scene",
    "arrangement", "config", "use", "as", "with", "if", "else", "for", "in", "out", "it", "tempo", "meter", "at", "env",
    "hush", "panic", "reset", "unsolo",
];

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---- rename -------------------------------------------------------------------------------------

/// The range and name under the cursor, if it is something defined in this file.
pub fn prepare_rename(doc: &Document, offset: usize) -> Option<(usize, usize, String)> {
    let (word, start, end) = text::word_at(&doc.text, offset)?;
    doc.symbols.iter().find(|s| s.name == word)?;
    Some((start, end, word.to_string()))
}

/// Every place `old` is written in the file, as ranges, if `new` is a fine name to give it.
pub fn rename(doc: &Document, offset: usize, new: &str) -> Result<Vec<(usize, usize)>, String> {
    let Some((_, _, old)) = prepare_rename(doc, offset) else {
        return Err("only a name defined in this file can be renamed here".into());
    };
    if !is_name(new) {
        return Err(format!("`{new}` isn't a name: use letters, digits and `_`, not starting with a digit"));
    }
    let taken = doc.symbols.iter().any(|s| s.name == new)
        || doc.imports.iter().any(|i| i.symbols.iter().any(|s| s.name == new))
        || builtin_ops().iter().any(|o| o.name == new)
        || pattern_ops().iter().any(|o| o.name == new)
        || waves().any(|w| w == new)
        || KEYWORDS.contains(&new);
    if taken && new != old {
        return Err(format!("`{new}` is already a name in use"));
    }
    let tokens = lex(&doc.text).map_err(|d| format!("the file doesn't read yet: {}", d.message))?;
    Ok(tokens
        .iter()
        .filter(|t| matches!(&t.tok, Tok::Ident(n) if *n == old))
        .map(|t| (t.span.start, t.span.end))
        .collect())
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
        parameters: op.params.iter().map(|p| format!("{}{}: {}", p.name, if p.required { "" } else { "?" }, p.ty)).collect(),
        receiver,
    }
}

fn symbol_signature(doc: &Document, symbol: &Symbol, method: bool) -> Signature {
    Signature {
        label: symbol.detail.clone(),
        documentation: doc.comment_of(symbol),
        parameters: symbol.params.iter().map(|p| p.label.clone()).collect(),
        receiver: usize::from(method && symbol.kind == SymbolKind::Opcode),
    }
}

/// What to show while the cursor is inside the brackets of a call.
pub fn signature_help(doc: &Document, offset: usize) -> Value {
    let before = &doc.text[..offset.min(doc.text.len())];
    let Ok(tokens) = lex(before) else { return Value::Null };
    let tokens: Vec<&Token> = tokens.iter().filter(|t| !matches!(t.tok, Tok::Eof | Tok::Newline)).collect();

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
    let Some(Tok::Ident(callee)) = open.checked_sub(1).map(|i| &tokens[i].tok) else { return Value::Null };
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
        (Some(Token { tok: Tok::Ident(n), .. }), Some(Token { tok: Tok::Colon, .. })) => Some(n.as_str()),
        _ => None,
    };

    let signature = doc
        .find(callee)
        .filter(|(s, ..)| matches!(s.kind, SymbolKind::Instrument | SymbolKind::Opcode))
        .map(|(s, ..)| symbol_signature(doc, s, method))
        .or_else(|| method.then(|| pattern_ops().into_iter().find(|o| o.name == *callee)).flatten().map(|o| op_signature(&o, 0)))
        .or_else(|| builtin_ops().into_iter().find(|o| o.name == *callee).map(|o| op_signature(&o, usize::from(method))));
    let Some(signature) = signature else { return Value::Null };

    let active = match named {
        Some(name) => signature.parameters.iter().position(|p| p.trim_end_matches('?').split(':').next().is_some_and(|n| n.trim_end_matches('?') == name)),
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

/// The whole file's highlighted names, in the encoding LSP uses: for each, how far from the last
/// one it is (lines, then columns), its length, its type and its modifiers.
pub fn semantic_tokens(doc: &Document) -> Vec<u32> {
    let Ok(tokens) = lex(&doc.text) else { return Vec::new() };
    let builtins: Vec<String> = builtin_ops().into_iter().chain(pattern_ops()).map(|o| o.name).collect();
    let line_starts: Vec<usize> = std::iter::once(0).chain(doc.text.match_indices('\n').map(|(i, _)| i + 1)).collect();

    let mut data = Vec::new();
    let (mut last_line, mut last_char) = (0usize, 0usize);
    for (i, t) in tokens.iter().enumerate() {
        let Tok::Ident(name) = &t.tok else { continue };
        let before = i.checked_sub(1).map(|i| &tokens[i].tok);
        let after = tokens.get(i + 1).map(|t| &t.tok);
        let kind = if KEYWORDS.contains(&name.as_str()) {
            None
        } else if let Some((symbol, ..)) = doc.find(name) {
            Some(match symbol.kind {
                SymbolKind::Instrument | SymbolKind::Opcode => FUNCTION,
                SymbolKind::Track | SymbolKind::Bus | SymbolKind::Sample | SymbolKind::Kit => CLASS,
                SymbolKind::Control | SymbolKind::Pattern => VARIABLE,
                SymbolKind::Clip | SymbolKind::Scene | SymbolKind::Arrangement => NAMESPACE,
            })
        } else if (after == Some(&Tok::LParen) || before == Some(&Tok::Dot)) && builtins.contains(name) {
            Some(FUNCTION)
        } else if before == Some(&Tok::LParen) && after == Some(&Tok::Comma) && waves().any(|w| w == name) {
            Some(ENUM_MEMBER)
        } else if after == Some(&Tok::Colon) && before != Some(&Tok::Dot) {
            Some(PARAMETER)
        } else {
            None
        };
        let Some(kind) = kind else { continue };

        let line = line_starts.partition_point(|&s| s <= t.span.start) - 1;
        let character: usize = doc.text[line_starts[line]..t.span.start].chars().map(char::len_utf16).sum();
        let length: usize = doc.text[t.span.start..t.span.end].chars().map(char::len_utf16).sum();
        let (delta_line, delta_char) = (line - last_line, if line == last_line { character - last_char } else { character });
        data.extend([delta_line as u32, delta_char as u32, length as u32, kind, 0]);
        (last_line, last_char) = (line, character);
    }
    data
}
