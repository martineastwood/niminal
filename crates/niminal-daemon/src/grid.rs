//! The musical grid of the transport: where bars and beats fall in samples.

/// Tempo, meter and bar origin, as sample positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub sample_rate: f64,
    pub bpm: f64,
    pub beats_per_bar: f64,
    /// The sample where bar 1 starts: where the last `origin` landed. It is before the start of the
    /// session when a slower tempo or longer bar has stretched the bars played so far.
    pub origin: i64,
}

impl Grid {
    pub fn beat_seconds(&self) -> f64 {
        60.0 / self.bpm
    }

    /// Beats since bar 1 started, fractions included.
    pub fn beats(&self, sample: u64) -> f64 {
        (sample as i64 - self.origin) as f64 / self.sample_rate / self.beat_seconds()
    }

    /// `bar:beat` of a sample, counting from 1.
    pub fn position(&self, sample: u64) -> (u64, f64) {
        let beats = self.beats(sample);
        let bar = (beats / self.beats_per_bar).floor();
        (bar as u64 + 1, beats - bar * self.beats_per_bar + 1.0)
    }
}
