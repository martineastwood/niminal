//! A live session: the sequencer and engine running for as long as the page wants sound, stepped a chunk at a time.
//!
//! This is [`niminal_sequencer::Offline`] used as a live engine: time moves only when asked, so the host
//! (a worker, in a browser) decides how fast: it renders a little ahead of what is being heard and evaluates
//! code between chunks, the way Stage and the VS Code extension send code to a running server.

use std::path::Path;

use niminal_lang::{Layout, Problem};
use niminal_sequencer::Offline;

pub struct LiveSession {
    offline: Offline,
    /// The last chunk, one buffer per channel.
    pub out: Vec<Vec<f32>>,
}

/// What an accepted evaluation will do, for the page to say.
pub struct Landed {
    /// Where it lands: a bar (1-based) and a beat within it.
    pub bar: u64,
    pub beat: f64,
    /// What it changes, as sentences ("start clip melody").
    pub changes: Vec<String>,
}

/// Why a session could not start or a snippet could not run.
pub enum LiveFailure {
    /// Mistakes in the piece, located in the source given.
    Source(Vec<niminal_lang::Diagnostic>),
    /// Mistakes found while running a snippet in the session, located in the snippet.
    Snippet(Vec<Problem>),
    Other(String),
}

impl LiveSession {
    /// Start a session for `source` at `sample_rate` and run all of it, now. The source decides the master layout
    /// (`config { channels: ... }`), so it is read first.
    pub fn start(source: &str, sample_rate: f32) -> Result<(LiveSession, Landed), LiveFailure> {
        let expanded = niminal_compose::expand(source, Path::new(".")).map_err(|d| LiveFailure::Source(vec![d]))?;
        let located = |errors: Vec<niminal_lang::Diagnostic>| {
            LiveFailure::Source(errors.into_iter().map(|d| expanded.locate(d)).collect())
        };
        let options = niminal_lang::CompileOptions::default();
        // a tempo ramp is made as the music plays, which a session does by itself
        let program = niminal_compose::compile_with(&expanded.text, &options).map_err(located)?;
        let layout: Layout = program.master;
        let mut session = LiveSession {
            offline: Offline::new(sample_rate, layout),
            out: Vec::new(),
        };
        // the session is given the text with its `use` lines expanded, as the render path does; the piece has
        // just compiled, so a mistake found here is rare and its position is in the expanded text
        let landed = session.eval(&expanded.text, true)?;
        Ok((session, landed))
    }

    /// Run `source` (a selection, a block, or everything). `now` lands it at once; otherwise each change lands
    /// where it asks, or on its kind's default boundary.
    pub fn eval(&mut self, source: &str, now: bool) -> Result<Landed, LiveFailure> {
        let accepted = self
            .offline
            .eval(source, now.then_some("now"))
            .map_err(LiveFailure::Snippet)?;
        Ok(Landed {
            bar: accepted.position.0,
            beat: accepted.position.1,
            changes: accepted.changes,
        })
    }

    /// Mix the next `frames` samples into [`Self::out`].
    pub fn process(&mut self, frames: usize) {
        self.out = self.offline.process(frames);
    }

    /// Move a control now, in `unit` if given (`hz`, `db`, ...), over the control's own smoothing.
    pub fn set_control(&mut self, name: &str, value: f64, unit: Option<&str>) -> Result<(), String> {
        self.offline
            .set_control(name, value, unit, None, None)
            .map_err(|problems| problems.first().map_or_else(|| "the control could not be set".to_owned(), |p| p.message.clone()))
    }

    /// Stop every sound and clip at once, keeping what has been defined.
    pub fn hush(&mut self) {
        // nothing sensible to report if there was nothing playing
        let _ = self.offline.hush();
    }

    /// Where the music is and what is playing, as JSON for the page's "now playing" line.
    pub fn status(&self) -> String {
        let t = self.offline.transport();
        let clips: Vec<_> = self
            .offline
            .clips()
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "instrument": c.instrument,
                    "running": c.running,
                    "muted": c.muted,
                    "looped": c.looped,
                })
            })
            .collect();
        serde_json::json!({
            "bar": t.bar,
            "beat": t.beat,
            "bpm": t.bpm,
            "voices": t.voices,
            "pending": t.pending,
            "seconds": t.seconds,
            "clips": clips,
        })
        .to_string()
    }
}
