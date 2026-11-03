//! Feed-forward compressor with a dB-domain envelope, optional soft knee and sidechain.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// A compressor. `threshold` and `makeup` are linear gains (the language
/// converts db). `ratio` is how hard it squeezes above the threshold;
/// `attack` and `release` are times in seconds. `sidechain` replaces the
/// detector input when wired; omit it to key off the main signal. `knee` (a linear gain, so
/// `6db` is a knee six decibels wide) rounds the corner at the threshold: the squeeze eases in over
/// that many decibels centred on it. The default of 1 (0db) is a hard knee.
#[derive(Clone)]
pub struct Compressor {
    envelope_db: f32,
    sample_rate: f32,
    cached_for: (f32, f32, f32),
    attack_coeff: f32,
    release_coeff: f32,
    knee_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            envelope_db: 0.0,
            sample_rate: 48_000.0,
            cached_for: (f32::NAN, f32::NAN, f32::NAN),
            attack_coeff: 0.0,
            release_coeff: 0.0,
            knee_db: 0.0,
        }
    }
}

impl Opcode for Compressor {
    fn reset(&mut self) {
        self.envelope_db = 0.0;
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
        self.cached_for = (f32::NAN, f32::NAN, f32::NAN);
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.envelope_db = old.envelope_db;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let sr = self.sample_rate.max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            let controls = (
                inputs[3][i].max(0.0),
                inputs[4][i].max(0.0),
                inputs[7][i].max(1.0),
            );
            if controls != self.cached_for {
                self.attack_coeff = (-1.0 / (controls.0 * sr).max(1.0)).exp();
                self.release_coeff = (-1.0 / (controls.1 * sr).max(1.0)).exp();
                self.knee_db = 20.0 * controls.2.log10();
                self.cached_for = controls;
            }
            let knee_db = self.knee_db;
            let x = inputs[0][i];
            let threshold = inputs[1][i].abs().max(1e-8);
            let ratio = inputs[2][i].max(1.0);
            let makeup = inputs[5][i].max(0.0);
            let side = inputs[6][i];
            let detect = if side.is_nan() { x } else { side }.abs();

            // The detector smooths only the excess above threshold, in dB.
            // The floor makes silence safe without adding a DC offset to the envelope.
            let over_db = 20.0 * (detect.max(1e-20) / threshold).log10();
            let target_db = if knee_db <= 0.0 {
                over_db.max(0.0)
            } else if over_db <= -knee_db / 2.0 {
                0.0
            } else if over_db >= knee_db / 2.0 {
                over_db
            } else {
                (over_db + knee_db / 2.0).powi(2) / (2.0 * knee_db)
            };
            let coeff = if target_db > self.envelope_db {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.envelope_db = target_db + coeff * (self.envelope_db - target_db);
            let reduction_db = self.envelope_db * (1.0 / ratio - 1.0);
            let gain = 10f32.powf(reduction_db / 20.0);
            *o = x * gain * makeup;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_key(c: &mut Compressor, key: f32, n: usize) -> Vec<f32> {
        let inputs = [1.0, 0.25, 4.0, 0.01, 0.1, 1.0, key, 1.0].map(|value| vec![value; n]);
        let mut out = vec![0.0; n];
        c.process(
            &ProcessCtx::new(48_000.0, true),
            &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            &mut out,
        );
        out
    }

    #[test]
    fn attack_and_release_follow_db_time_constants() {
        let mut c = Compressor::default();
        c.prepare(48_000.0);
        // A constant sidechain 12.04 dB above threshold asks for 9.03 dB reduction.
        let target = 20.0 * 4.0f32.log10();
        let attack = run_key(&mut c, 1.0, 480);
        let expected = target * (1.0 - (-1.0f32).exp());
        assert!((c.envelope_db - expected).abs() < 0.001);
        assert!((20.0 * attack[479].log10() + expected * 0.75).abs() < 0.001);
        // A silent wired sidechain releases even though the main input remains loud.
        let released = run_key(&mut c, 0.0, 4800);
        let expected = expected * (-1.0f32).exp();
        assert!((c.envelope_db - expected).abs() < 0.002);
        assert!((20.0 * released[4799].log10() + expected * 0.75).abs() < 0.002);
    }

    #[test]
    fn detector_is_independent_of_block_boundaries_and_resets_cleanly() {
        let mut whole = Compressor::default();
        let expected = run_key(&mut whole, 1.0, 480);
        let mut split = Compressor::default();
        let actual: Vec<_> = (0..10).flat_map(|_| run_key(&mut split, 1.0, 48)).collect();
        assert_eq!(actual, expected);
        split.reset();
        assert_eq!(run_key(&mut split, 1.0, 480), expected);
        // Silence at startup must neither reduce gain nor produce NaN.
        split.reset();
        assert_eq!(run_key(&mut split, 0.0, 48), vec![1.0; 48]);
    }

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
