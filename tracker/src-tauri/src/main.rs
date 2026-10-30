#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod browse;
mod commands;
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

/// Copies audio files into the project; returns them with how long each plays.
fn import_files(dir: &std::path::Path, paths: &[PathBuf]) -> Result<Vec<ImportedSample>, String> {
    paths
        .iter()
        .map(|path| {
            let file = files::import_sample(dir, path)?;
            let seconds = slicing::seconds(dir, &file).unwrap_or(0.0);
            Ok(ImportedSample { file, seconds })
        })
        .collect()
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
    tauri::async_runtime::spawn_blocking(move || import_files(&dir, &paths))
        .await
        .map_err(|e| e.to_string())?
}

/// What was dropped on the window, without anything that is not an audio file.
fn audio_files(mut paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    paths.retain(|path| niminal_lang::is_audio(path));
    if paths.is_empty() { Err("Drop audio files: WAV, FLAC, MP3, OGG or AIFF".into()) } else { Ok(paths) }
}

/// Copies audio files that were dropped on the window into the project.
#[tauri::command]
async fn import_dropped(app: tauri::AppHandle, id: String, paths: Vec<PathBuf>) -> Result<Vec<ImportedSample>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let paths = audio_files(paths)?;
    tauri::async_runtime::spawn_blocking(move || import_files(&dir, &paths))
        .await
        .map_err(|e| e.to_string())?
}

/// Deletes the samples of the project that no instrument plays, after asking. `code` is everything in the project that
/// may name a sample. Returns how many were deleted.
#[tauri::command]
async fn clean_samples(app: tauri::AppHandle, id: String, code: String) -> Result<usize, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let unused = { let dir = dir.clone(); tauri::async_runtime::spawn_blocking(move || files::unused_samples(&dir, &code)).await.map_err(|e| e.to_string())? };
    if unused.is_empty() {
        return Ok(0);
    }
    let shown = unused.iter().take(12).map(|f| f.trim_start_matches("samples/")).collect::<Vec<_>>().join("\n");
    let more = if unused.len() > 12 { format!("\n…and {} more", unused.len() - 12) } else { String::new() };
    let answer = rfd::AsyncMessageDialog::new()
        .set_title("Remove unused samples")
        .set_description(format!("Delete {} sample{} that no instrument plays? This cannot be undone.\n\n{shown}{more}", unused.len(), if unused.len() == 1 { "" } else { "s" }))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        .await;
    if answer != rfd::MessageDialogResult::Yes {
        return Ok(0);
    }
    let count = unused.len();
    tauri::async_runtime::spawn_blocking(move || files::delete_samples(&dir, &unused)).await.map_err(|e| e.to_string())??;
    Ok(count)
}

/// The audio files under a folder of the sample library.
#[tauri::command]
async fn list_library(dir: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || browse::list(std::path::Path::new(&dir)))
        .await
        .map_err(|e| e.to_string())
}

/// A sample of the library as WAV bytes, for the window to audition.
#[tauri::command]
async fn read_sample(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || browse::read(std::path::Path::new(&path)))
        .await
        .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Asks for the folder the sample library shows.
#[tauri::command]
async fn pick_library() -> Option<String> {
    rfd::AsyncFileDialog::new().set_title("Choose a folder of samples").pick_folder().await.map(|f| f.path().to_string_lossy().into_owned())
}

/// A folder of samples copied into the project to play as a kit.
#[derive(serde::Serialize)]
struct ImportedKit {
    folder: String,
    members: Vec<String>,
}

/// Copies audio files into a kit folder of the project, named for the folder they came from.
fn import_kit_files(dir: &std::path::Path, paths: &[PathBuf]) -> Result<Option<ImportedKit>, String> {
    let name = paths.first().and_then(|p| p.parent()).and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("kit");
    let (folder, members) = files::import_kit(dir, paths, name)?;
    Ok(Some(ImportedKit { folder, members }))
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
    tauri::async_runtime::spawn_blocking(move || import_kit_files(&dir, &paths))
        .await
        .map_err(|e| e.to_string())?
}

