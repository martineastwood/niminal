use std::fmt;

use niminal_engine::{BLOCK, MAX_CHANNELS, Master, Mixer, VoiceId};
use niminal_daemon::Session;
use niminal_lang::{Layout, NotePlan, Program};
use niminal_score::Event;

pub const SAMPLE_RATE: u32 = 48_000;

/// The master limiter's ceiling: -0.3db.
const CEILING: f32 = 0.966_051;

/// Once nothing is playing, rendering carries on until the output has stayed
/// quieter than this (-90db) for [`TAIL_SILENCE`] seconds, so echoes and reverb
/// tails aren't cut off. The silence itself is trimmed from the result.
const TAIL_THRESHOLD: f32 = 3.162_277_7e-5;
const TAIL_SILENCE: f32 = 1.0;
/// A tail that never settles (a runaway effect) is cut off here.
const MAX_TAIL_SECONDS: f32 = 60.0;

pub struct RenderOptions {
    /// The safety limiter is on unless an offline render turns it off.
    pub limiter: bool,
    /// Stop starting notes after this many seconds. Needed when a clip plays
    /// until stopped; the notes already sounding still ring out.
    pub until: Option<f64>,
    /// Play this arrangement instead of the file's own `play` and `launch` commands.
    pub arrangement: Option<String>,
    /// Also keep what each track put out and what each bus carried.
    pub stems: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { limiter: true, until: None, arrangement: None, stems: false }
    }
}

pub struct RenderOutput {
    /// One buffer per channel of the master layout, all the same length.
    pub channels: Vec<Vec<f32>>,
    /// Voices that produced NaN or infinity and were silenced.
    pub silenced_voices: usize,
    /// With `RenderOptions::stems`: each track and bus, before the master stage, as long as `channels`.
    pub stems: Vec<Stem>,
}

/// One track's or bus's sound on its own.
pub struct Stem {
    pub name: String,
    pub channels: Vec<Vec<f32>>,
}

impl RenderOutput {
    /// Length in samples per channel.
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The loudest sample on any channel.
    pub fn peak(&self) -> f32 {
        self.channels.iter().flatten().fold(0.0, |p, s| p.max(s.abs()))
    }
}

#[derive(Debug, PartialEq)]
pub struct RenderError(pub String);

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RenderError {}

struct Note {
    start: usize,
    off: usize,
    plan: NotePlan,
}

