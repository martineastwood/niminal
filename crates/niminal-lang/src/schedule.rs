//! Turning the performance layer into notes.
//!
//! A [`Schedule`] knows which clip plays on which track and when, and which
//! tracks are muted, and can say what notes fall in any stretch of time. It is
//! stateless between calls, so a renderer can ask for the whole piece at once
//! and a live session can ask for a short lookahead window, and both get the
//! same notes.

use std::fmt;

use niminal_pattern::{Rational, Span as CycleSpan};
use niminal_score::{Event, Tempo, Time, Value};

use crate::performance::{Action, Clip, EnvLane, Scheduled, Step};
use crate::program::Program;

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

/// A track parameter moving because an envelope lane says so: from `at` seconds it glides to
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
        let &(t0, c0, spc) = self.pieces.iter().rev().find(|p| p.0 <= seconds).unwrap_or(&self.pieces[0]);
        c0 + (seconds - t0) / spc
    }

    fn seconds_at(&self, cycles: f64) -> f64 {
        let &(t0, c0, spc) = self.pieces.iter().rev().find(|p| p.1 <= cycles).unwrap_or(&self.pieces[0]);
        t0 + (cycles - c0) * spc
    }
}

/// A clip playing on a track from `start` until `end` (seconds).
struct Segment {
    /// The tempo and meter it plays in when it starts.
    tempo: Tempo,
    clock: Clock,
    track: usize,
    clip: Clip,
    start: f64,
    end: Option<f64>,
}

#[derive(Clone, Copy)]
enum Gate {
    Mute(usize),
    Unmute(usize),
    Solo(usize),
    Unsolo(Option<usize>),
}

/// What a track is doing at a moment, for showing in an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackState {
    pub clip: Option<String>,
    pub muted: bool,
    pub soloed: bool,
}

/// How close to the end of a clip a note may start and still count as after it, in seconds.
const END_SLACK: f64 = 1e-9;

/// Padding, in cycles, around the part of a clip that a search looks at.
const PAD_CYCLES: f64 = 1e-4;

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
    /// Envelope lanes moving track parameters in the same stretch, in time order.
    pub controls: Vec<ControlSet>,
    pub problem: Option<ScheduleError>,
    /// Tracks whose clips used up their allowance.
    pub overloaded: Vec<usize>,
}

pub struct Schedule {
    track_names: Vec<String>,
    segments: Vec<Segment>,
    /// Mute and solo changes, in time order.
    gates: Vec<(f64, Gate)>,
    panics: Vec<f64>,
    /// The tempo the piece starts in, and where its sections change it.
    tempo: Tempo,
    sections: Vec<(f64, Tempo)>,
    /// Envelopes over the whole piece (an arrangement's lanes): the control they move and the shape.
    lanes: Vec<(String, EnvLane)>,
}

impl Program {
    /// The notes this program's `play`, `launch` and related commands produce.
    pub fn schedule(&self) -> Schedule {
        Schedule::new(self.tempo, self.tracks.iter().map(|t| t.name.clone()).collect(), &self.performance, &[], &[])
    }
}

impl Program {
    /// The notes of the arrangement called `name`, played from the start.
    pub fn arrangement_schedule(&self, name: &str) -> Result<Schedule, crate::program::ArgError> {
        let Some(arrangement) = self.arrangements.iter().find(|a| a.name == name) else {
            return Err(crate::program::ArgError {
                message: format!("no arrangement named `{name}`"),
                help: match crate::diag::closest(name, self.arrangements.iter().map(|a| a.name.as_str())) {
                    Some(c) => Some(format!("did you mean `{c}`?")),
                    None if self.arrangements.is_empty() => Some("this file has no `arrangement`".into()),
                    None => None,
                },
            });
        };
        Ok(Schedule::new(self.tempo, self.tracks.iter().map(|t| t.name.clone()).collect(), &arrangement.actions, &arrangement.tempos, &arrangement.lanes))
    }
}

impl Schedule {
    /// A schedule from commands directly. Track indices in `commands` index
    /// `track_names`. The commands' `at` times are what they are; their
    /// `quantize` is ignored (a live session has already applied it).
    pub fn from_commands(tempo: Tempo, track_names: Vec<String>, commands: &[Scheduled]) -> Schedule {
        Schedule::new(tempo, track_names, commands, &[], &[])
    }

