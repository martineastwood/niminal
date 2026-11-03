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
