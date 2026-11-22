//! Turning the performance layer into notes.
//!
//! A [`Schedule`] knows which clip plays on which part and when, and which
//! nodes are muted, and can say what notes fall in any stretch of time. It is
//! stateless between calls, so a renderer can ask for the whole piece at once
//! and a live session can ask for a short lookahead window, and both get the
//! same notes.

use std::fmt;

use niminal_pattern::{Rational, Span as CycleSpan};
use niminal_score::{Event, Tempo, Time, Value};

use crate::compile::Composition;
use crate::performance::{Action, Clip, EnvLane, Scheduled};

/// The pattern queries and events one clip may spend in a search: a base
/// amount plus more for a longer stretch of time.
/// The notes one clip may start: a base amount plus so many for each second searched.
const EVENTS_BASE: usize = 16;
const EVENTS_PER_SECOND: f64 = 800.0;
const BASE_WORK: usize = 2_000;
const WORK_PER_SECOND: f64 = 2_000.0;

/// The most notes one search of the schedule keeps: the earliest ones.
const MAX_EVENTS: usize = 2_048;

/// The most control changes one search keeps: a looping envelope can ask for a great many.
const MAX_CONTROL_SETS: usize = 50_000;

/// A curved envelope segment is cut into this many glides at most, and at least this many.
const MIN_PIECES: usize = 4;
const MAX_PIECES: usize = 32;
/// About one glide for each this many seconds of a curved segment.
const PIECE_SECONDS: f64 = 0.02;

/// How much time one search covers when asked for a long stretch.
const CHUNK_SECONDS: f64 = 8.0;

/// How often a signal lane is read, in seconds: the parameter glides from one reading to the next.
const SIGNAL_STEP: f64 = 0.02;

/// A part parameter moving because an envelope lane says so: from `at` seconds it glides to
/// `target` (in the control's engine units) over `glide` seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlSet {
    pub at: f64,
    /// The control's name, such as `bass::tone`.
    pub name: String,
    pub target: f64,
    pub glide: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleError(pub String);

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScheduleError {}

/// How a clip's cycles (bars) line up with seconds. The tempo may change while the clip plays, as
/// it does in a gradual change over an arrangement, so this is a run of stretches, each with its own
/// cycle length.
struct Clock {
    /// Where each stretch starts in seconds and in cycles since the clip started, and how long a cycle is in it.
    pieces: Vec<(f64, f64, f64)>,
}

impl Clock {
    /// A clip starting at `start` in `tempo`, with the tempo changing to those of `later` (seconds, tempo)
    /// as time goes on.
    fn new(start: f64, tempo: Tempo, later: &[(f64, Tempo)]) -> Clock {
        let cycle = |t: Tempo| t.beats_per_bar * 60.0 / t.bpm;
        let mut pieces = vec![(start, 0.0, cycle(tempo))];
        for (t, tempo) in later.iter().filter(|(t, _)| *t > start) {
            let &(t0, c0, spc) = pieces.last().expect("one piece to begin with");
            pieces.push((*t, c0 + (t - t0) / spc, cycle(*tempo)));
        }
        Clock { pieces }
    }

    fn cycles_at(&self, seconds: f64) -> f64 {
        let &(t0, c0, spc) = self
            .pieces
            .iter()
            .rev()
            .find(|p| p.0 <= seconds)
            .unwrap_or(&self.pieces[0]);
        c0 + (seconds - t0) / spc
    }

    fn seconds_at(&self, cycles: f64) -> f64 {
        let &(t0, c0, spc) = self
            .pieces
            .iter()
            .rev()
            .find(|p| p.1 <= cycles)
            .unwrap_or(&self.pieces[0]);
        t0 + (cycles - c0) * spc
    }
}

