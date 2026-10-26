//! Problems in source code as text, in the same style as compile errors.

use niminal_server::Problem;
use serde_json::Value;

/// A problem in `source` as text, in the same style as compile errors.
pub fn render_problem(
    message: &str,
    help: Option<&str>,
    line: Option<usize>,
    column: Option<usize>,
    source: &str,
    name: &str,
) -> String {
    let mut out = format!("error: {message}\n");
    if let (Some(line), Some(column)) = (line, column) {
        let text = source.lines().nth(line - 1).unwrap_or("");
        let gutter = line.to_string();
        let pad = " ".repeat(gutter.len());
        out.push_str(&format!(
            "{pad}--> {name}:{line}:{column}\n{pad} |\n{gutter} | {text}\n{pad} | {}^\n",
            " ".repeat(column - 1)
        ));
    }
    if let Some(help) = help {
        out.push_str(&format!("  = help: {help}\n"));
    }
    out
}

pub fn render_problems(problems: &[Problem], source: &str, name: &str) -> String {
    problems
        .iter()
        .map(|p| {
            let mut text = render_problem(
                &p.message,
                p.help.as_deref(),
                p.line,
                p.column,
                source,
                name,
            );
            if let Some(key) = &p.in_definition {
                text.push_str(&format!(
                    "  = note: this is in the earlier definition `{key}`\n"
                ));
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The problems in an error reply from the server, as text.
pub fn render_error(error: &Value, source: &str, name: &str) -> String {
    let problems = error["data"]["problems"].as_array();
    match problems {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|p| {
                let get = |k: &str| p[k].as_u64().map(|n| n as usize);
                let mut text = render_problem(
                    p["message"].as_str().unwrap_or("error"),
                    p["help"].as_str(),
                    get("line"),
                    get("column"),
                    source,
                    name,
                );
                if let Some(key) = p["in_definition"].as_str() {
                    text.push_str(&format!(
                        "  = note: this is in the earlier definition `{key}`\n"
                    ));
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => format!(
            "error: {}\n",
            error["message"]
                .as_str()
                .unwrap_or("the server refused that")
        ),
    }
}
