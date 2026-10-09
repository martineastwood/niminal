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

use crate::performance::{Action, Clip, Scheduled};
use crate::program::Program;

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleError(pub String);

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScheduleError {}

/// A clip playing on a track from `start` until `end` (seconds).
struct Segment {
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

pub struct Schedule {
    tempo: Tempo,
    track_names: Vec<String>,
    segments: Vec<Segment>,
    /// Mute and solo changes, in time order.
    gates: Vec<(f64, Gate)>,
    panics: Vec<f64>,
}

impl Program {
    /// The notes this program's `play`, `launch` and related commands produce.
    pub fn schedule(&self) -> Schedule {
        Schedule::new(self.tempo, self.tracks.iter().map(|t| t.name.clone()).collect(), &self.performance)
    }
}

impl Schedule {
    /// A schedule from commands directly. Track indices in `commands` index
    /// `track_names`. The commands' `at` times are what they are; their
    /// `quantize` is ignored (a live session has already applied it).
    pub fn from_commands(tempo: Tempo, track_names: Vec<String>, commands: &[Scheduled]) -> Schedule {
        Schedule::new(tempo, track_names, commands)
    }

    fn new(tempo: Tempo, track_names: Vec<String>, commands: &[Scheduled]) -> Schedule {
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
                    segments.push(Segment { track: *track, clip: clip.clone(), start: at, end: None });
                }
                Action::Stop { track } => close(&mut segments, Some(*track), at),
                Action::Hush => close(&mut segments, None, at),
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
        Schedule { tempo, track_names, segments, gates, panics }
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

    /// Seconds in one pattern cycle: a bar.
    fn cycle(&self) -> f64 {
        self.tempo.beats_per_bar * 60.0 / self.tempo.bpm
    }

    fn audible(&self, track: usize, at: f64) -> bool {
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
        !muted.contains(&track) && (soloed.is_empty() || soloed.contains(&track))
    }

