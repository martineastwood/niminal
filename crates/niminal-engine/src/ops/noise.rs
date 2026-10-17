//! White and pink noise sources.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Which spectrum the noise has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseColor {
    White,
    Pink,
}

/// A noise source. White is uniform in [-1, 1]; pink is white through a
/// three-pole filter (Paul Kellet's economical approximation).
#[derive(Clone)]
pub struct Noise {
    color: NoiseColor,
    /// xorshift32 state; never zero.
    rng: u32,
    /// Pink filter memory.
    b0: f32,
    b1: f32,
    b2: f32,
}

impl Noise {
    pub fn white() -> Self {
        Self::new(NoiseColor::White)
    }

    pub fn pink() -> Self {
        Self::new(NoiseColor::Pink)
    }

    fn new(color: NoiseColor) -> Self {
        Noise { color, rng: 0xDEAD_BEEF, b0: 0.0, b1: 0.0, b2: 0.0 }
    }

    fn next_white(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = if x == 0 { 1 } else { x };
        // map u32 to [-1, 1]
        (self.rng as f32 / (u32::MAX as f32 / 2.0)) - 1.0
    }
}

impl Opcode for Noise {
    fn reset(&mut self) {
        self.rng = 0xDEAD_BEEF;
        self.b0 = 0.0;
        self.b1 = 0.0;
        self.b2 = 0.0;
    }

    fn name(&self) -> &str {
        match self.color {
            NoiseColor::White => "noise",
            NoiseColor::Pink => "pinknoise",
        }
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.color))
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && old.color == self.color
        {
            self.rng = old.rng;
            self.b0 = old.b0;
            self.b1 = old.b1;
            self.b2 = old.b2;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        match self.color {
            NoiseColor::White => {
                for o in out.iter_mut() {
                    *o = self.next_white();
                }
            }
            NoiseColor::Pink => {
                for o in out.iter_mut() {
                    let white = self.next_white();
                    // Paul Kellet's economical pink noise
                    self.b0 = 0.99765 * self.b0 + white * 0.0990460;
                    self.b1 = 0.96300 * self.b1 + white * 0.2965164;
                    self.b2 = 0.57000 * self.b2 + white * 1.0526913;
                    *o = (self.b0 + self.b1 + self.b2 + white * 0.1848) * 0.05;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::ProcessCtx;

    #[test]
    fn white_noise_fills_the_block_and_stays_in_range() {
        let mut n = Noise::white();
        let mut out = [0.0f32; 32];
        n.process(&ProcessCtx { sample_rate: 48_000.0, gate: true }, &[], &mut out);
        assert!(out.iter().any(|s| s.abs() > 0.01));
        assert!(out.iter().all(|s| s.abs() <= 1.0 && s.is_finite()));
    }

    #[test]
    fn pink_noise_is_quieter_than_raw_white() {
        let mut white = Noise::white();
        let mut pink = Noise::pink();
        let mut a = [0.0f32; 32];
        let mut b = [0.0f32; 32];
        white.process(&ProcessCtx { sample_rate: 48_000.0, gate: true }, &[], &mut a);
        pink.process(&ProcessCtx { sample_rate: 48_000.0, gate: true }, &[], &mut b);
        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        assert!(rms(&b) < rms(&a));
        assert!(b.iter().all(|s| s.is_finite()));
    }
}
