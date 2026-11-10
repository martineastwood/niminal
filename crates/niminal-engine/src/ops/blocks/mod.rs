//! Stateful DSP behind the modular-synth shelf. The shelf supplies musical defaults and enums.
use crate::opcode::{Opcode, Port, ProcessCtx};

// These modules have the same scalar/block adapters; each owns its state and sample algorithm.
macro_rules! block_opcode {
    ($ty:ty, $name:literal, $outputs:expr, $capacity:expr) => {
        impl Opcode for $ty {
            fn name(&self) -> &str {
                $name
            }
            fn ports(&self) -> &[Port] {
                self.input_ports()
            }
            fn outputs(&self) -> usize {
                $outputs
            }
            fn box_clone(&self) -> Box<dyn Opcode> {
                Box::new(self.fresh())
            }
            fn prepare(&mut self, sample_rate: f32) {
                self.sample_rate = f64::from(sample_rate);
                self.clear();
            }
            fn reset(&mut self) {
                self.clear();
            }
            fn carry_state(&mut self, old: &mut dyn Opcode) {
                if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
                    self.carry(old);
                }
            }
            fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
                let mut values = [0.0; $capacity];
                for (i, output) in out.iter_mut().enumerate() {
                    for (value, input) in values.iter_mut().zip(inputs) {
                        *value = input[i];
                    }
                    *output = self.step(ctx, &values[..inputs.len()])[0];
                }
            }
            fn process_outputs(
                &mut self,
                ctx: &ProcessCtx,
                inputs: &[&[f32]],
                outs: &mut [Vec<f32>],
                frames: usize,
            ) {
                let mut values = [0.0; $capacity];
                for i in 0..frames {
                    for (value, input) in values.iter_mut().zip(inputs) {
                        *value = input[i];
                    }
                    let result = self.step(ctx, &values[..inputs.len()]);
                    for (out, value) in outs.iter_mut().zip(result) {
                        out[i] = value;
                    }
                }
            }
            fn max_inputs(&self) -> usize {
                $capacity
            }
            fn process_slots(
                &mut self,
                ctx: &ProcessCtx,
                slots: &[Vec<f32>],
                indices: &[usize],
                outs: &mut [Vec<f32>],
                frames: usize,
            ) {
                let mut inputs: [&[f32]; $capacity] = [&[]; $capacity];
                for (input, &index) in inputs.iter_mut().zip(indices) {
                    *input = &slots[index][..frames];
                }
                self.process_outputs(ctx, &inputs[..indices.len()], outs, frames);
            }
            fn has_process_one(&self) -> bool {
                $outputs == 1
            }
            fn process_one(&mut self, ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
                self.step(ctx, inputs)[0]
            }
        }
    };
}

mod control;
mod filter;
mod frequency;
mod tape;
mod taps;
pub use control::{Contour, Slew};
pub use filter::MultimodeFilter;
pub use frequency::FreqShifter;
pub use tape::FreezeReverse;
pub use taps::TapDelay;

fn finite(x: f32) -> f64 {
    if x.is_finite() { f64::from(x) } else { 0.0 }
}
fn bounded(x: f32, low: f64, high: f64) -> f64 {
    finite(x).clamp(low, high)
}
fn mix(a: f64, b: f64, amount: f64) -> f64 {
    a + (b - a) * amount
}

fn sample(value: f64) -> f32 {
    let value = value as f32;
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(op: &dyn Opcode, value: f32) -> Vec<Vec<f32>> {
        op.ports()
            .iter()
            .map(|p| {
                vec![
                    if p.name == "x" {
                        value
                    } else {
                        p.default.unwrap_or(0.03)
                    };
                    64
                ]
            })
            .collect()
    }
    fn outputs(op: &dyn Opcode) -> Vec<Vec<f32>> {
        vec![vec![0.0; 64]; op.outputs()]
    }

    #[test]
    fn native_modules_stay_finite_and_recover_from_nonfinite_inputs() {
        let mut ops: Vec<Box<dyn Opcode>> = vec![
            Box::new(Contour::default()),
            Box::new(Slew::default()),
            Box::new(MultimodeFilter::default()),
            Box::new(FreqShifter::default()),
            Box::new(FreezeReverse::new(16384)),
            Box::new(TapDelay::new(16384, 16)),
        ];
        for rate in [44100.0, 48000.0, 96000.0] {
            let ctx = ProcessCtx::new(rate, true);
            for op in &mut ops {
                op.prepare(rate);
                let mut out = outputs(op.as_ref());
                for extreme in [
                    f32::MAX,
                    -f32::MAX,
                    -1e30,
                    -1.0,
                    0.0,
                    0.5,
                    1.0,
                    1e30,
                    f32::NAN,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    0.25,
                ] {
                    let mut input = inputs(op.as_ref(), extreme);
                    for block in 0..4 {
                        // Exercise the controls too, then verify recovery with their defaults.
                        if block == 1 {
                            for port in &mut input {
                                port.fill(extreme);
                            }
                        }
                        if block == 2 {
                            input = inputs(op.as_ref(), 0.25);
                        }
                        let slices: Vec<_> = input.iter().map(Vec::as_slice).collect();
                        op.process_outputs(&ctx, &slices, &mut out, 64);
                        assert!(
                            out.iter().flatten().all(|x| x.is_finite()),
                            "{} {rate} {extreme}",
                            op.name()
                        );
                    }
                }
            }
        }
    }

    fn carries_and_resets<T: Opcode + Clone>(mut old: T) {
        let ctx = ProcessCtx::new(48000.0, true);
        old.prepare(48000.0);
        let input = inputs(&old, 0.5);
        let slices: Vec<_> = input.iter().map(Vec::as_slice).collect();
        let mut out = outputs(&old);
        for _ in 0..20 {
            old.process_outputs(&ctx, &slices, &mut out, 64);
        }
        let mut expected = old.clone();
        let mut new = old.box_clone();
        new.prepare(48000.0);
        new.carry_state(&mut old);
        let mut actual = outputs(new.as_ref());
        for _ in 0..20 {
            expected.process_outputs(&ctx, &slices, &mut out, 64);
            new.process_outputs(&ctx, &slices, &mut actual, 64);
            assert_eq!(actual, out, "{} carries state", new.name());
        }
        let mut fresh = new.box_clone();
        fresh.prepare(48000.0);
        new.reset();
        for _ in 0..20 {
            fresh.process_outputs(&ctx, &slices, &mut out, 64);
            new.process_outputs(&ctx, &slices, &mut actual, 64);
            assert_eq!(actual, out, "{} resets state", new.name());
        }
    }
    #[test]
    fn native_modules_carry_compatible_state_and_reset_without_rebuilding() {
        carries_and_resets(Contour::default());
        carries_and_resets(Slew::default());
        carries_and_resets(MultimodeFilter::default());
        carries_and_resets(FreqShifter::default());
        carries_and_resets(FreezeReverse::new(16384));
        carries_and_resets(TapDelay::new(16384, 16));
    }
}
