//! The performance layer's data: what clips and scenes are, and the timed
//! commands that start, stop and mute them. Evaluating the source into this
//! lives in `perform.rs`; turning it into notes lives in `schedule.rs`.

use niminal_engine::ops::Curve;
use niminal_pattern::{Pattern, Rational, Span, with_budget};
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
    /// Playback pitch in semitones, for sample notes.
    pub pitch: Option<f64>,
    /// Play this note on another node (a grid row that names a drum strip).
    pub node: Option<usize>,
    /// Where the note was written, as (evaluation, start, end) byte offsets in what that evaluation sent,
    /// so an editor can light it as it plays.
    pub src: Option<(u64, usize, usize)>,
}

impl From<Value> for Step {
    fn from(value: Value) -> Step {
        Step {
            value,
            gain: None,
            pan: None,
            begin: None,
            pitch: None,
            node: None,
            src: None,
        }
    }
}

impl Step {
    /// The same note and controls with a different value.
    pub fn with(self, value: Value) -> Step {
        Step { value, ..self }
    }
}

/// A note written with exact times in an `events { }` block (or loaded from a score).
#[derive(Clone, Debug, PartialEq)]
pub struct ScoredNote {
    /// Start in cycles from the beginning of the clip.
    pub at: Rational,
    /// Length in cycles.
    pub dur: Rational,
    pub step: Step,
    /// Extra arguments for the instrument (`bright: 0.5`, …).
    pub args: Vec<(String, Value)>,
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
    /// Starts over when it ends. A clip with `loop: false` plays once and is then finished.
    pub looped: bool,
    /// `notes` is the pitch lane; other lanes set parameters or universal
    /// controls note by note.
    pub lanes: Vec<(String, Pattern<Step>)>,
    /// Exact timed notes from `events { }` or `load(...)`.
    pub scored: Vec<ScoredNote>,
    /// `keys: midi.keys(channel: 1)`: played from a keyboard instead of by `notes`.
    pub keys: Option<Keys>,
    /// Lanes drawn as an envelope, `cut: env[300hz 1bar 3khz]`: they move a node's
    /// parameter continuously, on the clip's timeline.
    pub curves: Vec<(String, EnvLane)>,
    /// Lanes that are signals (`cut: sine.range(300hz, 3khz)`), once placed on a node: the
    /// parameter's unit. They move the parameter continuously instead of stepping it at each note.
    pub signals: Vec<(String, niminal_lang::Unit)>,
    /// Explicit fx parameter lanes, with their units and default smoothing.
    pub fx_params: Vec<(String, niminal_lang::Unit, f64)>,
}

/// An `env[...]` in a lane: a run of glides between levels, from the start of the clip.
#[derive(Clone, Debug)]
pub struct EnvLane {
    pub start: Value,
    /// How long to take, the curve, and the level reached.
    pub segments: Vec<(Time, Curve, Value)>,
    /// Starts over when it ends, instead of holding its last level.
    pub looped: bool,
    /// The unit of the parameter it moves, once it is placed on a node.
    pub unit: Option<niminal_lang::Unit>,
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

