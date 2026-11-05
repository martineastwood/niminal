//! A matched-pole low-pass and its complementary high-pass.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

#[derive(Clone)]
pub struct OnePole {
    highpass: bool,
    sample_rate: f64,
    y: f64,
    cached_cutoff: f32,
    coefficient: f64,
}

impl OnePole {
    pub fn lowpass() -> Self {
        Self {
            highpass: false,
            sample_rate: 48_000.0,
            y: 0.0,
            cached_cutoff: f32::NAN,
            coefficient: 0.0,
        }
    }

    pub fn highpass() -> Self {
        Self {
            highpass: true,
            ..Self::lowpass()
        }
    }

    fn step(&mut self, x: f32, cutoff: f32) -> f32 {
        if !x.is_finite() {
            self.y = 0.0;
            return 0.0;
        }
        // NaN and negative cutoff close the filter; +infinity opens it to Nyquist.
        let cutoff = cutoff.max(0.0).min((self.sample_rate * 0.5) as f32);
        if cutoff != self.cached_cutoff {
            self.cached_cutoff = cutoff;
            self.coefficient =
                (-std::f64::consts::TAU * f64::from(cutoff) / self.sample_rate).exp();
        }
        let x = f64::from(x);
        self.y = flush(x * (1.0 - self.coefficient) + self.y * self.coefficient);
        let y = if self.highpass { x - self.y } else { self.y };
        y.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
    }
}

impl Opcode for OnePole {
    fn name(&self) -> &'static str {
        if self.highpass {
            "onepole_hp"
        } else {
            "onepole_lp"
        }
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::required("cutoff")];
        PORTS
    }

    fn reset(&mut self) {
        self.y = 0.0;
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(sample_rate).max(1.0);
        self.cached_cutoff = f32::NAN;
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(if self.highpass {
            Self::highpass()
        } else {
            Self::lowpass()
        })
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.y = old.y;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.step(inputs[0][i], inputs[1][i]);
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(inputs[0], inputs[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complementary_outputs_reconstruct_even_while_cutoff_moves() {
        for sr in [8_000.0, 44_100.0, 48_000.0, 96_000.0] {
            let (mut lp, mut hp) = (OnePole::lowpass(), OnePole::highpass());
            lp.prepare(sr);
            hp.prepare(sr);
            for i in 0..10_000 {
                let x = (i as f32 * 0.13).sin();
                let cutoff = (i as f32 * 0.003).sin() * sr;
                let (lo, hi) = (lp.step(x, cutoff), hp.step(x, cutoff));
                assert!((lo + hi - x).abs() < 2e-7);
                assert!(lo.abs() <= 1.0 && hi.abs() <= 2.0);
            }
        }
    }

    #[test]
    fn extreme_cutoffs_stay_bounded_and_bad_input_does_not_poison_state() {
        for cutoff in [-1e30, 0.0, 1e30, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for mut filter in [OnePole::lowpass(), OnePole::highpass()] {
                for _ in 0..1_000 {
                    assert!(filter.step(1.0, cutoff).abs() <= 1.0);
                }
                assert_eq!(filter.step(f32::NAN, cutoff), 0.0);
                assert_eq!(filter.step(f32::INFINITY, cutoff), 0.0);
                assert!(filter.step(0.5, 1000.0).is_finite());
                for x in [f32::MAX, -f32::MAX, f32::MAX] {
                    assert!(filter.step(x, 1000.0).is_finite());
                }
            }
        }
    }

    #[test]
    fn block_and_scalar_paths_agree_with_modulation() {
        let ctx = ProcessCtx::new(48_000.0, true);
        let x: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.2).sin()).collect();
        let cutoff: Vec<f32> = (0..1024).map(|i| i as f32 * 31.0 - 500.0).collect();
        for mut block in [OnePole::lowpass(), OnePole::highpass()] {
            let mut scalar = block.clone();
            let mut out = vec![0.0; x.len()];
            block.process(&ctx, &[&x, &cutoff], &mut out);
            for i in 0..x.len() {
                assert_eq!(out[i], scalar.process_one(&ctx, &[x[i], cutoff[i]]));
            }
        }
    }

    #[test]
    fn lowpass_has_the_expected_impulse_and_state_survives_redefinition() {
        let mut filter = OnePole::lowpass();
        let a = (-std::f64::consts::TAU * 1000.0 / 48_000.0).exp();
        for i in 0..100 {
            let got = filter.step(if i == 0 { 1.0 } else { 0.0 }, 1000.0);
            assert!((f64::from(got) - (1.0 - a) * a.powi(i)).abs() < 1e-8);
        }
        let mut replacement = OnePole::lowpass();
        replacement.carry_state(&mut filter);
        assert_eq!(replacement.step(0.0, 1000.0), filter.step(0.0, 1000.0));
        replacement.reset();
        assert_eq!(replacement.step(0.0, 1000.0), 0.0);
    }
}