/// The passes of a clip, counting from 0, that overlap the cycles `first..last` since it began. An endless
/// clip is one long pass, and a clip that plays `times: n` has just the n.
fn loops(clip: &Clip, first: f64, last: f64) -> (i64, i64) {
    if clip.endless {
        return (0, 1);
    }
    let length = clip.length.to_f64();
    let first_loop = (first / length).floor().max(0.0) as i64;
    let last_loop = ((last / length).ceil() as i64).max(1);
    match clip.plays {
        None => (first_loop, last_loop),
        Some(n) => (first_loop.min(i64::from(n)), last_loop.min(i64::from(n))),
    }
}

/// A clip playing on a part from `start` until `end` (seconds).
struct Segment {
    /// The tempo and meter it plays in when it starts.
    tempo: Tempo,
    clock: Clock,
    node: usize,
    clip: Clip,
    start: f64,
    end: Option<f64>,
    /// When the clip first began, which is earlier than `start` if it was changed while it played:
    /// its cycles are counted from here.
    origin: f64,
}

impl Segment {
    /// Has a clip that plays once come to its end by `at`?
    fn finished(&self, at: f64) -> bool {
        self.clip.plays.is_some_and(|n| {
            !self.clip.endless
                && self.clock.cycles_at(at) >= self.clip.length.to_f64() * f64::from(n)
        })
    }
}

#[derive(Clone, Copy)]
enum Gate {
    Mute(usize),
    Unmute(usize),
    Solo(usize),
    Unsolo(Option<usize>),
    /// `reset` and `new`: nothing is muted or soloed any more.
    Clear,
}

/// A clip that will play its last pass, and what it asks for then.
#[derive(Debug, Clone, PartialEq)]
pub struct Finish {
    pub node: usize,
    /// When the clip began, which tells one run of a clip from the next.
    pub origin: f64,
    /// When its last pass ends, in seconds.
    pub at: f64,
    /// The statements from its `then:`.
    pub then: Vec<String>,
}

/// What a part is doing at a moment, for showing in an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartState {
    pub clip: Option<String>,
    pub muted: bool,
    pub soloed: bool,
}

/// How close to the end of a clip a note may start and still count as after it, in seconds.
const END_SLACK: f64 = 1e-9;

/// Padding, in cycles, around the part of a clip that a search looks at.
const PAD_CYCLES: f64 = 1e-4;

/// A time rounded to the nearest nanosecond. A note on the edge of a window can come out a hair either side
/// of it depending on how the tempo changes before it were added up, and the window before it and the one after
/// must agree which of them it is in, or it is in neither.
fn snap(seconds: f64) -> f64 {
    (seconds * 1e9).round() / 1e9
}

trait Micro {
    fn floor_to_micro(self) -> i64;
    fn ceil_to_micro(self) -> i64;
}

impl Micro for f64 {
    fn floor_to_micro(self) -> i64 {
        (self * 1e6).floor() as i64
    }

    fn ceil_to_micro(self) -> i64 {
        (self * 1e6).ceil() as i64
    }
}

/// A time in millionths of a cycle, as an exact fraction.
fn micro_cycles(micro: i64) -> Rational {
    Rational::new(micro, 1_000_000)
}

/// What a search of the schedule found.
pub struct Found {
    pub events: Vec<Event>,
    /// Envelope lanes moving part parameters in the same stretch, in time order.
    pub controls: Vec<ControlSet>,
    pub problem: Option<ScheduleError>,
    /// Nodes whose clips used up their allowance.
    pub overloaded: Vec<usize>,
}

pub struct Schedule {
    node_names: Vec<String>,
    segments: Vec<Segment>,
    /// Mute and solo changes, in time order.
    gates: Vec<(f64, Gate)>,
    panics: Vec<f64>,
    /// The tempo the piece starts in, and where its sections change it.
    tempo: Tempo,
    sections: Vec<(f64, Tempo)>,
    /// Envelopes over the whole piece (an arrangement's lanes): the control they move and the shape.
    lanes: Vec<(String, EnvLane, Time)>,
}

