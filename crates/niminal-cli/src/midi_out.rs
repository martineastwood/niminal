//! Notes for `midi_out` instruments, sent to MIDI ports. The renderer stamps each with the sample it
//! belongs to; this thread holds it until the sound card has played that far, so the gear and the
//! sound here stay together.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use midir::{MidiOutput as Midir, MidiOutputConnection};
use niminal_daemon::MidiEvent;
use rtrb::Consumer;

use crate::worker::Worker;

/// Somewhere a MIDI message can be sent: a port, found by part of its name.
pub trait Ports {
    fn send(&mut self, port: &str, bytes: [u8; 3]);
}

/// What is waiting to go, and what has gone and not yet ended.
#[derive(Default)]
pub struct Pump {
    waiting: VecDeque<MidiEvent>,
    sounding: Vec<(Arc<str>, u8, u8)>,
}

impl Pump {
    /// Send every waiting event the sound card has played up to (`played` is how many samples it has played),
    /// taking new ones from `incoming` first. Returns how many went.
    pub fn run(&mut self, played: u64, incoming: &mut Consumer<MidiEvent>, ports: &mut impl Ports) -> usize {
        while let Ok(event) = incoming.pop() {
            self.waiting.push_back(event);
        }
        let mut sent = 0;
        while self.waiting.front().is_some_and(|e| e.at <= played) {
            let event = self.waiting.pop_front().expect("just seen");
            ports.send(&event.port, event.bytes());
            let key = (event.port.clone(), event.channel, event.note);
            if event.on {
                self.sounding.push(key);
            } else if let Some(i) = self.sounding.iter().position(|s| *s == key) {
                self.sounding.swap_remove(i);
            }
            sent += 1;
        }
        sent
    }

    /// End every note that is still sounding, as when the daemon stops.
    pub fn all_off(&mut self, ports: &mut impl Ports) {
        for (port, channel, note) in self.sounding.drain(..) {
            ports.send(&port, MidiEvent { at: 0, on: false, port: port.clone(), channel, note, velocity: 0 }.bytes());
        }
    }
}

/// The MIDI outputs of this computer, opened the first time a note goes to them.
#[derive(Default)]
struct Devices {
    open: Vec<(String, Option<MidiOutputConnection>)>,
}

impl Devices {
    fn connect(name: &str) -> Result<MidiOutputConnection, String> {
        let output = Midir::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
        let found = output.ports().into_iter().find(|p| output.port_name(p).is_ok_and(|n| n.contains(name)));
        match found {
            Some(port) => output.connect(&port, "niminal-out").map_err(|e| format!("can't open MIDI output `{name}`: {e}")),
            None => {
                // no such port: make one of that name for other software to listen to
                #[cfg(unix)]
                {
                    use midir::os::unix::VirtualOutput;
                    output.create_virtual(name).map_err(|e| format!("can't make a MIDI output called `{name}`: {e}"))
                }
                #[cfg(not(unix))]
                {
                    Err(format!("no MIDI output matches `{name}`"))
                }
            }
        }
    }
}

impl Ports for Devices {
    fn send(&mut self, port: &str, bytes: [u8; 3]) {
        if !self.open.iter().any(|(name, _)| name == port) {
            let connection = Devices::connect(port).map_err(|e| eprintln!("{e}")).ok();
            self.open.push((port.to_string(), connection));
        }
        if let Some((_, Some(connection))) = self.open.iter_mut().find(|(name, _)| name == port) {
            let _ = connection.send(&bytes);
        }
    }
}

/// Sends the notes of `midi_out` instruments until dropped.
pub struct MidiOutput {
    _worker: Worker,
}

impl MidiOutput {
    /// `played` counts the samples the sound card has played; `incoming` brings the events.
    pub fn start(played: Arc<AtomicU64>, mut incoming: Consumer<MidiEvent>) -> MidiOutput {
        MidiOutput {
            _worker: Worker::spawn(move |stop| {
                let (mut pump, mut devices) = (Pump::default(), Devices::default());
                while !stop.load(Ordering::Relaxed) {
                    pump.run(played.load(Ordering::Acquire), &mut incoming, &mut devices);
                    thread::sleep(Duration::from_millis(1));
                }
                pump.all_off(&mut devices);
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorded(Vec<(String, [u8; 3])>);

    impl Ports for Recorded {
        fn send(&mut self, port: &str, bytes: [u8; 3]) {
            self.0.push((port.to_string(), bytes));
        }
    }

    fn event(at: u64, on: bool, note: u8) -> MidiEvent {
        MidiEvent { at, on, port: "IAC".into(), channel: 1, note, velocity: 100 }
    }

    #[test]
    fn events_wait_until_the_sound_card_has_played_to_them() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(16);
        let (mut pump, mut ports) = (Pump::default(), Recorded::default());
        tx.push(event(1_000, true, 60)).unwrap();
        tx.push(event(5_000, false, 60)).unwrap();
        assert_eq!(pump.run(500, &mut rx, &mut ports), 0, "not yet");
        assert_eq!(pump.run(1_000, &mut rx, &mut ports), 1);
        assert_eq!(ports.0, [("IAC".to_string(), [0x90, 60, 100])]);
        assert_eq!(pump.run(4_999, &mut rx, &mut ports), 0);
        assert_eq!(pump.run(9_000, &mut rx, &mut ports), 1);
        assert_eq!(ports.0[1], ("IAC".to_string(), [0x80, 60, 0]));
    }

    #[test]
    fn notes_still_sounding_are_ended_when_it_stops() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(16);
        let (mut pump, mut ports) = (Pump::default(), Recorded::default());
        tx.push(event(0, true, 60)).unwrap();
        tx.push(event(0, true, 64)).unwrap();
        tx.push(event(10, false, 64)).unwrap();
        pump.run(100, &mut rx, &mut ports);
        ports.0.clear();
        pump.all_off(&mut ports);
        assert_eq!(ports.0, [("IAC".to_string(), [0x80, 60, 0])], "only the note still held");
    }
}
