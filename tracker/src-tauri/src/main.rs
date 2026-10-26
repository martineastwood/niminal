#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod commands;
mod engine;
mod files;
mod midi;
mod mixer;
mod modulation;
mod pattern;
mod slicing;
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

// The same LSP implementation as `niminal-live lsp`, without stdio framing or a child
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

/// A sample copied into the project: its path relative to the project folder, and how long it plays.
#[derive(serde::Serialize)]
struct ImportedSample {
    file: String,
    seconds: f64,
}

/// Asks for audio files and copies them into the project; returns them with how long each plays.
#[tauri::command]
async fn import_samples(app: tauri::AppHandle, id: String) -> Result<Vec<ImportedSample>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let Some(picked) = rfd::AsyncFileDialog::new()
        .add_filter("Audio samples", niminal_lang::AUDIO_EXTENSIONS)
        .pick_files()
        .await
    else {
        return Ok(vec![]);
    };
    let paths: Vec<_> = picked.iter().map(|f| f.path().to_path_buf()).collect();
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|path| {
                let file = files::import_sample(&dir, path)?;
                let seconds = slicing::seconds(&dir, &file).unwrap_or(0.0);
                Ok(ImportedSample { file, seconds })
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A folder of samples copied into the project to play as a kit.
#[derive(serde::Serialize)]
struct ImportedKit {
    folder: String,
    members: Vec<String>,
}

/// Asks for audio files and copies them into a kit folder of the project, named for the folder they came from.
#[tauri::command]
async fn import_kit(app: tauri::AppHandle, id: String) -> Result<Option<ImportedKit>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let Some(picked) = rfd::AsyncFileDialog::new()
        .set_title("Choose the samples of the kit")
        .add_filter("Audio samples", niminal_lang::AUDIO_EXTENSIONS)
        .pick_files()
        .await
    else {
        return Ok(None);
    };
    let paths: Vec<_> = picked.iter().map(|f| f.path().to_path_buf()).collect();
    let name = paths.first().and_then(|p| p.parent()).and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("kit").to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let (folder, members) = files::import_kit(&dir, &paths, &name)?;
        Ok(Some(ImportedKit { folder, members }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The waveform of an imported sample, for drawing.
#[tauri::command]
async fn sample_peaks(app: tauri::AppHandle, id: String, file: String, buckets: usize) -> Result<slicing::Peaks, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    tauri::async_runtime::spawn_blocking(move || slicing::peaks(&dir, &file, buckets))
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

/// Where the hits of an imported sample begin, as shares of its length, for cutting it into pieces there.
#[tauri::command]
async fn detect_slices(app: tauri::AppHandle, id: String, file: String, max: usize) -> Result<Vec<f64>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    tauri::async_runtime::spawn_blocking(move || slicing::detect(&dir, &file, max))
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
        .item(&item("import-kit", "Import Samples as a Kit…", None)?)
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
        .item(&item("cut-track", "Cut Track", None)?)
        .separator()
        .item(&item("reverse-selection", "Reverse Selection", None)?)
        .item(&item("randomize-selection", "Randomize Selection", None)?)
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
        .item(&item("sample", "Sample", None)?)
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
            import_kit,
            sample_peaks,
            export_path,
            detect_slices,
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
    #[test]
    fn the_code_the_tracker_writes_for_samplers_and_slices_has_no_editor_errors() {
        // a real file for the sample, so only the code itself can be wrong
        let dir = std::env::temp_dir().join(format!("niminal-tracker-slices-lsp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("samples")).unwrap();
        let mut wav = Vec::new();
        wav.extend(b"RIFF"); wav.extend((36u32 + 2000).to_le_bytes()); wav.extend(b"WAVEfmt "); wav.extend(16u32.to_le_bytes());
        wav.extend([1, 0, 1, 0]); wav.extend(48000u32.to_le_bytes()); wav.extend(96000u32.to_le_bytes()); wav.extend([2, 0, 16, 0]);
        wav.extend(b"data"); wav.extend(2000u32.to_le_bytes()); wav.extend(vec![0u8; 2000]);
        std::fs::write(dir.join("samples/amen.wav"), wav).unwrap();
        let mut server = niminal_lsp::Server::new();
        let uri = format!("file://{}/slices.nml", dir.display());
        let head = "/// Plays samples/amen.wav in pieces.\namen_sample = sample(\"samples/amen.wav\").root(c4)\n";
        let wrapper = "instr amen(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1, stretch: 0.25..4 = 1, stretch_mode: 0..1 = 0, from_bpm: 0..400 = 0, gate: 0..1 = 1, release: 0ms..2sec = 10ms) {\n  amen_slices(sample: sample, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode) * (1 - gate + gate * env[1 | release 0])\n}\n";
        for cut in ["4", "[0, 0.18, 0.43]", "[0, 0.18, 0.43, 0.99]"] {
            let source = format!("{head}amen_slices = amen_sample.slices({cut})\n{wrapper}");
            let replies = server.handle(&json!({"method":"textDocument/didOpen", "params": {
                "textDocument": {"uri":uri, "text":source, "languageId":"niminal", "version":1}
            }}));
            assert_eq!(replies[0]["params"]["diagnostics"], json!([]), "{cut}");
            server.handle(&json!({"method":"textDocument/didClose", "params": {"textDocument": {"uri":uri}}}));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