impl Composition {
    /// The notes this program's `play`, `launch` and related commands produce.
    pub fn schedule(&self) -> Schedule {
        Schedule::new(
            self.tempo,
            self.nodes.iter().map(|t| t.name.clone()).collect(),
            &self.performance,
            &[],
            &[],
        )
    }
}

impl Composition {
    /// The notes of the arrangement called `name`, played from the start.
    pub fn arrangement_schedule(&self, name: &str) -> Result<Schedule, niminal_lang::ArgError> {
        let Some(arrangement) = self.arrangements.iter().find(|a| a.name == name) else {
            return Err(niminal_lang::ArgError {
                message: format!("no arrangement named `{name}`"),
                help: match niminal_lang::closest(
                    name,
                    self.arrangements.iter().map(|a| a.name.as_str()),
                ) {
                    Some(c) => Some(format!("did you mean `{c}`?")),
                    None if self.arrangements.is_empty() => {
                        Some("this file has no `arrangement`".into())
                    }
                    None => None,
                },
            });
        };
        Ok(Schedule::new(
            self.tempo,
            self.nodes.iter().map(|t| t.name.clone()).collect(),
            &arrangement.actions,
            &arrangement.tempos,
            &arrangement.lanes,
        ))
    }
}

impl Schedule {
    /// A schedule from commands directly. Node indices in `commands` index
    /// `node_names`. The commands' `at` times are what they are; their
    /// `quantize` is ignored (a live session has already applied it).
    pub fn from_commands(
        tempo: Tempo,
        node_names: Vec<String>,
        commands: &[Scheduled],
    ) -> Schedule {
        Schedule::new(tempo, node_names, commands, &[], &[])
    }

    /// `sections` says where the tempo and meter change from `tempo`, as seconds and what they become.
    fn new(
        tempo: Tempo,
        node_names: Vec<String>,
        commands: &[Scheduled],
        sections: &[(f64, Tempo)],
        lanes: &[(String, EnvLane, Time)],
    ) -> Schedule {
        let tempo_at = |at: f64| {
            sections
                .iter()
                .rev()
                .find(|(start, _)| *start <= at)
                .map_or(tempo, |(_, t)| *t)
        };
        let mut timed: Vec<(f64, &Action)> = commands
            .iter()
            .map(|c| (c.at.to_seconds(tempo), &c.action))
            .collect();
        // Simultaneous commands keep the order they were written in.
        timed.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut segments: Vec<Segment> = Vec::new();
        let mut gates = Vec::new();
        let mut panics = Vec::new();
        // lanes the arrangement placed, and lanes that commands started (a live arrangement's)
        let mut placed_lanes = lanes.to_vec();

        let close = |segments: &mut Vec<Segment>, node: Option<usize>, at: f64| {
            for s in segments
                .iter_mut()
                .filter(|s| s.end.is_none() && node.is_none_or(|t| t == s.node))
            {
                s.end = Some(at.max(s.start));
            }
        };
        // Clips that a `stop` closed, and where: a `start` for the same clip at the very same moment
        // undoes it, so a clip in two scenes carries on when one is swapped for the other.
        let mut stopped: Vec<(usize, f64, usize)> = Vec::new();

        for (at, action) in timed {
            match action {
                Action::Start { node, clip } => {
                    if let Some(k) = stopped.iter().position(|(n, t, _)| n == node && *t == at) {
                        let (_, _, i) = stopped.remove(k);
                        segments[i].end = None;
                        continue;
                    }
                    let running = segments
                        .iter()
                        .any(|s| s.node == *node && s.end.is_none() && !s.finished(at));
                    if running {
                        continue;
                    }
                    close(&mut segments, Some(*node), at);
                    segments.push(Segment {
                        node: *node,
                        clip: clip.clone(),
                        start: at,
                        end: None,
                        tempo: tempo_at(at),
                        clock: Clock::new(at, tempo_at(at), sections),
                        origin: at,
                    });
                }
                Action::Update { node, clip } => {
                    // the new version carries on from where the old one had got to
                    let (origin, begun) = segments
                        .iter()
                        .rev()
                        .find(|s| s.node == *node && s.end.is_none())
                        .map_or((at, tempo_at(at)), |s| (s.origin, s.tempo));
                    close(&mut segments, Some(*node), at);
                    segments.push(Segment {
                        node: *node,
                        clip: clip.clone(),
                        start: at,
                        end: None,
                        tempo: begun,
                        clock: Clock::new(origin, begun, sections),
                        origin,
                    });
                }
                Action::Stop { node } => {
                    if let Some(i) = segments
                        .iter()
                        .rposition(|s| s.node == *node && s.end.is_none())
                    {
                        stopped.push((*node, at, i));
                    }
                    close(&mut segments, Some(*node), at);
                }
                Action::Hush => close(&mut segments, None, at),
                Action::Reset => {
                    close(&mut segments, None, at);
                    gates.push((at, Gate::Clear));
                }
                Action::Panic => {
                    close(&mut segments, None, at);
                    panics.push(at);
                }
                Action::Mute { node } => gates.push((at, Gate::Mute(*node))),
                Action::Unmute { node } => gates.push((at, Gate::Unmute(*node))),
                Action::Solo { node } => gates.push((at, Gate::Solo(*node))),
                Action::Unsolo { node } => gates.push((at, Gate::Unsolo(*node))),
                Action::Lane { control, lane } => {
                    placed_lanes.push((control.clone(), lane.clone(), Time::Seconds(at)));
                }
                Action::Capture { .. } | Action::Cancel { .. } => {}
            }
        }
        Schedule {
            node_names,
            segments,
            gates,
            panics,
            tempo,
            sections: sections.to_vec(),
            lanes: placed_lanes,
        }
    }

