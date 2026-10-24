//! Feed-forward compressor with an optional soft knee and sidechain.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// A compressor. `threshold` and `makeup` are linear gains (the language
/// converts db). `ratio` is how hard it squeezes above the threshold;
/// `attack` and `release` are times in seconds. `sidechain` replaces the
/// detector input when wired; leave it at 0 to key off the main signal. `knee` (a linear gain, so
/// `6db` is a knee six decibels wide) rounds the corner at the threshold: the squeeze eases in over
/// that many decibels centred on it. The default of 1 (0db) is a hard knee.
#[derive(Clone)]
pub struct Compressor {
    envelope: f32,
    sample_rate: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            envelope: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

impl Opcode for Compressor {
    fn reset(&mut self) {
        self.envelope = 0.0;
    }

    fn name(&self) -> &str {
        "compress"
    }

    fn ports(&self) -> &[Port] {
        // threshold default ≈ -12db as linear; makeup 1; ratio 4; attack 5ms; release 50ms.
        // sidechain defaults to NaN: "not wired, key off x". A wired sidechain replaces the detector.
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("threshold", 0.251_188_64), // -12db
            Port::optional("ratio", 4.0),
            Port::optional("attack", 0.005),
            Port::optional("release", 0.05),
            Port::optional("makeup", 1.0),
            Port::optional("sidechain", f32::NAN),
            Port::optional("knee", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.envelope = old.envelope;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let sr = self.sample_rate.max(1.0);
        let attack_coeff = (-1.0 / (inputs[3][0].max(0.0) * sr).max(1.0)).exp();
        let release_coeff = (-1.0 / (inputs[4][0].max(0.0) * sr).max(1.0)).exp();
        // the knee's width in decibels, and how far below the threshold the squeeze begins
        let knee_db = 20.0 * inputs[7][0].max(1.0).log10();
        let below_knee = 10f32.powf(-knee_db / 40.0);
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            let threshold = inputs[1][i].abs().max(1e-8);
            let ratio = inputs[2][i].max(1.0);
            let makeup = inputs[5][i].max(0.0);
            let side = inputs[6][i];
            let detect = if side.is_nan() { x } else { side }.abs();

            let coeff = if detect > self.envelope {
                attack_coeff
            } else {
                release_coeff
            };
            self.envelope = detect + coeff * (self.envelope - detect);

            let gain = if knee_db <= 0.0 {
                if self.envelope <= threshold {
                    1.0
                } else {
                    (self.envelope / threshold).powf(1.0 / ratio - 1.0)
                }
            } else if self.envelope <= threshold * below_knee {
                1.0
            } else {
                // work in decibels: past the knee the squeeze is a straight line, and inside it a
                // curve that meets the line where the knee ends
                let over = 20.0 * (self.envelope / threshold).log10();
                let slope = 1.0 / ratio - 1.0;
                let reduction = if over >= knee_db / 2.0 {
                    slope * over
                } else {
                    slope * (over + knee_db / 2.0).powi(2) / (2.0 * knee_db)
                };
                10f32.powf(reduction / 20.0)
            };
            *o = x * gain * makeup;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain, in decibels, once the level has settled at `level` with the threshold at 0.25.
    fn settled_reduction(level: f32, knee: f32) -> f32 {
        let mut c = Compressor::default();
        c.prepare(48_000.0);
        let ctx = ProcessCtx::new(48_000.0, true);
        let n = 48_000;
        let (x, threshold, ratio, attack, release, makeup, side, knee) = (
            vec![level; n],
            vec![0.25; n],
            vec![4.0; n],
            vec![0.001; n],
            vec![0.05; n],
            vec![1.0; n],
            vec![f32::NAN; n],
            vec![knee; n],
        );
        let mut out = vec![0.0; n];
        c.process(
            &ctx,
            &[
                &x, &threshold, &ratio, &attack, &release, &makeup, &side, &knee,
            ],
            &mut out,
        );
        20.0 * (out[n - 1] / level).log10()
    }

    #[test]
    fn a_knee_of_zero_is_the_hard_knee_it_always_was() {
        assert_eq!(settled_reduction(0.1, 1.0), 0.0);
        // 12db over the threshold at ratio 4: three quarters of it is taken off
        let over = 20.0 * (1.0f32 / 0.25).log10();
        assert!(
            (settled_reduction(1.0, 1.0) - (over * (0.25 - 1.0))).abs() < 0.05,
            "{}",
            settled_reduction(1.0, 1.0)
        );
    }

    #[test]
    fn a_soft_knee_eases_the_squeeze_in_around_the_threshold() {
        let knee = 10f32.powf(6.0 / 20.0); // six decibels wide
        // well below the knee, nothing is taken off
        assert_eq!(settled_reduction(0.1, knee), 0.0);
        // at the threshold: (1/ratio - 1) * (width / 2)^2 / (2 * width) = -0.75 * 0.75 db
        let at = settled_reduction(0.25, knee);
        assert!((at - (-0.5625)).abs() < 0.02, "at the threshold: {at}");
        // past the knee it rejoins the straight line
        let over = 20.0 * (1.0f32 / 0.25).log10();
        let past = settled_reduction(1.0, knee);
        assert!(
            (past - over * (0.25 - 1.0)).abs() < 0.05,
            "past the knee: {past}"
        );
        // and it is never harder than the hard knee, nor softer than nothing
        let inside = settled_reduction(0.25 * 10f32.powf(2.0 / 20.0), knee);
        assert!(inside < 0.0 && inside > over * (0.25 - 1.0), "{inside}");
    }
}
