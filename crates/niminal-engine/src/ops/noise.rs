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
    /// `seed:`: the same stream for every note. Without one, each note of an instrument
    /// and each noise source in it gets its own, the same on every run.
    seed: Option<u32>,
    /// Tells the sources of one instrument apart.
    salt: u32,
    /// Has the stream been started for this note?
    started: bool,
    /// Pink filter memory.
    b0: f32,
    b1: f32,
    b2: f32,
}

impl Noise {
    pub fn white() -> Self {
        Self::new(NoiseColor::White, None, 0)
    }

    pub fn pink() -> Self {
        Self::new(NoiseColor::Pink, None, 0)
    }

    pub fn new(color: NoiseColor, seed: Option<u32>, salt: u32) -> Self {
        Noise {
            color,
            rng: 0xDEAD_BEEF,
            seed,
            salt,
            started: false,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
        }
    }

    /// Start the stream: from the seed, or from this source and note.
    fn start(&mut self, note: u32) {
        let mut x = match self.seed {
            Some(seed) => seed ^ 0xDEAD_BEEF,
            None => {
                0xDEAD_BEEF ^ self.salt.wrapping_mul(0x9E37_79B1) ^ note.wrapping_mul(0x85EB_CA6B)
            }
        };
        // murmur3's finalizer, so that neighbouring numbers don't start alike
        x ^= x >> 16;
        x = x.wrapping_mul(0x85EB_CA6B);
        x ^= x >> 13;
        x = x.wrapping_mul(0xC2B2_AE35);
        x ^= x >> 16;
        self.rng = if x == 0 { 1 } else { x };
        self.started = true;
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
        self.started = false;
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
        Box::new(Self::new(self.color, self.seed, self.salt))
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && old.color == self.color
        {
            self.rng = old.rng;
            self.started = old.started;
            self.b0 = old.b0;
            self.b1 = old.b1;
            self.b2 = old.b2;
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        if !self.started {
            self.start(ctx.index);
        }
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
        n.process(&ProcessCtx::new(48_000.0, true), &[], &mut out);
        assert!(out.iter().any(|s| s.abs() > 0.01));
        assert!(out.iter().all(|s| s.abs() <= 1.0 && s.is_finite()));
    }

    #[test]
    fn pink_noise_is_quieter_than_raw_white() {
        let mut white = Noise::white();
        let mut pink = Noise::pink();
        let mut a = [0.0f32; 32];
        let mut b = [0.0f32; 32];
        white.process(&ProcessCtx::new(48_000.0, true), &[], &mut a);
        pink.process(&ProcessCtx::new(48_000.0, true), &[], &mut b);
        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        assert!(rms(&b) < rms(&a));
        assert!(b.iter().all(|s| s.is_finite()));
    }
}
