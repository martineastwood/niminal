#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod session;

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, State};

use session::{EvalOutcome, Snapshot};

#[derive(Clone, Default)]
struct LanguageServer(Arc<Mutex<niminal_lsp::Server>>);

#[derive(Clone)]
struct AppState {
    shared: Arc<session::Shared>,
    lsp: LanguageServer,
}

#[tauri::command]
async fn connect(state: State<'_, AppState>, port: u16, token: Option<String>) -> Result<(), String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || {
        session::connect(&shared, port, token.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || {
        session::disconnect(&shared);
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || session::snapshot(&shared))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn eval(
    state: State<'_, AppState>,
    source: String,
    quantize: Option<String>,
    dir: Option<String>,
    origin: Option<u64>,
) -> Result<EvalOutcome, String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || {
        session::eval(
            &shared,
            &source,
            quantize.as_deref(),
            dir.as_deref().map(std::path::Path::new),
            origin.unwrap_or(0),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start, stop, mute and solo clips and scenes by name, as the sidebar's buttons do.
#[tauri::command]
async fn perform(
    state: State<'_, AppState>,
    action: String,
    target: Option<String>,
    change: Option<String>,
    quantize: Option<String>,
) -> Result<EvalOutcome, String> {
    if action == "panic" {
        let shared = state.shared.clone();
        tauri::async_runtime::spawn_blocking(move || session::panic(&shared))
            .await
            .map_err(|e| e.to_string())??;
        return Ok(EvalOutcome {
            accepted: None,
            problems: Vec::new(),
        });
    }
    let source = match (action.as_str(), target.as_deref(), change.as_deref()) {
        ("hush", _, _) => "hush".to_string(),
        ("unsolo", None, _) => "unsolo".to_string(),
        ("edit", Some(target), Some(change)) => format!("{target}.{change}"),
        ("edit", _, _) => {
            return Err("`edit` needs a clip and a change".into());
        }
        (_, Some(target), _) => format!("{target}.{action}"),
        (_, None, _) => {
            return Err(format!("`{action}` needs a clip or scene to act on"));
        }
    };
    eval(state, source, quantize.or_else(|| Some("now".into())), None, None).await
}

#[tauri::command]
async fn cancel(state: State<'_, AppState>, id: Option<u64>) -> Result<usize, String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || session::cancel(&shared, id))
        .await
        .map_err(|e| e.to_string())?
}

/// A `.nml` file the window opened, and where its samples and modules are found.
#[derive(Serialize)]
struct OpenedFile {
    path: String,
    dir: String,
    source: String,
}

#[tauri::command]
async fn open_file() -> Result<Option<OpenedFile>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("niminal", &["nml"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(Some(OpenedFile {
        path: path.to_string_lossy().into_owned(),
        dir: path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned(),
        source,
    }))
}

#[tauri::command]
async fn save_file(path: String, source: String) -> Result<(), String> {
    std::fs::write(path, source).map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_file_as(source: String) -> Result<Option<String>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("niminal", &["nml"])
        .set_file_name("piece.nml")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    std::fs::write(&path, source).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// The same LSP implementation as `nimlive lsp`, without a child process. Compilation stays off the
/// window thread and the frontend keeps messages in document order.
#[tauri::command]
async fn lsp_message(
    message: Value,
    state: State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    let server = state.lsp.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut server = server.0.lock().map_err(|e| e.to_string())?;
        Ok(server.handle(&message))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    let shared = Arc::new(session::Shared::new());
    session::start_worker(shared.clone());
    tauri::Builder::default()
        .manage(AppState {
            shared: shared.clone(),
            lsp: LanguageServer::default(),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            snapshot,
            eval,
            perform,
            cancel,
            open_file,
            save_file,
            save_file_as,
            lsp_message
        ])
        .build(tauri::generate_context!())
        .expect("the stage starts")
        .run(move |app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<AppState>().shared.shutdown();
            }
        });
}
