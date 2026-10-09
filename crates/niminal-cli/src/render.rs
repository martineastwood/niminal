use std::fmt;

use niminal_engine::{BLOCK, MAX_CHANNELS, Master, Mixer, VoiceId};
use niminal_lang::{NotePlan, Program};
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
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { limiter: true }
    }
}

pub struct RenderOutput {
    /// One buffer per channel of the master layout, all the same length.
    pub channels: Vec<Vec<f32>>,
    /// Voices that produced NaN or infinity and were silenced.
    pub silenced_voices: usize,
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
    let mut notes = Vec::new();
    for event in program.notes.iter().chain(extra) {
        let plan = program
            .plan(event)
            .map_err(|e| RenderError(format!("note for `{}`: {e}", event.target)))?;

        let start = (event.at.to_seconds(program.tempo) * sr).round().max(0.0) as usize;
        let dur = (event.dur.to_seconds(program.tempo) * sr).round().max(0.0) as usize;
        notes.push(Note { start, off: start + dur, plan });
    }
    notes.sort_by_key(|n| n.start);

    let mut mixer = program.mixer(SAMPLE_RATE as f32);
    let n_channels = mixer.channels();
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); n_channels];
    // Notes that have started but not yet been released, in no particular order.
    let mut held: Vec<(usize, VoiceId)> = Vec::new();
    let mut silenced_voices = 0;
    let mut next = 0;
    let mut pos = 0;

    loop {
        while next < notes.len() && notes[next].start == pos {
            let note = &notes[next];
            let graph = program.instruments[note.plan.instrument].graph.clone();
            let id = mixer.note_on(note.plan.track, graph, &note.plan.params);
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
                    return Ok(finish(out, silenced_voices + mixer.take_silenced(), options));
                }
                Some(n) => {
                    // Nothing sounding: skip the silence.
                    out.iter_mut().for_each(|c| c.resize(n.start, 0.0));
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
fn finish(mut channels: Vec<Vec<f32>>, silenced_voices: usize, options: &RenderOptions) -> RenderOutput {
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
    RenderOutput { channels, silenced_voices }
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