/// Copies audio files that were dropped on the window into a kit folder of the project.
#[tauri::command]
async fn import_dropped_kit(app: tauri::AppHandle, id: String, paths: Vec<PathBuf>) -> Result<Option<ImportedKit>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    let paths = audio_files(paths)?;
    tauri::async_runtime::spawn_blocking(move || import_kit_files(&dir, &paths))
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
async fn detect_slices(app: tauri::AppHandle, id: String, file: String, max: usize, sensitivity: Option<f32>) -> Result<Vec<f64>, String> {
    let dir = files::project_dir(&app_data(&app)?, &id)?;
    tauri::async_runtime::spawn_blocking(move || slicing::detect(&dir, &file, max, sensitivity.unwrap_or(0.5)))
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
        .item(&item("clean-samples", "Remove Unused Samples…", None)?)
        .separator()
        .item(&item("export-wav", "Export WAV…", None)?)
        .item(&item("export-stems", "Export WAV and Track Stems…", None)?)
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
        .item(&item("toggle-sequence", "Show or Hide Sequence Bar", None)?)
        .item(&item("toggle-side", "Show or Hide Right Bar", None)?)
        .item(&item("focus-mode", "Focus Mode", None)?)
        .separator()
        .item(&item("zoom-in", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&item("zoom-out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&item("zoom-reset", "Actual Size", Some("CmdOrCtrl+0"))?)
        .item(&item("code-font-larger", "Larger Code Font", None)?)
        .item(&item("code-font-smaller", "Smaller Code Font", None)?)
        .item(&item("contrast", "High Contrast", None)?)
        .separator()
        .item(&item("palette", "Command Palette… (⌘K)", None)?)
        .item(&item("shortcuts", "Keyboard Shortcuts (⌘/)", None)?)
        .build()?;
    let mut menu = MenuBuilder::new(app);
    if cfg!(target_os = "macos") {
        let name = SubmenuBuilder::new(app, "niminal tracker")
            .hide()
            .separator()
            .quit()
            .build()?;
        menu = menu.item(&name);
    }
    menu.items(&[&file, &edit, &song, &sound, &view]).build()
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
            import_dropped,
            list_library,
            clean_samples,
            read_sample,
            pick_library,
            import_kit,
            import_dropped_kit,
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
    fn editor_understands_macros_and_instrumentless_gestures() {
        let mut server = niminal_lsp::Server::new();
        let uri = "file:///niminal-tracker/gesture.nml";
        let source = "fx gate(open: 0..1 = 0) { out = in * open }\nmacro lift {\n  gate.open: 0..1\n}\nclip hit {\n  length: 1bar\n  times: 1\n  gate {\n    open: .... xxxx .... ....\n  }\n}\nhit.start\n";
        let replies = server.handle(&json!({"method":"textDocument/didOpen", "params": {
            "textDocument":{"uri":uri, "text":source, "languageId":"niminal", "version":1}
        }}));
        assert_eq!(replies[0]["params"]["diagnostics"], json!([]), "{replies:?}");
        let replies = server.handle(&json!({"id":1, "method":"textDocument/semanticTokens/full", "params": {
            "textDocument":{"uri":uri}
        }}));
        assert!(!replies[0]["result"]["data"].as_array().unwrap().is_empty());
    }

    #[test]
    fn editor_presets_have_documentation_and_report_errors() {
        let mut server = niminal_lsp::Server::new();
        let uri = "file:///niminal-tracker/tone.nml";
        // The tracker's own instrument and send, and every definition in the library that comes with niminal.
        let mut sources = vec![include_str!("../../src/presets/tone.nml"), include_str!("../../src/presets/send.nml")];
        sources.extend(niminal_lang::presets::all().iter().filter(|p| p.shelf != "std/drums").map(|p| p.source.as_str()));
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
