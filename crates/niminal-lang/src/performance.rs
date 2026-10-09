//! The performance layer's data: what clips and scenes are, and the timed
//! commands that start, stop and mute them. Evaluating the source into this
//! lives in `perform.rs`; turning it into notes lives in `schedule.rs`.

use niminal_pattern::{Pattern, Rational};
use niminal_score::{Time, Value};

/// A clip: lanes of patterns sharing one timeline.
///
/// One pattern cycle is one bar. A clip plays `length` cycles and then starts
/// over, so a one-cycle pattern in a four-cycle clip simply repeats.
#[derive(Clone)]
pub struct Clip {
    /// Where the clip came from, for messages.
    pub name: String,
    /// Length in cycles (bars). Defaults to one.
    pub length: Rational,
    /// `notes` is the pitch lane; other lanes set parameters or universal
    /// controls note by note.
    pub lanes: Vec<(String, Pattern<Value>)>,
}

impl Clip {
    pub fn lane(&self, name: &str) -> Option<&Pattern<Value>> {
        self.lanes.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }
}

/// A named set of clips, one per track. `None` stops that track.
#[derive(Clone)]
pub struct Scene {
    pub name: String,
    pub entries: Vec<(usize, Option<Clip>)>,
}

/// What a command does. Tracks are indices into `Program::tracks`.
#[derive(Clone)]
pub enum Action {
    Play { track: usize, clip: Clip },
    Stop { track: usize },
    Mute { track: usize },
    Unmute { track: usize },
    Solo { track: usize },
    /// Clear all solos, or just this track's.
    Unsolo { track: Option<usize> },
    /// Stop every track, letting notes ring out.
    Hush,
    /// Stop everything at once.
    Panic,
}

/// Which grid a change waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizeRelation {
    /// Land on the grid: `@ bar`, `@ 4 bars`.
    OnGrid,
    /// The next line of the grid: `@ next 4 bars`.
    Next,
    /// Counted from now: `@ in 4 bars`.
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizeUnit {
    Now,
    Beat,
    Bar,
    Cycle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantize {
    pub relation: QuantizeRelation,
    pub count: f64,
    pub unit: QuantizeUnit,
}

/// A command with the time it was written for. `at` is when it happens in a
/// rendered file; `quantize` says how a live session should delay it.
#[derive(Clone)]
pub struct Scheduled {
    pub at: Time,
    pub action: Action,
    pub quantize: Option<Quantize>,
}
