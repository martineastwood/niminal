//! What a MIDI keyboard is doing: which keys are down, which the sustain pedal is holding, and the
//! bend, pressure and slide each channel is at. It decides which notes to let go of and when; the
//! session starts and stops the notes.

use crate::log::Expression;

/// The key of a note that is not held open by one.
pub(crate) const NO_KEY: u32 = u32::MAX;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(crate) struct Keyboard {
    /// The notes each key started, by channel and MIDI note.
    down: BTreeMap<(u8, u8), Vec<u32>>,
    /// Keys let go of while the pedal was down: channel, MIDI note, key.
    sustained: Vec<(u8, u8, u32)>,
    pedals: BTreeSet<u8>,
    bends: BTreeMap<u8, f64>,
    pressures: BTreeMap<u8, f64>,
    slides: BTreeMap<u8, f64>,
    next_key: u32,
}

impl Keyboard {
    /// A key came up. The notes to let go of now; those the pedal holds are kept until it comes up.
    /// The second value is how many notes the key had.
    pub fn lift(&mut self, channel: u8, note: u8) -> (Vec<u32>, usize) {
        let keys = self.down.remove(&(channel, note)).unwrap_or_default();
        let total = keys.len();
        if self.pedals.contains(&channel) {
            self.sustained
                .extend(keys.iter().map(|&key| (channel, note, key)));
            return (Vec::new(), total);
        }
        (keys, total)
    }

    /// A key went down: the notes of the same key that are still sounding, to be let go of first.
    pub fn restrike(&mut self, channel: u8, note: u8) -> Vec<u32> {
        let mut keys = self.down.remove(&(channel, note)).unwrap_or_default();
        self.sustained.retain(|&(c, n, key)| {
            let same = c == channel && n == note;
            if same {
                keys.push(key);
            }
            !same
        });
        keys
    }

    /// Remember the notes a key just started.
    pub fn hold(&mut self, channel: u8, note: u8, keys: Vec<u32>) {
        if !keys.is_empty() {
            self.down.insert((channel, note), keys);
        }
    }

    /// A number for a note started from a key, never `NO_KEY`.
    pub fn new_key(&mut self) -> u32 {
        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1) % NO_KEY;
        key
    }

    /// The pedal went down or up. Coming up, the notes it was holding are returned to be let go of.
    pub fn pedal(&mut self, channel: u8, down: bool) -> Vec<u32> {
        if down {
            self.pedals.insert(channel);
            return Vec::new();
        }
        self.pedals.remove(&channel);
        let mut released = Vec::new();
        self.sustained.retain(|&(c, _, key)| {
            if c == channel {
                released.push(key);
            }
            c != channel
        });
        released
    }

    /// Notes on `channel` (only those of MIDI note `note`, if given) that are down or held by the pedal.
    pub fn keys_of(&self, channel: u8, note: Option<u8>) -> Vec<u32> {
        let down = self
            .down
            .iter()
            .filter(|((c, n), _)| *c == channel && note.is_none_or(|k| k == *n))
            .flat_map(|(_, keys)| keys.iter().copied());
        let held = self
            .sustained
            .iter()
            .filter(|(c, n, _)| *c == channel && note.is_none_or(|k| k == *n))
            .map(|s| s.2);
        down.chain(held).collect()
    }

    /// Remember the level of a bend, pressure or slide on a channel, for keys pressed from now on.
    pub fn set_level(&mut self, what: Expression, channel: u8, value: f64) {
        let map = match what {
            Expression::Bend => &mut self.bends,
            Expression::Pressure => &mut self.pressures,
            _ => &mut self.slides,
        };
        map.insert(channel, value);
    }

    /// The bend, pressure and slide a key pressed now starts with, where they are not at rest.
    pub fn starting(&self, channel: u8) -> [(&'static str, Option<f64>); 3] {
        let at = |map: &BTreeMap<u8, f64>| map.get(&channel).copied().filter(|v| *v != 0.0);
        [
            ("bend", at(&self.bends)),
            ("pressure", at(&self.pressures)),
            ("slide", at(&self.slides)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pedal_holds_keys_that_come_up() {
        let mut keyboard = Keyboard::default();
        keyboard.hold(1, 60, vec![7]);
        keyboard.pedal(1, true);
        assert_eq!(keyboard.lift(1, 60), (vec![], 1));
        assert_eq!(keyboard.keys_of(1, Some(60)), vec![7]);
        assert_eq!(keyboard.pedal(1, false), vec![7]);
        assert!(keyboard.keys_of(1, None).is_empty());
    }

    #[test]
    fn a_key_struck_again_lets_the_first_go() {
        let mut keyboard = Keyboard::default();
        keyboard.hold(1, 60, vec![1]);
        keyboard.pedal(1, true);
        keyboard.lift(1, 60);
        keyboard.hold(1, 60, vec![2]);
        let mut gone = keyboard.restrike(1, 60);
        gone.sort();
        assert_eq!(gone, vec![1, 2]);
    }

    #[test]
    fn keys_up_without_a_pedal_are_released() {
        let mut keyboard = Keyboard::default();
        keyboard.hold(2, 64, vec![3]);
        assert_eq!(keyboard.lift(2, 64), (vec![3], 1));
        assert_eq!(keyboard.lift(2, 64), (vec![], 0));
    }

    #[test]
    fn notes_start_with_the_channels_level() {
        let mut keyboard = Keyboard::default();
        keyboard.set_level(Expression::Bend, 1, 2.0);
        assert_eq!(keyboard.starting(1)[0], ("bend", Some(2.0)));
        assert_eq!(keyboard.starting(2)[0], ("bend", None));
    }
}
