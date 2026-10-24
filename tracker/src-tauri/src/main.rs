#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod commands;
mod engine;
mod files;
mod midi;
mod mixer;
mod modulation;
mod pattern;
mod sound;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::Value;
use tauri::{
    Emitter, Manager,
    menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
};

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

/// Asks a yes or no question in a native dialog.
#[tauri::command]
async fn confirm(message: String) -> bool {
    rfd::AsyncMessageDialog::new()
        .set_title("niminal tracker")
        .set_description(message)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        .await
        == rfd::MessageDialogResult::Yes
}

fn app_data(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// The project's working folder, where imported samples live and where the engine looks for them.
#[tauri::command]
fn project_dir(app: tauri::AppHandle, id: String) -> Result<String, String> {
    Ok(files::project_dir(&app_data(&app)?, &id)?
        .to_string_lossy()
        .into_owned())
}

/// Asks for WAV files and copies them into the project; returns their paths relative to it.
#[tauri::command]
async fn import_samples(app: tauri::AppHandle, id: String) -> Result<Vec<String>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let Some(picked) = rfd::AsyncFileDialog::new()
        .add_filter("WAV samples", &["wav"])
        .pick_files()
        .await
    else {
        return Ok(vec![]);
    };
    let paths: Vec<_> = picked.iter().map(|f| f.path().to_path_buf()).collect();
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|path| files::import_sample(&dir, path))
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Opens a project's `project.json`, bringing along the `samples` folder beside it.
#[tauri::command]
async fn open_project(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("niminal tracker project", &["json"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    let base = app_data(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if let Ok(project) = serde_json::from_str::<Value>(&text)
            && let Some(id) = project["id"].as_str()
            && let Some(parent) = path.parent()
        {
            let dir = files::project_dir(&base, id)?;
            files::copy_samples(&parent.join("samples"), &dir.join("samples"))?;
        }
        Ok(Some(text))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Asks where to write an exported WAV; None if the dialog is dismissed.
#[tauri::command]
async fn export_path() -> Option<String> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("WAV audio", &["wav"])
        .set_file_name("untitled.wav")
        .save_file()
        .await?;
    Some(file.path().with_extension("wav").to_string_lossy().into_owned())
}

/// Saves a project as a folder: `project.json` plus the samples it uses.
#[tauri::command]
async fn save_project(app: tauri::AppHandle, source: String, id: String) -> Result<bool, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_file_name("untitled.nimtracker")
        .save_file()
        .await
    else {
        return Ok(false);
    };
    let target = file.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        std::fs::write(target.join("project.json"), source).map_err(|e| e.to_string())?;
        files::copy_samples(&dir.join("samples"), &target.join("samples"))?;
        Ok(true)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The menu bar. Items send their id to the page as a `menu` event, which does the work, so the
/// menu and the keyboard run the same code. Cut, copy and paste are the system's: the grid and the
/// editors both listen for those. Select All is left out because the grid answers Cmd+A itself.
fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let item = |id: &str, text: &str, keys: Option<&str>| {
        let builder = MenuItemBuilder::with_id(id, text);
        match keys {
            Some(keys) => builder.accelerator(keys).build(app),
            None => builder.build(app),
        }
    };
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new", "New Project", Some("CmdOrCtrl+N"))?)
        .item(&item("open", "Open…", Some("CmdOrCtrl+O"))?)
        .item(&item("save", "Save…", Some("CmdOrCtrl+S"))?)
        .separator()
        .item(&item("import-samples", "Import Samples…", None)?)
        .separator()
        .item(&item("export-wav", "Export WAV…", None)?)
        .item(&item("export-stems", "Export WAV and Track Stems…", None)?)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&item("undo", "Undo", Some("CmdOrCtrl+Z"))?)
        .item(&item("redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?)
        .separator()
        .cut()
        .copy()
        .paste()
        .separator()
        .item(&item("select-track", "Select Track (⇧⌘A)", None)?)
        .build()?;
    let song = SubmenuBuilder::new(app, "Song")
        .item(&item("play-pattern", "Play Pattern", None)?)
        .item(&item("play-song", "Play Song", None)?)
        .item(&item("stop", "Stop", None)?)
        .separator()
        .item(&item("add-track", "Add Track", None)?)
        .item(&item("duplicate-track", "Duplicate Track", None)?)
        .item(&item("new-pattern", "New Pattern", None)?)
        .separator()
        .item(&item("reconnect", "Restart Engine Connection", None)?)
        .build()?;
    let sound = SubmenuBuilder::new(app, "Sound")
        .item(&item("new-instrument", "New Instrument", None)?)
        .item(&item("new-effect", "New Effect", None)?)
        .item(&item("apply", "Apply and Audition (⌘↩ in the editor)", None)?)
        .separator()
        .item(&item("browser", "Show Browser", Some("CmdOrCtrl+B"))?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("code", "Show Code", None)?)
        .item(&item("mixer", "Mixer", None)?)
        .item(&item("matrix", "Matrix", None)?)
        .separator()
        .fullscreen()
        .build()?;
    let window = SubmenuBuilder::new(app, "Window").minimize().maximize().build()?;
    let mut menu = MenuBuilder::new(app);
    if cfg!(target_os = "macos") {
        let name = SubmenuBuilder::new(app, "niminal tracker")
            .about(None)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .quit()
            .build()?;
        menu = menu.item(&name);
    }
    menu.items(&[&file, &edit, &song, &sound, &view, &window]).build()
}

fn main() {
    let app = tauri::Builder::default()
        .menu(menu)
        .on_menu_event(|app, event| {
            let _ = app.emit("menu", event.id().as_ref());
        })
        .manage(LanguageServer::default())
        .manage(audio::Audio::new())
        .manage(midi::Midi::default())
        .invoke_handler(tauri::generate_handler![
            midi::midi_ports,
            midi::midi_connect,
            lsp_message,
            confirm,
            project_dir,
            import_samples,
            export_path,
            open_project,
            save_project,
            audio::audio_command,
            audio::audio_status,
            audio::audio_scopes
        ])
        .build(tauri::generate_context!())
        .expect("could not build niminal tracker");
    app.run(|app, event| {
        // an engine the tracker started goes with it
        if let tauri::RunEvent::Exit = event {
            app.state::<audio::Audio>().quit();
        }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn editor_presets_have_documentation_and_report_errors() {
        let mut server = niminal_lsp::Server::new();
        let uri = "file:///niminal-tracker/tone.nml";
        // The tracker's own instrument and send, and every definition in the library that comes with niminal.
        let definitions = |text: &'static str| -> Vec<&'static str> {
            text.split("\n\n").filter(|block| block.lines().any(|l| l.starts_with("fx ") || l.starts_with("instr "))).collect()
        };
        let mut sources = vec![include_str!("../../src/presets/tone.nml"), include_str!("../../src/presets/send.nml")];
        sources.extend(definitions(include_str!("../../../crates/niminal-lang/std/instruments.nml")));
        sources.extend(definitions(include_str!("../../../crates/niminal-lang/std/fx.nml")));
        assert!(sources.len() >= 24);
        for source in sources {
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
