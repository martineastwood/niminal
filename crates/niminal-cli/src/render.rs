use std::fmt;

use niminal_engine::{BLOCK, DEFAULT_CEILING, Master};
use niminal_lang::{Layout, Program};
use niminal_live::Offline;
use niminal_score::Event;

pub const SAMPLE_RATE: u32 = 48_000;

/// How much a session is asked for at a time.
const STEP: usize = BLOCK * 8;

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
    /// What tracks that read `input(..)` hear: a buffer for each channel, from the start.
    pub input: Vec<Vec<f32>>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { limiter: true, until: None, arrangement: None, stems: false, input: Vec::new() }
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

/// Render the program's own notes plus `extra` events (for example from a
/// `.nms` score). Voices run until their envelopes finish, and effect tails
/// ring out, so the result includes every release.
pub fn render(program: &Program, extra: &[Event], options: &RenderOptions) -> Result<RenderOutput, RenderError> {
    let sr = f64::from(SAMPLE_RATE);
    let error = |e: &dyn fmt::Display| RenderError(e.to_string());

    // Notes from `play` and `launch` commands.
    let schedule = match &options.arrangement {
        Some(name) => program.arrangement_schedule(name).map_err(|e| error(&e))?,
        None => program.schedule(),
    };
    let until = options.until.or(schedule.end());
    let mut performed = Vec::new();
    if !schedule.is_empty() {
        let Some(until) = until else {
            return Err(RenderError(
                "this file plays until it is stopped: say how long to render, for example with `--bars 16`".into(),
            ));
        };
        performed = schedule.events(0.0, until).map_err(|e| error(&e))?;
    }
    // mistakes in a note are reported before anything is rendered
    for event in program.notes.iter().chain(&performed) {
        program.plan(event).map_err(|e| RenderError(format!("note for `{}`: {e}", event.target)))?;
    }

    // the notes written out in the file and sent in count towards how long it plays, as the schedule does
    let note_end = program.notes.iter().chain(extra)
        .map(|e| (e.at.to_seconds(program.tempo) + e.dur.to_seconds(program.tempo)) * sr)
        .fold(0.0, f64::max);
    let input_len = options.input.iter().map(Vec::len).max().unwrap_or(0);
    let stop = ((until.unwrap_or(0.0) * sr).max(note_end).round() as usize).max(input_len) as u64;

    let mut session = Offline::new(SAMPLE_RATE as f32, program.master).without_limiter();
    session.play_program(program.clone(), schedule);
    session.sequencer().send_notes(extra).map_err(|ps| problems(&ps))?;
    if options.stems {
        session.capture_stems();
    }
    session.set_audio_input(options.input.clone());
    let mut out = vec![Vec::new(); session.channels()];
    // exactly to `stop`, so that no note starts after it
    while session.clock() < stop {
        let frames = ((stop - session.clock()) as usize).min(STEP);
        run(&mut session, &mut out, frames);
    }
    session.end_schedule();
    Ok(collect(session, out, options.limiter))
}

fn problems(ps: &[niminal_daemon::Problem]) -> RenderError {
    RenderError(ps.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join("\n"))
}

/// Play `session` on, adding to `out`, until `done`.
fn run_until(session: &mut Offline, out: &mut [Vec<f32>], mut done: impl FnMut(&Offline) -> bool) {
    loop {
        run(session, out, STEP);
        if done(session) {
            return;
        }
    }
}

fn run(session: &mut Offline, out: &mut [Vec<f32>], frames: usize) {
    for (o, block) in out.iter_mut().zip(session.process(frames)) {
        o.extend(block);
    }
}

/// Let what is still sounding ring out, then name the stems and apply the master stage.
fn collect(mut session: Offline, mut out: Vec<Vec<f32>>, limiter: bool) -> RenderOutput {
    ring_out(&mut out, |out| run(&mut session, out, STEP));
    let len = out[0].len();
    let silenced = session.take_silenced();
    let captured = session.take_stems();
    let stems = name_stems(session.engine().session().program(), captured, len);
    finish(out, silenced, stems, limiter)
}

/// Keep mixing after the last voice has ended, until the tracks' effects have
/// died away, then trim the trailing silence. `more` adds audio to the end of `out`.
fn ring_out(out: &mut Vec<Vec<f32>>, mut more: impl FnMut(&mut Vec<Vec<f32>>)) {
    let needed = (TAIL_SILENCE * SAMPLE_RATE as f32) as usize;
    let limit = out[0].len() + (MAX_TAIL_SECONDS * SAMPLE_RATE as f32) as usize;
    let quiet_at = |out: &[Vec<f32>], i: usize| out.iter().all(|c| c[i].abs() < TAIL_THRESHOLD);

    let mut silent = (0..out[0].len()).rev().take_while(|&i| quiet_at(out, i)).count();
    while silent < needed && out[0].len() < limit {
        let start = out[0].len();
        more(out);
        for i in start..out[0].len() {
            silent = if quiet_at(out, i) { silent + 1 } else { 0 };
        }
    }
    let keep = out[0].len() - silent;
    out.iter_mut().for_each(|c| c.truncate(keep));
}

/// Apply the master stage, removing the limiter's lookahead delay so notes
/// still land exactly where they were scheduled.
fn finish(mut channels: Vec<Vec<f32>>, silenced_voices: usize, stems: Vec<Stem>, limiter: bool) -> RenderOutput {
    if limiter {
        let mut master = Master::with_channels(SAMPLE_RATE as f32, DEFAULT_CEILING, channels.len());
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
fn name_stems(program: &Program, captured: Option<niminal_engine::Stems>, len: usize) -> Vec<Stem> {
    let Some(captured) = captured else { return Vec::new() };
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
    for ramp in statements.iter().filter(|s| s.ramp().is_some()) {
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
/// This is how a tempo ramp (`tempo 140bpm @ next bar over 8bars`) is heard, since a ramp is a run of
/// changes made as the music goes. Everything still sounding when the time is up rings out.
///
/// `source` has its `use` lines already joined in. The file's own `tempo` stays the starting tempo; each
/// ramp is sent after the rest, so it sets off from there at the boundary it names.
pub fn render_session(source: &str, dir: &std::path::Path, layout: Layout, end: End, limiter: bool, stems: bool, input: Vec<Vec<f32>>) -> Result<RenderOutput, RenderError> {
    let (rest, ramps) = split_ramps(source).map_err(|d| RenderError(d.message))?;
    let mut session = Offline::new(SAMPLE_RATE as f32, layout).without_limiter();
    session.set_sample_dir(dir);
    session.eval(&rest, Some("now")).map_err(|ps| problems(&ps))?;
    for ramp in &ramps {
        session.eval(ramp, Some("now")).map_err(|ps| problems(&ps))?;
    }
    if stems {
        session.capture_stems();
    }
    session.set_audio_input(input);

    let mut out: Vec<Vec<f32>> = vec![Vec::new(); layout.channels()];
    run_until(&mut session, &mut out, |session| {
        let t = session.transport();
        match end {
            End::Bars(bars) => (t.bar - 1) as f64 + (t.beat - 1.0) / session.program().tempo.beats_per_bar >= bars,
            End::Seconds(s) => t.seconds >= s,
        }
    });
    session.hush().map_err(|_| RenderError("couldn't stop the piece".into()))?;
    Ok(collect(session, out, limiter))
}
