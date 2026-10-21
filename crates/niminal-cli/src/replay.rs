//! Pieces of the commands that don't need a terminal: choosing a layout, and replaying a log to audio.

use niminal_lang::Layout;
use niminal_server::{LogEntry, Session};

use crate::render::SAMPLE_RATE;

/// Play a recorded session back to audio: one buffer per channel, with the
/// limiter's delay removed and trailing silence trimmed.
pub fn replay(
    log: &[LogEntry],
    channels: u16,
    sample_rate: u32,
    limiter: bool,
) -> Result<Vec<Vec<f32>>, String> {
    const TAIL_SECONDS: u64 = 30;
    const QUIET: f32 = 3.162_277_7e-5;

    let layout = Layout::standard(channels)?;
    let last = log.iter().map(|e| e.sample).max().unwrap_or(0);
    let frames = last + TAIL_SECONDS * u64::from(sample_rate);
    let mut audio = Session::replay(sample_rate as f32, layout, log, frames, limiter);

    if limiter {
        let latency = Session::new(sample_rate as f32, layout).latency();
        for c in &mut audio {
            c.drain(..latency.min(c.len()));
        }
    }
    let loud_until = (0..audio[0].len())
        .rev()
        .find(|&i| audio.iter().any(|c| c[i].abs() >= QUIET))
        .map_or(0, |i| i + 1);
    audio.iter_mut().for_each(|c| c.truncate(loud_until));
    Ok(audio)
}

pub const DEFAULT_SAMPLE_RATE: u32 = SAMPLE_RATE;
