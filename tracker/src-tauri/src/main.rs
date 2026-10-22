#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use serde_json::Value;

#[derive(Default, Clone)]
struct LanguageServer(Arc<Mutex<niminal_lsp::Server>>);

// The same LSP implementation as `nimlive lsp`, without stdio framing or a child
// process. Keep compilation off the window thread; the frontend orders messages.
#[tauri::command]
async fn lsp_message(
    message: Value,
    server: tauri::State<'_, LanguageServer>,
) -> Result<Vec<Value>, String> {
    let server = server.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut server = server.0.lock().map_err(|e| e.to_string())?;
        Ok(server.handle(&message))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn open_project() -> Result<Option<String>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("niminal tracker project", &["json"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || std::fs::read_to_string(path))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_project(source: String) -> Result<bool, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("niminal tracker project", &["json"])
        .set_file_name("untitled.nimtracker.json")
        .save_file()
        .await
    else {
        return Ok(false);
    };
    file.write(source.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    Ok(true)
}

fn main() {
    tauri::Builder::default()
        .manage(LanguageServer::default())
        .invoke_handler(tauri::generate_handler![
            lsp_message,
            open_project,
            save_project
        ])
        .run(tauri::generate_context!())
        .expect("could not run niminal tracker");
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn editor_presets_have_documentation_and_report_errors() {
        let mut server = niminal_lsp::Server::new();
        let uri = "file:///niminal-tracker/tone.nml";
        for source in [
            include_str!("../../src/presets/tone.nml"),
            include_str!("../../src/presets/soften.nml"),
        ] {
            let replies = server.handle(&json!({"method":"textDocument/didOpen", "params": {
                "textDocument": {"uri":uri, "text":source, "languageId":"niminal", "version":1}
            }}));
            assert_eq!(
                replies[0]["params"]["diagnostics"],
                json!([]),
                "{replies:?}"
            );
        }
        let replies = server.handle(&json!({"method":"textDocument/didChange", "params": {
            "textDocument": {"uri":uri, "version":2},
            "contentChanges": [{"text":include_str!("../../src/presets/tone.nml")}]
        }}));
        assert_eq!(replies[0]["params"]["diagnostics"], json!([]));
        let hover = server.handle(&json!({"id":1, "method":"textDocument/hover", "params": {
            "textDocument":{"uri":uri}, "position":{"line":2,"character":7}
        }}));
        assert!(
            hover[0]["result"]["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("warm")
        );
        let replies = server.handle(&json!({"method":"textDocument/didChange", "params": {
            "textDocument":{"uri":uri,"version":3},
            "contentChanges":[{"text":"instr broken(freq: hz) { osc(sawz, freq) }"}]
        }}));
        assert!(
            !replies[0]["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
