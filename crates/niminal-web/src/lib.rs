//! niminal in a browser: a piece of source in, audio out.
//!
//! [`render_piece`] is `niminal render` without the files: the same expansion, compile and renderer, so a
//! piece gives the same audio here as on the command line. The `niminal_*` functions below it are a plain C
//! interface to it for a host that loads the module as WebAssembly (no generated glue to ship): the host
//! writes the source into memory from [`niminal_alloc`], calls [`niminal_render`], and reads the audio or
//! the error list back out of memory.
//!
//! The module imports one function, `env.niminal_now_ms` (a browser answers it with `performance.now()`),
//! because `wasm32-unknown-unknown` has no clock of its own.

mod api;

use std::path::Path;

use niminal_lang::Diagnostic;
use niminal_sequencer::render::{End, RenderOptions, SAMPLE_RATE, render, render_session, split_ramps};

/// The longest piece rendered, in seconds. A browser tab has limited memory and a long render blocks a worker.
pub const MAX_SECONDS: f64 = 180.0;

pub struct Rendered {
    /// One buffer per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
    pub sample_rate: u32,
    /// Things worth telling the author that did not stop the render.
    pub warnings: Vec<String>,
}

/// Why a piece could not be rendered.
pub enum Failure {
    /// Mistakes in the source, located in it.
    Source(Vec<Diagnostic>),
    /// Anything else, as a sentence.
    Other(String),
}

/// Render `source` for `bars` bars (or until it ends by itself, if it does and `bars` is zero or less).
pub fn render_piece(source: &str, bars: f64) -> Result<Rendered, Failure> {
    // there is no file system: a `use` of a file next to the piece fails and says so; the library is built in
    let expanded = niminal_compose::expand(source, Path::new(".")).map_err(|d| Failure::Source(vec![d]))?;
    let located = |errors: Vec<Diagnostic>| {
        Failure::Source(errors.into_iter().map(|d| expanded.locate(d)).collect())
    };
    // a tempo ramp is made as the music plays, so a piece with one is played through a session
    let (rest, ramps) = split_ramps(&expanded.text).map_err(|d| located(vec![d]))?;
    let options = niminal_lang::CompileOptions::default();
    let program = niminal_compose::compile_with(&rest, &options).map_err(located)?;

    let bar_seconds = program.tempo.beats_per_bar * 60.0 / program.tempo.bpm;
    let mut warnings = Vec::new();
    let mut until = (bars > 0.0).then_some(bars * bar_seconds);
    if until.is_some_and(|u| u > MAX_SECONDS) {
        warnings.push(format!("rendering stops after {MAX_SECONDS} seconds"));
        until = Some(MAX_SECONDS);
    }
    // a clip's `then:` runs when its last pass ends, which only a session does
    let hands_over = !program.schedule().finishes().is_empty();
    let output = if !ramps.is_empty() || hands_over {
        let Some(seconds) = until else {
            return Err(Failure::Other(
                "this piece changes as it plays (a tempo ramp or a `then:`), so say how many bars to render".into(),
            ));
        };
        render_session(
            &expanded.text,
            Path::new("."),
            program.master,
            End::Seconds(seconds),
            true,
            false,
            Vec::new(),
        )
    } else {
        render(
            &program,
            &[],
            &RenderOptions {
                until,
                ..RenderOptions::default()
            },
        )
    }
    .map_err(|e| Failure::Other(e.to_string()))?;
    if output.is_empty() {
        return Err(Failure::Other(
            "nothing to render: the piece plays no notes (start a clip, or write notes)".into(),
        ));
    }
    if output.silenced_voices > 0 {
        warnings.push(format!(
            "{} voice(s) produced NaN or infinity and were silenced",
            output.silenced_voices
        ));
    }
    Ok(Rendered {
        channels: output.channels,
        sample_rate: SAMPLE_RATE,
        warnings,
    })
}
