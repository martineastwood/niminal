//! A named, sample-smoothed value. Updates are delivered by the owning renderer.
use crate::{Opcode, Port, ProcessCtx};

pub struct Control {
    key: String,
    initial: f32,
    value: f32,
    target: f32,
    start: f32,
    frames: u64,
    elapsed: u64,
}

impl Control {
    pub fn new(key: String, initial: f32) -> Self {
        Self { key, initial, value: initial, target: initial, start: initial, frames: 0, elapsed: 0 }
    }
}

impl Opcode for Control {
    fn name(&self) -> &str { &self.key }
    fn ports(&self) -> &[Port] { &[] }
    fn box_clone(&self) -> Box<dyn Opcode> { Box::new(Self::new(self.key.clone(), self.initial)) }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], out: &mut [f32]) {
        for sample in out {
            if self.elapsed < self.frames {
                self.elapsed += 1;
                self.value = (f64::from(self.start) + (f64::from(self.target) - f64::from(self.start))
                    * (self.elapsed as f64 / self.frames as f64)) as f32;
                if self.elapsed == self.frames { self.value = self.target; }
            }
            *sample = self.value;
        }
    }
    fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        if key != self.key { return; }
        self.start = start;
        self.target = target;
        self.frames = frames;
        self.elapsed = elapsed.min(frames);
        self.value = if frames == 0 || self.elapsed == frames { target }
            else { (f64::from(start) + (f64::from(target) - f64::from(start))
                * (self.elapsed as f64 / frames as f64)) as f32 };
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.key == old.key
        {
            self.value = old.value; self.target = old.target;
            self.start = old.start; self.frames = old.frames; self.elapsed = old.elapsed;
        }
    }
}