    /// `sections` says where the tempo and meter change from `tempo`, as seconds and what they become.
    fn new(tempo: Tempo, track_names: Vec<String>, commands: &[Scheduled], sections: &[(f64, Tempo)], lanes: &[(String, EnvLane)]) -> Schedule {
        let tempo_at = |at: f64| sections.iter().rev().find(|(start, _)| *start <= at).map_or(tempo, |(_, t)| *t);
        let mut timed: Vec<(f64, &Action)> = commands.iter().map(|c| (c.at.to_seconds(tempo), &c.action)).collect();
        // Simultaneous commands keep the order they were written in.
        timed.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut segments: Vec<Segment> = Vec::new();
        let mut gates = Vec::new();
        let mut panics = Vec::new();

        let close = |segments: &mut Vec<Segment>, track: Option<usize>, at: f64| {
            for s in segments.iter_mut().filter(|s| s.end.is_none() && track.is_none_or(|t| t == s.track)) {
                s.end = Some(at.max(s.start));
            }
        };

        for (at, action) in timed {
            match action {
                Action::Play { track, clip } => {
                    close(&mut segments, Some(*track), at);
                    segments.push(Segment { track: *track, clip: clip.clone(), start: at, end: None, tempo: tempo_at(at), clock: Clock::new(at, tempo_at(at), sections) });
                }
                Action::Stop { track } => close(&mut segments, Some(*track), at),
                Action::Hush | Action::Reset => close(&mut segments, None, at),
                Action::Panic => {
                    close(&mut segments, None, at);
                    panics.push(at);
                }
                Action::Mute { track } => gates.push((at, Gate::Mute(*track))),
                Action::Unmute { track } => gates.push((at, Gate::Unmute(*track))),
                Action::Solo { track } => gates.push((at, Gate::Solo(*track))),
                Action::Unsolo { track } => gates.push((at, Gate::Unsolo(*track))),
            }
        }
        Schedule { track_names, segments, gates, panics, tempo, sections: sections.to_vec(), lanes: lanes.to_vec() }
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

    /// The times at which `panic` silences everything.
    pub fn panics(&self) -> &[f64] {
        &self.panics
    }

    /// Does anything play at all?
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The tracks that are muted and soloed at `at`.
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
            }
        }
        (muted, soloed)
    }

    fn audible(&self, track: usize, at: f64) -> bool {
        let (muted, soloed) = self.gate_state(at);
        !muted.contains(&track) && (soloed.is_empty() || soloed.contains(&track))
    }

    /// What `track` is doing at `at` seconds: the clip playing on it, and
    /// whether it was muted or soloed.
    pub fn track_state(&self, track: usize, at: f64) -> TrackState {
        let (muted, soloed) = self.gate_state(at);
        let playing = self.segments.iter().rev().find(|s| s.track == track && s.start <= at && s.end.is_none_or(|e| at < e));
        TrackState {
            clip: playing.map(|s| s.clip.name.clone()),
            muted: muted.contains(&track),
            soloed: soloed.contains(&track),
        }
    }

    /// Is a clip played from an MPE keyboard playing on any track at `at` seconds?
    pub fn plays_mpe(&self, at: f64) -> bool {
        self.segments.iter().any(|s| s.start <= at && s.end.is_none_or(|e| at < e) && s.clip.keys.is_some_and(|k| k.mpe))
    }

    /// The note a key press plays on `track` at `at` seconds, if a clip played from a keyboard is
    /// playing there, on this MIDI channel, and the track is audible. `note` is the MIDI note
    /// number and `gain` how loud, in db. The clip's other lanes give the note their value at the
    /// start of a cycle.
    pub fn key_event(&self, track: usize, at: f64, channel: u8, note: i32, gain: f64) -> Option<Event> {
        let segment = self.segments.iter().rev().find(|s| s.track == track && s.start <= at && s.end.is_none_or(|e| at < e))?;
        segment.clip.keys.filter(|k| k.accepts(channel))?;
        if !self.audible(track, at) {
            return None;
        }
        let mut args = std::collections::BTreeMap::new();
        for (lane, pattern) in &segment.clip.lanes {
            if matches!(lane.as_str(), "dur" | "nudge") || segment.clip.signals.iter().any(|(l, _)| l == lane) {
                continue;
            }
            if let Some(hap) = pattern.query(CycleSpan::new(Rational::ZERO, Rational::ZERO)).into_iter().next() {
                args.insert(lane.clone(), hap.value.value);
            }
        }
        args.insert(crate::perform::PITCH_PARAM.to_string(), Value::Note(note));
        args.insert(crate::lower::GAIN_PARAM.to_string(), Value::Db(gain));
        Some(Event { target: self.track_names[track].clone(), at: Time::Seconds(at), dur: Time::Seconds(0.0), args })
    }

    /// Every change an envelope lane makes to a track parameter in `from..to` seconds, in time order.
    pub fn controls(&self, from: f64, to: f64) -> Result<Vec<ControlSet>, ScheduleError> {
        // a long stretch is searched a piece at a time, each with its own limit
        let mut all = Vec::new();
        let mut start = from;
        while start < to {
            let end = (start + CHUNK_SECONDS).min(to);
            let mut chunk = Vec::new();
            for segment in &self.segments {
                self.segment_controls(segment, start, end, &mut chunk)?;
            }
            self.lane_controls(start, end, &mut chunk)?;
            all.extend(chunk);
            start = end;
        }
        all.sort_by(|a, b| a.at.total_cmp(&b.at));
        Ok(all)
    }

    /// Every note that starts in `from..to` seconds, in time order. A pattern
    /// that asks for more work than a clip is allowed is an error.
    pub fn events(&self, from: f64, to: f64) -> Result<Vec<Event>, ScheduleError> {
        // A long stretch is searched a piece at a time, each with its own limits.
        let mut out = Vec::new();
        let mut start = from;
        while start < to {
            let end = (start + CHUNK_SECONDS).min(to);
            let found = self.events_lenient(start, end, &[]);
            match found.problem {
                None => out.extend(found.events),
                Some(problem) => return Err(problem),
            }
            start = end;
        }
        Ok(out)
    }

    /// Like [`Schedule::events`], but a live session would rather play what
    /// it can than nothing: this returns the notes found, the problem that
    /// cut the search short if any, and the tracks whose clips asked for more
    /// than they are allowed (which the caller should stop searching, since
    /// searching again costs the same). Tracks in `skip` are not searched.
    pub fn events_lenient(&self, from: f64, to: f64, skip: &[usize]) -> Found {
        let mut out: Vec<(f64, Event)> = Vec::new();
        let mut controls: Vec<ControlSet> = Vec::new();
        let mut problem = None;
        let mut overloaded = Vec::new();
        if let Err(e) = self.lane_controls(from, to, &mut controls) {
            problem.get_or_insert(e);
        }
        let work = BASE_WORK + (WORK_PER_SECOND * (to - from).max(0.0)) as usize;
        for segment in self.segments.iter().filter(|s| !skip.contains(&s.track)) {
            if let Err(e) = self.segment_controls(segment, from, to, &mut controls) {
                problem.get_or_insert(e);
            }
            let before = out.len();
            let (result, exhausted) =
                niminal_pattern::with_budget(work, || self.segment_events(segment, from, to, &mut out));
            if let Err(e) = result {
                problem.get_or_insert(e);
            }
            let allowed = EVENTS_BASE + (EVENTS_PER_SECOND * (to - from).max(0.0)) as usize;
            let too_many = out.len() - before > allowed;
            if too_many {
                out.truncate(before + allowed);
            }
            if exhausted || too_many {
                overloaded.push(segment.track);
                problem.get_or_insert_with(|| {
                    ScheduleError(format!(
                        "the clip on track `{}` plays more notes than niminal can schedule; it is stopped until you evaluate again",
                        self.track_names[segment.track]
                    ))
                });
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        if out.len() > MAX_EVENTS {
            out.truncate(MAX_EVENTS);
            problem.get_or_insert_with(|| {
                ScheduleError(format!("more than {MAX_EVENTS} notes start within {:.1} seconds; the latest are dropped", to - from))
            });
        }
        controls.sort_by(|a, b| a.at.total_cmp(&b.at));
        Found { events: out.into_iter().map(|(_, e)| e).collect(), controls, problem, overloaded }
    }

    fn segment_events(
        &self,
        segment: &Segment,
        from: f64,
        to: f64,
        out: &mut Vec<(f64, Event)>,
    ) -> Result<(), ScheduleError> {
        let Some(notes) = segment.clip.lane("notes") else { return Ok(()) };
        let clock = &segment.clock;
        let length = segment.clip.length.to_f64();
        let until = segment.end.map_or(to, |e| e.min(to));
        let window_start = from.max(segment.start);
        if window_start >= until || length <= 0.0 {
            return Ok(());
        }

        // The window in cycles since the clip started, and the loops of the clip that overlap it.
        let (first_cycle, last_cycle) = (clock.cycles_at(window_start), clock.cycles_at(until));
        let (first_loop, last_loop) = if segment.clip.endless {
            (0, 1)
        } else {
            ((first_cycle / length).floor().max(0.0) as i64, ((last_cycle / length).ceil() as i64).max(1))
        };

        for k in first_loop..last_loop {
            let base = if segment.clip.endless { 0.0 } else { k as f64 * length };
            // Only the part of the loop inside the window, padded a little so that
            // rounding can't lose a note on an edge; the check below decides.
            let a = (first_cycle - base).max(0.0) - PAD_CYCLES;
            let b = if segment.clip.endless { last_cycle - base } else { (last_cycle - base).min(length) } + PAD_CYCLES;
            let span = CycleSpan::new(
                micro_cycles(a.floor_to_micro()).max(Rational::ZERO),
                if segment.clip.endless { micro_cycles(b.ceil_to_micro()) } else { micro_cycles(b.ceil_to_micro()).min(segment.clip.length) },
            );
            if span.start >= span.end {
                continue;
            }
            for hap in notes.onsets(span) {
                let whole = hap.whole.expect("notes have a duration");
                let onset = clock.seconds_at(base + whole.start.to_f64());
                if onset < window_start || onset >= until {
                    continue;
                }
                // a note meant for the first instant after the clip ends can land a hair early, when the
                // end was found by adding up the steps of a gradual tempo change
                if segment.end.is_some_and(|end| onset >= end - END_SLACK) {
                    continue;
                }
                let track = segment.track;
                if !self.audible(track, onset) {
                    continue;
                }
                out.push((onset, self.note(segment, whole, base, &hap.value, onset)?));
            }
        }
        Ok(())
    }

    /// What the piece's own envelope lanes (an arrangement's) do to track parameters in `from..to`.
    fn lane_controls(&self, from: f64, to: f64, out: &mut Vec<ControlSet>) -> Result<(), ScheduleError> {
        for (control, lane) in &self.lanes {
            let fail = |why: String| ScheduleError(format!("arrangement lane `{control}`: {why}"));
            let unit = lane.unit.ok_or_else(|| fail("the envelope was never placed on a parameter".into()))?;
            let level = |v: Value| crate::program::engine_value(unit, v, self.tempo).map_err(|e| fail(e.to_string()));
            let start = level(lane.start)?;
            let mut steps = Vec::with_capacity(lane.segments.len());
            let mut at = 0.0;
            for (dur, shape, to_level) in &lane.segments {
                let seconds = self.seconds_from(at, *dur);
                steps.push((seconds, *shape, level(*to_level)?));
                at += seconds;
            }
            emit_envelope(control, start, &steps, lane.looped, 0.0, from.max(0.0), to, out);
            if out.len() > MAX_CONTROL_SETS {
                return Err(fail(format!("it changes the parameter more than {MAX_CONTROL_SETS} times in {:.1} seconds", to - from)));
            }
        }
        Ok(())
    }

    /// How many seconds `dur` takes when it starts `at` seconds into the piece, following the
    /// tempo and meter of the sections it runs through.
    fn seconds_from(&self, at: f64, dur: Time) -> f64 {
        let (mut left, bars) = match dur {
            Time::Seconds(s) => return s.max(0.0),
            Time::Beats(b) => (b.max(0.0), false),
            Time::Bars(b) => (b.max(0.0), true),
        };
        let mut t = at;
        loop {
            let tempo = self.sections.iter().rev().find(|(start, _)| *start <= t).map_or(self.tempo, |(_, tempo)| *tempo);
            let per_second = tempo.bpm / 60.0 / if bars { tempo.beats_per_bar } else { 1.0 };
            match self.sections.iter().find(|(start, _)| *start > t) {
                Some((next, _)) if left > (next - t) * per_second => {
                    left -= (next - t) * per_second;
                    t = *next;
                }
                _ => return t + left / per_second - at,
            }
        }
    }

    /// What `segment`'s envelope lanes do to their track's parameters in `from..to` seconds. A curved
    /// segment becomes a few straight glides that follow the same curve an `env` in an instrument does.
    fn segment_controls(&self, segment: &Segment, from: f64, to: f64, out: &mut Vec<ControlSet>) -> Result<(), ScheduleError> {
        if segment.clip.curves.is_empty() && segment.clip.signals.is_empty() {
            return Ok(());
        }
        let clock = &segment.clock;
        let length = segment.clip.length.to_f64();
        let until = segment.end.map_or(to, |e| e.min(to));
        let window_start = from.max(segment.start);
        if window_start >= until || length <= 0.0 {
            return Ok(());
        }
        let (first_cycle, last_cycle) = (clock.cycles_at(window_start), clock.cycles_at(until));
        let (first_loop, last_loop) = if segment.clip.endless {
            (0, 1)
        } else {
            ((first_cycle / length).floor().max(0.0) as i64, ((last_cycle / length).ceil() as i64).max(1))
        };
        let name = &self.track_names[segment.track];
        let clip = &segment.clip;

        // signal lanes: the parameter follows the signal, read every SIGNAL_STEP seconds
        for (lane, unit) in &clip.signals {
            let Some(pattern) = clip.lane(lane) else { continue };
            let fail = |why: String| ScheduleError(format!("clip `{}` on track `{name}`, lane `{lane}`: {why}", clip.name));
            let control = format!("{name}::{lane}");
            let first = ((window_start / SIGNAL_STEP).floor() * SIGNAL_STEP).max(segment.start);
            let mut at = first;
            while at < until {
                if at >= window_start {
                    // where this moment falls in the clip, which starts over if it has a length
                    let into = clock.cycles_at(at);
                    let in_loop = if clip.endless { into } else { into.rem_euclid(length) };
                    let place = Rational::from_f64(in_loop);
                    if let Some(hap) = pattern.query(CycleSpan::new(place, place)).into_iter().next() {
                        let target = crate::program::engine_value(*unit, hap.value.value, segment.tempo).map_err(|e| fail(e.to_string()))?;
                        out.push(ControlSet { at, name: control.clone(), target, glide: SIGNAL_STEP });
                    }
                    if out.len() > MAX_CONTROL_SETS {
                        return Err(fail(format!("it changes the parameter more than {MAX_CONTROL_SETS} times in {:.1} seconds", to - from)));
                    }
                }
                at += SIGNAL_STEP;
            }
        }

        for (lane, curve) in &clip.curves {
            let fail = |why: String| ScheduleError(format!("clip `{}` on track `{name}`, lane `{lane}`: {why}", clip.name));
            let unit = curve.unit.ok_or_else(|| fail("the envelope was never placed on a track".into()))?;
            let level = |v: Value| crate::program::engine_value(unit, v, segment.tempo).map_err(|e| fail(e.to_string()));
            let start = level(curve.start)?;
            let mut steps = Vec::with_capacity(curve.segments.len());
            for (dur, shape, to_level) in &curve.segments {
                steps.push((dur.to_seconds(segment.tempo).max(0.0), *shape, level(*to_level)?));
            }
            let control = format!("{name}::{lane}");

            for k in first_loop..last_loop {
                let loop_start = if clip.endless { segment.start } else { clock.seconds_at(k as f64 * length) };
                // nothing from this pass of the clip is heard after the clip starts over
                let stop = if clip.endless { until } else { until.min(clock.seconds_at((k + 1) as f64 * length)) };
                emit_envelope(&control, start, &steps, curve.looped, loop_start, window_start, stop, out);
                if out.len() > MAX_CONTROL_SETS {
                    return Err(fail(format!("it changes the parameter more than {MAX_CONTROL_SETS} times in {:.1} seconds", to - from)));
                }
            }
        }
        Ok(())
    }

    /// One note: its pitch from the `notes` lane, and every other lane sampled
    /// where the note starts.
    fn note(
        &self,
        segment: &Segment,
        whole: CycleSpan,
        base: f64,
        step: &Step,
        onset: f64,
    ) -> Result<Event, ScheduleError> {
        let pitch = &step.value;
        let name = &self.track_names[segment.track];
        let clip = &segment.clip;

        let mut args = std::collections::BTreeMap::new();
        match pitch {
            Value::Note(_) | Value::Hz(_) => args.insert(crate::perform::PITCH_PARAM.to_string(), *pitch),
            Value::Sample(_) => args.insert("sample".to_string(), *pitch),
            other => {
                return Err(ScheduleError(format!(
                    "clip `{}` on track `{name}`: a note must be a note name or a frequency, found `{other}`",
                    clip.name
                )));
            }
        };
        let mut dur = Time::Seconds(segment.clock.seconds_at(base + whole.end.to_f64()) - onset);
        let mut at = onset;

        for (lane, pattern) in &clip.lanes {
            if lane == "notes" || clip.signals.iter().any(|(l, _)| l == lane) {
                continue;
            }
            // The value in force at the moment the note starts.
            let Some(hap) = pattern.query(CycleSpan::new(whole.start, whole.start)).into_iter().next() else {
                continue;
            };
            match (lane.as_str(), hap.value.value) {
                ("dur", v) => {
                    dur = v.as_time().map_err(|e| lane_error(clip, name, lane, &e.to_string()))?;
                }
                ("nudge", v) => {
                    let nudge = v.as_time().map_err(|e| lane_error(clip, name, lane, &e.to_string()))?;
                    at = (at + nudge.to_seconds(segment.tempo)).max(0.0);
                }
                (_, v) => {
                    args.insert(lane.clone(), v);
                }
            }
        }
        // controls that belong to this note alone win over the lanes
        if let Some(db) = step.gain {
            args.insert(crate::lower::GAIN_PARAM.to_string(), Value::Db(db));
        }
        if let Some(degrees) = step.pan {
            args.insert("pan".to_string(), Value::Degrees(degrees));
        }
        if let Some(share) = step.begin {
            args.insert("begin".to_string(), Value::Num(share));
        }
        Ok(Event { target: name.clone(), at: Time::Seconds(at), dur, args })
    }
}

/// Play an envelope that starts at `origin` seconds: its first level at once, then a glide for each
/// segment, a curved one cut into a few straight pieces that follow the curve an instrument's `env` has.
/// Only changes from `window_start` up to `stop` are kept. A looping envelope starts over when it ends.
#[allow(clippy::too_many_arguments)]
fn emit_envelope(
    control: &str,
    start: f64,
    steps: &[(f64, niminal_engine::ops::Curve, f64)],
    looped: bool,
    origin: f64,
    window_start: f64,
    stop: f64,
    out: &mut Vec<ControlSet>,
) {
    let pass_len: f64 = steps.iter().map(|s| s.0).sum();
    let (first_pass, passes) = if looped && pass_len > 0.0 {
        let first = ((window_start - origin) / pass_len).floor().max(0.0);
        (first as u64, u64::MAX)
    } else {
        (0, 1)
    };
    for pass in first_pass..first_pass.saturating_add(passes) {
        let pass_start = origin + pass as f64 * pass_len;
        if pass_start >= stop || out.len() > MAX_CONTROL_SETS {
            break;
        }
        let mut push = |at: f64, target: f64, glide: f64| {
            if at >= window_start && at < stop {
                out.push(ControlSet { at, name: control.to_string(), target, glide });
            }
        };
        push(pass_start, start, 0.0);
        let (mut t, mut from_level) = (pass_start, start);
        for &(dur, shape, to_level) in steps {
            let pieces = if dur <= 0.0 || matches!(shape, niminal_engine::ops::Curve::Linear) {
                1
            } else {
                ((dur / PIECE_SECONDS).ceil() as usize).clamp(MIN_PIECES, MAX_PIECES)
            };
            for j in 0..pieces {
                let at = t + dur * j as f64 / pieces as f64;
                let reached = niminal_engine::ops::shape(from_level as f32, to_level as f32, (j + 1) as f64 / pieces as f64, shape.bend());
                push(at, f64::from(reached), dur / pieces as f64);
                if at >= stop {
                    break;
                }
            }
            t += dur;
            from_level = to_level;
            if t >= stop {
                break;
            }
        }
    }
}

fn lane_error(clip: &Clip, track: &str, lane: &str, why: &str) -> ScheduleError {
    ScheduleError(format!("clip `{}` on track `{track}`, lane `{lane}`: {why}", clip.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    const STAGE: &str = "
tempo 120bpm
instr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: freq * (2 + bright * 6)) }
track lead { instrument = pluck }
track bass { instrument = pluck }
";

    #[test]
    fn musical_durations_follow_the_tempo_of_the_sections_they_cross() {
        let tempo = Tempo::new(120.0);
        let quick = Tempo { bpm: 240.0, beats_per_bar: 4.0 };
        let waltz = Tempo { bpm: 120.0, beats_per_bar: 3.0 };
        let s = Schedule::new(tempo, vec![], &[], &[(4.0, quick), (5.0, waltz)], &[]);
        // two bars at 120bpm are 4s, the next bar at 240bpm is 1s
        assert!((s.seconds_from(0.0, Time::Bars(3.0)) - 5.0).abs() < 1e-9);
        // starting in the quick section, four beats take a second
        assert!((s.seconds_from(4.0, Time::Beats(4.0)) - 1.0).abs() < 1e-9);
        // a bar of 3/4 at 120bpm is 1.5s, so 2.5 bars from the start are 4s + 1s (one quick bar) + 0.5 bar of 3/4
        assert!((s.seconds_from(0.0, Time::Bars(3.5)) - 5.75).abs() < 1e-9);
        assert_eq!(s.seconds_from(1.0, Time::Seconds(2.5)), 2.5);
    }

    fn schedule(body: &str) -> Schedule {
        let src = format!("{STAGE}{body}");
        compile(&src)
            .unwrap_or_else(|e| panic!("{}", e.iter().map(|d| d.render("t", &src)).collect::<String>()))
            .schedule()
    }

    /// (seconds, track, freq as MIDI) of each note starting in the window.
    fn notes(s: &Schedule, from: f64, to: f64) -> Vec<(f64, String, i32)> {
        s.events(from, to)
            .unwrap()
            .into_iter()
            .map(|e| {
                let Time::Seconds(at) = e.at else { panic!("scheduled notes are in seconds") };
                let Value::Note(m) = e.args["freq"] else { panic!("pitch is a note") };
                (at, e.target, m)
            })
            .collect()
    }

    fn times(s: &Schedule, from: f64, to: f64) -> Vec<f64> {
        s.events(from, to).unwrap().into_iter().map(|e| e.at.to_seconds(Tempo::new(120.0))).collect()
    }

    fn song(body: &str) -> Schedule {
        let src = format!("{STAGE}scene s {{ lead: [c4 c4 c4 c4] }}\n{body}");
        compile(&src)
            .unwrap_or_else(|e| panic!("{}", e.iter().map(|d| d.render("t", &src)).collect::<String>()))
            .arrangement_schedule("song")
            .unwrap()
    }

    #[test]
    fn a_gradual_tempo_change_speeds_up_the_notes_of_the_clip_playing_through_it() {
        // 4 notes a bar, for 8 bars, the tempo rising evenly from 60bpm to 240bpm
        let s = song("arrangement song { sections: [s.over(8 bars)]\n tempo: env[60bpm 8bars 240bpm] }");
        let at: Vec<f64> = s.events(0.0, 60.0).unwrap().iter().map(|e| e.at.to_seconds(Tempo::new(120.0))).collect();
        assert_eq!(at.len(), 32, "4 notes in each of 8 bars");
        let gaps: Vec<f64> = at.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.windows(2).all(|g| g[1] <= g[0] + 1e-9), "each gap is no longer than the one before: {gaps:?}");
        assert!((gaps[0] - 1.0).abs() < 0.05, "a beat at 60bpm is a second: {}", gaps[0]);
        assert!(*gaps.last().unwrap() < 0.3, "and at the end it is near a quarter of that: {}", gaps.last().unwrap());
        // 240 / (60 + 22.5 bar) summed over the bars: about 14.8 seconds in all
        let last = *at.last().unwrap();
        assert!(last > 13.0 && last < 14.8, "{last}");
        assert!(s.end().unwrap() > 14.0 && s.end().unwrap() < 15.5, "{:?}", s.end());
    }

    #[test]
    fn a_tempo_that_holds_and_then_changes_only_changes_where_it_is_written() {
        // steady for 4 bars at 120bpm (2s a bar), then a rise to 240bpm over the next 4
        let s = song("arrangement song { sections: [s.over(8 bars)]\n tempo: env[120bpm 4bars 120bpm 4bars 240bpm] }");
        let at: Vec<f64> = s.events(0.0, 60.0).unwrap().iter().map(|e| e.at.to_seconds(Tempo::new(120.0))).collect();
        assert_eq!(at[0], 0.0);
        assert!((at[16] - 8.0).abs() < 1e-6, "four bars of 2 seconds: {}", at[16]);
        assert!((at[1] - at[0] - 0.5).abs() < 1e-9 && (at[15] - at[14] - 0.5).abs() < 1e-9, "a steady half second");
        assert!(at[31] - at[30] < 0.4);
    }

    #[test]
    fn a_tempo_envelope_is_checked() {
        let error = |body: &str| {
            let src = format!("{STAGE}scene s {{ lead: [c4] }}\n{body}");
            compile(&src).err().map(|e| e.iter().map(|d| d.message.clone()).collect::<String>()).unwrap_or_default()
        };
        assert!(error("arrangement song { sections: [s.over(8 bars)]\n tempo: env[60 8bars 240bpm] }").contains("positive number of bpm"));
        assert!(error("arrangement song { sections: [s.over(8 bars)]\n tempo: env[60bpm 8sec 240bpm] }").contains("lengths in bars"));
        assert!(error("arrangement song { sections: [s.over(8 bars)]\n tempo: env[60bpm 8bars 240bpm].loop }").contains("runs once"));
        assert!(error("arrangement song { sections: [s.over(8 bars)]\n tempo: [60bpm]\n tempo: env[60bpm 8bars 240bpm] }").contains("given twice"));
    }

    #[test]
    fn a_looping_pattern_produces_notes_on_the_beat() {
        // 120bpm in 4/4: a bar is 2 seconds, so two steps are 1 second apart
        let s = schedule("lead.play([c4 e4])");
        let n = notes(&s, 0.0, 4.0);
        assert_eq!(
            n,
            [
                (0.0, "lead".into(), 60),
                (1.0, "lead".into(), 64),
                (2.0, "lead".into(), 60),
                (3.0, "lead".into(), 64)
            ]
        );
        let e = s.events(0.0, 1.0).unwrap();
        assert_eq!(e[0].dur, Time::Seconds(1.0));
        assert_eq!(s.end(), None, "it plays until stopped");
    }

    #[test]
    fn windows_select_the_notes_that_start_in_them() {
        let s = schedule("lead.play([c4 e4 g4 b4])");
        assert_eq!(times(&s, 0.5, 2.0), [0.5, 1.0, 1.5]);
        assert_eq!(times(&s, 2.0, 2.0), Vec::<f64>::new());
        assert_eq!(times(&s, 0.0, 0.5), [0.0]);
        // adjacent windows tile exactly: nothing doubled, nothing dropped
        let whole = times(&s, 0.0, 8.0);
        let mut pieces = times(&s, 0.0, 3.3);
        pieces.extend(times(&s, 3.3, 5.7));
        pieces.extend(times(&s, 5.7, 8.0));
        assert_eq!(whole, pieces);
        assert_eq!(whole.len(), 16);
    }

    #[test]
    fn a_clip_loops_every_length_and_restarts_its_alternations() {
        // the lane spans the three bars, so it is one cycle of the clip: it restarts every 6s
        let s = schedule("clip c { length: 3 bars\nnotes: [<c4 e4>] }\nlead.play(c)");
        let n: Vec<i32> = notes(&s, 0.0, 12.0).into_iter().map(|e| e.2).collect();
        assert_eq!(n, [60, 60]);
        // a short pattern in a long clip is spread over it
        let s = schedule("clip c { length: 2 bars\nnotes: [c4 e4] }\nlead.play(c)");
        assert_eq!(times(&s, 0.0, 8.0), [0.0, 2.0, 4.0, 6.0]);
    }

    #[test]
    fn lanes_span_a_longer_clip_and_line_up() {
        let s = schedule("clip c { length: 2 bars\nnotes: [c4 e4 g4 b4]\ngain: [0db -6db -12db -18db] }\nlead.play(c)");
        // four notes spread over the two bars (4s), then the clip starts again
        assert_eq!(times(&s, 0.0, 8.0), [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        let gains: Vec<Value> = s.events(0.0, 4.0).unwrap().into_iter().map(|e| e.args["gain"]).collect();
        assert_eq!(gains, [Value::Db(0.0), Value::Db(-6.0), Value::Db(-12.0), Value::Db(-18.0)]);
    }

    #[test]
    fn a_clip_keeps_counting_cycles_until_it_is_given_a_length() {
        let order = |s: &Schedule| notes(s, 0.0, 8.0).into_iter().map(|e| e.2).collect::<Vec<_>>();
        assert_eq!(order(&schedule("lead.play([<c4 e4>])")), [60, 64, 60, 64]);
        assert_eq!(order(&schedule("lead.play([c4 e4].every(2, it.reverse))")), [64, 60, 60, 64, 64, 60, 60, 64]);
        assert_eq!(order(&schedule("clip c { notes: [<c4 e4>] }\nlead.play(c)")), [60, 64, 60, 64]);
        assert_eq!(order(&schedule("clip c { length: 1 bar\nnotes: [<c4 e4>] }\nlead.play(c)")), [60, 60, 60, 60]);
    }

    #[test]
    fn launching_a_scene_stops_the_tracks_it_does_not_name() {
        let s = schedule("scene a { lead: [c4], bass: [c2] }\nscene b { lead: [e4] }\na.start\nat +2 bars b.start");
        let tracks = |from, to| notes(&s, from, to).into_iter().map(|e| e.1).collect::<Vec<_>>();
        assert_eq!(tracks(0.0, 2.0), ["lead", "bass"]);
        assert_eq!(tracks(4.0, 6.0), ["lead"], "bass was not in scene b");
    }

    #[test]
    fn set_gives_a_clip_or_a_pattern_lanes_in_place_of_its_own() {
        let bright = |s: &Schedule| -> Vec<Value> { s.events(0.0, 4.0).unwrap().iter().map(|e| e.args["bright"]).collect() };
        let s = schedule("clip c { notes: [c4 e4]\nbright: [0.2] }\nlead.play(c.set(bright: 0.9))");
        assert_eq!(bright(&s)[..2], [Value::Num(0.9), Value::Num(0.9)], "replaced");
        let s = schedule("lead.play([c4 e4].set(bright: [0.1 0.7]))");
        assert_eq!(bright(&s)[..2], [Value::Num(0.1), Value::Num(0.7)], "added to a bare pattern");
        let s = schedule("lead.play([c4].reverse.set(gain: -6db))");
        assert_eq!(s.events(0.0, 1.0).unwrap()[0].args["gain"], Value::Db(-6.0));
    }

    #[test]
    fn other_lanes_are_read_where_each_note_starts() {
        let s = schedule("clip c { notes: [c4 e4 g4 b4]\nbright: [0.2 0.8] }\nlead.play(c)");
        let e = s.events(0.0, 2.0).unwrap();
        let brights: Vec<_> = e.iter().map(|e| e.args["bright"]).collect();
        assert_eq!(brights, [Value::Num(0.2), Value::Num(0.2), Value::Num(0.8), Value::Num(0.8)]);
        assert!(e.iter().all(|e| e.args.contains_key("freq")));
    }

    #[test]
    fn dur_and_nudge_lanes_shape_each_note() {
        let s = schedule("clip c { notes: [c4 e4]\ndur: [1/4beat]\nnudge: [0ms 100ms] }\nlead.play(c)");
        let e = s.events(0.0, 2.0).unwrap();
        assert_eq!(e[0].dur, Time::Beats(0.25));
        assert_eq!(e[0].at, Time::Seconds(0.0));
        assert_eq!(e[1].at, Time::Seconds(1.1), "100ms late");
        assert!(!e[0].args.contains_key("dur") && !e[0].args.contains_key("nudge"), "they aren't instrument arguments");
        // a nudge never pushes a note before the start
        let early = schedule("clip c { notes: [c4]\nnudge: [-50ms] }\nlead.play(c)");
        assert_eq!(early.events(0.0, 1.0).unwrap()[0].at, Time::Seconds(0.0));
    }

    #[test]
    fn commands_start_replace_and_stop_clips() {
        let s = schedule("
lead.play([c4])
at 2 bars lead.play([e4])
at 3 bars lead.stop");
        // bar = 2s: c4 for the first 4s, e4 for 4s..6s, then silence
        let n = notes(&s, 0.0, 20.0);
        assert_eq!(n.iter().map(|e| e.2).collect::<Vec<_>>(), [60, 60, 64]);
        assert_eq!(n.iter().map(|e| e.0).collect::<Vec<_>>(), [0.0, 2.0, 4.0]);
        assert_eq!(s.end(), Some(6.0));
    }

    #[test]
    fn a_clip_started_later_starts_its_own_cycle_there() {
        let s = schedule("at bar 3 lead.play([c4 e4])");
        assert_eq!(times(&s, 0.0, 8.0), [4.0, 5.0, 6.0, 7.0]);
        let mid_beat = schedule("at 1beat lead.play([c4 e4])");
        assert_eq!(times(&mid_beat, 0.0, 3.0), [0.5, 1.5, 2.5]);
    }

    #[test]
    fn tracks_play_independently_and_hush_ends_them_all() {
        let s = schedule("lead.play([c4])\nbass.play([c2 g2])\nat 2 bars hush");
        let n = notes(&s, 0.0, 20.0);
        assert_eq!(n.iter().filter(|e| e.1 == "lead").count(), 2);
        assert_eq!(n.iter().filter(|e| e.1 == "bass").count(), 4);
        assert_eq!(s.end(), Some(4.0));
        assert!(!s.is_empty());
    }

    #[test]
    fn mute_and_solo_silence_tracks_without_stopping_them() {
        let s = schedule("lead.play([c4])\nbass.play([c2])\nat 1 bar lead.mute\nat 3 bars lead.unmute");
        let lead: Vec<f64> = notes(&s, 0.0, 10.0).into_iter().filter(|e| e.1 == "lead").map(|e| e.0).collect();
        assert_eq!(lead, [0.0, 6.0, 8.0], "silent between 2s and 6s; the clip kept its place");
        assert_eq!(notes(&s, 0.0, 10.0).iter().filter(|e| e.1 == "bass").count(), 5);

        let solo = schedule("lead.play([c4])\nbass.play([c2])\nat 1 bar bass.solo\nat 2 bars unsolo");
        let who = |t0, t1| notes(&solo, t0, t1).into_iter().map(|e| e.1).collect::<Vec<_>>();
        assert_eq!(who(0.0, 2.0), ["lead", "bass"]);
        assert_eq!(who(2.0, 4.0), ["bass"], "only the soloed track");
        assert_eq!(who(4.0, 6.0), ["lead", "bass"]);
        let one = schedule("lead.play([c4])\nbass.play([c2])\nlead.solo\nbass.solo\nat 1 bar bass.unsolo");
        assert_eq!(notes(&one, 2.0, 4.0).iter().map(|e| e.1.as_str()).collect::<Vec<_>>(), ["lead"]);
    }

    #[test]
    fn panic_is_reported_and_ends_every_clip() {
        let s = schedule("lead.play([c4])\nat 3 bars panic");
        assert_eq!(s.panics(), [6.0]);
        assert_eq!(s.end(), Some(6.0));
        assert_eq!(times(&s, 0.0, 20.0), [0.0, 2.0, 4.0]);
    }

    #[test]
    fn scenes_launch_whole_sets_of_clips() {
        let s = schedule("
clip riff { notes: [c2 g2] }
scene verse { bass: riff, lead: [c4] }
scene bridge { bass: ~, lead: [e4] }
verse.start
at 2 bars bridge.start");
        let n = notes(&s, 0.0, 8.0);
        let bass = n.iter().filter(|e| e.1 == "bass").count();
        assert_eq!(bass, 4, "two bars of two notes, then stopped");
        let lead: Vec<i32> = n.iter().filter(|e| e.1 == "lead").map(|e| e.2).collect();
        assert_eq!(lead, [60, 60, 64, 64]);
    }

    #[test]
    fn thinned_patterns_are_the_same_whichever_way_they_are_asked_for() {
        let s = schedule("lead.play([c4*8].thin(40%))");
        let whole = times(&s, 0.0, 16.0);
        let mut pieces = Vec::new();
        for i in 0..8 {
            pieces.extend(times(&s, f64::from(i) * 2.0, f64::from(i + 1) * 2.0));
        }
        assert_eq!(whole, pieces);
        assert!(whole.len() < 64 && whole.len() > 20, "{}", whole.len());
    }

    #[test]
    fn notes_that_are_not_pitches_are_errors() {
        let s = schedule("lead.play([0.5])");
        let e = s.events(0.0, 2.0).unwrap_err();
        assert!(e.0.contains("must be a note name or a frequency"), "{e}");
        let s = schedule("clip c { notes: [c4]\ndur: [440hz] }\nlead.play(c)");
        assert!(s.events(0.0, 2.0).unwrap_err().0.contains("lane `dur`"));
    }

    #[test]
    fn a_program_with_no_commands_schedules_nothing() {
        let s = schedule("lead(freq: c4) for 1beat");
        assert!(s.is_empty());
        assert!(s.events(0.0, 100.0).unwrap().is_empty());
        assert_eq!(s.end(), Some(0.0));
    }

    #[test]
    fn other_meters_change_the_length_of_a_cycle() {
        let src = "meter 3/4\ntempo 120bpm\ninstr p(freq: hz) { osc(sine, freq) }\ntrack t { instrument = p }\nt.play([c4 e4 g4])";
        let s = compile(src).unwrap().schedule();
        // a 3/4 bar at 120bpm is 1.5 seconds, so three steps are half a second apart
        assert_eq!(times(&s, 0.0, 1.6), [0.0, 0.5, 1.0, 1.5]);
    }
}
