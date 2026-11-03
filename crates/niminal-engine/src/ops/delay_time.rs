//! A tape-style read head: bounded speed, with a short ease into its target.

#[derive(Clone, Copy)]
pub(super) struct DelayTime {
    current: Option<f64>,
    coefficient: f64,
}

impl DelayTime {
    pub(super) fn new(sample_rate: f32) -> Self {
        Self {
            current: None,
            // 60 dB convergence in 20 ms for small moves. The speed limit
            // bounds playback to 0.5..1.5 times forward speed on large moves.
            coefficient: 1.0 - (-6.907_755 / (0.02 * f64::from(sample_rate))).exp(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.current = None;
    }

    pub(super) fn next(&mut self, target: f64) -> f64 {
        let target = if target.is_finite() {
            target.max(1.0)
        } else {
            self.current.unwrap_or(1.0)
        };
        let current = self.current.unwrap_or(target);
        let delta = target - current;
        let next = if delta.abs() < 1e-6 {
            target
        } else {
            current + (delta * self.coefficient).clamp(-0.5, 0.5)
        };
        self.current = Some(next);
        next
    }
}
