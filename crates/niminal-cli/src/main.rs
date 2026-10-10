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
#[command(name = "niminal", version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("NIMINAL_BUILD"), ")"))]
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
        /// An `arrangement` in the file to render, in place of its `play` and `launch` commands.
        #[arg(requires = "file")]
        arrangement: Option<String>,
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
        /// Start the audio at this position, such as `bar 64` (an arrangement or timeline).
        #[arg(long, requires = "file")]
        from: Option<String>,
        /// Stop starting notes at this position, such as `bar 80`. Their tails still ring out.
        #[arg(long, requires = "file", conflicts_with_all = ["bars", "seconds"])]
        to: Option<String>,
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
        /// Record everything sent to the session here, to replay later. By default
        /// it goes in `~/.niminal/sessions`.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Don't record the session.
        #[arg(long, conflicts_with = "log")]
        no_log: bool,
        /// Require clients to give this token when they say hello.
        #[arg(long)]
        token: Option<String>,
        /// The address to listen on. Only local addresses are accepted for now.
        #[arg(long, default_value = "127.0.0.1")]
        listen: IpAddr,
        /// Take MIDI control changes from every input whose name contains this
        /// (give an empty name for all inputs), for `ctl x = midi.cc(n)`.
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
        midi: Option<String>,
        /// Take OSC over UDP on this local port, for `ctl x = osc_in("/address")`.
        #[arg(long, value_name = "PORT")]
        osc_port: Option<u16>,
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
        Command::Render { file: Some(file), arrangement, score, out, no_limiter, bars, seconds, from, to, .. } => {
            run_render(&file, arrangement, score.as_deref(), &out, no_limiter, bars, seconds, from.as_deref(), to.as_deref())
        }
        Command::Render { replay: Some(log), out, no_limiter, channels, .. } => {
            run_replay(&log, &out, channels, no_limiter)
        }
        Command::Render { .. } => Err("error: give a file to render, or --replay a log".into()),
        Command::Daemon { file, port, channels, sample_rate, no_audio, log, no_log, token, listen, midi, osc_port } => {
            install_crash_log();
            let log = log.or_else(|| {
                if no_log {
                    return None;
                }
                let dir = state_dir()?.join("sessions");
                prune_sessions(&dir, 20);
                let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                Some(dir.join(format!("{now}.log")))
            });
            run_daemon(file.as_deref(), port, channels, sample_rate, no_audio, log.as_deref(), token, listen, midi.as_deref(), osc_port)
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

/// `~/.niminal`, where sessions are logged and crashes recorded.
fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let dir = PathBuf::from(home).join(".niminal");
    std::fs::create_dir_all(dir.join("sessions")).ok()?;
    Some(dir)
}

