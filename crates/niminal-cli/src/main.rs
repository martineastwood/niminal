use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use clap::{Parser, Subcommand};
use niminal_cli::audio::{NullClock, Output, Shared, lock};
use niminal_cli::live::{DEFAULT_SAMPLE_RATE, layout_for, render_error, render_problems, replay};
use niminal_cli::render::{RenderOptions, SAMPLE_RATE, render, write_wav};
use niminal_cli::server::{Client, Server, ServerConfig};
use niminal_daemon::{Daemon, Session, from_lines, to_lines};
use niminal_score::Section;
use serde_json::json;

#[derive(Parser)]
#[command(name = "niminal", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a niminal file to a WAV file, or replay a recorded live session.
    Render {
        /// The .nml source to render.
        #[arg(required_unless_present = "replay", conflicts_with = "replay")]
        file: Option<PathBuf>,
        /// A .nms score whose events are played on the file's instruments.
        #[arg(long, requires = "file")]
        score: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Turn off the master limiter (offline renders only).
        #[arg(long)]
        no_limiter: bool,
        /// Render this many bars of a piece that plays until stopped.
        #[arg(long, conflicts_with = "seconds", requires = "file")]
        bars: Option<f64>,
        /// Render this many seconds of a piece that plays until stopped.
        #[arg(long, requires = "file")]
        seconds: Option<f64>,
        /// Re-render a live set from the log a daemon recorded with `--log`.
        #[arg(long)]
        replay: Option<PathBuf>,
        /// The channels of the daemon that recorded the log.
        #[arg(long, default_value_t = 2, requires = "replay")]
        channels: u16,
    },
    /// Run the live daemon: plays sound, and takes code over a WebSocket.
    Daemon {
        /// A project to load, and to load again whenever it is saved.
        file: Option<PathBuf>,
        #[arg(long, default_value_t = 7400)]
        port: u16,
        /// Output channels: 1, 2 (stereo), 4, 6 (5.1) or 8 (7.1).
        #[arg(long, default_value_t = 2)]
        channels: u16,
        #[arg(long)]
        sample_rate: Option<u32>,
        /// Run without a sound card, at real speed, throwing the audio away.
        #[arg(long)]
        no_audio: bool,
        /// Record everything sent to the session, to replay later.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Require clients to give this token when they say hello.
        #[arg(long)]
        token: Option<String>,
        /// The address to listen on. Only local addresses are accepted for now.
        #[arg(long, default_value = "127.0.0.1")]
        listen: IpAddr,
    },
    /// Send code to a running daemon.
    Send {
        /// The code to evaluate. With no code and no --file, it is read from the standard input.
        code: Option<String>,
        #[arg(long, conflicts_with = "code")]
        file: Option<PathBuf>,
        #[arg(long, default_value_t = 7400)]
        port: u16,
        /// When it should land, as in `now` or `next 4 bars`.
        #[arg(long)]
        quantize: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Type code at a running daemon, one statement at a time.
    Repl {
        #[arg(long, default_value_t = 7400)]
        port: u16,
        /// When each statement should land, as in `now` or `next 4 bars`.
        #[arg(long)]
        quantize: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Render { file: Some(file), score, out, no_limiter, bars, seconds, .. } => {
            run_render(&file, score.as_deref(), &out, no_limiter, bars, seconds)
        }
        Command::Render { replay: Some(log), out, no_limiter, channels, .. } => {
            run_replay(&log, &out, channels, no_limiter)
        }
        Command::Render { .. } => Err("error: give a file to render, or --replay a log".into()),
        Command::Daemon { file, port, channels, sample_rate, no_audio, log, token, listen } => {
            run_daemon(file.as_deref(), port, channels, sample_rate, no_audio, log.as_deref(), token, listen)
        }
        Command::Send { code, file, port, quantize, token } => {
            run_send(code, file.as_deref(), port, quantize.as_deref(), token.as_deref())
        }
        Command::Repl { port, quantize, token } => run_repl(port, quantize, token.as_deref()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprint!("{message}");
            if !message.ends_with('\n') {
                eprintln!();
            }
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("error: can't read {}: {e}", path.display()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn run_render(
    file: &Path,
    score: Option<&Path>,
    out: &Path,
    no_limiter: bool,
    bars: Option<f64>,
    seconds: Option<f64>,
) -> Result<(), String> {
    let source = read(file)?;
    let name = file.display().to_string();
    let program = niminal_lang::compile(&source)
        .map_err(|errors| errors.iter().map(|d| d.render(&name, &source)).collect::<Vec<_>>().join("\n"))?;

    let bar_seconds = program.tempo.beats_per_bar * 60.0 / program.tempo.bpm;
    let until = match (bars, seconds) {
        (Some(b), _) if b > 0.0 => Some(b * bar_seconds),
        (_, Some(s)) if s > 0.0 => Some(s),
        (None, None) => None,
        _ => return Err("error: --bars and --seconds must be above zero".into()),
    };
    let options = RenderOptions { limiter: !no_limiter, until };

    let events = match score {
        None => Vec::new(),
        Some(path) => {
            let section = Section::from_json(&read(path)?)
                .map_err(|e| format!("error: {}: {e}", path.display()))?;
            section.events
        }
    };

    let rendered = render(&program, &events, &options).map_err(|e| format!("error: {e}"))?;
    if rendered.silenced_voices > 0 {
        eprintln!("warning: {} voice(s) produced NaN or infinity and were silenced", rendered.silenced_voices);
    }
    if rendered.is_empty() {
        return Err("error: nothing to render: the file plays no notes".into());
    }

    let peak = rendered.peak();
    if peak > 1.0 {
        eprintln!("warning: output peaks at {peak:.2}, which will clip (the limiter is off)");
    }
    write_wav(out, &rendered.channels).map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!(
        "wrote {} ({:.2}s, {} channel{})",
        out.display(),
        rendered.len() as f64 / f64::from(SAMPLE_RATE),
        rendered.channels.len(),
        plural(rendered.channels.len())
    );
    Ok(())
}

fn run_replay(log: &Path, out: &Path, channels: u16, no_limiter: bool) -> Result<(), String> {
    let entries = from_lines(&read(log)?).map_err(|e| format!("error: {}: {e}", log.display()))?;
    let audio = replay(&entries, channels, DEFAULT_SAMPLE_RATE, !no_limiter)?;
    if audio[0].is_empty() {
        return Err("error: that session made no sound".into());
    }
    write_wav(out, &audio).map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!(
        "replayed {} input{}: wrote {} ({:.2}s, {} channel{})",
        entries.len(),
        plural(entries.len()),
        out.display(),
        audio[0].len() as f64 / f64::from(DEFAULT_SAMPLE_RATE),
        audio.len(),
        plural(audio.len())
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_daemon(
    file: Option<&Path>,
    port: u16,
    channels: u16,
    sample_rate: Option<u32>,
    no_audio: bool,
    log: Option<&Path>,
    token: Option<String>,
    listen: IpAddr,
) -> Result<(), String> {
    let layout = layout_for(channels)?;
    let output = if no_audio { None } else { Some(Output::open(channels, sample_rate)?) };
    let rate = output.as_ref().map(Output::sample_rate).or(sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE);
    let where_to = output.as_ref().map_or_else(|| "no audio output".to_string(), |o| format!("audio to {}", o.name()));

    let daemon: Shared = Arc::new(Mutex::new(Daemon::new(Session::new(rate as f32, layout))));
    let _audio = match output {
        Some(o) => Some(o.start(daemon.clone())?),
        None => None,
    };
    let _clock = no_audio.then(|| NullClock::start(daemon.clone(), 1.0));
    let server = Server::start(daemon.clone(), ServerConfig { listen, port, token })?;
    println!("niminal daemon listening on ws://{listen}:{} ({layout}, {rate} Hz, {where_to})", server.port());

    let mut watched = None;
    if let Some(path) = file {
        watched = Some(load(&daemon, path, Some("now")));
    }
    let mut written = 0;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        if let (Some(path), Some(last)) = (file, watched.as_mut()) {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if modified.is_some() && modified != *last {
                *last = load(&daemon, path, None);
            }
        }
        if let Some(path) = log {
            let d = lock(&daemon);
            let entries = d.session().log();
            if entries.len() != written {
                written = entries.len();
                let _ = std::fs::write(path, to_lines(entries));
            }
        }
    }
}

/// Evaluate a project file, reporting what happened. Returns its modification
/// time, to notice when it changes again.
fn load(daemon: &Shared, path: &Path, quantize: Option<&str>) -> Option<SystemTime> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let name = path.display().to_string();
    let source = match read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return modified;
        }
    };
    let result = lock(daemon).session_mut().eval(&source, quantize);
    match result {
        Ok(a) => match a.id {
            Some(_) => println!("{name}: lands at bar {} beat {:.2}", a.position.0, a.position.1),
            None => println!("{name}: nothing to do"),
        },
        Err(problems) => eprint!("{}", render_problems(&problems, &source, &name)),
    }
    modified
}

fn run_send(
    code: Option<String>,
    file: Option<&Path>,
    port: u16,
    quantize: Option<&str>,
    token: Option<&str>,
) -> Result<(), String> {
    let (source, name) = match (code, file) {
        (Some(c), _) => (c, "<code>".to_string()),
        (None, Some(path)) => (read(path)?, path.display().to_string()),
        (None, None) => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|e| format!("error: can't read the standard input: {e}"))?;
            (text, "<stdin>".to_string())
        }
    };
    let mut client = Client::connect(port, token).map_err(|e| format!("error: {e}"))?;
    println!("{}", eval(&mut client, &source, quantize, &name)?);
    Ok(())
}

