use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use clap::{Args, Parser, Subcommand};
use niminal_compose::ScoreFile;
use niminal_live::{Live, LiveServer};
use niminal_ws::{Client, Server, ServerConfig, lock};
use niminal_live_cli::input::LiveShared;
use niminal_live_cli::render::{End, RenderOptions, SAMPLE_RATE, render, render_session, write_wav};
use niminal_live_cli::report::{render_error, render_problems};
use serde_json::json;

#[derive(Parser)]
#[command(name = "niminal-live", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct RenderArgs {
    /// The source to render.
    file: PathBuf,
    /// An `arrangement` in the file to render, in place of its `play` and `launch` commands.
    arrangement: Option<String>,
    /// A .nms score whose events are played on the file's instruments.
    #[arg(long)]
    score: Option<PathBuf>,
    /// A WAV file, or with `--stems` a folder.
    #[arg(long)]
    out: PathBuf,
    /// Write a file for every node and bus as well as the mix, into the folder given by `--out`.
    #[arg(long)]
    stems: bool,
    /// A WAV file for fx declarations that read `input(..)` to hear. The render lasts as long as it does,
    /// unless `--bars` or `--seconds` says otherwise.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Turn off the master limiter (offline renders only).
    #[arg(long)]
    no_limiter: bool,
    /// Samples between the runs of a `control { ... }` scope, from 1 to 1024.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_CONTROL_STEP)]
    control_step: usize,
    /// Render this many bars of a piece that plays until stopped.
    #[arg(long, conflicts_with = "seconds")]
    bars: Option<f64>,
    /// Render this many seconds of a piece that plays until stopped.
    #[arg(long)]
    seconds: Option<f64>,
    /// Start the audio at this position, such as `bar 64` (an arrangement or timeline).
    #[arg(long)]
    from: Option<String>,
    /// Stop starting notes at this position, such as `bar 80`. Their tails still ring out.
    #[arg(long, conflicts_with_all = ["bars", "seconds"])]
    to: Option<String>,
}

/// A live performance client of the generic server.
#[derive(Args)]
struct LiveArgs {
    /// A project to load and watch.
    file: Option<PathBuf>,
    /// Private live-control port (0 chooses a free port).
    #[arg(long, default_value_t = 7400)]
    port: u16,
    /// The generic server to connect to.
    #[arg(long, default_value_t = 7401)]
    engine_port: u16,
    #[arg(long)]
    token: Option<String>,
    /// Exit when the owning editor closes stdin.
    #[arg(long)]
    parent_stdio: bool,
    /// Take MIDI control changes from every input whose name contains this
    /// (give an empty name for all inputs), for `ctl x = midi.cc(n)`.
    #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
    midi: Option<String>,
    /// Take OSC over UDP on this local port, for `ctl x = osc_in("/address")`.
    #[arg(long, value_name = "PORT")]
    osc_port: Option<u16>,
}

