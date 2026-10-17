//! A signal read from outside the audio thread: `x.osc_out("/addr", port: 9000)` passes `x`
//! through and keeps its latest value where another thread can read it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::opcode::{Opcode, Port, ProcessCtx};

/// The latest value of a tapped signal: written by the audio thread, read by whoever sends it on.
#[derive(Debug, Default)]
pub struct Tap {
    latest: AtomicU32,
    cursor: AtomicU32,
    events: Option<Box<[AtomicU64]>>,
}

impl Tap {
    pub fn new() -> Arc<Tap> {
        Arc::new(Tap::default())
    }

    pub fn triggered() -> Arc<Tap> {
        Arc::new(Self { events: Some((0..256).map(|_| AtomicU64::new(0)).collect()), ..Self::default() })
    }

    pub fn is_triggered(&self) -> bool { self.events.is_some() }

    /// Drain a reader's cursor. Overflow discards the oldest events; rendering never waits.
    pub fn read_events(&self, cursor: &mut u32) -> Vec<f32> {
        let Some(events) = &self.events else { return Vec::new(); };
        let end = self.cursor.load(Ordering::Acquire);
        if end.wrapping_sub(*cursor) > events.len() as u32 { *cursor = end.wrapping_sub(events.len() as u32); }
        let mut values = Vec::new();
        while *cursor != end {
            let packed = events[*cursor as usize % events.len()].load(Ordering::Acquire);
            if (packed >> 32) as u32 != cursor.wrapping_add(1) { break; }
            values.push(f32::from_bits(packed as u32));
            *cursor = cursor.wrapping_add(1);
        }
        values
    }

    pub fn read(&self) -> f32 {
        f32::from_bits(self.latest.load(Ordering::Relaxed))
    }

    fn write(&self, value: f32) {
        self.latest.store(value.to_bits(), Ordering::Relaxed);
        if let Some(events) = &self.events {
            let cursor = self.cursor.fetch_add(1, Ordering::Relaxed);
            let packed = (u64::from(cursor.wrapping_add(1)) << 32) | u64::from(value.to_bits());
            events[cursor as usize % events.len()].store(packed, Ordering::Release);
        }
    }
}

/// Passes its input through, remembering the last sample of each block in a [`Tap`]. Every copy of
/// the opcode shares the one tap.
#[derive(Clone)]
pub struct TapOut {
    tap: Arc<Tap>,
    positive: bool,
}

impl TapOut {
    pub fn new(tap: Arc<Tap>) -> Self {
        TapOut { tap, positive: false }
    }
}

impl Opcode for TapOut {
    fn name(&self) -> &str {
        "osc_out"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        const TRIGGERED: &[Port] = &[Port::required("x"), Port::required("on")];
        if self.tap.is_triggered() { TRIGGERED } else { PORTS }
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.tap.clone()))
    }

    fn reset(&mut self) { self.positive = false; }

    fn has_side_effects(&self) -> bool { true }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        out.copy_from_slice(inputs[0]);
        if self.tap.is_triggered() {
            for (value, trigger) in out.iter().zip(inputs[1]) {
                let positive = *trigger > 0.0;
                if positive && !self.positive && value.is_finite() { self.tap.write(*value); }
                self.positive = positive;
            }
        } else if let Some(last) = out.last().filter(|v| v.is_finite()) {
            self.tap.write(*last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn triggered_queue_is_bounded_and_keeps_the_latest_events_in_order() {
        let tap = Tap::triggered();
        for i in 0..300 { tap.write(i as f32); }
        let mut cursor = 0;
        assert_eq!(tap.read_events(&mut cursor), (44..300).map(|i| i as f32).collect::<Vec<_>>());
        assert!(tap.read_events(&mut cursor).is_empty());
        tap.write(299.0);
        assert_eq!(tap.read_events(&mut cursor), [299.0]);
    }
}
