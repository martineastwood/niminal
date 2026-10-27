//! Outside control: MIDI devices and OSC senders moving the `ctl`s bound to them.
//! Each message goes to the session as a logged control change, so a set replays.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use midir::{Ignore, MidiInput, MidiInputConnection};
use niminal_live::{ControlSignal, LiveServer, Realtime};

use niminal_ws::lock;
use niminal_ws::worker::Worker;

/// A control change (`1011nnnn 0ccccccc 0vvvvvvv`) or a key going down (`1001nnnn`) or up (`1000nnnn`).
/// A key down with velocity 0 is a key up.
pub fn parse_midi(bytes: &[u8]) -> Option<ControlSignal<'static>> {
    match bytes {
        [status @ 0x90..=0x9F, note, velocity] if *note < 128 && *velocity < 128 => {
            Some(ControlSignal::MidiNote {
                note: *note,
                channel: (status & 0x0F) + 1,
                velocity: *velocity,
            })
        }
        [status @ 0x80..=0x8F, note, velocity] if *note < 128 && *velocity < 128 => {
            Some(ControlSignal::MidiNote {
                note: *note,
                channel: (status & 0x0F) + 1,
                velocity: 0,
            })
        }
        [status @ 0xA0..=0xAF, note, value] if *note < 128 && *value < 128 => {
            Some(ControlSignal::MidiPressure {
                channel: (status & 0x0F) + 1,
                note: Some(*note),
                value: *value,
            })
        }
        [status @ 0xD0..=0xDF, value] if *value < 128 => Some(ControlSignal::MidiPressure {
            channel: (status & 0x0F) + 1,
            note: None,
            value: *value,
        }),
        [status @ 0xE0..=0xEF, low, high] if *low < 128 && *high < 128 => {
            Some(ControlSignal::MidiBend {
                channel: (status & 0x0F) + 1,
                value: u16::from(*high) << 7 | u16::from(*low),
            })
        }
        [status @ 0xB0..=0xBF, cc, value] if *cc < 128 && *value < 128 => {
            Some(ControlSignal::MidiCc {
                cc: *cc,
                channel: (status & 0x0F) + 1,
                value: *value,
            })
        }
        _ => None,
    }
}

/// The numeric first argument of every message in an OSC packet, with its address.
/// Bundles are unpacked; messages without a number are skipped.
pub fn parse_osc(packet: &[u8], out: &mut Vec<(String, f64)>) {
    if let Some(bundle) = packet.strip_prefix(b"#bundle\0") {
        let mut rest = bundle.get(8..).unwrap_or_default();
        while let Some((size, tail)) = rest.split_first_chunk::<4>() {
            let size = u32::from_be_bytes(*size) as usize;
            let Some(element) = tail.get(..size) else {
                return;
            };
            parse_osc(element, out);
            rest = &tail[size..];
        }
        return;
    }
    let Some((address, rest)) = padded_string(packet) else {
        return;
    };
    let Some((tags, args)) = padded_string(rest) else {
        return;
    };
    let value = match (tags.strip_prefix(',').and_then(|t| t.chars().next()), args) {
        (Some('f'), [a, b, c, d, ..]) => f64::from(f32::from_be_bytes([*a, *b, *c, *d])),
        (Some('i'), [a, b, c, d, ..]) => f64::from(i32::from_be_bytes([*a, *b, *c, *d])),
        (Some('d'), [a, b, c, d, e, f, g, h, ..]) => {
            f64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])
        }
        _ => return,
    };
    if address.starts_with('/') {
        out.push((address.to_string(), value));
    }
}

/// A NUL-terminated string padded to a multiple of four bytes, and what follows it.
fn padded_string(bytes: &[u8]) -> Option<(&str, &[u8])> {
    let end = bytes.iter().position(|&b| b == 0)?;
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    Some((text, bytes.get((end / 4 + 1) * 4..).unwrap_or_default()))
}

/// The sequencer, shared between the threads that serve clients and take MIDI and OSC.
pub type LiveShared = std::sync::Arc<std::sync::Mutex<LiveServer>>;

/// Listens for OSC over UDP on this machine until dropped.
pub struct OscInput {
    port: u16,
    _worker: Worker,
}