#[derive(Subcommand)]
enum Command {
    /// Render a piece to a WAV file.
    Render(RenderArgs),
    /// Run the live sequencer as a client of the niminal server.
    Live(LiveArgs),
    /// Send code to a running live sequencer.
    Send {
        /// The code to evaluate. With no code and no --file, it is read from the standard input.
        code: Option<String>,
        #[arg(long, conflicts_with = "code")]
        file: Option<PathBuf>,
        #[arg(long, default_value_t = 7400)]
        port: u16,
        /// When it should land, as in `now` or `next 4bars`.
        #[arg(long)]
        quantize: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Show what each instrument runs every sample and what runs once per block.
    Explain {
        /// The `.nml` file to compile.
        file: PathBuf,
        /// Samples between the runs of a `control { ... }` scope, as for `niminal server`.
        #[arg(long, default_value_t = niminal_engine::DEFAULT_CONTROL_STEP)]
        control_step: usize,
    },
    /// Run the language server (LSP over stdin and stdout) for an editor.
    Lsp,
    /// Type code at a running live sequencer, one statement at a time.
    Repl {
        #[arg(long, default_value_t = 7400)]
        port: u16,
        /// When each statement should land, as in `now` or `next 4bars`.
        #[arg(long)]
        quantize: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
}

fn run_explain(file: &Path, control_step: usize) -> Result<(), String> {
    niminal_engine::set_control_step(control_step).map_err(|e| format!("error: {e}"))?;
    let source = read(file)?;
    let name = file.display().to_string();
    let dir = file.parent().unwrap_or(Path::new("."));
    let options = niminal_lang::CompileOptions {
        samples: niminal_lang::Samples::new(dir),
        ..Default::default()
    };
    let expanded = niminal_compose::expand(&source, dir).map_err(|d| d.render(&name, &source))?;
    let composition =
        niminal_compose::compile_with(&expanded.text, &options).map_err(|errors| {
            errors
                .into_iter()
                .map(|d| expanded.locate(d).render(&name, &source))
                .collect::<Vec<_>>()
                .join("\n")
        })?;
    print!("{}", composition.program.explain());
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Render(args) => run_render(args),
        Command::Live(args) => run_live(args),
        Command::Explain { file, control_step } => run_explain(&file, control_step),
        Command::Lsp => {
            if niminal_lsp::run(std::io::stdin().lock(), std::io::stdout().lock()) == 0 {
                Ok(())
            } else {
                Err(String::new())
            }
        }
        Command::Send {
            code,
            file,
            port,
            quantize,
            token,
        } => run_send(
            code,
            file.as_deref(),
            port,
            quantize.as_deref(),
            token.as_deref(),
        ),
        Command::Repl {
            port,
            quantize,
            token,
        } => run_repl(port, quantize, token.as_deref()),
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

/// A position such as `bar 64` or `64`, as seconds from the start.
fn position_seconds(text: &str, bar_seconds: f64) -> Result<f64, String> {
    let number = text.trim().strip_prefix("bar").unwrap_or(text).trim();
    match number.parse::<f64>() {
        Ok(bar) if bar >= 1.0 => Ok((bar - 1.0) * bar_seconds),
        _ => Err(format!(
            "error: `{text}` isn't a position: write `bar 64`, counting from bar 1"
        )),
    }
}

fn run_render(args: RenderArgs) -> Result<(), String> {
    let RenderArgs {
        file,
        arrangement,
        score,
        out,
        stems,
        input,
        no_limiter,
        bars,
        seconds,
        from,
        to,
        control_step,
        ..
    } = args;
    niminal_engine::set_control_step(control_step).map_err(|e| format!("error: {e}"))?;
    let file = file.as_path();
    let (score, input, from, to) = (
        score.as_deref(),
        input.as_deref(),
        from.as_deref(),
        to.as_deref(),
    );
    let out = out.as_path();
    let mut source = read(file)?;
    let mut arrangement = arrangement;
    let mut score_events = Vec::new();
    if let Some(path) = score {
        match ScoreFile::from_json(&read(path)?)
            .map_err(|e| format!("error: {}: {e}", path.display()))?
        {
            ScoreFile::Section(section) => score_events = section.events,
            ScoreFile::Project(project) => {
                if arrangement.is_none() {
                    arrangement = project.arrangement.as_ref().map(|a| a.name.clone());
                }
                source.push('\n');
                source.push_str(&project.to_source());
            }
        }
    }
    let name = file.display().to_string();
    let options = niminal_lang::CompileOptions {
        samples: niminal_lang::Samples::new(file.parent().unwrap_or(Path::new("."))),
        ..Default::default()
    };
    let expanded = niminal_compose::expand(&source, file.parent().unwrap_or(Path::new(".")))
        .map_err(|d| d.render(&name, &source))?;
    // a tempo ramp is made as the music plays, so a file with one is played through a session
    let (rest, ramps) =
        niminal_live_cli::render::split_ramps(&expanded.text).map_err(|d| d.render(&name, &source))?;
    let program = niminal_compose::compile_with(&rest, &options).map_err(|errors| {
        errors
            .into_iter()
            .map(|d| expanded.locate(d).render(&name, &source))
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    for instrument in program.instruments.iter().filter(|i| i.midi.is_some()) {
        eprintln!(
            "warning: `{}` sends MIDI to a port, so a render has no sound from it",
            instrument.name
        );
    }
    // audio for fx declarations that read `input(..)`
    let audio = match input {
        None => Vec::new(),
        Some(path) => {
            let data = niminal_lang::Samples::new(path.parent().unwrap_or(Path::new(".")))
                .load(
                    &path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                )
                .map_err(|e| format!("error: {e}"))?;
            let rate = data
                .first()
                .map_or(SAMPLE_RATE as f32, |c| c.sample_rate_or(SAMPLE_RATE as f32));
            if (rate - SAMPLE_RATE as f32).abs() > 0.5 {
                return Err(format!(
                    "error: {} is {rate} Hz, but niminal renders at {SAMPLE_RATE} Hz; convert it first",
                    path.display()
                ));
            }
            data.iter().map(|c| c.data().to_vec()).collect()
        }
    };
    let bar_seconds = program.tempo.beats_per_bar * 60.0 / program.tempo.bpm;
    let until = match (bars, seconds, to) {
        (_, _, Some(to)) => Some(position_seconds(to, bar_seconds)?),
        (Some(b), _, _) if b > 0.0 => Some(b * bar_seconds),
        (_, Some(s), _) if s > 0.0 => Some(s),
        (None, None, None) => None,
        _ => return Err("error: --bars and --seconds must be above zero".into()),
    };
    // an input with no length given plays through to its end
    let until = until.or_else(|| {
        audio
            .first()
            .map(|c| c.len() as f64 / f64::from(SAMPLE_RATE))
    });
    let from_seconds = from
        .map(|f| position_seconds(f, bar_seconds))
        .transpose()?
        .unwrap_or(0.0);
    let options = RenderOptions {
        limiter: !no_limiter,
        until,
        arrangement,
        stems,
        input: audio.clone(),
    };

    if !ramps.is_empty() {
        if options.arrangement.is_some() || score.is_some() || from.is_some() || to.is_some() {
            return Err("error: a file with a tempo ramp renders as it is played: --arrangement, --score, --from and --to aren't available for it".into());
        }
        let end = match (bars, seconds) {
            (Some(b), _) if b > 0.0 => End::Bars(b),
            (_, Some(s)) if s > 0.0 => End::Seconds(s),
            _ => return Err("error: a file with a tempo ramp renders by the clock: say how long with `--bars 16` or `--seconds 30`".into()),
        };
        let dir = file.parent().unwrap_or(Path::new("."));
        let rendered = render_session(
            &expanded.text,
            dir,
            program.master,
            end,
            !no_limiter,
            stems,
            audio,
        )
        .map_err(|e| format!("error: {e}"))?;
        return write_output(&rendered, out, stems);
    }

    let mut rendered =
        render(&program, &score_events, &options).map_err(|e| format!("error: {e}"))?;
    if rendered.silenced_voices > 0 {
        eprintln!(
            "warning: {} voice(s) produced NaN or infinity and were silenced",
            rendered.silenced_voices
        );
    }
    if rendered.is_empty() {
        return Err("error: nothing to render: the file plays no notes".into());
    }

    // `--from` drops the audio before it, which the earlier bars still shaped (their tails are kept).
    let skip = (from_seconds * f64::from(SAMPLE_RATE)).round() as usize;
    if skip >= rendered.len() {
        return Err(format!(
            "error: the piece ends before {}",
            from.unwrap_or_default()
        ));
    }
    for channel in rendered.channels.iter_mut().chain(
        rendered
            .stems
            .iter_mut()
            .flat_map(|s| s.channels.iter_mut()),
    ) {
        channel.drain(..skip);
    }

    write_output(&rendered, out, stems)
}

fn run_live(args: LiveArgs) -> Result<(), String> {
    use niminal_server::EngineReply;
    let LiveArgs {
        file,
        port,
        engine_port,
        token,
        parent_stdio,
        midi,
        osc_port,
    } = args;
    let link = niminal_live::WsLink::connect(engine_port, token.as_deref(), "niminal-live")?.exit_when_lost();
    let EngineReply::Hello {
        sample_rate,
        channels,
        ..
    } = link.hello.clone()
    else {
        return Err("the server did not return its audio format".into());
    };
    let layout = niminal_lang::Layout::standard(channels as u16)?;
    // Over a socket each look at the engine's clock is a round trip that locks the engine, so count
    // between looks: the sequencer works 100ms ahead and the count is never ahead of the engine.
    let new_live = move |link: Box<dyn niminal_live::EngineLink>| {
        Live::new(link, sample_rate, layout).with_clock_sync(Duration::from_millis(250))
    };
    // A client that names a chair in its hello gets a performance of its own, over a connection of its own.
    let chair_token = token.clone();
    let live: LiveShared = Arc::new(Mutex::new(
        LiveServer::new(new_live(Box::new(link))).with_chairs(move || {
            let link = niminal_live::WsLink::connect(engine_port, chair_token.as_deref(), "niminal-live")?.exit_when_lost();
            Ok(new_live(Box::new(link)))
        }),
    ));
    let server = Server::start(
        live.clone(),
        ServerConfig {
            port,
            token,
            ..ServerConfig::default()
        },
    )?;
    let _midi = midi
        .as_deref()
        .map(|filter| niminal_live_cli::input::start_midi(&live, filter))
        .transpose()?;
    let _osc = osc_port
        .map(|port| niminal_live_cli::input::OscInput::start(live.clone(), port))
        .transpose()?;
    let mut watched = file.as_deref().map(|path| load(&live, path, Some("now")));
    if parent_stdio {
        std::thread::spawn(|| {
            let mut buffer = [0; 256];
            while matches!(std::io::stdin().read(&mut buffer), Ok(n) if n > 0) {}
            // The OS closes the engine connection, cancelling this client's pending events.
            std::process::exit(0);
        });
    }
    println!("niminal-live listening on ws://127.0.0.1:{}", server.port());
    // Events go to the server every 25ms; the files are looked at a tenth as often.
    let mut ticks = 0u32;
    loop {
        lock(&live).advance();
        ticks = ticks.wrapping_add(1);
        if ticks.is_multiple_of(10)
            && let (Some(path), Some((files, last))) = (file.as_deref(), watched.as_mut())
        {
            let modified = newest(files);
            if modified.is_some() && modified != *last {
                (*files, *last) = load(&live, path, None);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The latest modification time among `files`.
fn newest(files: &[PathBuf]) -> Option<SystemTime> {
    files
        .iter()
        .filter_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .max()
}

/// Evaluate a project file, reporting what happened. Returns the files it is
/// made of and when they were last changed, to notice when any changes again.
fn load(
    server: &LiveShared,
    path: &Path,
    quantize: Option<&str>,
) -> (Vec<PathBuf>, Option<SystemTime>) {
    let mut files = vec![path.to_path_buf()];
    let name = path.display().to_string();
    let source = match read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return (files.clone(), newest(&files));
        }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let parsed = niminal_live::ParsedEval::from_dir(&source, dir, quantize);
    files.extend(parsed.module_files());
    let modified = newest(&files);
    let begun = {
        let mut d = lock(server);
        d.live_mut()
            .set_sample_dir(path.parent().unwrap_or(Path::new(".")));
        d.live_mut().begin_parsed(parsed)
    };
    let result = match begun {
        niminal_live::Begin::Done(result) => result,
        niminal_live::Begin::Job(job) => {
            let mut done = job.compile();
            loop {
                let retry = {
                    let mut d = lock(server);
                    match d.live().retry_eval(&done) {
                        Some(job) => job,
                        None => break d.live_mut().finish_eval(done),
                    }
                };
                done = retry.compile();
            }
        }
    };
    match result {
        Ok(a) => match a.id {
            Some(_) => println!(
                "{name}: lands at bar {} beat {:.2}",
                a.position.0, a.position.1
            ),
            None => println!("{name}: nothing to do"),
        },
        Err(problems) => eprint!("{}", render_problems(&problems, &source, &name)),
    }
    (files, modified)
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
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| format!("error: can't read the standard input: {e}"))?;
            (text, "<stdin>".to_string())
        }
    };
    let mut client = Client::connect(port, token).map_err(|e| format!("error: {e}"))?;
    println!("{}", eval(&mut client, &source, quantize, &name)?);
    Ok(())
}

/// Evaluate code on the server, describing when it lands.
fn eval(
    client: &mut Client,
    source: &str,
    quantize: Option<&str>,
    name: &str,
) -> Result<String, String> {
    let mut params = json!({ "source": source });
    if let Ok(dir) = std::env::current_dir() {
        params["dir"] = json!(dir);
    }
    if let Some(q) = quantize {
        params["quantize"] = json!(q);
    }
    match client
        .call_raw("eval", params)
        .map_err(|e| format!("error: {e}"))?
    {
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
        println!(
            "connected to the server on port {port}. :quantize WHEN sets when code lands, :quit leaves."
        );
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

/// Write a finished render: one WAV, or with `stems` a folder of them.
fn write_output(
    rendered: &niminal_live_cli::render::RenderOutput,
    out: &Path,
    stems: bool,
) -> Result<(), String> {
    let peak = rendered.peak();
    if peak > 1.0 {
        eprintln!("warning: output peaks at {peak:.2}, which will clip (the limiter is off)");
    }
    if stems {
        std::fs::create_dir_all(out)
            .map_err(|e| format!("error: can't make {}: {e}", out.display()))?;
        let mix = out.join("mix.wav");
        write_wav(&mix, &rendered.channels)
            .map_err(|e| format!("error: can't write {}: {e}", mix.display()))?;
        for stem in &rendered.stems {
            let path = out.join(format!("{}.wav", stem.name));
            write_wav(&path, &stem.channels)
                .map_err(|e| format!("error: can't write {}: {e}", path.display()))?;
        }
        println!(
            "wrote {} files into {} ({:.2}s): mix{}",
            rendered.stems.len() + 1,
            out.display(),
            rendered.len() as f64 / f64::from(SAMPLE_RATE),
            rendered
                .stems
                .iter()
                .map(|s| format!(", {}", s.name))
                .collect::<String>()
        );
        return Ok(());
    }
    write_wav(out, &rendered.channels)
        .map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!(
        "wrote {} ({:.2}s, {} channel{})",
        out.display(),
        rendered.len() as f64 / f64::from(SAMPLE_RATE),
        rendered.channels.len(),
        plural(rendered.channels.len())
    );
    Ok(())
}