    /// Every note that starts in `from..to` seconds, in time order.
    pub fn events(&self, from: f64, to: f64) -> Result<Vec<Event>, ScheduleError> {
        let mut out: Vec<(f64, Event)> = Vec::new();
        for segment in &self.segments {
            self.segment_events(segment, from, to, &mut out)?;
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(out.into_iter().map(|(_, e)| e).collect())
    }

    fn segment_events(
        &self,
        segment: &Segment,
        from: f64,
        to: f64,
        out: &mut Vec<(f64, Event)>,
    ) -> Result<(), ScheduleError> {
        let Some(notes) = segment.clip.lane("notes") else { return Ok(()) };
        let cycle = self.cycle();
        let loop_len = segment.clip.length.to_f64() * cycle;
        let until = segment.end.map_or(to, |e| e.min(to));
        let window_start = from.max(segment.start);
        if window_start >= until || loop_len <= 0.0 {
            return Ok(());
        }

        // The loops of the clip that overlap the window.
        let first_loop = ((window_start - segment.start) / loop_len).floor().max(0.0) as i64;
        let last_loop = ((until - segment.start) / loop_len).ceil() as i64;

        for k in first_loop..last_loop {
            let loop_start = segment.start + k as f64 * loop_len;
            let span = CycleSpan::new(Rational::ZERO, segment.clip.length);
            for hap in notes.onsets(span) {
                let whole = hap.whole.expect("notes have a duration");
                let onset = loop_start + whole.start.to_f64() * cycle;
                if onset < window_start || onset >= until {
                    continue;
                }
                let track = segment.track;
                if !self.audible(track, onset) {
                    continue;
                }
                out.push((onset, self.note(segment, whole, &hap.value, onset)?));
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
        pitch: &Value,
        onset: f64,
    ) -> Result<Event, ScheduleError> {
        let cycle = self.cycle();
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
        let mut dur = Time::Seconds((whole.end - whole.start).to_f64() * cycle);
        let mut at = onset;

        for (lane, pattern) in &clip.lanes {
            if lane == "notes" {
                continue;
            }
            // The value in force at the moment the note starts.
            let Some(hap) = pattern.query(CycleSpan::new(whole.start, whole.start)).into_iter().next() else {
                continue;
            };
            match (lane.as_str(), hap.value) {
                ("dur", v) => {
                    dur = v.as_time().map_err(|e| lane_error(clip, name, lane, &e.to_string()))?;
                }
                ("nudge", v) => {
                    let nudge = v.as_time().map_err(|e| lane_error(clip, name, lane, &e.to_string()))?;
                    at = (at + nudge.to_seconds(self.tempo)).max(0.0);
                }
                (_, v) => {
                    args.insert(lane.clone(), v);
                }
            }
        }
        Ok(Event { target: name.clone(), at: Time::Seconds(at), dur, args })
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

    #[test]
    fn a_looping_pattern_produces_notes_on_the_beat() {
        // 120bpm in 4/4: a bar is 2 seconds, so two steps are 1 second apart
        let s = schedule("play lead = [c4 e4]");
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
        let s = schedule("play lead = [c4 e4 g4 b4]");
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
        let s = schedule("clip c { length: 3 bars\nnotes: [<c4 e4>] }\nplay lead = c");
        // one note per bar (2s); alternation over bars 0,1,2 then restart
        let n: Vec<i32> = notes(&s, 0.0, 12.0).into_iter().map(|e| e.2).collect();
        assert_eq!(n, [60, 64, 60, 60, 64, 60]);
        // a short pattern in a long clip simply repeats
        let s = schedule("clip c { length: 2 bars\nnotes: [c4 e4] }\nplay lead = c");
        assert_eq!(times(&s, 0.0, 8.0).len(), 8);
    }

    #[test]
    fn other_lanes_are_read_where_each_note_starts() {
        let s = schedule("clip c { notes: [c4 e4 g4 b4]\nbright: [0.2 0.8] }\nplay lead = c");
        let e = s.events(0.0, 2.0).unwrap();
        let brights: Vec<_> = e.iter().map(|e| e.args["bright"]).collect();
        assert_eq!(brights, [Value::Num(0.2), Value::Num(0.2), Value::Num(0.8), Value::Num(0.8)]);
        assert!(e.iter().all(|e| e.args.contains_key("freq")));
    }

    #[test]
    fn dur_and_nudge_lanes_shape_each_note() {
        let s = schedule("clip c { notes: [c4 e4]\ndur: [1/4beat]\nnudge: [0ms 100ms] }\nplay lead = c");
        let e = s.events(0.0, 2.0).unwrap();
        assert_eq!(e[0].dur, Time::Beats(0.25));
        assert_eq!(e[0].at, Time::Seconds(0.0));
        assert_eq!(e[1].at, Time::Seconds(1.1), "100ms late");
        assert!(!e[0].args.contains_key("dur") && !e[0].args.contains_key("nudge"), "they aren't instrument arguments");
        // a nudge never pushes a note before the start
        let early = schedule("clip c { notes: [c4]\nnudge: [-50ms] }\nplay lead = c");
        assert_eq!(early.events(0.0, 1.0).unwrap()[0].at, Time::Seconds(0.0));
    }

    #[test]
    fn commands_start_replace_and_stop_clips() {
        let s = schedule("
play lead = [c4]
at 2 bars play lead = [e4]
at 3 bars stop lead");
        // bar = 2s: c4 for the first 4s, e4 for 4s..6s, then silence
        let n = notes(&s, 0.0, 20.0);
        assert_eq!(n.iter().map(|e| e.2).collect::<Vec<_>>(), [60, 60, 64]);
        assert_eq!(n.iter().map(|e| e.0).collect::<Vec<_>>(), [0.0, 2.0, 4.0]);
        assert_eq!(s.end(), Some(6.0));
    }

    #[test]
    fn a_clip_started_later_starts_its_own_cycle_there() {
        let s = schedule("at bar 3 play lead = [c4 e4]");
        assert_eq!(times(&s, 0.0, 8.0), [4.0, 5.0, 6.0, 7.0]);
        let mid_beat = schedule("at 1beat play lead = [c4 e4]");
        assert_eq!(times(&mid_beat, 0.0, 3.0), [0.5, 1.5, 2.5]);
    }

    #[test]
    fn tracks_play_independently_and_hush_ends_them_all() {
        let s = schedule("play lead = [c4]\nplay bass = [c2 g2]\nat 2 bars hush");
        let n = notes(&s, 0.0, 20.0);
        assert_eq!(n.iter().filter(|e| e.1 == "lead").count(), 2);
        assert_eq!(n.iter().filter(|e| e.1 == "bass").count(), 4);
        assert_eq!(s.end(), Some(4.0));
        assert!(!s.is_empty());
    }

    #[test]
    fn mute_and_solo_silence_tracks_without_stopping_them() {
        let s = schedule("play lead = [c4]\nplay bass = [c2]\nat 1 bar mute lead\nat 3 bars unmute lead");
        let lead: Vec<f64> = notes(&s, 0.0, 10.0).into_iter().filter(|e| e.1 == "lead").map(|e| e.0).collect();
        assert_eq!(lead, [0.0, 6.0, 8.0], "silent between 2s and 6s; the clip kept its place");
        assert_eq!(notes(&s, 0.0, 10.0).iter().filter(|e| e.1 == "bass").count(), 5);

        let solo = schedule("play lead = [c4]\nplay bass = [c2]\nat 1 bar solo bass\nat 2 bars unsolo");
        let who = |t0, t1| notes(&solo, t0, t1).into_iter().map(|e| e.1).collect::<Vec<_>>();
        assert_eq!(who(0.0, 2.0), ["lead", "bass"]);
        assert_eq!(who(2.0, 4.0), ["bass"], "only the soloed track");
        assert_eq!(who(4.0, 6.0), ["lead", "bass"]);
        let one = schedule("play lead = [c4]\nplay bass = [c2]\nsolo lead\nsolo bass\nat 1 bar unsolo bass");
        assert_eq!(notes(&one, 2.0, 4.0).iter().map(|e| e.1.as_str()).collect::<Vec<_>>(), ["lead"]);
    }

    #[test]
    fn panic_is_reported_and_ends_every_clip() {
        let s = schedule("play lead = [c4]\nat 3 bars panic");
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
launch verse
at 2 bars launch bridge");
        let n = notes(&s, 0.0, 8.0);
        let bass = n.iter().filter(|e| e.1 == "bass").count();
        assert_eq!(bass, 4, "two bars of two notes, then stopped");
        let lead: Vec<i32> = n.iter().filter(|e| e.1 == "lead").map(|e| e.2).collect();
        assert_eq!(lead, [60, 60, 64, 64]);
    }

    #[test]
    fn thinned_patterns_are_the_same_whichever_way_they_are_asked_for() {
        let s = schedule("play lead = [c4*8].thin(40%)");
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
        let s = schedule("play lead = [0.5]");
        let e = s.events(0.0, 2.0).unwrap_err();
        assert!(e.0.contains("must be a note name or a frequency"), "{e}");
        let s = schedule("clip c { notes: [c4]\ndur: [440hz] }\nplay lead = c");
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
        let src = "meter 3/4\ntempo 120bpm\ninstr p(freq: hz) { osc(sine, freq) }\ntrack t { instrument = p }\nplay t = [c4 e4 g4]";
        let s = compile(src).unwrap().schedule();
        // a 3/4 bar at 120bpm is 1.5 seconds, so three steps are half a second apart
        assert_eq!(times(&s, 0.0, 1.6), [0.0, 0.5, 1.0, 1.5]);
    }
}
