//! A signal read from outside the audio thread: `x.osc_out("/addr", port: 9000)` passes `x`
//! through and keeps its latest value where another thread can read it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::opcode::{Opcode, Port, ProcessCtx};

/// The latest value of a tapped signal: written by the audio thread, read by whoever sends it on.
#[derive(Debug, Default)]
pub struct Tap(AtomicU32);

impl Tap {
    pub fn new() -> Arc<Tap> {
        Arc::new(Tap(AtomicU32::new(0f32.to_bits())))
    }

    pub fn read(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn write(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// Passes its input through, remembering the last sample of each block in a [`Tap`]. Every copy of
/// the opcode shares the one tap.
#[derive(Clone)]
pub struct TapOut {
    tap: Arc<Tap>,
}

impl TapOut {
    pub fn new(tap: Arc<Tap>) -> Self {
        TapOut { tap }
    }
}

impl Opcode for TapOut {
    fn name(&self) -> &str {
        "osc_out"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        out.copy_from_slice(inputs[0]);
        if let Some(last) = out.last().filter(|v| v.is_finite()) {
            self.tap.write(*last);
        }
    }
}
