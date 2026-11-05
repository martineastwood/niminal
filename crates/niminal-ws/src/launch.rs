//! What an app needs of the macOS download it comes in: recording its crashes, and offering the editor extension.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The folder of the macOS download that the app is part of: `Niminal` beside `Niminal/Some App.app`, where
/// the VS Code extension sits.
fn download_folder(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" || app.extension()? != "app" {
        return None;
    }
    Some(app.parent()?.to_path_buf())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub use crate::crash::{defer_panic_reports, install_crash_log};

/// The editors on this Mac that take VS Code extensions, with their command line programs. The apps' own
/// copies come first because the `code` command is rarely on the PATH of a program opened from Finder.
fn editors() -> Vec<(&'static str, PathBuf)> {
    let apps = [
        ("VS Code", "Visual Studio Code.app/Contents/Resources/app/bin/code", "code"),
        ("Cursor", "Cursor.app/Contents/Resources/app/bin/cursor", "cursor"),
    ];
    let mut found = Vec::new();
    for (name, inside, program) in apps {
        let mut places = vec![PathBuf::from("/Applications").join(inside)];
        places.extend(home().map(|home| home.join("Applications").join(inside)));
        places.extend(std::env::var_os("PATH").into_iter().flat_map(|paths| std::env::split_paths(&paths).map(|dir| dir.join(program)).collect::<Vec<_>>()));
        if let Some(path) = places.into_iter().find(|path| path.is_file()) {
            found.push((name, path));
        }
    }
    found
}

/// Offer to install the niminal extension from the download into VS Code and Cursor. Unless `force` is set
/// (the menu item) it asks once, and only when an editor is installed.
pub fn offer_vscode_install(force: bool, ask: impl Fn(&str) -> bool, tell: impl Fn(&str)) {
    let Some(vsix) = std::env::current_exe().ok().and_then(|exe| Some(download_folder(&exe)?.join("niminal-vscode.vsix"))).filter(|vsix| vsix.is_file()) else {
        if force {
            tell("The VS Code extension comes with the Niminal download; this copy of the app is not part of one.");
        }
        return;
    };
    let editors = editors();
    if editors.is_empty() {
        if force {
            tell(&format!("VS Code and Cursor were not found. In your editor, run \"Extensions: Install from VSIX...\" and choose:\n\n{}", vsix.display()));
        }
        return;
    }
    let marker = home().map(|home| home.join(".niminal").join("vscode-offered"));
    if !force && marker.as_ref().is_none_or(|m| m.exists()) {
        return;
    }
    if let Some(marker) = &marker {
        let _ = std::fs::create_dir_all(marker.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(marker, "");
    }
    let names: Vec<&str> = editors.iter().map(|(name, _)| *name).collect();
    if !ask(&format!("Install the niminal extension into {}?\n\nIt adds syntax colours, errors, and playing from the editor.", names.join(" and "))) {
        return;
    }
    let results: Vec<String> = editors
        .iter()
        .map(|(name, program)| {
            let output = Command::new(program).arg("--install-extension").arg(&vsix).arg("--force").output();
            match output {
                Ok(out) if out.status.success() => format!("{name}: installed"),
                Ok(out) => format!("{name}: {}", String::from_utf8_lossy(&out.stderr).trim()),
                Err(e) => format!("{name}: {e}"),
            }
        })
        .collect();
    tell(&format!("{}\n\nReload the editor window to use it.", results.join("\n")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_found_beside_the_app_however_the_download_was_placed() {
        for root in ["/Applications/Niminal", "/Users/player/My Music/Niminal"] {
            let exe = Path::new(root).join("Niminal Stage.app/Contents/MacOS/niminal-stage");
            assert_eq!(download_folder(&exe), Some(PathBuf::from(root)));
        }
        assert_eq!(download_folder(Path::new("/repo/stage/src-tauri/target/debug/niminal-stage")), None);
    }
}
