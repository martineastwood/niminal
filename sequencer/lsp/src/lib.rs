//! The niminal language server. It speaks LSP over stdio and answers from the
//! compiler itself, so it can never disagree with the daemon about the language.

mod complete;
mod features;
mod text;

use std::collections::HashMap;
use std::io::{BufRead, Write};

use niminal_lang::{CompileOptions, Layout, Samples, Symbol, SymbolKind, builtin_ops, compile_with, symbols, waves};
use serde_json::{Value, json};

/// A module the document uses.
struct Import {
    path: std::path::PathBuf,
    alias: Option<String>,
    text: String,
    /// What it defines that is not private.
    symbols: Vec<Symbol>,
}

struct Document {
    text: String,
    imports: Vec<Import>,
    /// The names from the last version of the text that parsed, so completion
    /// and hover keep working while a statement is half typed.
    symbols: Vec<Symbol>,
}

#[derive(Default)]
pub struct Server {
    documents: HashMap<String, Document>,
    shutdown: bool,
    /// Set by `exit`: the process exit code.
    pub exit: Option<i32>,
}

impl Server {
    pub fn new() -> Server {
        Server::default()
    }

    /// Handle one message; the messages to send back (a response, and any notifications).
    pub fn handle(&mut self, message: &Value) -> Vec<Value> {
        let Some(method) = message["method"].as_str() else { return Vec::new() };
        let params = &message["params"];
        let id = message.get("id").cloned();
        let mut out = Vec::new();
        let result = match method {
            "initialize" => Some(json!({
                "capabilities": {
                    "textDocumentSync": 2,
                    "completionProvider": { "triggerCharacters": ["."] },
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "documentSymbolProvider": true,
                    "renameProvider": { "prepareProvider": true },
                    "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
                    "semanticTokensProvider": {
                        "legend": { "tokenTypes": features::TOKEN_TYPES, "tokenModifiers": [] },
                        "full": true,
                    },
                },
                "serverInfo": { "name": "niminal", "version": env!("CARGO_PKG_VERSION") },
            })),
            "shutdown" => {
                self.shutdown = true;
                Some(Value::Null)
            }
            "exit" => {
                self.exit = Some(if self.shutdown { 0 } else { 1 });
                None
            }
            "textDocument/didOpen" => {
                let doc = &params["textDocument"];
                if let (Some(uri), Some(text)) = (doc["uri"].as_str(), doc["text"].as_str()) {
                    self.update(uri, text, &mut out);
                }
                None
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str();
                // incremental sync: each change replaces a range of the text (or all of it, with no range),
                // one after the other
                if let (Some(uri), Some(changes)) = (uri, params["contentChanges"].as_array())
                    && let Some(mut text) = self.documents.get(uri).map(|d| d.text.clone()).or_else(|| changes.iter().any(|c| c.get("range").is_none()).then(String::new))
                {
                    for change in changes {
                        let Some(new) = change["text"].as_str() else { continue };
                        match change.get("range").filter(|r| r.is_object()) {
                            None => text = new.to_string(),
                            Some(range) => {
                                let start = text::offset(&text, &range["start"]);
                                let end = text::offset(&text, &range["end"]).max(start);
                                text.replace_range(start..end, new);
                            }
                        }
                    }
                    self.update(uri, &text, &mut out);
                }
                None
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                    out.push(notification("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": [] })));
                }
                None
            }
            "textDocument/completion" => Some(self.completion(params)),
            "textDocument/hover" => Some(self.hover(params)),
            "textDocument/definition" => Some(self.definition(params)),
            "textDocument/documentSymbol" => Some(self.document_symbols(params)),
            "textDocument/prepareRename" => Some(self.prepare_rename(params)),
            "textDocument/rename" => match self.rename(params) {
                Ok(edit) => Some(edit),
                Err(message) => {
                    out.push(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32803, "message": message } }));
                    return out;
                }
            },
            "textDocument/signatureHelp" => Some(self.signature_help(params)),
            "textDocument/semanticTokens/full" => Some(self.semantic_tokens(params)),
            _ if id.is_some() => {
                out.push(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("`{method}` isn't supported") } }));
                return out;
            }
            _ => None,
        };
        if let (Some(id), Some(result)) = (id, result) {
            out.insert(0, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
        out
    }

    fn update(&mut self, uri: &str, text: &str, out: &mut Vec<Value>) {
        let previous = self.documents.remove(uri).map(|d| d.symbols);
        let expanded = niminal_lang::expand(text, &text::directory_of(uri));
        let (symbols, imports) = match &expanded {
            Ok(e) => (
                symbols(&e.text).ok().map(|all| all.into_iter().filter(|s| s.span.end <= e.root_len).collect()),
                e.imports()
                    .map(|i| Import {
                        path: i.path.to_path_buf(),
                        alias: i.alias.map(str::to_string),
                        text: i.source.to_string(),
                        symbols: symbols(i.source).unwrap_or_default().into_iter().filter(|s| !s.name.starts_with('_')).collect(),
                    })
                    .collect(),
            ),
            Err(_) => (None, Vec::new()),
        };
        let symbols = symbols
            .or_else(|| {
                // it doesn't parse: take the names from around the line that is wrong
                let lenient = features::symbols_lenient(text, &text::directory_of(uri));
                (!lenient.is_empty()).then_some(lenient)
            })
            .or(previous)
            .unwrap_or_default();
        out.push(notification("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": diagnostics(uri, text, expanded) })));
        self.documents.insert(uri.to_string(), Document { text: text.to_string(), imports, symbols });
    }

    fn document(&self, params: &Value) -> Option<(&Document, usize)> {
        let doc = self.documents.get(params["textDocument"]["uri"].as_str()?)?;
        Some((doc, text::offset(&doc.text, &params["position"])))
    }

    fn completion(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return json!([]) };
        let before = &doc.text[..offset];
        let docs = |s: &Symbol| doc.comment_of(s);
        // `alias::` offers what that module defines
        if let Some(alias) = text::qualifier(before) {
            let names: Vec<Symbol> = doc.imports.iter().filter(|i| i.alias.as_deref() == Some(alias)).flat_map(|i| i.symbols.clone()).collect();
            return json!({ "isIncomplete": false, "items": complete::symbol_items(&names, &docs) });
        }
        let mut symbols = doc.symbols.clone();
        symbols.extend(doc.imports.iter().filter(|i| i.alias.is_none()).flat_map(|i| i.symbols.clone()));
        json!({ "isIncomplete": false, "items": complete::items(before, &symbols, &docs) })
    }

    fn hover(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return Value::Null };
        let Some((word, start, end)) = text::word_at(&doc.text, offset) else { return Value::Null };
        let code = |text: &str| format!("```niminal\n{text}\n```");
        let contents = if let Some((symbol, text, _)) = doc.find(word) {
            let comment = doc_comment(text, symbol);
            if comment.is_empty() { code(&symbol.detail) } else { format!("{}\n\n{comment}", code(&symbol.detail)) }
        } else if let Some(op) = builtin_ops().into_iter().find(|o| o.name == word) {
            format!("{}\n\n{}", code(&op.signature()), op.doc)
        } else if waves().any(|w| w == word) {
            format!("{}\n\nA waveform for `osc`.", code(word))
        } else {
            return Value::Null;
        };
        json!({ "contents": { "kind": "markdown", "value": contents }, "range": text::range(&doc.text, start, end) })
    }

    fn definition(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return Value::Null };
        let Some((word, ..)) = text::word_at(&doc.text, offset) else { return Value::Null };
        match doc.find(word) {
            Some((s, text, file)) => json!({
                "uri": file.map_or_else(|| params["textDocument"]["uri"].clone(), |f| json!(text::file_uri(f))),
                "range": text::range(text, s.name_span.start, s.name_span.end),
            }),
            None => Value::Null,
        }
    }

    fn prepare_rename(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return Value::Null };
        match features::prepare_rename(doc, offset) {
            Some((start, end, name)) => json!({ "range": text::range(&doc.text, start, end), "placeholder": name }),
            None => Value::Null,
        }
    }

    fn rename(&self, params: &Value) -> Result<Value, String> {
        let (_, offset) = self.document(params).ok_or("that file isn't open")?;
        let new = params["newName"].as_str().ok_or("no new name was given")?;
        self.rename_edit(params["textDocument"]["uri"].as_str().unwrap_or_default(), offset, new)
    }

    fn signature_help(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return Value::Null };
        features::signature_help(doc, offset)
    }

    fn semantic_tokens(&self, params: &Value) -> Value {
        let Some(doc) = params["textDocument"]["uri"].as_str().and_then(|u| self.documents.get(u)) else { return json!({ "data": [] }) };
        json!({ "data": features::semantic_tokens(doc) })
    }

    fn document_symbols(&self, params: &Value) -> Value {
        let Some(doc) = params["textDocument"]["uri"].as_str().and_then(|u| self.documents.get(u)) else { return json!([]) };
        doc.symbols
            .iter()
            .map(|s| json!({
                "name": s.name,
                "detail": s.detail,
                "kind": match s.kind {
                    SymbolKind::Instrument | SymbolKind::Function => 12, // Function
                    SymbolKind::Control => 13,                          // Variable
                    SymbolKind::Bus | SymbolKind::Track => 5,           // Class
                    SymbolKind::Sample | SymbolKind::Kit => 19,         // Object
                    SymbolKind::Clip | SymbolKind::Scene | SymbolKind::Arrangement => 2, // Module
                    SymbolKind::Pattern => 14,                          // Constant
                },
                "range": text::range(&doc.text, s.span.start, s.span.end),
                "selectionRange": text::range(&doc.text, s.name_span.start, s.name_span.end),
            }))
            .collect::<Vec<_>>()
            .into()
    }
}