/// Render the program's own notes plus `extra` events (for example from a
/// `.nms` score). Voices run until their envelopes finish, and effect tails
/// ring out, so the result includes every release.
pub fn render(program: &Program, extra: &[Event], options: &RenderOptions) -> Result<RenderOutput, RenderError> {
    let sr = f64::from(SAMPLE_RATE);

    // Notes from `play` and `launch` commands.
    let schedule = match &options.arrangement {
        Some(name) => program.arrangement_schedule(name).map_err(|e| RenderError(e.to_string()))?,
        None => program.schedule(),
    };
    let performed = if schedule.is_empty() {
        Vec::new()
    } else {
        let Some(until) = options.until.or(schedule.end()) else {
            return Err(RenderError(
                "this file plays until it is stopped: say how long to render, for example with `--bars 16`".into(),
            ));
        };
        schedule.events(0.0, until).map_err(|e| RenderError(e.to_string()))?
    };
    let mut panics: Vec<usize> = schedule.panics().iter().map(|t| (t * sr).round() as usize).collect();
    panics.sort_unstable();

    let mut notes = Vec::new();
    for event in program.notes.iter().chain(extra).chain(&performed) {
        let plan = program
            .plan(event)
            .map_err(|e| RenderError(format!("note for `{}`: {e}", event.target)))?;

        let start = (event.at.to_seconds(program.tempo) * sr).round().max(0.0) as usize;
        let dur = (event.dur.to_seconds(program.tempo) * sr).round().max(0.0) as usize;
        notes.push(Note { start, off: start + dur, plan });
    }
    notes.sort_by_key(|n| n.start);

    let mut mixer = program.mixer(SAMPLE_RATE as f32);
    if options.stems {
        mixer.capture_stems();
    }
    let n_channels = mixer.channels();
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); n_channels];
    // Notes that have started but not yet been released, in no particular order.
    let mut held: Vec<(usize, VoiceId)> = Vec::new();
    // each control's last glide
    let mut glides: Vec<Glide> = program.controls.iter().map(|c| Glide::at_rest(c.initial)).collect();
    // what envelope lanes do to track parameters, as (sample, control index, target, glide frames)
    let mut moves: Vec<(usize, usize, f32, u64)> = if schedule.is_empty() {
        Vec::new()
    } else {
        let until = options.until.or(schedule.end()).unwrap_or(0.0);
        let sets = schedule.controls(0.0, until).map_err(|e| RenderError(e.to_string()))?;
        sets.iter()
            .filter_map(|s| {
                let i = program.controls.iter().position(|c| c.name == s.name)?;
                Some(((s.at * sr).round().max(0.0) as usize, i, s.target as f32, (s.glide * sr).round() as u64))
            })
            .collect()
    };
    moves.sort_by_key(|m| m.0);
    let mut next_move = 0;
    let mut silenced_voices = 0;
    let mut next = 0;
    let mut next_panic = 0;
    let mut pos = 0;

    loop {
        // `panic` silences everything at once.
        while panics.get(next_panic).is_some_and(|&p| p <= pos) {
            mixer.panic();
            held.clear();
            next_panic += 1;
        }

        while moves.get(next_move).is_some_and(|m| m.0 <= pos) {
            let (at, i, target, frames) = moves[next_move];
            glides[i].set(&mut mixer, &program.controls[i].key, target, frames, at, pos);
            next_move += 1;
        }

        while next < notes.len() && notes[next].start == pos {
            let note = &notes[next];
            for (key, value, smooth) in &note.plan.controls {
                if let Some(i) = program.controls.iter().position(|c| c.key == *key) {
                    glides[i].set(&mut mixer, key, *value, (smooth * sr).round() as u64, pos, pos);
                }
            }
            let graph = program.instruments[note.plan.instrument].graph.clone();
            let id = mixer.note_on_with(note.plan.track, graph, &note.plan.params, note.plan.choke, note.plan.rule);
            held.push((note.off, id));
            next += 1;
        }
        held.retain(|&(off, id)| {
            if off <= pos {
                mixer.release(id);
            }
            off > pos
        });

        if mixer.active_voices() == 0 {
            match notes.get(next) {
                None => {
                    ring_out(&mut mixer, &mut out);
                    let stems = collect_stems(program, &mut mixer, out[0].len());
                    return Ok(finish(out, silenced_voices + mixer.take_silenced(), stems, options));
                }
                Some(n) => {
                    // Nothing sounding: skip the silence, applying any panics on the way
                    // (they still clear effect tails).
                    while panics.get(next_panic).is_some_and(|&p| p <= n.start) {
                        mixer.panic();
                        next_panic += 1;
                    }
                    out.iter_mut().for_each(|c| c.resize(n.start, 0.0));
                    mixer.pad_stems(n.start);
                    pos = n.start;
                    continue;
                }
            }
        }

        // Run to the next block edge, note start or note end, whichever is first,
        // so that every note starts and stops on exactly the right sample.
        let mut n = BLOCK;
        if let Some(note) = notes.get(next) {
            n = n.min(note.start - pos);
        }
        for &(off, _) in &held {
            n = n.min(off - pos);
        }
        if let Some(&p) = panics.get(next_panic) {
            n = n.min(p - pos);
        }
        if let Some(m) = moves.get(next_move) {
            n = n.min(m.0 - pos);
        }

        mix_block(&mut mixer, &mut out, n);
        silenced_voices += mixer.take_silenced();
        pos += n;
    }
}

/// Mix `frames` more samples onto the end of every channel.
fn mix_block(mixer: &mut Mixer, out: &mut [Vec<f32>], frames: usize) {
    let mut block = [[0.0f32; BLOCK]; MAX_CHANNELS];
    mixer.process(&mut block[..out.len()], frames);
    for (channel, mixed) in out.iter_mut().zip(&block) {
        channel.extend_from_slice(&mixed[..frames]);
    }
}

