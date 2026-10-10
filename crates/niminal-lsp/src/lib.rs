//! The niminal language server. It speaks LSP over stdio and answers from the
//! compiler itself, so it can never disagree with the daemon about the language.

mod complete;
mod text;

use std::collections::HashMap;
use std::io::{BufRead, Write};

use niminal_lang::{CompileOptions, Layout, Samples, Symbol, SymbolKind, builtin_ops, compile_with, symbols, waves};
use serde_json::{Value, json};

struct Document {
    text: String,
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
                    "textDocumentSync": 1,
                    "completionProvider": { "triggerCharacters": ["."] },
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "documentSymbolProvider": true,
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
                // full-document sync: the last change is the whole text
                let text = params["contentChanges"].as_array().and_then(|c| c.last()).and_then(|c| c["text"].as_str());
                if let (Some(uri), Some(text)) = (uri, text) {
                    self.update(uri, text, &mut out);
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
        let symbols = symbols(text).ok().or(previous).unwrap_or_default();
        out.push(notification("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": diagnostics(uri, text) })));
        self.documents.insert(uri.to_string(), Document { text: text.to_string(), symbols });
    }

    fn document(&self, params: &Value) -> Option<(&Document, usize)> {
        let doc = self.documents.get(params["textDocument"]["uri"].as_str()?)?;
        Some((doc, text::offset(&doc.text, &params["position"])))
    }

    fn completion(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return json!([]) };
        let items = complete::items(&doc.text[..offset], &doc.symbols, &|s| doc_comment(&doc.text, s));
        json!({ "isIncomplete": false, "items": items })
    }

    fn hover(&self, params: &Value) -> Value {
        let Some((doc, offset)) = self.document(params) else { return Value::Null };
        let Some((word, start, end)) = text::word_at(&doc.text, offset) else { return Value::Null };
        let code = |text: &str| format!("```niminal\n{text}\n```");
        let contents = if let Some(symbol) = doc.symbols.iter().find(|s| s.name == word) {
            let comment = doc_comment(&doc.text, symbol);
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
        match doc.symbols.iter().find(|s| s.name == word) {
            Some(s) => json!({
                "uri": params["textDocument"]["uri"],
                "range": text::range(&doc.text, s.name_span.start, s.name_span.end),
            }),
            None => Value::Null,
        }
    }

    fn document_symbols(&self, params: &Value) -> Value {
        let Some(doc) = params["textDocument"]["uri"].as_str().and_then(|u| self.documents.get(u)) else { return json!([]) };
        doc.symbols
            .iter()
            .map(|s| json!({
                "name": s.name,
                "detail": s.detail,
                "kind": match s.kind {
                    SymbolKind::Instrument | SymbolKind::Opcode => 12, // Function
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

fn diagnostics(uri: &str, text: &str) -> Vec<Value> {
    let options = CompileOptions { default_layout: Layout::Mono, samples: Samples::new(text::directory_of(uri)) };
    // a compiler bug must not take the editor's language support down with it
    let result = std::panic::catch_unwind(|| compile_with(text, &options));
    let errors = match result {
        Ok(Ok(_)) => return Vec::new(),
        Ok(Err(errors)) => errors,
        Err(_) => return Vec::new(),
    };
    errors
        .iter()
        .map(|d| {
            let message = match &d.help {
                Some(help) => format!("{}\nhelp: {help}", d.message),
                None => d.message.clone(),
            };
            json!({ "range": text::range(text, d.span.start, d.span.end), "severity": 1, "source": "niminal", "message": message })
        })
        .collect()
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
        server.handle(&json!({ "method": "textDocument/didOpen", "params": { "textDocument": { "uri": URI, "text": text } } }))
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
        assert_eq!(diagnostics[0]["message"], "expected a waveform\nhelp: one of sine, saw, square, tri");
        assert_eq!(diagnostics[0]["range"]["start"]["character"], 27);
        // fixing it clears them
        let out = server.handle(&json!({ "method": "textDocument/didChange", "params": {
            "textDocument": { "uri": URI }, "contentChanges": [{ "text": "instr tone(freq: hz) { osc(sine, freq) }" }] } }));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]));
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
}
