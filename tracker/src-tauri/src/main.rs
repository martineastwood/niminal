#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod commands;
mod files;
mod mixer;
mod modulation;
mod pattern;
mod sound;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::Value;
use tauri::Manager;

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

fn main() {
    tauri::Builder::default()
        .manage(LanguageServer::default())
        .manage(audio::Audio::new())
        .invoke_handler(tauri::generate_handler![
            lsp_message,
            project_dir,
            import_samples,
            open_project,
            save_project,
            audio::audio_command,
            audio::audio_status,
            audio::audio_scopes
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
            include_str!("../../src/presets/delay.nml"),
            include_str!("../../src/presets/chorus.nml"),
            include_str!("../../src/presets/lowpass.nml"),
            include_str!("../../src/presets/highpass.nml"),
            include_str!("../../src/presets/bandpass.nml"),
            include_str!("../../src/presets/notch.nml"),
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
