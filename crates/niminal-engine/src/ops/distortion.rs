//! Tube-style asymmetric distortion with tone control and a DC blocker.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// Asymmetric tube-style distortion. `drive` controls how hard the signal
/// is pushed (1 = clean, higher = more harmonics); `tone` is a post-
/// distortion tilt EQ (0 = dark, 0.5 = neutral, 1 = bright); `mix`
/// blends wet with dry. The asymmetry that gives the even harmonics also shifts the signal
/// off zero, so a high-pass at 10Hz takes that offset out before the tone filter.
///
/// It is not oversampled, so a bright signal pushed hard folds harmonics back down; wrap it in
/// `oversample(4) { ... }` for that.
#[derive(Clone)]
pub struct Distortion {
    sample_rate: f32,
    /// One-pole state for the tone filter.
    tone_state: f64,
    /// The tone filter's coefficient, and the tone setting it was worked out for.
    tone_coeff: f64,
    tone_for: f64,
    /// The DC blocker's memory: its last input and output.
    dc_in: f64,
    dc_out: f64,
}

/// The corner of the DC blocker.
const DC_CUTOFF_HZ: f64 = 10.0;

impl Default for Distortion {
    fn default() -> Self {
        Distortion {
            sample_rate: 48_000.0,
            tone_state: 0.0,
            tone_coeff: 0.0,
            tone_for: f64::NAN,
            dc_in: 0.0,
            dc_out: 0.0,
        }
    }
}

impl Opcode for Distortion {
    fn reset(&mut self) {
        self.tone_state = 0.0;
        self.dc_in = 0.0;
        self.dc_out = 0.0;
    }

    fn name(&self) -> &str {
        "distortion"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("drive", 4.0),
            Port::optional("tone", 0.5),
            Port::optional("mix", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.tone_for = f64::NAN;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.tone_state = old.tone_state;
            self.dc_in = old.dc_in;
            self.dc_out = old.dc_out;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        let dc_r = (-std::f64::consts::TAU * DC_CUTOFF_HZ / sr).exp();
        for (i, o) in out.iter_mut().enumerate() {
            let dry = f64::from(inputs[0][i]);
            let drive = f64::from(inputs[1][i]).max(1.0);
            let tone = f64::from(inputs[2][i]).clamp(0.0, 1.0);
            let mix = f64::from(inputs[3][i]).clamp(0.0, 1.0);

            // asymmetric soft clipping: positive side clips harder
            let driven = dry * drive;
            let shaped = if driven >= 0.0 {
                1.0 - (-driven).exp()
            } else {
                -(1.0 - (driven * 0.7).exp()) / 0.7
            };

            // take the offset the asymmetry makes out of the signal
            let centred = flush(shaped - self.dc_in + dc_r * self.dc_out);
            self.dc_in = shaped;
            self.dc_out = centred;

            // one-pole tone: tone 0..1 maps to lowpass cutoff 200Hz..20kHz; the coefficient is only
            // worked out again when the setting moves
            if tone != self.tone_for {
                let freq = 200.0 * (100.0_f64).powf(tone);
                self.tone_coeff = (-std::f64::consts::TAU * freq / sr).exp();
                self.tone_for = tone;
            }
            let coeff = self.tone_coeff;
            self.tone_state = flush(centred * (1.0 - coeff) + self.tone_state * coeff);

            *o = (dry * (1.0 - mix) + self.tone_state * mix) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(drive: f32, tone: f32, input: &[f32]) -> Vec<f32> {
        let mut d = Distortion::default();
        d.prepare(48_000.0);
        let ctx = ProcessCtx::new(48_000.0, true);
        let (drives, tones, mixes) = (
            vec![drive; input.len()],
            vec![tone; input.len()],
            vec![1.0; input.len()],
        );
        let mut out = vec![0.0; input.len()];
        d.process(&ctx, &[input, &drives, &tones, &mixes], &mut out);
        out
    }

    #[test]
    fn hard_asymmetric_clipping_leaves_no_offset() {
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = run(8.0, 1.0, &sine);
        let tail = &out[24_000..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        let loudest = tail.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(
            mean.abs() < 0.01 * loudest,
            "offset {mean} against a peak of {loudest}"
        );
    }

    #[test]
    fn it_still_makes_even_harmonics() {
        // the asymmetry is what gives a second harmonic; the blocker must not take it away
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = run(4.0, 1.0, &sine);
        let bin = |hz: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in out[24_000..].iter().enumerate() {
                let p = std::f32::consts::TAU * hz * i as f32 / 48_000.0;
                re += v * p.cos();
                im += v * p.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(
            bin(2000.0) > 0.05 * bin(1000.0),
            "second harmonic {} against {}",
            bin(2000.0),
            bin(1000.0)
        );
    }
}
