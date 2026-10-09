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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Render { file, score, out, no_limiter } => {
            run_render(&file, score.as_deref(), &out, &RenderOptions { limiter: !no_limiter })
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

fn run_render(file: &Path, score: Option<&Path>, out: &Path, options: &RenderOptions) -> Result<(), String> {
    let source = read(file)?;
    let name = file.display().to_string();
    let program = niminal_lang::compile(&source)
        .map_err(|errors| errors.iter().map(|d| d.render(&name, &source)).collect::<Vec<_>>().join("\n"))?;

    let events = match score {
        None => Vec::new(),
        Some(path) => {
            let section = Section::from_json(&read(path)?)
                .map_err(|e| format!("error: {}: {e}", path.display()))?;
            section.events
        }
    };

    let rendered = render(&program, &events, options).map_err(|e| format!("error: {e}"))?;
    let samples = rendered.samples;
    if rendered.silenced_voices > 0 {
        eprintln!("warning: {} voice(s) produced NaN or infinity and were silenced", rendered.silenced_voices);
    }
    if samples.is_empty() {
        return Err("error: nothing to render: the file plays no notes".into());
    }

    let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    if peak > 1.0 {
        eprintln!("warning: output peaks at {peak:.2}, which will clip (the limiter is off)");
    }
    write_wav(out, &samples).map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!("wrote {} ({:.2}s)", out.display(), samples.len() as f64 / f64::from(SAMPLE_RATE));
    Ok(())
}