/// Evaluate code on the daemon, describing when it lands.
fn eval(client: &mut Client, source: &str, quantize: Option<&str>, name: &str) -> Result<String, String> {
    let mut params = json!({ "source": source });
    if let Some(q) = quantize {
        params["quantize"] = json!(q);
    }
    match client.call_raw("eval", params).map_err(|e| format!("error: {e}"))? {
        Ok(result) => Ok(if result["id"].is_null() {
            "nothing to do".to_string()
        } else if result["in_seconds"].as_f64().unwrap_or(0.0) == 0.0 {
            "applied".to_string()
        } else {
            format!(
                "lands at bar {} beat {:.2} (in {:.1}s)",
                result["position"]["bar"],
                result["position"]["beat"].as_f64().unwrap_or(1.0),
                result["in_seconds"].as_f64().unwrap_or(0.0)
            )
        }),
        Err(error) => Err(render_error(&error, source, name)),
    }
}

/// How many brackets a line leaves open.
fn open_brackets(line: &str) -> i32 {
    let code = line.split("//").next().unwrap_or("");
    code.chars().fold(0, |depth, c| match c {
        '{' | '[' | '(' => depth + 1,
        '}' | ']' | ')' => depth - 1,
        _ => depth,
    })
}

/// A statement runs once its brackets are closed, so a block can be typed
/// over several lines. `:quantize WHEN` (or `:quantize off`) sets when
/// statements land, and `:quit` leaves.
fn run_repl(port: u16, mut quantize: Option<String>, token: Option<&str>) -> Result<(), String> {
    use std::io::{BufRead, IsTerminal, Write};

    let mut client = Client::connect(port, token).map_err(|e| format!("error: {e}"))?;
    let interactive = std::io::stdin().is_terminal();
    let prompt = |depth: i32| {
        if interactive {
            print!("{}", if depth > 0 { "     ... " } else { "niminal> " });
            let _ = std::io::stdout().flush();
        }
    };
    if interactive {
        println!("connected to the daemon on port {port}. :quantize WHEN sets when code lands, :quit leaves.");
    }

    let mut pending = String::new();
    let mut depth = 0;
    prompt(depth);
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| format!("error: can't read the standard input: {e}"))?;
        let trimmed = line.trim();
        if depth == 0 && trimmed.starts_with(':') {
            match trimmed.split_once(' ').unwrap_or((trimmed, "")) {
                (":quit" | ":q", _) => break,
                (":quantize", "off" | "") => quantize = None,
                (":quantize", when) => quantize = Some(when.trim().to_string()),
                _ => eprintln!("error: unknown command `{trimmed}`"),
            }
        } else if !(depth == 0 && trimmed.is_empty()) {
            pending.push_str(&line);
            pending.push('\n');
            depth += open_brackets(&line);
            if depth <= 0 {
                depth = 0;
                match eval(&mut client, &pending, quantize.as_deref(), "<repl>") {
                    Ok(message) => println!("{message}"),
                    Err(message) => eprint!("{message}"),
                }
                pending.clear();
            }
        }
        prompt(depth);
    }
    Ok(())
}
