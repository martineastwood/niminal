//! Starts the niminal engine for the tracker when nothing is listening yet, so there is no server to run by hand.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// The `niminal` program: on the search path, in cargo's install folder, or beside the tracker.
fn find_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) { "niminal.exe" } else { "niminal" };
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    // a program opened from the Finder doesn't get the search path of a terminal
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        folders.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    folders.extend(std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)));
    folders.into_iter().map(|folder| folder.join(name)).find(|path| path.is_file())
}

/// Runs `niminal server` on `port`. The caller keeps the child and kills it when the tracker closes.
pub fn launch(port: u16) -> Result<Child, String> {
    let binary = find_binary().ok_or(
        "Could not find the niminal engine. Install it with `cargo install --path crates/niminal-cli`, or run `niminal server` yourself.",
    )?;
    Command::new(binary)
        .args(["server", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start the niminal engine: {e}"))
}