impl OscInput {
    pub fn start(server: LiveShared, port: u16) -> Result<OscInput, String> {
        let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], port)))
            .map_err(|e| format!("can't listen for OSC on 127.0.0.1:{port}: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        let worker = Worker::spawn(move |stop| {
            let mut buffer = vec![0u8; 65_536];
            let mut messages = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let Ok(length) = socket.recv(&mut buffer) else {
                    continue;
                };
                messages.clear();
                parse_osc(&buffer[..length], &mut messages);
                if messages.is_empty() {
                    continue;
                }
                let mut server = lock(&server);
                for (address, value) in &messages {
                    server.live_mut().control_input(&ControlSignal::Osc {
                        address,
                        value: *value,
                    });
                }
            }
        });
        Ok(OscInput {
            port,
            _worker: worker,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Open every MIDI input whose name contains `filter` (all of them if it is empty).
/// Returns the connections, which stay open until dropped, and their names.
pub fn start_midi(
    server: &LiveShared,
    filter: &str,
) -> Result<(Vec<MidiInputConnection<()>>, Vec<String>), String> {
    let server = server.clone();
    open_midi(filter, move |signal| {
        lock(&server).live_mut().control_input(signal);
    })
}

/// Like [`start_midi`], handing each message that parses to `deliver`, for a client that holds its
/// sequencer another way.
pub fn open_midi(
    filter: &str,
    deliver: impl Fn(&ControlSignal) + Clone + Send + 'static,
) -> Result<(Vec<MidiInputConnection<()>>, Vec<String>), String> {
    open_inputs(filter, Ignore::SysexAndTime, move |bytes| {
        if let Some(signal) = parse_midi(bytes) {
            deliver(&signal);
        }
    })
}

/// Open every MIDI input whose name contains `filter` for its clock: `deliver` gets each tick, start,
/// continue and stop with the Unix time in microseconds at which it arrived.
pub fn open_clock_in(
    filter: &str,
    deliver: impl Fn(Realtime, u64) + Clone + Send + 'static,
) -> Result<(Vec<MidiInputConnection<()>>, Vec<String>), String> {
    open_inputs(filter, Ignore::SysexAndActiveSense, move |bytes| {
        let arrived = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as u64);
        if let [byte] = bytes
            && let Some(message) = Realtime::from_byte(*byte)
        {
            deliver(message, arrived);
        }
    })
}

fn open_inputs(
    filter: &str,
    ignore: Ignore,
    callback: impl Fn(&[u8]) + Clone + Send + 'static,
) -> Result<(Vec<MidiInputConnection<()>>, Vec<String>), String> {
    let probe = MidiInput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
    let mut connections = Vec::new();
    let mut names = Vec::new();
    for port in probe.ports() {
        let Ok(name) = probe.port_name(&port) else {
            continue;
        };
        if !name.contains(filter) {
            continue;
        }
        let mut input = MidiInput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
        input.ignore(ignore);
        let callback = callback.clone();
        let connection = input
            .connect(&port, "niminal-in", move |_, bytes, _| callback(bytes), ())
            .map_err(|e| format!("can't open MIDI input `{name}`: {e}"))?;
        connections.push(connection);
        names.push(name);
    }
    if connections.is_empty() {
        return Err(if filter.is_empty() {
            "no MIDI inputs found".into()
        } else {
            format!("no MIDI input matches `{filter}`")
        });
    }
    Ok((connections, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(address: &str, tags: &str, args: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for text in [address, tags] {
            out.extend(text.as_bytes());
            out.resize(out.len() + 4 - out.len() % 4, 0);
        }
        out.extend(args);
        out
    }

    #[test]
    fn midi_control_changes_parse_and_everything_else_is_ignored() {
        assert_eq!(
            parse_midi(&[0xB1, 7, 100]),
            Some(ControlSignal::MidiCc {
                cc: 7,
                channel: 2,
                value: 100
            })
        );
        assert_eq!(
            parse_midi(&[0x90, 60, 100]),
            Some(ControlSignal::MidiNote {
                note: 60,
                channel: 1,
                velocity: 100
            })
        );
        assert_eq!(
            parse_midi(&[0x93, 60, 0]),
            Some(ControlSignal::MidiNote {
                note: 60,
                channel: 4,
                velocity: 0
            })
        );
        assert_eq!(
            parse_midi(&[0x80, 60, 64]),
            Some(ControlSignal::MidiNote {
                note: 60,
                channel: 1,
                velocity: 0
            })
        );
        // centre, and all the way up, of the pitch bend
        assert_eq!(
            parse_midi(&[0xE0, 0, 64]),
            Some(ControlSignal::MidiBend {
                channel: 1,
                value: 8192
            })
        );
        assert_eq!(
            parse_midi(&[0xE2, 127, 127]),
            Some(ControlSignal::MidiBend {
                channel: 3,
                value: 16383
            })
        );
        assert_eq!(
            parse_midi(&[0xD0, 90]),
            Some(ControlSignal::MidiPressure {
                channel: 1,
                note: None,
                value: 90
            })
        );
        assert_eq!(
            parse_midi(&[0xA1, 60, 30]),
            Some(ControlSignal::MidiPressure {
                channel: 2,
                note: Some(60),
                value: 30
            })
        );
        assert_eq!(parse_midi(&[0xC0, 5]), None);
        assert_eq!(parse_midi(&[0xB0, 7]), None);
    }

    #[test]
    fn osc_messages_and_bundles_parse() {
        let mut out = Vec::new();
        parse_osc(&message("/fx/width", ",f", &0.5f32.to_be_bytes()), &mut out);
        parse_osc(&message("/a", ",i", &3i32.to_be_bytes()), &mut out);
        parse_osc(&message("/s", ",s", b"hi\0\0"), &mut out);
        assert_eq!(
            out,
            vec![("/fx/width".to_string(), 0.5), ("/a".to_string(), 3.0)]
        );

        let inner = message("/b", ",f", &1.0f32.to_be_bytes());
        let mut bundle = b"#bundle\0".to_vec();
        bundle.extend([0u8; 8]);
        bundle.extend((inner.len() as u32).to_be_bytes());
        bundle.extend(&inner);
        let mut out = Vec::new();
        parse_osc(&bundle, &mut out);
        assert_eq!(out, vec![("/b".to_string(), 1.0)]);
        parse_osc(b"junk", &mut out);
        assert_eq!(out.len(), 1);
    }
}