    /// This clip's lanes over one pass, for drawing: each note's place in the clip, in cycles from
    /// the start of the window, in the order and with the gaps the patterns (and any changes made to
    /// them) give. An endless clip has no end to draw, so the cycle it is playing now is drawn.
    pub fn lane_preview(&self, cycles: f64) -> Vec<LanePreview> {
        // A finite clip starts its patterns again each pass, so the window is the whole clip. An
        // endless one keeps counting, so draw the cycle it is in: `every` and `<a b>` are visible.
        let (base, end) = if self.endless {
            let base = cycles.floor().max(0.0);
            let base = Rational::int(base as i64);
            (base, base + Rational::ONE)
        } else {
            (Rational::ZERO, self.length)
        };
        let offset = base.to_f64();
        let window = (end - base).to_f64();
        let mut lanes: Vec<LanePreview> = self
            .lanes
            .iter()
            .map(|(name, pattern)| {
                let (haps, truncated) =
                    with_budget(4096, || pattern.query(Span::new(base, end)));
                let mut events: Vec<LaneEvent> = haps
                    .into_iter()
                    .filter_map(|hap| {
                        let whole = hap.whole?;
                        if whole.start < base || whole.start >= end {
                            return None;
                        }
                        let start = (whole.start.to_f64() - offset).clamp(0.0, window);
                        let stop = (whole.end.to_f64() - offset).clamp(start, window);
                        Some(LaneEvent {
                            start,
                            dur: stop - start,
                            value: hap.value.value,
                            gain: hap.value.gain,
                            pan: hap.value.pan,
                            src: hap.value.src.map(|(_, start, end)| (start, end)),
                            eval: hap.value.src.map(|(eval, ..)| eval),
                        })
                    })
                    .collect();
                events.sort_by(|a, b| a.start.total_cmp(&b.start));
                LanePreview {
                    name: name.clone(),
                    events,
                    truncated,
                }
            })
            .collect();
        if !self.scored.is_empty() && offset == 0.0 {
            lanes.push(LanePreview {
                name: "notes".into(),
                events: self
                    .scored
                    .iter()
                    .map(|note| LaneEvent {
                        start: note.at.to_f64(),
                        dur: note.dur.to_f64(),
                        value: note.step.value,
                        gain: note.step.gain,
                        pan: note.step.pan,
                        src: note.step.src.map(|(_, start, end)| (start, end)),
                        eval: note.step.src.map(|(eval, ..)| eval),
                    })
                    .collect(),
                truncated: false,
            });
        }
        lanes
    }
}

/// One lane of a clip, as an editor draws it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LanePreview {
    pub name: String,
    /// The lane's events, earliest first, in cycles from the start of the clip.
    pub events: Vec<LaneEvent>,
    /// There were more events than an editor can draw.
    pub truncated: bool,
}

/// One event in a lane preview: where it starts and how long it lasts, in cycles.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LaneEvent {
    pub start: f64,
    pub dur: f64,
    pub value: Value,
    pub gain: Option<f64>,
    pub pan: Option<f64>,
    /// Where the note is written in the source, as byte offsets.
    pub src: Option<(usize, usize)>,
    /// Which evaluation sent that text.
    pub eval: Option<u64>,
}

/// A named list of clips, started and stopped together. Clips are nodes, by index.
#[derive(Clone)]
pub struct Scene {
    pub name: String,
    pub clips: Vec<usize>,
}

/// What a command does. Nodes are indices into `Program::nodes`.
#[derive(Clone)]
pub enum Action {
    /// Start a clip. Nothing happens if it is already running.
    Start {
        node: usize,
        clip: Clip,
    },
    /// Change what a running clip plays, keeping its place in the loop. A clip that is not running starts.
    Update {
        node: usize,
        clip: Clip,
    },
    Stop {
        node: usize,
    },
    Mute {
        node: usize,
    },
    Unmute {
        node: usize,
    },
    Solo {
        node: usize,
    },
    /// Clear all solos, or just this node's.
    Unsolo {
        node: Option<usize>,
    },
    /// Stop every node, letting notes ring out.
    Hush,
    /// Stop everything at once.
    Panic,
    /// Stop every node, and count bars from here. Only a live session can do the
    /// counting; a render treats it as `hush`.
    Reset,
    /// `song.stop`: drop what the arrangement `song` has not got to yet. Only a live session can; a render
    /// has nothing to drop.
    Cancel {
        arrangement: String,
    },
    /// Write the last `bars` of playing clips to `score/{name}.nms`. Only a live
    /// session does the write; a render ignores it.
    Capture {
        bars: f64,
        name: String,
    },
}

/// Which grid a change waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizeRelation {
    /// Land on the grid: `@ bar`, `@ 4bars`.
    OnGrid,
    /// The next line of the grid: `@ next 4bars`.
    Next,
    /// Counted from now: `@ in 4bars`.
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
    /// Envelopes moving part parameters over the piece: the control's name (`bass::tone`) and the shape.
    pub lanes: Vec<(String, EnvLane)>,
}
