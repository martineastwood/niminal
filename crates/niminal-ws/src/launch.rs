//! Finding the `niminal` programs that come with an app, and starting a server for the apps when none is running.

use std::fs::File;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The tools that sit beside an app in the macOS download: `Niminal/bin` next to `Niminal/Some App.app`.
fn packaged_bin(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" || app.extension()? != "app" {
        return None;
    }
    Some(app.parent()?.join("bin"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Find `program` in the macOS download that the calling app is part of, then on the PATH and in the places
/// development builds go.
pub fn find_binary(program: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let mut folders = Vec::new();
    folders.extend(exe.as_deref().and_then(packaged_bin));
    folders.push(PathBuf::from("/Applications/Niminal/bin"));
    if let Some(paths) = std::env::var_os("PATH") {
        folders.extend(std::env::split_paths(&paths));
    }
    if let Some(home) = home() {
        folders.push(home.join(".cargo").join("bin"));
    }
    if let Some(parent) = exe.as_deref().and_then(Path::parent) {
        folders.push(parent.to_path_buf());
    }
    folders.into_iter().map(|folder| folder.join(program)).find(|path| path.is_file())
}

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(200)).is_ok()
}

/// Record a panic of `app` (with its `version`) in `~/.niminal/crash.log`, beside the ones `niminal` writes, as
/// well as reporting it as before. So a crash in an app can be looked at afterwards.
pub fn install_crash_log(app: &'static str, version: &'static str) {
    let log = home().map(|home| home.join(".niminal"));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let Some(dir) = &log else { return };
        let _ = std::fs::create_dir_all(dir);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let thread = std::thread::current();
        let report = format!(
            "[{now}] {app} {version} panicked in thread `{}`: {info}\n{}\n",
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            use std::io::Write;
            let _ = file.write_all(report.as_bytes());
        }
    }));
}

/// Make sure a server is listening on `port`, starting one if not. It is started to stop itself when its last
/// client goes (`--exit-when-unused`), so no app owns it: closing one app leaves it for the others, and it is
/// never left running on its own. A server that was already running is left as it is.
pub fn ensure_server(port: u16, token: Option<&str>) -> Result<(), String> {
    if listening(port) {
        return Ok(());
    }
    let binary = find_binary("niminal").ok_or("Could not find the niminal program to start the server with.")?;
    let log = home().map(|home| home.join(".niminal")).and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        // the last run's log is kept as server.log.old, so what a crashed server said survives a restart
        let _ = std::fs::rename(dir.join("server.log"), dir.join("server.log.old"));
        File::create(dir.join("server.log")).ok()
    });
    let mut command = Command::new(binary);
    command.args(["server", "--port", &port.to_string(), "--exit-when-unused"]);
    if let Some(token) = token {
        command.args(["--token", token]);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null());
    command.stderr(log.map_or_else(Stdio::null, Stdio::from));
    // its own process group, so a Ctrl+C in the terminal an app was started from does not take it down
    let mut child = command.process_group(0).spawn().map_err(|e| format!("Could not start the niminal server: {e}"))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        if listening(port) {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("The niminal server stopped as it started ({status}). ~/.niminal/server.log says why."));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("The niminal server did not start in time. ~/.niminal/server.log may say why.".into())
}

const LINK_DIR: &str = "/usr/local/bin";
const PROGRAMS: [&str; 2] = ["niminal", "niminal-live"];

/// The `bin` folder of the macOS download the calling app is part of.
fn download_bin() -> Option<PathBuf> {
    packaged_bin(&std::env::current_exe().ok()?)
}

fn cli_linked(bin: &Path) -> bool {
    PROGRAMS.iter().all(|name| std::fs::read_link(Path::new(LINK_DIR).join(name)).is_ok_and(|to| to == bin.join(name)))
}

/// Offer to link `niminal` and `niminal-live` into `/usr/local/bin`, so Terminal and editors find them.
/// Unless `force` is set (the menu item) it asks once, and only from the macOS download with the tools not
/// linked yet. `ask` shows a yes/no question and `tell` shows a message.
pub fn offer_cli_install(force: bool, ask: impl Fn(&str) -> bool, tell: impl Fn(&str)) {
    let Some(bin) = download_bin() else {
        if force {
            tell("The command line tools are linked from the Niminal download; this copy of the app is not part of one.");
        }
        return;
    };
    let marker = home().map(|home| home.join(".niminal").join("cli-offered"));
    if !force && (cli_linked(&bin) || marker.as_ref().is_none_or(|m| m.exists())) {
        return;
    }
    if let Some(dir) = marker.as_ref().and_then(|m| m.parent()) {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Some(marker) = &marker {
        let _ = std::fs::write(marker, "");
    }
    if !ask("Install the niminal and niminal-live commands?\n\nThis links them into /usr/local/bin so Terminal and editors can find them. macOS will ask for your password.") {
        return;
    }
    match link_cli(&bin) {
        Ok(()) => tell("Installed. Open a new Terminal window and run: niminal --version"),
        Err(message) => tell(&format!("Could not install the command line tools: {message}")),
    }
}

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
    let Some(vsix) = download_bin().and_then(|bin| Some(bin.parent()?.join("niminal-vscode.vsix"))).filter(|vsix| vsix.is_file()) else {
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

fn link_cli(bin: &Path) -> Result<(), String> {
    let bin = bin.to_str().filter(|b| !b.contains(['"', '\\', '\''])).ok_or("the download's folder has a quote or backslash in its path")?;
    let links: Vec<String> = PROGRAMS.iter().map(|name| format!("ln -sf '{bin}/{name}' {LINK_DIR}/{name}")).collect();
    let script = format!("do shell script \"mkdir -p {LINK_DIR} && {}\" with administrator privileges", links.join(" && "));
    let output = Command::new("osascript").arg("-e").arg(script).output().map_err(|e| e.to_string())?;
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_follow_the_complete_download_even_when_it_is_moved() {
        for root in ["/Applications/Niminal", "/Users/player/My Music/Niminal"] {
            let exe = Path::new(root).join("Niminal Stage.app/Contents/MacOS/niminal-stage");
            assert_eq!(packaged_bin(&exe), Some(Path::new(root).join("bin")));
        }
        assert_eq!(packaged_bin(Path::new("/repo/stage/src-tauri/target/debug/niminal-stage")), None);
    }
}