/// Keep mixing after the last voice has ended, until the tracks' effects have
/// died away, then trim the trailing silence.
fn ring_out(mixer: &mut Mixer, out: &mut [Vec<f32>]) {
    let needed = (TAIL_SILENCE * SAMPLE_RATE as f32) as usize;
    let limit = out[0].len() + (MAX_TAIL_SECONDS * SAMPLE_RATE as f32) as usize;
    let quiet_at = |out: &[Vec<f32>], i: usize| out.iter().all(|c| c[i].abs() < TAIL_THRESHOLD);

    let mut silent = (0..out[0].len()).rev().take_while(|&i| quiet_at(out, i)).count();
    while silent < needed && out[0].len() < limit {
        let start = out[0].len();
        mix_block(mixer, out, BLOCK);
        for i in start..start + BLOCK {
            silent = if quiet_at(out, i) { silent + 1 } else { 0 };
        }
    }
    let keep = out[0].len() - silent;
    out.iter_mut().for_each(|c| c.truncate(keep));
}

/// Apply the master stage, removing the limiter's lookahead delay so notes
/// still land exactly where they were scheduled.
fn finish(mut channels: Vec<Vec<f32>>, silenced_voices: usize, stems: Vec<Stem>, options: &RenderOptions) -> RenderOutput {
    if options.limiter {
        let mut master = Master::with_channels(SAMPLE_RATE as f32, CEILING, channels.len());
        let latency = master.latency();
        for c in &mut channels {
            c.extend(std::iter::repeat_n(0.0, latency));
        }
        let mut views: Vec<&mut [f32]> = channels.iter_mut().map(Vec::as_mut_slice).collect();
        master.process_linked(&mut views);
        for c in &mut channels {
            c.drain(..latency);
        }
    }
    RenderOutput { channels, silenced_voices, stems }
}

/// The captured stems, named for their track or bus and cut to `len` samples. The track that plays
/// bare instruments is left out if it was never heard.
fn collect_stems(program: &Program, mixer: &mut Mixer, len: usize) -> Vec<Stem> {
    let Some(captured) = mixer.take_stems() else { return Vec::new() };
    let named = captured.tracks.into_iter().enumerate().map(|(i, c)| (program.tracks[i].name.clone(), c))
        .chain(captured.buses.into_iter().enumerate().map(|(i, c)| (program.buses[i].clone(), c)));
    let mut stems = Vec::new();
    for (i, (name, mut channels)) in named.enumerate() {
        channels.iter_mut().for_each(|c| c.resize(len, 0.0));
        let silent = channels.iter().flatten().all(|s| *s == 0.0);
        // the implicit track has no name; say what it is
        match (i, name.is_empty()) {
            (0, true) if silent => continue,
            (0, true) => stems.push(Stem { name: "direct".into(), channels }),
            _ => stems.push(Stem { name, channels }),
        }
    }
    stems
}

/// Write the channels as an interleaved 32-bit float WAV.
pub fn write_wav(path: &std::path::Path, channels: &[Vec<f32>]) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    let len = channels.first().map_or(0, Vec::len);
    for i in 0..len {
        for c in channels {
            writer.write_sample(c[i])?;
        }
    }
    writer.finalize()
}

/// The file without its tempo ramps (their lines left blank so everything stays where it was), and the
/// ramps. Empty if there are none.
pub fn split_ramps(source: &str) -> Result<(String, Vec<String>), niminal_lang::Diagnostic> {
    let statements = niminal_lang::analyze(source)?;
    let mut rest = source.to_string();
    let mut ramps = Vec::new();
    for ramp in statements.iter().filter(|s| s.ramp.is_some()) {
        let blank: String = ramp.text.chars().map(|c| if c == '\n' { '\n' } else { ' ' }).collect();
        rest.replace_range(ramp.offset..ramp.offset + ramp.text.len(), &blank);
        ramps.push(ramp.text.clone());
    }
    Ok((rest, ramps))
}

