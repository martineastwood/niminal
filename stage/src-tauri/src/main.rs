#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod project;
mod browse;
mod session;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, State};

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

/// Listen to the MIDI inputs whose names contain `filter` (all of them when empty); returns their names.
#[tauri::command]
async fn midi_open(state: State<'_, AppState>, filter: Option<String>) -> Result<Vec<String>, String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || session::open_midi(&shared, filter.as_deref().unwrap_or("")))
        .await
        .map_err(|e| e.to_string())?
}

/// The MIDI inputs and outputs there are.
#[tauri::command]
async fn midi_ports() -> Result<session::MidiPorts, String> {
    tauri::async_runtime::spawn_blocking(session::midi_ports)
        .await
        .map_err(|e| e.to_string())?
}

/// Send MIDI clock to the output called `name`, or stop when there is none.
#[tauri::command]
async fn clock_out(state: State<'_, AppState>, name: Option<String>) -> Result<(), String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || session::set_clock_out(&shared, name.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

/// Follow the MIDI clock of the input called `name`, or stop when there is none.
#[tauri::command]
async fn clock_in(state: State<'_, AppState>, name: Option<String>) -> Result<(), String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || session::set_clock_in(&shared, name.as_deref()))
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
    eval(state, source, sidebar_quantize(&action, quantize), None, None).await
}

/// When a sidebar button lands. Starting, stopping and changing a clip wait for the same moment as the
/// same words typed in the code would (the session's default, or the clip's or scene's own `quantize`),
/// unless the land box says otherwise. Only hush is immediate, since it is for stopping things now.
fn sidebar_quantize(action: &str, quantize: Option<String>) -> Option<String> {
    match action {
        "hush" => quantize.or_else(|| Some("now".into())),
        _ => quantize,
    }
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

/// A `.nml` file chosen in the file browse panel.
#[tauri::command]
async fn open_path(path: String) -> Result<OpenedFile, String> {
    let path = PathBuf::from(path);
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(OpenedFile {
        dir: path.parent().unwrap_or_else(|| Path::new(".")).to_string_lossy().into_owned(),
        path: path.to_string_lossy().into_owned(),
        source,
    })
}

/// A folder of `.nml` files: what to open as tabs, and the file to evaluate to play them.
#[derive(Serialize)]
struct OpenedProject {
    dir: String,
    entry: Option<String>,
    files: Vec<OpenedFile>,
}

#[tauri::command]
async fn open_project() -> Result<Option<OpenedProject>, String> {
    let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await else {
        return Ok(None);
    };
    let dir = folder.path().to_path_buf();
    let (entry, paths) = project::files(&dir).map_err(|e| e.to_string())?;
    if paths.is_empty() {
        return Err("no .nml files in that folder".into());
    }
    let files = paths
        .iter()
        .map(|path| {
            Ok(OpenedFile {
                path: path.to_string_lossy().into_owned(),
                dir: path.parent().unwrap_or(&dir).to_string_lossy().into_owned(),
                source: std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Some(OpenedProject {
        dir: dir.to_string_lossy().into_owned(),
        entry: entry.map(|p| p.to_string_lossy().into_owned()),
        files,
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

/// Save what was evaluated during a take as a script, for reading or for pasting back into a piece.
#[tauri::command]
async fn save_transcript(state: State<'_, AppState>, from: u64, to: u64) -> Result<Option<String>, String> {
    let text = session::transcript(&state.shared, from, to)?;
    if text.is_empty() {
        return Err("Nothing was evaluated during that take.".into());
    }
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("niminal", &["nml"])
        .set_file_name("take.nml")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Render a take to audio by replaying the engine's session log over its stretch.
#[tauri::command]
async fn render_take(state: State<'_, AppState>, from: u64, to: u64) -> Result<Option<String>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("WAV audio", &["wav"])
        .set_file_name("take.wav")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let (shared, path) = (state.shared.clone(), file.path().to_path_buf());
    tauri::async_runtime::spawn_blocking(move || session::render_take(&shared, from, to, &path))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
}

/// The piece and audio files under a folder, for the file browse panel.
#[tauri::command]
async fn list_files(dir: String) -> Result<Vec<browse::BrowseFile>, String> {
    tauri::async_runtime::spawn_blocking(move || browse::list(Path::new(&dir)))
        .await
        .map_err(|e| e.to_string())
}

/// An audio file as WAV bytes, sent as they are rather than as a list of numbers.
#[tauri::command]
async fn read_sample(path: String) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || browse::read(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
        .map(tauri::ipc::Response::new)
}

/// Ask for a folder; its path, or nothing if the dialog was closed.
#[tauri::command]
async fn pick_folder() -> Result<Option<String>, String> {
    Ok(rfd::AsyncFileDialog::new()
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().into_owned()))
}

/// The same LSP implementation as `niminal-live lsp`, without a child process. Compilation stays off the
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

/// The menu bar. Items send their id to the page as a `menu` event, which runs what its button or key
/// already does. Shortcuts the page handles itself are shown in the label, not bound here, so they
/// do not fire twice. Cut, copy, paste and select all are the system's.
fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let item = |id: &str, text: &str, keys: Option<&str>| {
        let builder = MenuItemBuilder::with_id(id, text);
        match keys {
            Some(keys) => builder.accelerator(keys).build(app),
            None => builder.build(app),
        }
    };
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new", "New Tab (⌘T)", None)?)
        .item(&item("open", "Open…", Some("CmdOrCtrl+O"))?)
        .item(&item("open-project", "Open Project Folder…", Some("CmdOrCtrl+Shift+O"))?)
        .separator()
        .item(&item("save", "Save", Some("CmdOrCtrl+S"))?)
        .item(&item("save-all", "Save All", Some("CmdOrCtrl+Alt+S"))?)
        .separator()
        .item(&item("close-tab", "Close Tab", Some("CmdOrCtrl+W"))?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&item("undo", "Undo", Some("CmdOrCtrl+Z"))?)
        .item(&item("redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("browse", "Browse (⇧⌘L)", None)?)
        .item(&item("sidebar", "Sidebar (⌘B)", None)?)
        .item(&item("perform", "Performance Mode (⇧⌘P)", None)?)
        .separator()
        .item(&item("zoom-in", "Bigger Text (⌘+)", None)?)
        .item(&item("zoom-out", "Smaller Text (⌘−)", None)?)
        .item(&item("zoom-reset", "Actual Size (⌘0)", None)?)
        .separator()
        .item(&item("previous-tab", "Previous Tab (⇧⌘[)", None)?)
        .item(&item("next-tab", "Next Tab (⇧⌘])", None)?)
        .build()?;
    let mut menu = MenuBuilder::new(app);
    if cfg!(target_os = "macos") {
        let name = SubmenuBuilder::new(app, "niminal stage")
            .hide()
            .separator()
            .quit()
            .build()?;
        menu = menu.item(&name);
    }
    menu.items(&[&file, &edit, &view]).build()
}

fn main() {
    let shared = Arc::new(session::Shared::new());
    session::start_worker(shared.clone());
    tauri::Builder::default()
        .menu(menu)
        .on_menu_event(|app, event| {
            let _ = app.emit("menu", event.id().as_ref());
        })
        .manage(AppState {
            shared: shared.clone(),
            lsp: LanguageServer::default(),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            midi_open,
            midi_ports,
            clock_out,
            clock_in,
            snapshot,
            eval,
            perform,
            cancel,
            open_file,
            open_project,
            save_file,
            save_file_as,
            list_files,
            open_path,
            read_sample,
            pick_folder,
            save_transcript,
            render_take,
            lsp_message
        ])
        .build(tauri::generate_context!())
        .expect("the stage starts")
        .run(|_, _| {});
}

#[cfg(test)]
mod tests {
    use super::sidebar_quantize;

    #[test]
    fn sidebar_buttons_wait_for_the_default_except_hush() {
        assert_eq!(sidebar_quantize("start", None), None);
        assert_eq!(sidebar_quantize("edit", None), None);
        assert_eq!(sidebar_quantize("start", Some("next bar".into())), Some("next bar".into()));
        assert_eq!(sidebar_quantize("hush", None), Some("now".into()));
    }
}
