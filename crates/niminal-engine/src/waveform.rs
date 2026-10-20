//! Bounded, single-renderer waveform capture; readers never hold up audio.
use crate::{BLOCK, MAX_CHANNELS};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub const WAVEFORM_FRAMES: usize = 4096;
#[derive(Debug)]
pub struct Waveform {
    pub enabled: AtomicBool,
    pub consumers: AtomicUsize,
    sequence: AtomicU64,
    end: AtomicU64,
    start: AtomicU64,
    channels: AtomicU32,
    values: Box<[AtomicU32]>,
}
impl Default for Waveform {
    fn default() -> Self {
        Self {
            consumers: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
            end: AtomicU64::new(0),
            start: AtomicU64::new(0),
            channels: AtomicU32::new(0),
            values: (0..WAVEFORM_FRAMES * MAX_CHANNELS)
                .map(|_| AtomicU32::new(0))
                .collect(),
        }
    }
}
impl Waveform {
    pub fn end_sample(&self) -> u64 {
        self.end.load(Ordering::Acquire)
    }
    pub fn observe(&self, blocks: &[[f32; BLOCK]], frames: usize, sample: u64) {
        if !self.enabled.load(Ordering::Relaxed) && self.consumers.load(Ordering::Relaxed) == 0 {
            return;
        }
        self.sequence.fetch_add(1, Ordering::AcqRel);
        if self.end.load(Ordering::Relaxed) != sample {
            self.start.store(sample, Ordering::Relaxed);
        }
        for i in 0..frames {
            for (c, block) in blocks.iter().enumerate() {
                self.values[((sample as usize + i) % WAVEFORM_FRAMES) * MAX_CHANNELS + c]
                    .store(block[i].to_bits(), Ordering::Relaxed);
            }
        }
        self.channels.store(blocks.len() as u32, Ordering::Relaxed);
        self.end.store(sample + frames as u64, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }
    /// Return a coherent interleaved window, or skip this poll if rendering won the race.
    pub fn read(&self, frames: usize) -> Option<(u64, usize, Vec<f32>)> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence % 2 != 0 {
            return None;
        }
        let end = self.end.load(Ordering::Relaxed);
        let start = self
            .start
            .load(Ordering::Relaxed)
            .max(end.saturating_sub(frames.min(WAVEFORM_FRAMES) as u64));
        let channels = self.channels.load(Ordering::Relaxed) as usize;
        if channels == 0 || start == end {
            return None;
        }
        let mut values = Vec::with_capacity((end - start) as usize * channels);
        for s in start..end {
            for c in 0..channels {
                values.push(f32::from_bits(
                    self.values[(s as usize % WAVEFORM_FRAMES) * MAX_CHANNELS + c]
                        .load(Ordering::Relaxed),
                ));
            }
        }
        std::sync::atomic::fence(Ordering::Acquire);
        (self.sequence.load(Ordering::Relaxed) == sequence).then_some((start, channels, values))
    }
}
