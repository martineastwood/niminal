//! The performance layer's data: what clips and scenes are, and the timed
//! commands that start, stop and mute them. Evaluating the source into this
//! lives in `perform.rs`; turning it into notes lives in `schedule.rs`.

use niminal_engine::ops::Curve;
use niminal_pattern::{Pattern, Rational};
use niminal_score::{Time, Value};

/// One step of a pattern: a value, and the controls that belong to this note alone
/// (a lane is read once, where a note starts; these travel with the note itself).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub value: Value,
    /// How loud, in db.
    pub gain: Option<f64>,
    /// Where it sits among the speakers, in degrees: 0 is straight ahead and positive is right.
    pub pan: Option<f64>,
    /// Where in a sample it starts, as a share of its length.
    pub begin: Option<f64>,
}

impl From<Value> for Step {
    fn from(value: Value) -> Step {
        Step {
            value,
            gain: None,
            pan: None,
            begin: None,
        }
    }
}

impl Step {
    /// The same note and controls with a different value.
    pub fn with(self, value: Value) -> Step {
        Step { value, ..self }
    }
}

/// A clip: lanes of patterns sharing one timeline.
///
/// One pattern cycle is one bar. A clip plays `length` cycles and then starts
/// over, so a one-cycle pattern in a four-cycle clip simply repeats. A clip
/// with no `length` is `endless` instead: its cycles keep counting, so `<a b>`
/// and `every` alternate. A bare pattern such as `[c4 e4]` is a clip like that.
#[derive(Clone)]
pub struct Clip {
    /// Where the clip came from, for messages.
    pub name: String,
    /// Length in cycles (bars). Defaults to one.
    pub length: Rational,
    /// Never starts over, because no `length` was given: `length` is only the first cycle.
    pub endless: bool,
    /// `notes` is the pitch lane; other lanes set parameters or universal
    /// controls note by note.
    pub lanes: Vec<(String, Pattern<Step>)>,
    /// `keys: midi.keys(channel: 1)`: played from a keyboard instead of by `notes`.
    pub keys: Option<Keys>,
    /// Lanes drawn as an envelope, `cut: env[300hz 1bar 3khz]`: they move a track's
    /// parameter continuously, on the clip's timeline.
    pub curves: Vec<(String, EnvLane)>,
    /// Lanes that are signals (`cut: sine.range(300hz, 3khz)`), once placed on a track: the
    /// parameter's unit. They move the parameter continuously instead of stepping it at each note.
    pub signals: Vec<(String, crate::unit::Unit)>,
}

/// An `env[...]` in a lane: a run of glides between levels, from the start of the clip.
#[derive(Clone, Debug)]
pub struct EnvLane {
    pub start: Value,
    /// How long to take, the curve, and the level reached.
    pub segments: Vec<(Time, Curve, Value)>,
    /// Starts over when it ends, instead of holding its last level.
    pub looped: bool,
    /// The unit of the parameter it moves, once it is placed on a track.
    pub unit: Option<crate::unit::Unit>,
}

/// Where a clip's notes come from when it is played live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keys {
    /// The MIDI channel, from 1; any channel if none.
    pub channel: Option<u8>,
    /// MPE: each note on a channel of its own (2 to 16), with its own bend, pressure and slide, and a
    /// master channel (1) for what applies to all of them.
    pub mpe: bool,
}

impl Keys {
    /// Does a key on `channel` play this clip?
    pub fn accepts(&self, channel: u8) -> bool {
        if self.mpe {
            (2..=16).contains(&channel)
        } else {
            self.channel.is_none_or(|c| c == channel)
        }
    }
}

impl Clip {
    pub fn curve(&self, name: &str) -> Option<&EnvLane> {
        self.curves.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }

    pub fn lane(&self, name: &str) -> Option<&Pattern<Step>> {
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
    Play {
        track: usize,
        clip: Clip,
    },
    Stop {
        track: usize,
    },
    Mute {
        track: usize,
    },
    Unmute {
        track: usize,
    },
    Solo {
        track: usize,
    },
    /// Clear all solos, or just this track's.
    Unsolo {
        track: Option<usize>,
    },
    /// Stop every track, letting notes ring out.
    Hush,
    /// Stop everything at once.
    Panic,
    /// Stop every track, and count bars from here. Only a live session can do the
    /// counting; a render treats it as `hush`.
    Reset,
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

/// Scenes in order, each for a length: a finished piece.
#[derive(Clone)]
pub struct Arrangement {
    pub name: String,
    /// The commands that play it, from the start of the piece.
    pub actions: Vec<Scheduled>,
    /// Where each section's own tempo and meter begin, in seconds. Empty if the piece has just the file's.
    pub tempos: Vec<(f64, niminal_score::Tempo)>,
    /// Envelopes moving track parameters over the piece: the control's name (`bass::tone`) and the shape.
    pub lanes: Vec<(String, EnvLane)>,
}
