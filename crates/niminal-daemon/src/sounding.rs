//! The notes a session or renderer has started and not yet let go of, and the `midi_out` notes among them.

use niminal_engine::VoiceId;

#[derive(Default)]
pub(crate) struct Sounding {
    /// Each sounding note: when it is released, its voice, and the key holding it open (or `NO_KEY`).
    pub held: Vec<(u64, VoiceId, u32)>,
    /// Each `midi_out` note: when it ends, the key holding it, and the note.
    pub midi: Vec<(u64, u32, niminal_lang::MidiNote)>,
}

impl Sounding {
    /// Let go of the notes a key is holding, as of `now`.
    pub fn release_key(&mut self, key: u32, now: u64) {
        for held in self.held.iter_mut().filter(|h| h.2 == key) { held.0 = now; }
        for held in self.midi.iter_mut().filter(|h| h.1 == key) { held.0 = now; }
    }

    /// `n` frames from `clock`, cut short where a note comes to an end.
    pub fn limit(&self, clock: u64, mut n: u64) -> u64 {
        let ends = self.held.iter().map(|h| h.0).chain(self.midi.iter().map(|h| h.0));
        for off in ends { n = n.min(off.saturating_sub(clock).max(1)); }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stretch_ends_where_a_note_does() {
        let mut sounding = Sounding::default();
        sounding.held.push((150, VoiceId::default(), 1));
        assert_eq!(sounding.limit(100, 512), 50);
        assert_eq!(sounding.limit(100, 20), 20);
        sounding.release_key(1, 100);
        assert_eq!(sounding.limit(100, 512), 1);
    }
}