impl Document {
    /// The definition of a name as written here: in this file, or in a module
    /// as `alias::name` or plain `name`. With the text it is in and, for a
    /// module, the file.
    fn find(&self, word: &str) -> Option<(&Symbol, &str, Option<&std::path::Path>)> {
        if let Some(s) = self.symbols.iter().find(|s| s.name == word) {
            return Some((s, &self.text, None));
        }
        self.imports.iter().find_map(|i| {
            let name = match &i.alias {
                Some(alias) => word.strip_prefix(alias.as_str())?.strip_prefix("::")?,
                None => word,
            };
            i.symbols.iter().find(|s| s.name == name).map(|s| (s, i.text.as_str(), Some(i.path.as_path())))
        })
    }

    /// The comment above a definition, in this file or a module.
    fn comment_of(&self, symbol: &Symbol) -> String {
        if self.symbols.contains(symbol) {
            return doc_comment(&self.text, symbol);
        }
        let module = self.imports.iter().find(|i| i.symbols.contains(symbol));
        doc_comment(module.map_or(&self.text, |i| &i.text), symbol)
    }
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// The comment lines (`//` or `///`) directly above a definition.
fn doc_comment(text: &str, symbol: &Symbol) -> String {
    let before = text[..symbol.span.start].trim_end_matches([' ', '\t']);
    let mut lines: Vec<&str> = Vec::new();
    for line in before.lines().rev() {
        let Some(comment) = line.trim().strip_prefix("//") else { break };
        lines.push(comment.strip_prefix('/').unwrap_or(comment).trim());
    }
    lines.reverse();
    lines.join("\n")
}

fn diagnostics(uri: &str, text: &str, expanded: Result<niminal_lang::Expanded, niminal_lang::Diagnostic>) -> Vec<Value> {
    let options = CompileOptions { default_layout: Layout::Mono, samples: Samples::new(text::directory_of(uri)), ..Default::default() };
    let expanded = match expanded {
        Ok(e) => e,
        Err(d) => return vec![diagnostic(text, &d)],
    };
    // a compiler bug must not take the editor's language support down with it
    let result = std::panic::catch_unwind(|| compile_with(&expanded.text, &options));
    let errors = match result {
        Ok(Ok(_)) => return Vec::new(),
        Ok(Err(errors)) => errors.into_iter().map(|d| expanded.locate(d)).collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };
    errors.iter().map(|d| diagnostic(text, d)).collect()
}

fn diagnostic(text: &str, d: &niminal_lang::Diagnostic) -> Value {
    let message = match &d.help {
        Some(help) => format!("{}\nhelp: {help}", d.message),
        None => d.message.clone(),
    };
    json!({ "range": text::range(text, d.span.start, d.span.end), "severity": 1, "source": "niminal", "message": message })
}

/// Serve LSP over `reader` and `writer` until the editor says `exit` or hangs up.
/// Returns the process exit code.
pub fn run(mut reader: impl BufRead, mut writer: impl Write) -> i32 {
    let mut server = Server::new();
    loop {
        let Some(message) = read_message(&mut reader) else { return server.exit.unwrap_or(1) };
        for reply in server.handle(&message) {
            let body = reply.to_string();
            if write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len()).and_then(|()| writer.flush()).is_err() {
                return 1;
            }
        }
        if let Some(code) = server.exit {
            return code;
        }
    }
}

