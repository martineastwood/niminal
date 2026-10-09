use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use niminal_cli::render::{SAMPLE_RATE, render, write_wav};
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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Render { file, score, out } => run_render(&file, score.as_deref(), &out),
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

fn run_render(file: &Path, score: Option<&Path>, out: &Path) -> Result<(), String> {
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

    let samples = render(&program, &events).map_err(|e| format!("error: {e}"))?;
    if samples.is_empty() {
        return Err("error: nothing to render: the file plays no notes".into());
    }

    let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    if peak > 1.0 {
        eprintln!("warning: output peaks at {peak:.2}, which will clip");
    }
    write_wav(out, &samples).map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!("wrote {} ({:.2}s)", out.display(), samples.len() as f64 / f64::from(SAMPLE_RATE));
    Ok(())
}