    /// When the last clip stops, in seconds; `None` if something plays until
    /// stopped, in which case a render must be given a length.
    pub fn end(&self) -> Option<f64> {
        let mut end = 0.0f64;
        for s in &self.segments {
            end = end.max(s.end?);
        }
        Some(end)
    }

    /// The clips with a `then:` that play through to their last pass, and when that ends. A clip that
    /// is stopped or replaced before then is not among them.
    pub fn finishes(&self) -> Vec<Finish> {
        self.segments
            .iter()
            .filter_map(|s| {
                let n = s.clip.plays?;
                if s.clip.endless || s.clip.then.is_empty() {
                    return None;
                }
                let at = s.clock.seconds_at(s.clip.length.to_f64() * f64::from(n));
                s.end
                    .is_none_or(|end| at <= end + END_SLACK)
                    .then(|| Finish {
                        node: s.node,
                        origin: s.origin,
                        at,
                        then: s.clip.then.clone(),
                    })
            })
            .collect()
    }

    /// The times at which `panic` silences everything.
    pub fn panics(&self) -> &[f64] {
        &self.panics
    }

    /// Does anything play at all?
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The nodes that are muted and soloed at `at`.
    fn gate_state(&self, at: f64) -> (Vec<usize>, Vec<usize>) {
        let mut muted: Vec<usize> = Vec::new();
        let mut soloed: Vec<usize> = Vec::new();
        for (when, gate) in &self.gates {
            if *when > at {
                break;
            }
            match *gate {
                Gate::Mute(t) => {
                    if !muted.contains(&t) {
                        muted.push(t);
                    }
                }
                Gate::Unmute(t) => muted.retain(|m| *m != t),
                Gate::Solo(t) => {
                    if !soloed.contains(&t) {
                        soloed.push(t);
                    }
                }
                Gate::Unsolo(None) => soloed.clear(),
                Gate::Unsolo(Some(t)) => soloed.retain(|s| *s != t),
                Gate::Clear => {
                    muted.clear();
                    soloed.clear();
                }
            }
        }
        (muted, soloed)
    }

    fn audible(&self, node: usize, at: f64) -> bool {
        let (muted, soloed) = self.gate_state(at);
        !muted.contains(&node) && (soloed.is_empty() || soloed.contains(&node))
    }