/// One framed message, or `None` when the editor has gone.
fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let length = length?;
        if length > 64 << 20 {
            return None;
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        // a message that isn't JSON is skipped; the editor shouldn't lose its server over it
        if let Ok(message) = serde_json::from_slice(&body) {
            return Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "file:///tmp/set.nml";
    const SOURCE: &str = "// A bright lead.\n/// Plays one note.\ninstr tone(freq: hz) {\n  osc(saw, freq).lpf(cutoff: 800)\n}\n";

    fn request(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
        server.handle(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).remove(0)["result"].clone()
    }

    fn open(server: &mut Server, text: &str) -> Vec<Value> {
        open_at(server, URI, text)
    }

    fn open_at(server: &mut Server, uri: &str, text: &str) -> Vec<Value> {
        server.handle(&json!({ "method": "textDocument/didOpen", "params": { "textDocument": { "uri": uri, "text": text } } }))
    }

    fn at(line: u32, character: u32) -> Value {
        json!({ "textDocument": { "uri": URI }, "position": { "line": line, "character": character } })
    }

    #[test]
    fn opening_a_file_publishes_its_errors_with_help() {
        let mut server = Server::new();
        let out = open(&mut server, "instr tone(freq: hz) { osc(sinee, freq) }");
        let diagnostics = &out[0]["params"]["diagnostics"];
        assert_eq!(diagnostics.as_array().unwrap().len(), 1, "{out:?}");
        assert_eq!(diagnostics[0]["range"]["start"]["line"], 0);
        assert_eq!(diagnostics[0]["message"], "expected a waveform or a table\nhelp: one of sine, saw, square, tri, or an array of numbers");
        assert_eq!(diagnostics[0]["range"]["start"]["character"], 27);
        // fixing it clears them
        let out = server.handle(&json!({ "method": "textDocument/didChange", "params": {
            "textDocument": { "uri": URI }, "contentChanges": [{ "text": "instr tone(freq: hz) { osc(sine, freq) }" }] } }));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]));
    }

    #[test]
    fn edits_arrive_as_ranges_and_are_applied_in_order() {
        let mut server = Server::new();
        open(&mut server, "instr tone(freq: hz) { osc(sinee, freq) }\n// done\n");
        let mut change = |changes: Value| {
            server.handle(&json!({ "method": "textDocument/didChange", "params": {
                "textDocument": { "uri": URI }, "contentChanges": changes } }))
        };
        // fix the wave: `sinee` is columns 27 to 32
        let out = change(json!([{ "range": { "start": { "line": 0, "character": 27 }, "end": { "line": 0, "character": 32 } }, "text": "sine" }]));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]), "{out:?}");
        // two edits in one message: the second sees the first (add a line, then a word on it)
        let out = change(json!([
            { "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } }, "text": "x = [c4 e4]\n" },
            { "range": { "start": { "line": 1, "character": 5 }, "end": { "line": 1, "character": 6 } }, "text": "[c4 zz" },
        ]));
        let diagnostics = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{out:?}");
        assert_eq!(diagnostics[0]["range"]["start"]["line"], 1);
        // a change without a range is the whole text again
        let out = change(json!([{ "text": "instr tone(freq: hz) { osc(sine, freq) }" }]));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]));
        // multibyte characters count in UTF-16 units
        let out = change(json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "text": "// é😀\n" },
            { "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 5 } }, "text": "instr" }]));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]), "{out:?}");
    }

    #[test]
    fn hover_shows_signatures_and_doc_comments() {
        let mut server = Server::new();
        open(&mut server, SOURCE);
        let hover = request(&mut server, 1, "textDocument/hover", at(2, 7));
        let text = hover["contents"]["value"].as_str().unwrap();
        assert!(text.contains("instr tone(freq: hz)") && text.contains("A bright lead.\nPlays one note."), "{text}");
        let hover = request(&mut server, 2, "textDocument/hover", at(3, 17));
        assert!(hover["contents"]["value"].as_str().unwrap().contains("lpf(x: number, cutoff: hz, res?: number)"));
        assert_eq!(request(&mut server, 3, "textDocument/hover", at(3, 0)), Value::Null);
    }

    #[test]
    fn definition_jumps_to_the_name_and_symbols_list_the_file() {
        let mut server = Server::new();
        open(&mut server, &format!("{SOURCE}track lead {{ instrument = tone }}\n"));
        let location = request(&mut server, 1, "textDocument/definition", at(5, 28));
        assert_eq!(location["range"]["start"], json!({ "line": 2, "character": 6 }));
        let symbols = request(&mut server, 2, "textDocument/documentSymbol", json!({ "textDocument": { "uri": URI } }));
        assert_eq!(symbols.as_array().unwrap().len(), 2);
        assert_eq!(symbols[1]["name"], "lead");
    }

    #[test]
    fn completion_works_on_half_typed_code_using_the_last_good_names() {
        let mut server = Server::new();
        open(&mut server, SOURCE);
        // the file no longer parses, but `tone` is still known
        open(&mut server, &format!("{SOURCE}track lead {{ instrument = to"));
        let completion = request(&mut server, 1, "textDocument/completion", at(5, 28));
        let labels: Vec<_> = completion["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert!(labels.contains(&"tone") && labels.contains(&"lpf"), "{labels:?}");
    }

    const SONG: &str = "instr tone(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: 800) }\ntrack lead { instrument = tone }\nlead.play([c4])\nlead.start\n";

    #[test]
    fn rename_changes_every_use_of_a_name_and_refuses_bad_names() {
        let mut server = Server::new();
        open(&mut server, SONG);
        // on the definition, and on a use
        for position in [at(1, 7), at(2, 2)] {
            let prepared = request(&mut server, 1, "textDocument/prepareRename", position);
            assert_eq!(prepared["placeholder"], "lead");
        }
        let edit = request(&mut server, 2, "textDocument/rename", json!({ "textDocument": { "uri": URI }, "position": { "line": 1, "character": 7 }, "newName": "pad" }));
        let edits = edit["changes"][URI].as_array().unwrap();
        // the track's name, `lead.play`, `lead.start`; `lead` isn't touched inside `tone`
        assert_eq!(edits.len(), 3, "{edit}");
        assert!(edits.iter().all(|e| e["newText"] == "pad"));
        assert_eq!(edits[0]["range"]["start"], json!({ "line": 1, "character": 6 }));

        let mut error = |new: &str| {
            let reply = server.handle(&json!({ "jsonrpc": "2.0", "id": 9, "method": "textDocument/rename",
                "params": { "textDocument": { "uri": URI }, "position": { "line": 1, "character": 7 }, "newName": new } }));
            reply[0]["error"]["message"].as_str().unwrap_or_default().to_string()
        };
        assert!(error("tone").contains("already a name in use"));
        assert!(error("lpf").contains("already a name in use"));
        assert!(error("if").contains("already a name in use"));
        assert!(error("two words").contains("isn't a name"));
        assert!(error("9lives").contains("isn't a name"));
        // something that isn't defined here has nothing to rename
        assert_eq!(request(&mut server, 3, "textDocument/prepareRename", at(0, 40)), Value::Null);
    }

    #[test]
    fn signature_help_follows_the_argument_the_cursor_is_in() {
        let mut server = Server::new();
        let text = "instr tone(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: 800, res: 0.2) }\nx = tone(freq: 440hz, bright: 0.2)\n";
        open(&mut server, text);
        let help = |server: &mut Server, line: u32, character: u32| request(server, 1, "textDocument/signatureHelp", at(line, character));

        // `tone(` and after the first comma
        let h = help(&mut server, 1, 9);
        assert_eq!(h["signatures"][0]["label"], "instr tone(freq: hz, bright: 0..1 = 0.5)");
        assert_eq!(h["activeParameter"], 0);
        assert_eq!(h["signatures"][0]["parameters"][1]["label"], "bright: 0..1 = 0.5");
        assert_eq!(help(&mut server, 1, 24)["activeParameter"], 1);
        // a named argument picks its own parameter, whatever comes first
        let named = "x = tone(bright: 0.2, ";
        open(&mut server, &format!("{text}{named}"));
        assert_eq!(help(&mut server, 2, 18)["activeParameter"], 1);

        // a method call: the receiver supplies the first parameter, so `cutoff` is the second
        let h = help(&mut server, 0, 70);
        assert!(h["signatures"][0]["label"].as_str().unwrap().starts_with("lpf(x: number, cutoff: hz"), "{h}");
        assert_eq!(h["activeParameter"], 1);
        assert_eq!(help(&mut server, 0, 82)["activeParameter"], 2, "after the comma it is `res`");
        // outside any call there is nothing to show
        assert_eq!(help(&mut server, 1, 2), Value::Null);
        // and neither inside brackets that aren't a call
        open(&mut server, "x = (1 + 2");
        assert_eq!(help(&mut server, 0, 10), Value::Null);
    }

    #[test]
    fn semantic_tokens_tell_names_apart_by_what_they_are() {
        let mut server = Server::new();
        open(&mut server, SONG);
        let reply = request(&mut server, 1, "textDocument/semanticTokens/full", json!({ "textDocument": { "uri": URI } }));
        let data: Vec<u32> = reply["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
        assert_eq!(data.len() % 5, 0);
        // decode back to (line, column, length, type)
        let (mut line, mut column) = (0, 0);
        let mut found = Vec::new();
        for token in data.chunks(5) {
            line += token[0];
            column = if token[0] == 0 { column + token[1] } else { token[1] };
            let word = &SONG.lines().nth(line as usize).unwrap()[column as usize..(column + token[2]) as usize];
            found.push((word.to_string(), features::TOKEN_TYPES[token[3] as usize]));
        }
        let kind = |word: &str, nth: usize| found.iter().filter(|(w, _)| w == word).nth(nth).map(|(_, k)| *k);
        assert_eq!(kind("tone", 0), Some("function"), "{found:?}");
        assert_eq!(kind("freq", 0), Some("parameter"));
        assert_eq!(kind("saw", 0), Some("enumMember"));
        assert_eq!(kind("osc", 0), Some("function"));
        assert_eq!(kind("lpf", 0), Some("function"));
        assert_eq!(kind("cutoff", 0), Some("parameter"));
        assert_eq!(kind("lead", 0), Some("class"));
        assert_eq!(kind("lead", 1), Some("class"));
        assert_eq!(kind("tone", 1), Some("function"), "used as the track's instrument");
        // words of the language itself are left to the grammar
        assert_eq!(kind("instr", 0), None);
        assert_eq!(kind("track", 0), None);
    }

    fn rename_at(server: &mut Server, uri: &str, line: u32, character: u32, new: &str) -> Value {
        let reply = server.handle(&json!({ "jsonrpc": "2.0", "id": 7, "method": "textDocument/rename", "params": {
            "textDocument": { "uri": uri }, "position": { "line": line, "character": character }, "newName": new } }));
        reply[0].clone()
    }

    #[test]
    fn rename_leaves_alone_what_only_shares_the_spelling() {
        let text = "instr tone(lead: hz, bright: 0..1 = 0.5) { osc(saw, lead) * bright }\ntrack lead { instrument = tone(lead: 440hz) }\nscene verse { lead: lead }\nclip riff { notes: [c4]\n bright: [0.2] }\nbright = [0.1 0.9]\nfn drive(x) { x }\nbass = osc(saw, 100hz).drive\n";
        let mut server = Server::new();
        open(&mut server, text);
        let places = |reply: &Value| -> Vec<(u64, u64)> {
            let mut all: Vec<(u64, u64)> = reply["result"]["changes"][URI].as_array().unwrap().iter()
                .map(|e| (e["range"]["start"]["line"].as_u64().unwrap(), e["range"]["start"]["character"].as_u64().unwrap())).collect();
            all.sort();
            all
        };
        // the track: its definition, and both words of the scene entry; not the parameter or the argument
        let track = rename_at(&mut server, URI, 1, 7, "pad");
        assert_eq!(places(&track), [(1, 6), (2, 14), (2, 20)], "{track}");
        // the pattern: not the parameter (header and body), nor the lane key
        let pattern = rename_at(&mut server, URI, 5, 1, "level");
        assert_eq!(places(&pattern), [(5, 0)], "{pattern}");
        // a function is used after a dot
        let function = rename_at(&mut server, URI, 6, 4, "crush");
        assert_eq!(places(&function), [(6, 3), (7, 23)], "{function}");
    }

    #[test]
    fn rename_follows_a_name_into_the_files_that_use_it_as_a_module() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("niminal-lsp-rename-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        write("rig.nml", "instr tone(freq: hz) { osc(sine, freq) }\n");
        write("other.nml", "use rig\ntone(freq: 220hz) for 1beat\n");
        write("alone.nml", "instr tone(freq: hz) { osc(saw, freq) }\ntone(freq: 330hz) for 1beat\n");
        let song_uri = format!("file://{}/song.nml", dir.display());
        let mut server = Server::new();
        open_at(&mut server, &song_uri, "use rig as r\nr::tone(freq: 440hz) for 1beat\n");

        let reply = rename_at(&mut server, &song_uri, 1, 5, "bell");
        let changes = reply["result"]["changes"].as_object().unwrap_or_else(|| panic!("{reply}"));
        let files: std::collections::BTreeMap<String, &Value> = changes
            .iter()
            .map(|(uri, edits)| (text::path_of(uri).unwrap().file_name().unwrap().to_string_lossy().into_owned(), edits))
            .collect();
        assert_eq!(files.keys().map(String::as_str).collect::<Vec<_>>(), ["other.nml", "rig.nml", "song.nml"], "{reply}");
        // in the file using the alias only the name after `::` changes; the others change in place
        assert_eq!(files["song.nml"][0]["range"]["start"], json!({ "line": 1, "character": 3 }));
        assert_eq!(files["rig.nml"][0]["range"]["start"], json!({ "line": 0, "character": 6 }));
        assert_eq!(files["other.nml"][0]["range"]["start"], json!({ "line": 1, "character": 0 }));

        // a name that one of the users already has is refused, and says which
        write("other.nml", "use rig\nbell = [c4]\ntone(freq: 220hz) for 1beat\n");
        let clash = rename_at(&mut server, &song_uri, 1, 5, "bell");
        assert!(clash["error"]["message"].as_str().unwrap().contains("other.nml"), "{clash}");

        // from the module's own file the users are found too
        let rig_uri = format!("file://{}/rig.nml", dir.display());
        open_at(&mut server, &rig_uri, "instr tone(freq: hz) { osc(sine, freq) }\n");
        let reply = rename_at(&mut server, &rig_uri, 0, 8, "chime");
        let names: Vec<String> = reply["result"]["changes"].as_object().unwrap().keys().map(|u| text::path_of(u).unwrap().file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"song.nml".to_string()) && names.contains(&"rig.nml".to_string()), "{reply}");
        assert!(!names.contains(&"alone.nml".to_string()), "a file with its own `tone` doesn't use the module");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_line_that_does_not_read_costs_only_that_line() {
        let mut server = Server::new();
        let text = "instr tone(freq: hz) { osc(sine, freq) }\nx = \"never closed\ntrack lead { instrument = tone }\n";
        open(&mut server, text);
        let reply = request(&mut server, 1, "textDocument/semanticTokens/full", json!({ "textDocument": { "uri": URI } }));
        let lines: Vec<u64> = {
            let (mut line, mut seen) = (0, Vec::new());
            for t in reply["data"].as_array().unwrap().chunks(5) {
                line += t[0].as_u64().unwrap();
                seen.push(line);
            }
            seen
        };
        assert!(lines.contains(&0) && lines.contains(&2), "the lines either side are still coloured: {lines:?}");
        // typing a string in a call still shows the call
        open(&mut server, "instr tone(freq: hz) { osc(sine, freq) }\nx = tone(freq: \"ab");
        let help = request(&mut server, 2, "textDocument/signatureHelp", at(1, 17));
        assert_eq!(help["signatures"][0]["label"], "instr tone(freq: hz)", "{help}");
        // but a rename, which would have to guess at the broken line, says what is wrong first
        let reply = rename_at(&mut server, URI, 0, 8, "bell");
        assert!(reply["error"]["message"].as_str().unwrap().contains("doesn't read yet"), "{reply}");
    }

    #[test]
    fn the_stdio_loop_frames_messages_and_exits_cleanly() {
        let frame = |v: Value| format!("Content-Length: {}\r\n\r\n{v}", v.to_string().len());
        let input = [
            frame(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} })),
            frame(json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" })),
            frame(json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" })),
            frame(json!({ "jsonrpc": "2.0", "method": "exit" })),
        ]
        .concat();
        let mut output = Vec::new();
        assert_eq!(run(input.as_bytes(), &mut output), 0);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("hoverProvider") && output.contains("isn't supported"));
        assert_eq!(run("".as_bytes(), Vec::new()), 1);
    }

    #[test]
    fn names_from_used_modules_complete_hover_and_jump() {
        let dir = std::env::temp_dir().join(format!("niminal-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rig.nml"), "/// A plucked note.\ninstr tone(freq: hz) { osc(sine, freq) }\n_secret = [c4]\n").unwrap();
        std::fs::write(dir.join("plain.nml"), "instr bell(freq: hz) { osc(sine, freq) }\n").unwrap();
        let uri = format!("file://{}/song.nml", dir.display());
        let at = |line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });
        let mut server = Server::new();
        let text = "use rig as r\nuse plain\nr::tone(freq: 440hz) for 1beat\n";
        let out = server.handle(&json!({ "method": "textDocument/didOpen", "params": { "textDocument": { "uri": uri, "text": text } } }));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]), "{out:?}");

        let labels = |v: &Value| v["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let after_alias = request(&mut server, 1, "textDocument/completion", at(2, 3));
        assert_eq!(labels(&after_alias), ["tone"]);
        let anywhere = request(&mut server, 2, "textDocument/completion", at(1, 0));
        let anywhere = labels(&anywhere);
        assert!(anywhere.contains(&"bell".to_string()) && !anywhere.contains(&"tone".to_string()) && !anywhere.contains(&"_secret".to_string()));

        let hover = request(&mut server, 3, "textDocument/hover", at(2, 4));
        let hover = hover["contents"]["value"].as_str().unwrap();
        assert!(hover.contains("instr tone(freq: hz)") && hover.contains("A plucked note."), "{hover}");
        let location = request(&mut server, 4, "textDocument/definition", at(2, 4));
        assert!(location["uri"].as_str().unwrap().ends_with("/rig.nml"), "{location}");
        assert_eq!(location["range"]["start"], json!({ "line": 1, "character": 6 }));

        let out = server.handle(&json!({ "method": "textDocument/didChange", "params": {
            "textDocument": { "uri": uri }, "contentChanges": [{ "text": "use nowhere\n" }] } }));
        assert!(out[0]["params"]["diagnostics"][0]["message"].as_str().unwrap().contains("no module `nowhere`"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
