use niminal_pattern::{Rational, Span as CycleSpan};
use niminal_score::{Event, Time, Value};

use crate::performance::{Clip, Step};

use super::{
    BASE_WORK, CHUNK_SECONDS, ControlSet, END_SLACK, EVENTS_BASE, EVENTS_PER_SECOND, Found,
    MAX_CONTROL_SETS, MAX_EVENTS, MAX_PIECES, MIN_PIECES, Micro, PAD_CYCLES, PIECE_SECONDS,
    SIGNAL_STEP, Schedule, ScheduleError, Segment, WORK_PER_SECOND, loops, micro_cycles, snap,
};

impl Schedule {
    /// Every change an envelope lane makes to a part parameter in `from..to` seconds, in time order.
    pub fn controls(&self, from: f64, to: f64) -> Result<Vec<ControlSet>, ScheduleError> {
        // a long stretch is searched a piece at a time, each with its own limit
        let mut all = Vec::new();
        let mut start = from;
        while start < to {
            let end = (start + CHUNK_SECONDS).min(to);
            let mut chunk = Vec::new();
            for segment in &self.segments {
                self.segment_controls(segment, start, end, &mut chunk)?;
                self.release_controls(segment, start, end, &mut chunk);
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
    /// cut the search short if any, and the nodes whose clips asked for more
    /// than they are allowed (which the caller should stop searching, since
    /// searching again costs the same). Nodes in `skip` are not searched.
    pub fn events_lenient(&self, from: f64, to: f64, skip: &[usize]) -> Found {
        let mut out: Vec<(f64, Event)> = Vec::new();
        let mut controls: Vec<ControlSet> = Vec::new();
        let mut problem = None;
        let mut overloaded = Vec::new();
        if let Err(e) = self.lane_controls(from, to, &mut controls) {
            problem.get_or_insert(e);
        }
        let work = BASE_WORK + (WORK_PER_SECOND * (to - from).max(0.0)) as usize;
        for segment in self.segments.iter().filter(|s| !skip.contains(&s.node)) {
            if let Err(e) = self.segment_controls(segment, from, to, &mut controls) {
                problem.get_or_insert(e);
            }
            self.release_controls(segment, from, to, &mut controls);
            let before = out.len();
            let (result, exhausted) = niminal_pattern::with_budget(work, || {
                self.segment_events(segment, from, to, &mut out)
            });
            if let Err(e) = result {
                problem.get_or_insert(e);
            }
            for (at, event) in &mut out[before..] {
                for (name, unit, glide, _) in &segment.clip.fx_params {
                    if let Some(value) = event.args.remove(name) {
                        match niminal_lang::engine_value(*unit, value, segment.tempo) {
                            Ok(target) => controls.push(ControlSet {
                                at: *at,
                                name: name.clone(),
                                target,
                                glide: *glide,
                            }),
                            Err(e) => {
                                problem.get_or_insert_with(|| {
                                    ScheduleError(format!(
                                        "fx lane `{}`: {e}",
                                        crate::compile::written(name)
                                    ))
                                });
                            }
                        }
                    }
                }
            }
            let allowed = EVENTS_BASE + (EVENTS_PER_SECOND * (to - from).max(0.0)) as usize;
            let too_many = out.len() - before > allowed;
            if too_many {
                out.truncate(before + allowed);
            }
            if exhausted || too_many {
                overloaded.push(segment.node);
                problem.get_or_insert_with(|| {
                    ScheduleError(format!(
                        "the clip on node `{}` plays more notes than niminal can schedule; it is stopped until you evaluate again",
                        self.node_names[segment.node]
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
        Found {
            events: out.into_iter().map(|(_, e)| e).collect(),
            controls,
            problem,
            overloaded,
        }
    }

    fn segment_events(
        &self,
        segment: &Segment,
        from: f64,
        to: f64,
        out: &mut Vec<(f64, Event)>,
    ) -> Result<(), ScheduleError> {
        let clock = &segment.clock;
        let length = segment.clip.length.to_f64();
        let until = snap(segment.end.map_or(to, |e| e.min(to)));
        let window_start = snap(from.max(segment.start));
        if window_start >= until || length <= 0.0 {
            return Ok(());
        }

        // The window in cycles since the clip started, and the loops of the clip that overlap it.
        let (first_cycle, last_cycle) = (clock.cycles_at(window_start), clock.cycles_at(until));
        let (first_loop, last_loop) = loops(&segment.clip, first_cycle, last_cycle);

        for k in first_loop..last_loop {
            let base = if segment.clip.endless {
                0.0
            } else {
                k as f64 * length
            };
            // Only the part of the loop inside the window, padded a little so that
            // rounding can't lose a note on an edge; the check below decides.
            let a = (first_cycle - base).max(0.0) - PAD_CYCLES;
            let b = if segment.clip.endless {
                last_cycle - base
            } else {
                (last_cycle - base).min(length)
            } + PAD_CYCLES;
            let span = CycleSpan::new(
                micro_cycles(a.floor_to_micro()).max(Rational::ZERO),
                if segment.clip.endless {
                    micro_cycles(b.ceil_to_micro())
                } else {
                    micro_cycles(b.ceil_to_micro()).min(segment.clip.length)
                },
            );
            if span.start >= span.end {
                continue;
            }
            if let Some(notes) = segment.clip.lane("notes") {
                for hap in notes.onsets(span) {
                    let whole = hap.whole.expect("notes have a duration");
                    let onset = snap(clock.seconds_at(base + whole.start.to_f64()));
                    if onset < window_start || onset >= until {
                        continue;
                    }
                    // a note meant for the first instant after the clip ends can land a hair early, when the
                    // end was found by adding up the steps of a gradual tempo change
                    if segment.end.is_some_and(|end| onset >= end - END_SLACK) {
                        continue;
                    }
                    let node = segment.node;
                    if !self.audible(node, onset) {
                        continue;
                    }
                    out.push((onset, self.note(segment, whole, base, &hap.value, onset)?));
                }
            }
            for note in &segment.clip.scored {
                if note.at < span.start || note.at >= span.end {
                    continue;
                }
                let onset = snap(clock.seconds_at(base + note.at.to_f64()));
                if onset < window_start || onset >= until {
                    continue;
                }
                if segment.end.is_some_and(|end| onset >= end - END_SLACK) {
                    continue;
                }
                if !self.audible(segment.node, onset) {
                    continue;
                }
                let whole = CycleSpan::new(note.at, note.at + note.dur);
                let mut event = self.note(segment, whole, base, &note.step, onset)?;
                for (k, v) in &note.args {
                    event.args.insert(k.clone(), *v);
                }
                // scored notes carry their own duration
                event.dur =
                    Time::Seconds(clock.seconds_at(base + (note.at + note.dur).to_f64()) - onset);
                out.push((onset, event));
            }
        }
        Ok(())
    }

    /// What the piece's own envelope lanes (an arrangement's) do to part parameters in `from..to`.
    fn lane_controls(
        &self,
        from: f64,
        to: f64,
        out: &mut Vec<ControlSet>,
    ) -> Result<(), ScheduleError> {
        for (control, lane, placed) in &self.lanes {
            let origin = self.seconds_from(0.0, *placed);
            let fail = |why: String| ScheduleError(format!("arrangement lane `{control}`: {why}"));
            let unit = lane
                .unit
                .ok_or_else(|| fail("the envelope was never placed on a parameter".into()))?;
            let level = |v: Value| {
                niminal_lang::engine_value(unit, v, self.tempo).map_err(|e| fail(e.to_string()))
            };
            let start = level(lane.start)?;
            let mut steps = Vec::with_capacity(lane.segments.len());
            let mut at = origin;
            for (dur, shape, to_level) in &lane.segments {
                let seconds = self.seconds_from(at, *dur);
                steps.push((seconds, *shape, level(*to_level)?));
                at += seconds;
            }
            emit_envelope(
                control,
                start,
                &steps,
                lane.looped,
                origin,
                from.max(0.0),
                to,
                out,
            );
            if out.len() > MAX_CONTROL_SETS {
                return Err(fail(format!(
                    "it changes the parameter more than {MAX_CONTROL_SETS} times in {:.1} seconds",
                    to - from
                )));
            }
        }
        Ok(())
    }

    /// How many seconds `dur` takes when it starts `at` seconds into the piece, following the
    /// tempo and meter of the sections it runs through.
    pub(super) fn seconds_from(&self, at: f64, dur: Time) -> f64 {
        let (mut left, bars) = match dur {
            Time::Seconds(s) => return s.max(0.0),
            Time::Beats(b) => (b.max(0.0), false),
            Time::Bars(b) => (b.max(0.0), true),
        };
        let mut t = at;
        loop {
            let tempo = self
                .sections
                .iter()
                .rev()
                .find(|(start, _)| *start <= t)
                .map_or(self.tempo, |(_, tempo)| *tempo);
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

    /// When `segment` ends, if it does: where it was stopped or replaced, or the end of its last pass.
    fn ends_at(&self, segment: &Segment) -> Option<f64> {
        let finish = segment
            .clip
            .plays
            .filter(|_| !segment.clip.endless)
            .map(|n| {
                segment
                    .clock
                    .seconds_at(segment.clip.length.to_f64() * f64::from(n))
            });
        match (segment.end, finish) {
            (Some(end), Some(finish)) => Some(end.min(finish)),
            (end, finish) => end.or(finish),
        }
    }

    /// An effect keeps the last level a clip gave its parameters, so when `segment` ends in `from..to` seconds they
    /// go back to where the effect starts, unless a clip that carries on drives the same parameter.
    fn release_controls(&self, segment: &Segment, from: f64, to: f64, out: &mut Vec<ControlSet>) {
        let Some(end) = self
            .ends_at(segment)
            .filter(|end| *end >= from && *end < to)
        else {
            return;
        };
        for (name, _, glide, initial) in &segment.clip.fx_params {
            let carried_on = self.segments.iter().any(|other| {
                !std::ptr::eq(other, segment)
                    && other.start <= end + END_SLACK
                    && self.ends_at(other).is_none_or(|e| e > end + END_SLACK)
                    && other.clip.fx_params.iter().any(|(n, ..)| n == name)
            });
            if !carried_on {
                out.push(ControlSet {
                    at: end,
                    name: name.clone(),
                    target: *initial,
                    glide: *glide,
                });
            }
        }
    }

    /// Put back the effect parameters of every clip still running at `at` seconds, as if they all stopped there.
    pub fn release_all(&self, at: f64) -> Vec<ControlSet> {
        let mut out: Vec<ControlSet> = Vec::new();
        for segment in &self.segments {
            if segment.start > at || self.ends_at(segment).is_some_and(|end| end <= at) {
                continue;
            }
            for (name, _, glide, initial) in &segment.clip.fx_params {
                if !out.iter().any(|c| &c.name == name) {
                    out.push(ControlSet {
                        at,
                        name: name.clone(),
                        target: *initial,
                        glide: *glide,
                    });
                }
            }
        }
        out
    }

    /// What `segment`'s envelope lanes do to their node's parameters in `from..to` seconds. A curved
    /// segment becomes a few straight glides that follow the same curve an `env` in an instrument does.
    fn segment_controls(
        &self,
        segment: &Segment,
        from: f64,
        to: f64,
        out: &mut Vec<ControlSet>,
    ) -> Result<(), ScheduleError> {
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
        let (first_loop, last_loop) = loops(&segment.clip, first_cycle, last_cycle);
        let name = &self.node_names[segment.node];
        let clip = &segment.clip;

        // signal lanes: the parameter follows the signal, read every SIGNAL_STEP seconds
        for (lane, unit) in &clip.signals {
            let Some(pattern) = clip.lane(lane) else {
                continue;
            };
            let fail = |why: String| {
                ScheduleError(format!(
                    "clip `{}` on node `{name}`, lane `{}`: {why}",
                    clip.name,
                    crate::compile::written(lane)
                ))
            };
            let control = if lane.contains("::") {
                lane.clone()
            } else {
                format!("{name}::{lane}")
            };
            let first = ((window_start / SIGNAL_STEP).floor() * SIGNAL_STEP).max(segment.start);
            let mut at = first;
            while at < until {
                if at >= window_start {
                    // where this moment falls in the clip, which starts over if it has a length
                    let into = clock.cycles_at(at);
                    let in_loop = if clip.endless {
                        into
                    } else {
                        into.rem_euclid(length)
                    };
                    let place = Rational::from_f64(in_loop);
                    let over = clip
                        .plays
                        .is_some_and(|n| !clip.endless && into >= length * f64::from(n));
                    if let Some(hap) = pattern
                        .query(CycleSpan::new(place, place))
                        .into_iter()
                        .next()
                        .filter(|_| !over)
                    {
                        let target =
                            niminal_lang::engine_value(*unit, hap.value.value, segment.tempo)
                                .map_err(|e| fail(e.to_string()))?;
                        out.push(ControlSet {
                            at,
                            name: control.clone(),
                            target,
                            glide: SIGNAL_STEP,
                        });
                    }
                    if out.len() > MAX_CONTROL_SETS {
                        return Err(fail(format!(
                            "it changes the parameter more than {MAX_CONTROL_SETS} times in {:.1} seconds",
                            to - from
                        )));
                    }
                }
                at += SIGNAL_STEP;
            }
        }

        for (lane, curve) in &clip.curves {
            let fail = |why: String| {
                ScheduleError(format!(
                    "clip `{}` on node `{name}`, lane `{}`: {why}",
                    clip.name,
                    crate::compile::written(lane)
                ))
            };
            let unit = curve
                .unit
                .ok_or_else(|| fail("the envelope was never placed on a node".into()))?;
            let level = |v: Value| {
                niminal_lang::engine_value(unit, v, segment.tempo).map_err(|e| fail(e.to_string()))
            };
            let start = level(curve.start)?;
            let mut steps = Vec::with_capacity(curve.segments.len());
            for (dur, shape, to_level) in &curve.segments {
                steps.push((
                    dur.to_seconds(segment.tempo).max(0.0),
                    *shape,
                    level(*to_level)?,
                ));
            }
            let control = if lane.contains("::") {
                lane.clone()
            } else {
                format!("{name}::{lane}")
            };

            for k in first_loop..last_loop {
                let loop_start = if clip.endless {
                    segment.origin
                } else {
                    clock.seconds_at(k as f64 * length)
                };
                // nothing from this pass of the clip is heard after the clip starts over
                let stop = if clip.endless {
                    until
                } else {
                    until.min(clock.seconds_at((k + 1) as f64 * length))
                };
                emit_envelope(
                    &control,
                    start,
                    &steps,
                    curve.looped,
                    loop_start,
                    window_start,
                    stop,
                    out,
                );
                if out.len() > MAX_CONTROL_SETS {
                    return Err(fail(format!(
                        "it changes the parameter more than {MAX_CONTROL_SETS} times in {:.1} seconds",
                        to - from
                    )));
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
        let name = &self.node_names[segment.node];
        let clip = &segment.clip;

        let mut args = std::collections::BTreeMap::new();
        match pitch {
            Value::Note(_) | Value::Hz(_) => {
                args.insert(crate::perform::PITCH_PARAM.to_string(), *pitch)
            }
            Value::Sample(_) => args.insert("sample".to_string(), *pitch),
            other => {
                return Err(ScheduleError(format!(
                    "clip `{}` on node `{name}`: a note must be a note name or a frequency, found `{other}`",
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
            // The value in force at the moment the note starts
            let Some(hap) = pattern
                .query(CycleSpan::new(whole.start, whole.start))
                .into_iter()
                .next()
            else {
                continue;
            };
            match (lane.as_str(), hap.value.value) {
                ("dur", v) => {
                    dur = v
                        .as_time()
                        .map_err(|e| lane_error(clip, name, lane, &e.to_string()))?;
                }
                ("nudge", v) => {
                    let nudge = v
                        .as_time()
                        .map_err(|e| lane_error(clip, name, lane, &e.to_string()))?;
                    at = (at + nudge.to_seconds(segment.tempo)).max(0.0);
                }
                (_, v) => {
                    args.insert(lane.clone(), v);
                }
            }
        }
        // controls that belong to this note alone win over the lanes
        if let Some(db) = step.gain {
            args.insert(niminal_lang::GAIN_PARAM.to_string(), Value::Db(db));
        }
        if let Some(degrees) = step.pan {
            args.insert("pan".to_string(), Value::Degrees(degrees));
        }
        if let Some(share) = step.begin {
            args.insert("begin".to_string(), Value::Num(share));
        }
        if let Some(st) = step.pitch {
            args.insert("pitch".to_string(), Value::Semitones(st));
        }
        let target = step
            .node
            .map(|t| self.node_names[t].clone())
            .unwrap_or_else(|| name.clone());
        Ok(Event {
            target,
            at: Time::Seconds(at),
            dur,
            args,
        })
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
                out.push(ControlSet {
                    at,
                    name: control.to_string(),
                    target,
                    glide,
                });
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
                let reached = niminal_engine::ops::shape(
                    from_level as f32,
                    to_level as f32,
                    (j + 1) as f64 / pieces as f64,
                    shape.bend(),
                );
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

fn lane_error(clip: &Clip, node: &str, lane: &str, why: &str) -> ScheduleError {
    ScheduleError(format!(
        "clip `{}` on node `{node}`, lane `{}`: {why}",
        clip.name,
        crate::compile::written(lane)
    ))
}
