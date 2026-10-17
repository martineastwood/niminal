//! Wavefolding and hard clipping.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Folds a signal back into ±`drive` once it exceeds that level.
/// Higher `drive` folds more times and gets brighter.
#[derive(Clone, Default)]
pub struct Fold;

impl Opcode for Fold {
    fn name(&self) -> &str {
        "wavefold"
    }

    fn ports(&self) -> &[Port] {
        // drive is a linear gain: 1 folds at ±1; higher values fold more
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("drive", 1.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self)
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, (&x, &drive)) in out.iter_mut().zip(inputs[0].iter().zip(inputs[1])) {
            let d = drive.abs().max(1e-6);
            // triangle fold into [-d, d], then normalize to roughly [-1, 1]
            let scaled = x * d;
            let period = 4.0;
            let mut t = scaled + 1.0;
            t = t - period * (t / period).floor();
            let folded = if t < 2.0 { t - 1.0 } else { 3.0 - t };
            *o = folded;
        }
    }
}

/// Hard-clips a signal to ±`level` (linear). Defaults to ±1.
#[derive(Clone, Default)]
pub struct Clip;

impl Opcode for Clip {
    fn name(&self) -> &str {
        "clip"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("level", 1.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self)
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, (&x, &level)) in out.iter_mut().zip(inputs[0].iter().zip(inputs[1])) {
            let lim = level.abs().max(0.0);
            *o = x.clamp(-lim, lim);
        }
    }
}
