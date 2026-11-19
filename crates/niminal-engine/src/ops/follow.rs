//! Envelope follower: tracks the amplitude of a signal.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Tracks the amplitude envelope of a signal with adjustable attack and
/// release times. The output is always positive: the smoothed absolute
/// value of the input.
#[derive(Clone)]
pub struct Follow {
    envelope: f64,
    sample_rate: f32,
    /// The smoothing factor for the last attack and release times, which hold still for long stretches.
    attack: Option<(u32, f64)>,
    release: Option<(u32, f64)>,
}

/// The factor that moves a smoother `time` seconds towards its target, reusing the last one for the same time.
#[inline(always)]
fn smoothing(cache: &mut Option<(u32, f64)>, time: f32, sr: f64) -> f64 {
    match *cache {
        Some((bits, coeff)) if bits == time.to_bits() => coeff,
        _ => {
            let seconds = f64::from(time).max(0.0);
            let coeff = (-1.0 / (seconds * sr).max(1.0)).exp();
            *cache = Some((time.to_bits(), coeff));
            coeff
        }
    }
}

impl Default for Follow {
    fn default() -> Self {
        Follow {
            envelope: 0.0,
            sample_rate: 48_000.0,
            attack: None,
            release: None,
        }
    }
}

impl Opcode for Follow {
    fn reset(&mut self) {
        self.envelope = 0.0;
    }

    fn name(&self) -> &str {
        "follow"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("attack", 0.01),
            Port::optional("release", 0.1),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.attack = None;
        self.release = None;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.envelope = old.envelope;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate).max(1.0);
        let n = out.len();
        let (x, attack, release) = (&inputs[0][..n], &inputs[1][..n], &inputs[2][..n]);
        for (i, o) in out.iter_mut().enumerate() {
            let level = f64::from(x[i]).abs();
            let coeff = if level > self.envelope {
                smoothing(&mut self.attack, attack[i], sr)
            } else {
                smoothing(&mut self.release, release[i], sr)
            };
            self.envelope = level + coeff * (self.envelope - level);
            *o = self.envelope as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The remembered smoothing factors must give exactly what working them out each sample does,
    /// including when the times change in the middle of a block.
    #[test]
    fn remembered_factors_match_working_them_out_every_sample() {
        let n = 200;
        let x: Vec<f32> = (0..n).map(|i| ((i * 37 % 19) as f32 - 9.0) / 9.0).collect();
        let attack: Vec<f32> = (0..n).map(|i| if i < 90 { 0.01 } else if i < 140 { 0.002 } else { -1.0 }).collect();
        let release: Vec<f32> = (0..n).map(|i| if i % 50 < 25 { 0.1 } else { 0.3 }).collect();
        let mut follow = Follow::default();
        follow.prepare(44_100.0);
        let mut out = vec![0.0; n];
        follow.process(&ProcessCtx::new(44_100.0, true), &[&x, &attack, &release], &mut out);
        let mut envelope = 0.0f64;
        for i in 0..n {
            let level = f64::from(x[i]).abs();
            let time = if level > envelope { attack[i] } else { release[i] };
            let coeff = (-1.0 / (f64::from(time).max(0.0) * 44_100.0).max(1.0)).exp();
            envelope = level + coeff * (envelope - level);
            assert_eq!(out[i].to_bits(), (envelope as f32).to_bits(), "sample {i}");
        }
    }
}
