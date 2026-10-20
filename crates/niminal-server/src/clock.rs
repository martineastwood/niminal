//! What the audio callback tells the rest of the server about where the output is, so that a client
//! can line a sample up with the wall clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Written by the audio callback, read by anything that answers `clock`. All values are for the
/// start of the buffer the callback was handed.
#[derive(Default)]
pub struct ClockProbe {
    sample: AtomicU64,
    wall_micros: AtomicU64,
    latency_micros: AtomicU64,
}

impl ClockProbe {
    /// The callback is handing the device the buffer that starts at `sample`. Allocates nothing.
    pub fn record(&self, sample: u64) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
        self.wall_micros.store(now, Ordering::Release);
        self.sample.store(sample, Ordering::Release);
    }

    /// How long after the callback that buffer sounds, as the audio API reports it.
    pub fn set_latency_micros(&self, micros: u64) {
        self.latency_micros.store(micros, Ordering::Relaxed);
    }

    /// `(sample, wall_micros, latency_micros)`, or `None` if there is no output yet.
    pub fn read(&self) -> Option<(u64, u64, u64)> {
        let wall = self.wall_micros.load(Ordering::Acquire);
        let sample = self.sample.load(Ordering::Acquire);
        (wall != 0).then(|| (sample, wall, self.latency_micros.load(Ordering::Relaxed)))
    }
}