/// Record panics in `~/.niminal/crash.log` (as well as printing them) so that a
/// crash during a performance can be looked at afterwards.
fn install_crash_log() {
    let log = state_dir().map(|d| d.join("crash.log"));
    std::panic::set_hook(Box::new(move |info| {
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let thread = std::thread::current();
        let report = format!(
            "[{now}] niminal {} panicked in thread `{}`: {info}\n{}\n",
            env!("NIMINAL_BUILD"),
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        eprintln!("niminal: internal error: {info}");
        if let Some(path) = &log
            && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
        {
            use std::io::Write;
            let _ = file.write_all(report.as_bytes());
            eprintln!("niminal: details are in {}", path.display());
        }
    }));
}

/// The newest session logs are kept, the rest removed.
fn prune_sessions(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut logs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    logs.sort();
    for old in logs.iter().take(logs.len().saturating_sub(keep)) {
        let _ = std::fs::remove_file(old);
    }
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("error: can't read {}: {e}", path.display()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A position such as `bar 64` or `64`, as seconds from the start.
fn position_seconds(text: &str, bar_seconds: f64) -> Result<f64, String> {
    let number = text.trim().strip_prefix("bar").unwrap_or(text).trim();
    match number.parse::<f64>() {
        Ok(bar) if bar >= 1.0 => Ok((bar - 1.0) * bar_seconds),
        _ => Err(format!("error: `{text}` isn't a position: write `bar 64`, counting from bar 1")),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_render(
    file: &Path,
    arrangement: Option<String>,
    score: Option<&Path>,
    out: &Path,
    no_limiter: bool,
    bars: Option<f64>,
    seconds: Option<f64>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<(), String> {
    let source = read(file)?;
    let name = file.display().to_string();
    let options = niminal_lang::CompileOptions {
        samples: niminal_lang::Samples::new(file.parent().unwrap_or(Path::new("."))),
        ..Default::default()
    };
    let program = niminal_lang::compile_with(&source, &options)
        .map_err(|errors| errors.iter().map(|d| d.render(&name, &source)).collect::<Vec<_>>().join("\n"))?;

    let bar_seconds = program.tempo.beats_per_bar * 60.0 / program.tempo.bpm;
    let until = match (bars, seconds, to) {
        (_, _, Some(to)) => Some(position_seconds(to, bar_seconds)?),
        (Some(b), _, _) if b > 0.0 => Some(b * bar_seconds),
        (_, Some(s), _) if s > 0.0 => Some(s),
        (None, None, None) => None,
        _ => return Err("error: --bars and --seconds must be above zero".into()),
    };
    let from_seconds = from.map(|f| position_seconds(f, bar_seconds)).transpose()?.unwrap_or(0.0);
    let options = RenderOptions { limiter: !no_limiter, until, arrangement };

    let events = match score {
        None => Vec::new(),
        Some(path) => {
            let section = Section::from_json(&read(path)?)
                .map_err(|e| format!("error: {}: {e}", path.display()))?;
            section.events
        }
    };

    let mut rendered = render(&program, &events, &options).map_err(|e| format!("error: {e}"))?;
    if rendered.silenced_voices > 0 {
        eprintln!("warning: {} voice(s) produced NaN or infinity and were silenced", rendered.silenced_voices);
    }
    if rendered.is_empty() {
        return Err("error: nothing to render: the file plays no notes".into());
    }

    // `--from` drops the audio before it, which the earlier bars still shaped (their tails are kept).
    let skip = (from_seconds * f64::from(SAMPLE_RATE)).round() as usize;
    if skip >= rendered.len() {
        return Err(format!("error: the piece ends before {}", from.unwrap_or_default()));
    }
    for channel in &mut rendered.channels {
        channel.drain(..skip);
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
    midi: Option<&str>,
    osc_port: Option<u16>,
) -> Result<(), String> {
    let layout = layout_for(channels)?;
    let output = if no_audio { None } else { Some(Output::open(channels, sample_rate)?) };
    let rate = output.as_ref().map(Output::sample_rate).or(sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE);
    let where_to = output.as_ref().map_or_else(|| "no audio output".to_string(), |o| format!("audio to {}", o.name()));

    let daemon: Shared = Arc::new(Mutex::new(Daemon::new(Session::new(rate as f32, layout))));
    let health = output.as_ref().map(Output::health);
    let _audio = match output {
        Some(o) => Some(o.start(daemon.clone())?),
        None => None,
    };
    let _clock = no_audio.then(|| NullClock::start(daemon.clone(), 1.0));
    let server = Server::start(daemon.clone(), ServerConfig { listen, port, token })?;
    println!("niminal daemon listening on ws://{listen}:{} ({layout}, {rate} Hz, {where_to})", server.port());
    let _midi = match midi {
        Some(filter) => {
            let (connections, names) = niminal_cli::input::start_midi(&daemon, filter)?;
            println!("MIDI control from {}", names.join(", "));
            Some(connections)
        }
        None => None,
    };
    let _osc = match osc_port {
        Some(port) => {
            let osc = niminal_cli::input::OscInput::start(daemon.clone(), port)?;
            println!("OSC control on udp://127.0.0.1:{}", osc.port());
            Some(osc)
        }
        None => None,
    };

    let mut watched = None;
    if let Some(path) = file {
        watched = Some(load(&daemon, path, Some("now")));
    }
    let mut written = (0, 0);
    let mut reported = (0, 0, 0);
    let mut last_report = std::time::Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        if let Some(h) = &health
            && last_report.elapsed() >= Duration::from_secs(2)
        {
            use std::sync::atomic::Ordering::Relaxed;
            let now = (h.panics.load(Relaxed), h.errors.load(Relaxed), h.underruns.load(Relaxed));
            if now.0 > reported.0 {
                eprintln!("audio: the engine failed on {} buffer(s) and was silenced for them; see the crash log", now.0 - reported.0);
            }
            if now.1 > reported.1 {
                eprintln!("audio: {} device problem(s); check the output device", now.1 - reported.1);
            }
            if now.2 > reported.2 {
                eprintln!("audio: {} output buffer(s) ran dry and were silenced", now.2 - reported.2);
            }
            if now != reported {
                reported = now;
                last_report = std::time::Instant::now();
            }
        }
        if let (Some(path), Some(last)) = (file, watched.as_mut()) {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if modified.is_some() && modified != *last {
                *last = load(&daemon, path, None);
            }
        }
        if let Some(path) = log {
            // format under the lock, write to disk without it
            let text = {
                let mut d = lock(&daemon);
                d.session_mut().sync_realtime();
                let entries = d.session().log();
                let version = d.session().log_version();
                (version != written).then(|| {
                    written = version;
                    to_lines(entries)
                })
            };
            if let Some(text) = text {
                let _ = std::fs::write(path, text);
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
    let parsed = niminal_daemon::ParsedEval::new(&source, quantize);
    let begun = {
        let mut d = lock(daemon);
        d.session_mut().set_sample_dir(path.parent().unwrap_or(Path::new(".")));
        d.session_mut().begin_parsed(parsed)
    };
    let result = match begun {
        niminal_daemon::Begin::Done(result) => result,
        niminal_daemon::Begin::Job(job) => {
            let mut done = job.compile();
            loop {
                let retry = {
                    let mut d = lock(daemon);
                    match d.session().retry_eval(&done) {
                        Some(job) => job,
                        None => break d.session_mut().finish_eval(done),
                    }
                };
                done = retry.compile();
            }
        }
    };
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
    if let Ok(dir) = std::env::current_dir() {
        params["dir"] = json!(dir);
    }
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
