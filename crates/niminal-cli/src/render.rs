use std::fmt;

use niminal_engine::{BLOCK, Master, Voice};
use niminal_lang::Program;
use niminal_score::Event;

pub const SAMPLE_RATE: u32 = 48_000;

/// The master limiter's ceiling: -0.3db.
const CEILING: f32 = 0.966_051;

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
    pub samples: Vec<f32>,
    /// Voices that produced NaN or infinity and were silenced.
    pub silenced_voices: usize,
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
    instrument: usize,
    params: Vec<(usize, f32)>,
}

struct Playing {
    voice: Voice,
    off: usize,
}

/// Render the program's own notes plus `extra` events (for example from a
/// `.nms` score) to mono audio. Voices run until their envelopes finish, so
/// the result includes every release tail.
pub fn render(program: &Program, extra: &[Event], options: &RenderOptions) -> Result<RenderOutput, RenderError> {
    let sr = f64::from(SAMPLE_RATE);
    let mut notes = Vec::new();
    for event in program.notes.iter().chain(extra) {
        let Some(instrument) = program.instruments.iter().position(|i| i.name == event.target) else {
            return Err(RenderError(format!("no instrument named `{}`", event.target)));
        };
        let params = program.instruments[instrument]
            .bind_args(&event.args, program.tempo)
            .map_err(|e| RenderError(format!("note for `{}`: {e}", event.target)))?;

        let start = (event.at.to_seconds(program.tempo) * sr).round().max(0.0) as usize;
        let dur = (event.dur.to_seconds(program.tempo) * sr).round().max(0.0) as usize;
        notes.push(Note { start, off: start + dur, instrument, params });
    }
    notes.sort_by_key(|n| n.start);

    let mut out: Vec<f32> = Vec::new();
    let mut playing: Vec<Playing> = Vec::new();
    let mut scratch = [0.0f32; BLOCK];
    let mut silenced_voices = 0;
    let mut next = 0;
    let mut pos = 0;

    loop {
        while next < notes.len() && notes[next].start == pos {
            let note = &notes[next];
            let mut voice = Voice::new(program.instruments[note.instrument].graph.clone(), SAMPLE_RATE as f32);
            for &(index, value) in &note.params {
                voice.set_param(index, value);
            }
            playing.push(Playing { voice, off: note.off });
            next += 1;
        }
        for p in &mut playing {
            if !p.voice.is_released() && p.off <= pos {
                p.voice.release();
            }
        }

        if playing.is_empty() {
            match notes.get(next) {
                None => return Ok(finish(out, silenced_voices, options)),
                Some(n) => {
                    // Nothing sounding: skip the silence.
                    out.resize(n.start, 0.0);
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
        for p in playing.iter().filter(|p| !p.voice.is_released()) {
            n = n.min(p.off - pos);
        }

        out.resize(pos + n, 0.0);
        for p in &mut playing {
            p.voice.process(&mut scratch[..n]);
            for (o, s) in out[pos..].iter_mut().zip(&scratch[..n]) {
                *o += s;
            }
        }
        silenced_voices += playing.iter().filter(|p| p.voice.is_poisoned()).count();
        playing.retain(|p| !p.voice.is_finished());
        pos += n;
    }
}

/// Apply the master stage, removing the limiter's lookahead delay so notes
/// still land exactly where they were scheduled.
fn finish(mut samples: Vec<f32>, silenced_voices: usize, options: &RenderOptions) -> RenderOutput {
    if options.limiter {
        let mut master = Master::new(SAMPLE_RATE as f32, CEILING);
        let latency = master.latency();
        samples.extend(std::iter::repeat_n(0.0, latency));
        master.process(&mut samples);
        samples.drain(..latency);
    }
    RenderOutput { samples, silenced_voices }
}

pub fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for s in samples {
        writer.write_sample(*s)?;
    }
    writer.finalize()
}