    /// How far into its loops the clip running on `node` is at `at` seconds: the cycles since it began
    /// and the length of a loop in cycles. `None` if nothing is running there.
    pub fn loop_position(&self, node: usize, at: f64) -> Option<(f64, f64)> {
        let s = self
            .segments
            .iter()
            .rev()
            .find(|s| s.node == node && s.start <= at && s.end.is_none_or(|e| at < e))
            .filter(|s| !s.finished(at))?;
        Some((s.clock.cycles_at(at), s.clip.length.to_f64()))
    }

    /// The time in seconds when the clip running on `node` at `at` has been going for `cycles`.
    pub fn time_at_cycles(&self, node: usize, at: f64, cycles: f64) -> Option<f64> {
        let s = self
            .segments
            .iter()
            .rev()
            .find(|s| s.node == node && s.start <= at && s.end.is_none_or(|e| at < e))?;
        Some(s.clock.seconds_at(cycles))
    }

    /// The clip playing on `node` at `at` seconds, after any changes made to it live.
    pub fn clip_at(&self, node: usize, at: f64) -> Option<&Clip> {
        self.segments
            .iter()
            .rev()
            .find(|s| s.node == node && s.start <= at && s.end.is_none_or(|e| at < e))
            .filter(|s| !s.finished(at))
            .map(|s| &s.clip)
    }

    /// What `node` is doing at `at` seconds: the clip playing on it, and
    /// whether it was muted or soloed.
    pub fn node_state(&self, node: usize, at: f64) -> PartState {
        let (muted, soloed) = self.gate_state(at);
        let playing = self
            .segments
            .iter()
            .rev()
            .find(|s| s.node == node && s.start <= at && s.end.is_none_or(|e| at < e))
            .filter(|s| !s.finished(at));
        PartState {
            clip: playing.map(|s| s.clip.name.clone()),
            muted: muted.contains(&node),
            soloed: soloed.contains(&node),
        }
    }

    /// Is a clip played from an MPE keyboard playing on any part at `at` seconds?
    pub fn plays_mpe(&self, at: f64) -> bool {
        self.segments.iter().any(|s| {
            s.start <= at && s.end.is_none_or(|e| at < e) && s.clip.keys.is_some_and(|k| k.mpe)
        })
    }

    /// The note a key press plays on `node` at `at` seconds, if a clip played from a keyboard is
    /// playing there, on this MIDI channel, and the part is audible. `note` is the MIDI note
    /// number and `gain` how loud, in db. The clip's other lanes give the note their value at the
    /// start of a cycle.
    pub fn key_event(
        &self,
        node: usize,
        at: f64,
        channel: u8,
        note: i32,
        gain: f64,
    ) -> Option<Event> {
        let segment = self
            .segments
            .iter()
            .rev()
            .find(|s| s.node == node && s.start <= at && s.end.is_none_or(|e| at < e))?;
        segment.clip.keys.filter(|k| k.accepts(channel))?;
        if !self.audible(node, at) {
            return None;
        }
        let mut args = std::collections::BTreeMap::new();
        for (lane, pattern) in &segment.clip.lanes {
            if matches!(lane.as_str(), "dur" | "nudge")
                || segment.clip.signals.iter().any(|(l, _)| l == lane)
            {
                continue;
            }
            if let Some(hap) = pattern
                .query(CycleSpan::new(Rational::ZERO, Rational::ZERO))
                .into_iter()
                .next()
            {
                args.insert(lane.clone(), hap.value.value);
            }
        }
        args.insert(crate::perform::PITCH_PARAM.to_string(), Value::Note(note));
        args.insert(niminal_lang::GAIN_PARAM.to_string(), Value::Db(gain));
        Some(Event {
            target: self.node_names[node].clone(),
            at: Time::Seconds(at),
            dur: Time::Seconds(0.0),
            args,
        })
    }
}

mod query;
#[cfg(test)]
mod tests;
