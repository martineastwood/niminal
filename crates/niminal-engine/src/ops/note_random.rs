//! A random number from 0 to 1, picked once per note.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Picks a number from 0 to 1 when the note starts and holds it. Without a seed each
/// note of an instrument and each call in it picks its own, the same on every run; a seed gives a
/// different run of picks, still one per note.
#[derive(Clone)]
pub struct NoteRandom {
    seed: u32,
    /// Tells the calls in one instrument apart.
    salt: u32,
    /// This note's pick; `None` until its first sample.
    pick: Option<f32>,
}

impl NoteRandom {
    pub fn new(seed: Option<u32>, salt: u32) -> Self {
        NoteRandom {
            seed: seed.unwrap_or(0),
            salt,
            pick: None,
        }
    }

    fn draw(&self, note: u32) -> f32 {
        let mut x = 0xDEAD_BEEF
            ^ self.seed.wrapping_mul(0xC2B2_AE35)
            ^ self.salt.wrapping_mul(0x9E37_79B1)
            ^ note.wrapping_mul(0x85EB_CA6B);
        // murmur3's finalizer, so that neighbouring numbers don't land alike
        x ^= x >> 16;
        x = x.wrapping_mul(0x85EB_CA6B);
        x ^= x >> 13;
        x = x.wrapping_mul(0xC2B2_AE35);
        x ^= x >> 16;
        x as f32 / u32::MAX as f32
    }
}

impl Opcode for NoteRandom {
    fn reset(&mut self) {
        self.pick = None;
    }

    fn name(&self) -> &str {
        "note_random"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.pick = old.pick;
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        let pick = match self.pick {
            Some(pick) => pick,
            None => {
                let pick = self.draw(ctx.index);
                self.pick = Some(pick);
                pick
            }
        };
        out.fill(pick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_stay_between_the_bounds_and_differ_by_note_and_seed() {
        let picks = |seed, note| NoteRandom::new(seed, 1).draw(note);
        for note in 0..100 {
            assert!((0.0..=1.0).contains(&picks(None, note)));
        }
        assert_ne!(picks(None, 0), picks(None, 1));
        assert_ne!(picks(None, 3), picks(Some(7), 3));
        assert_eq!(picks(Some(7), 3), picks(Some(7), 3));
    }
}
