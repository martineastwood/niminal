#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod project;
mod session;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, State};

use session::{EvalOutcome, Snapshot};

struct AppState {
    shared: Arc<session::Shared>,
    lsp: Arc<Mutex<niminal_lsp::Server>>,
}

/// Run `work` off the window thread.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn connect(state: State<'_, AppState>, port: u16, token: Option<String>) -> Result<(), String> {
    let shared = state.shared.clone();
    blocking(move || session::connect(&shared, port, token.as_deref())).await?
}

/// Listen to the MIDI inputs whose names contain `filter` (all of them when empty); returns their names.
#[tauri::command]
async fn midi_open(state: State<'_, AppState>, filter: Option<String>) -> Result<Vec<String>, String> {
    let shared = state.shared.clone();
    blocking(move || session::open_midi(&shared, filter.as_deref().unwrap_or(""))).await?
}

/// The MIDI inputs and outputs there are.
#[tauri::command]
async fn midi_ports() -> Result<session::MidiPorts, String> {
    blocking(session::midi_ports).await?
}

/// Send MIDI clock to the output called `name`, or stop when there is none.
#[tauri::command]
async fn clock_out(state: State<'_, AppState>, name: Option<String>) -> Result<(), String> {
    let shared = state.shared.clone();
    blocking(move || session::set_clock_out(&shared, name.as_deref())).await?
}

/// Follow the MIDI clock of the input called `name`, or stop when there is none.
#[tauri::command]
async fn clock_in(state: State<'_, AppState>, name: Option<String>) -> Result<(), String> {
    let shared = state.shared.clone();
    blocking(move || session::set_clock_in(&shared, name.as_deref())).await?
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let shared = state.shared.clone();
    blocking(move || session::disconnect(&shared)).await
}

#[tauri::command]
async fn snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    let shared = state.shared.clone();
    blocking(move || session::snapshot(&shared)).await
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
    blocking(move || {
        session::eval(&shared, &source, quantize.as_deref(), dir.as_deref().map(Path::new), origin.unwrap_or(0))
    })
    .await?
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
        blocking(move || session::panic(&shared)).await??;
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
    // Buttons land when the same words typed in the code would, unless the land box says otherwise.
    // Only hush is immediate, since it is for stopping things now.
    let quantize = if action == "hush" { quantize.or_else(|| Some("now".into())) } else { quantize };
    eval(state, source, quantize, None, None).await
}

#[tauri::command]
async fn cancel(state: State<'_, AppState>, id: Option<u64>) -> Result<usize, String> {
    let shared = state.shared.clone();
    blocking(move || session::cancel(&shared, id)).await?
}

/// A `.nml` file the window opened.
#[derive(Serialize)]
struct OpenedFile {
    path: String,
    source: String,
}

impl OpenedFile {
    fn read(path: &Path) -> Result<OpenedFile, String> {
        Ok(OpenedFile {
            path: path.to_string_lossy().into_owned(),
            source: std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
        })
    }
}

#[tauri::command]
async fn open_file() -> Result<Option<OpenedFile>, String> {
    match rfd::AsyncFileDialog::new().add_filter("niminal", &["nml"]).pick_file().await {
        Some(file) => OpenedFile::read(file.path()).map(Some),
        None => Ok(None),
    }
}

/// A `.nml` file chosen in the file browse panel.
#[tauri::command]
async fn open_path(path: String) -> Result<OpenedFile, String> {
    OpenedFile::read(Path::new(&path))
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
    let files = paths.iter().map(|path| OpenedFile::read(path)).collect::<Result<_, String>>()?;
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

/// Ask where to save a file, with `name` suggested; None if the dialog was closed.
async fn ask_save_path(filter: &str, extension: &str, name: &str) -> Option<PathBuf> {
    let file = rfd::AsyncFileDialog::new().add_filter(filter, &[extension]).set_file_name(name).save_file().await?;
    Some(file.path().to_path_buf())
}

/// Ask where to save `text` as a `.nml` file and write it there; the path, or None if the dialog was closed.
async fn save_nml_as(text: String, name: &str) -> Result<Option<String>, String> {
    let Some(path) = ask_save_path("niminal", "nml", name).await else {
        return Ok(None);
    };
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn save_file_as(source: String) -> Result<Option<String>, String> {
    save_nml_as(source, "piece.nml").await
}

/// Save what was evaluated during a take as a script, for reading or for pasting back into a piece.
#[tauri::command]
async fn save_transcript(state: State<'_, AppState>, from: u64, to: u64) -> Result<Option<String>, String> {
    let text = session::transcript(&state.shared, from, to)?;
    if text.is_empty() {
        return Err("Nothing was evaluated during that take.".into());
    }
    save_nml_as(text, "take.nml").await
}

/// Render a take to audio by replaying the engine's session log over its stretch.
#[tauri::command]
async fn render_take(state: State<'_, AppState>, from: u64, to: u64) -> Result<Option<String>, String> {
    let Some(path) = ask_save_path("WAV audio", "wav", "take.wav").await else {
        return Ok(None);
    };
    let shared = state.shared.clone();
    blocking(move || session::render_take(&shared, from, to, &path)).await?.map(Some)
}

/// The piece and audio files under a folder, for the file browse panel.
#[tauri::command]
async fn list_files(dir: String) -> Result<Vec<String>, String> {
    blocking(move || {
        niminal_lang::files_under(Path::new(&dir), |path| {
            niminal_lang::is_audio(path) || path.extension().is_some_and(|e| e == "nml")
        })
    })
    .await
}

/// An audio file as WAV bytes, sent as they are rather than as a list of numbers.
#[tauri::command]
async fn read_sample(path: String) -> Result<tauri::ipc::Response, String> {
    blocking(move || niminal_lang::audition_wav(Path::new(&path))).await?.map(tauri::ipc::Response::new)
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
    blocking(move || Ok(server.lock().map_err(|e| e.to_string())?.handle(&message))).await?
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
            lsp: Arc::default(),
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
