//! `niminal lsp` is the language server for editors: it answers over stdin and stdout.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

fn read_message(reader: &mut impl BufRead) -> serde_json::Value {
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("a header line");
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(n) = line.strip_prefix("Content-Length: ") {
            length = n.parse().expect("a length");
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).expect("a body");
    serde_json::from_slice(&body).expect("json")
}

#[test]
fn the_niminal_command_serves_the_language_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_niminal"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the program starts");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());

    stdin
        .write_all(frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#).as_bytes())
        .unwrap();
    let reply = read_message(&mut out);
    assert_eq!(reply["id"], 1);
    assert!(reply["result"]["capabilities"].is_object(), "{reply}");

    // a mistake in the code is reported with its position
    let open = serde_json::json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": "file:///t.nml", "languageId": "niminal", "version": 1,
                                     "text": "clip bad { instrument: tone\n  notes: [c4 zz] }"}}
    });
    stdin.write_all(frame(&open.to_string()).as_bytes()).unwrap();
    let diagnostics = loop {
        let message = read_message(&mut out);
        if message["method"] == "textDocument/publishDiagnostics" {
            break message;
        }
    };
    assert!(!diagnostics["params"]["diagnostics"].as_array().unwrap().is_empty(), "{diagnostics}");

    stdin.write_all(frame(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#).as_bytes()).unwrap();
    let _ = read_message(&mut out);
    stdin.write_all(frame(r#"{"jsonrpc":"2.0","method":"exit"}"#).as_bytes()).unwrap();
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
}
