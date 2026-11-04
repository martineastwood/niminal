use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use engine_commands::{ReplayArgs, ServerArgs};
use play::PlayArgs;
use niminal_cli::render::{
    End, RenderOptions, SAMPLE_RATE, render, render_session, write_wav,
};

mod engine_commands;
mod play;

#[derive(Parser)]
#[command(name = "niminal", version = option_env!("NIMINAL_RELEASE_VERSION").unwrap_or(concat!(env!("CARGO_PKG_VERSION"), " (", env!("NIMINAL_BUILD"), ")")))]
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
    /// Samples per DSP block, a power of two from 16 to 1024, as for `niminal server`.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_BLOCK)]
    block_size: usize,
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

#[derive(Subcommand)]
enum Command {
    /// Play a file through the sound card, and again each time it changes.
    Play(PlayArgs),
    /// Run the audio engine as a server, for testing and for clients of your own.
    Server(ServerArgs),
    /// Re-render a session to a WAV file from the log a server recorded.
    Replay(ReplayArgs),
    /// List the sound cards, with the ids `--out-device` and `--in-device` take.
    Devices,
    /// Print the built-in opcode reference as Markdown, for the docs site.
    Docs,
    /// Render a piece to a WAV file.
    Render(RenderArgs),
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
        Command::Play(args) => play::run_play(args),
        Command::Server(mut args) => {
            niminal_ws::launch::install_crash_log("niminal", env!("NIMINAL_BUILD"));
            if let Some(release) = option_env!("NIMINAL_RELEASE_VERSION") {
                niminal_server::set_release(release);
            }
            if args.log.is_none() && !args.no_log {
                args.log = niminal_host::host::new_session_log();
            }
            engine_commands::run_server(args)
        }
        Command::Replay(args) => niminal_engine::set_block_size(args.block_size)
            .and_then(|()| niminal_engine::set_control_step(args.control_step))
            .map_err(|e| format!("error: {e}"))
            .and_then(|()| engine_commands::run_replay(&args)),
        Command::Devices => {
            engine_commands::print_devices();
            Ok(())
        }
        Command::Docs => {
            print!("{}", engine_commands::opcode_reference());
            Ok(())
        }
        Command::Render(args) => run_render(args),
        Command::Explain { file, control_step } => run_explain(&file, control_step),
        Command::Lsp => {
            if niminal_lsp::run(std::io::stdin().lock(), std::io::stdout().lock()) == 0 {
                Ok(())
            } else {
                Err(String::new())
            }
        }
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
        block_size,
        control_step,
        ..
    } = args;
    niminal_engine::set_block_size(block_size).map_err(|e| format!("error: {e}"))?;
    niminal_engine::set_control_step(control_step).map_err(|e| format!("error: {e}"))?;
    let file = file.as_path();
    let (score, input, from, to) = (
        score.as_deref(),
        input.as_deref(),
        from.as_deref(),
        to.as_deref(),
    );
    let out = out.as_path();
    let source = read(file)?;
    let mut score_events = Vec::new();
    if let Some(path) = score {
        score_events = niminal_compose::Section::from_json(&read(path)?)
            .map_err(|e| format!("error: {}: {e}", path.display()))?
            .events;
    }
    let name = file.display().to_string();
    let options = niminal_lang::CompileOptions {
        samples: niminal_lang::Samples::new(file.parent().unwrap_or(Path::new("."))),
        ..Default::default()
    };
    let expanded = niminal_compose::expand(&source, file.parent().unwrap_or(Path::new(".")))
        .map_err(|d| d.render(&name, &source))?;
    // a tempo ramp is made as the music plays, so a file with one is played through a session
    let (rest, ramps) = niminal_cli::render::split_ramps(&expanded.text)
        .map_err(|d| expanded.locate(d).render(&name, &source))?;
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

    // a clip's `then:` runs when its last pass ends, which only a session does
    let hands_over = options.arrangement.is_none() && !program.schedule().finishes().is_empty();
    if !ramps.is_empty() || hands_over {
        let why = if ramps.is_empty() {
            "a `then:`"
        } else {
            "a tempo ramp"
        };
        if options.arrangement.is_some() || score.is_some() || from.is_some() || to.is_some() {
            return Err(format!(
                "error: a file with {why} renders as it is played: --arrangement, --score, --from and --to aren't available for it"
            ));
        }
        let end = match (bars, seconds) {
            (Some(b), _) if b > 0.0 => End::Bars(b),
            (_, Some(s)) if s > 0.0 => End::Seconds(s),
            _ => {
                return Err(format!(
                    "error: a file with {why} renders by the clock: say how long with `--bars 16` or `--seconds 30`"
                ));
            }
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

/// Write a finished render: one WAV, or with `stems` a folder of them.
fn write_output(
    rendered: &niminal_cli::render::RenderOutput,
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
