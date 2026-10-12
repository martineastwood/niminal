//! Notes that go to external gear: what a `midi_out` instrument plays.

use std::sync::Arc;

/// A note starting or stopping on a MIDI output, at a sample of the session's clock.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiEvent {
    pub at: u64,
    pub on: bool,
    /// Part of the name of the output port it goes to.
    pub port: Arc<str>,
    /// From 1.
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
}

impl MidiEvent {
    /// The message as it goes down a cable: a status byte with the channel in it, the key, the velocity.
    pub fn bytes(&self) -> [u8; 3] {
        let status = if self.on { 0x90 } else { 0x80 };
        [status | (self.channel.clamp(1, 16) - 1), self.note.min(127), if self.on { self.velocity.clamp(1, 127) } else { 0 }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_three_bytes_with_the_channel_in_the_first() {
        let on = MidiEvent { at: 0, on: true, port: "x".into(), channel: 10, note: 36, velocity: 100 };
        assert_eq!(on.bytes(), [0x99, 36, 100]);
        let off = MidiEvent { on: false, channel: 1, ..on };
        assert_eq!(off.bytes(), [0x80, 36, 0]);
    }
}