/// How far to render a file that is played through a session.
#[derive(Clone, Copy)]
pub enum End {
    Bars(f64),
    Seconds(f64),
}

/// Render a file the way the daemon would play it: sent to a session all at once, then run for `end`.
/// This is how a tempo ramp (`tempo 140bpm @ next bar over 8 bars`) is heard, since a ramp is a run of
/// changes made as the music goes. Everything still sounding when the time is up rings out.
///
/// `source` has its `use` lines already joined in. The file's own `tempo` stays the starting tempo; each
/// ramp is sent after the rest, so it sets off from there at the boundary it names.
pub fn render_session(source: &str, dir: &std::path::Path, layout: Layout, end: End, limiter: bool) -> Result<RenderOutput, RenderError> {
    const STEP: usize = BLOCK * 8;
    let problems = |ps: Vec<niminal_daemon::Problem>| RenderError(ps.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join("\n"));
    let (rest, ramps) = split_ramps(source).map_err(|d| RenderError(d.message))?;
    let mut session = Session::new(SAMPLE_RATE as f32, layout).without_limiter();
    session.set_sample_dir(dir);
    session.eval(&rest, Some("now")).map_err(problems)?;
    for ramp in &ramps {
        session.eval(ramp, Some("now")).map_err(problems)?;
    }

    let mut out: Vec<Vec<f32>> = vec![Vec::new(); layout.channels()];
    let run = |session: &mut Session, out: &mut Vec<Vec<f32>>| {
        for (o, block) in out.iter_mut().zip(session.process(STEP)) {
            o.extend(block);
        }
    };
    loop {
        run(&mut session, &mut out);
        let t = session.transport();
        let done = match end {
            End::Bars(bars) => (t.bar - 1) as f64 + (t.beat - 1.0) / session.program().tempo.beats_per_bar >= bars,
            End::Seconds(s) => t.seconds >= s,
        };
        if done {
            break;
        }
    }
    session.hush().map_err(|_| RenderError("couldn't stop the piece".into()))?;

    let needed = (TAIL_SILENCE * SAMPLE_RATE as f32) as usize;
    let limit = out[0].len() + (MAX_TAIL_SECONDS * SAMPLE_RATE as f32) as usize;
    let quiet_at = |out: &[Vec<f32>], i: usize| out.iter().all(|c| c[i].abs() < TAIL_THRESHOLD);
    let mut silent = (0..out[0].len()).rev().take_while(|&i| quiet_at(&out, i)).count();
    while silent < needed && out[0].len() < limit {
        let start = out[0].len();
        run(&mut session, &mut out);
        for i in start..out[0].len() {
            silent = if quiet_at(&out, i) { silent + 1 } else { 0 };
        }
    }
    let keep = out[0].len() - silent;
    out.iter_mut().for_each(|c| c.truncate(keep));
    let silenced = session.take_silenced();
    Ok(finish(out, silenced, Vec::new(), &RenderOptions { limiter, ..Default::default() }))
}

/// Where a control is heading: from `from` towards `target` over `frames` samples starting at `at`.
struct Glide {
    from: f32,
    target: f32,
    at: usize,
    frames: u64,
}

impl Glide {
    fn at_rest(value: f32) -> Self {
        Glide { from: value, target: value, at: 0, frames: 0 }
    }

    fn value(&self, pos: usize) -> f32 {
        if self.frames == 0 {
            return self.target;
        }
        let done = (pos.saturating_sub(self.at) as u64).min(self.frames);
        self.from + (self.target - self.from) * done as f32 / self.frames as f32
    }

    /// Glide to `target` from where the control was at sample `at`, told at sample `pos` (which may be later).
    fn set(&mut self, mixer: &mut Mixer, key: &str, target: f32, frames: u64, at: usize, pos: usize) {
        let from = self.value(at);
        *self = Glide { from, target, at, frames };
        mixer.set_control(key, from, target, frames, pos.saturating_sub(at) as u64);
    }
}
