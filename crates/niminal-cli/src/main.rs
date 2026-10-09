use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use niminal_cli::render::{RenderOptions, SAMPLE_RATE, render, write_wav};
use niminal_score::Section;

#[derive(Parser)]
#[command(name = "niminal", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a niminal file to a WAV file.
    Render {
        /// The .nml source to render.
        file: PathBuf,
        /// A .nms score whose events are played on the file's instruments.
        #[arg(long)]
        score: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Turn off the master limiter (offline renders only).
        #[arg(long)]
        no_limiter: bool,
        /// Render this many bars of a piece that plays until stopped.
        #[arg(long, conflicts_with = "seconds")]
        bars: Option<f64>,
        /// Render this many seconds of a piece that plays until stopped.
        #[arg(long)]
        seconds: Option<f64>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Render { file, score, out, no_limiter, bars, seconds } => {
            run_render(&file, score.as_deref(), &out, no_limiter, bars, seconds)
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
        if rendered.channels.len() == 1 { "" } else { "s" }
    );
    Ok(())
}
