//! Outside control: MIDI devices and OSC senders moving the `ctl`s bound to them.
//! Each message goes to the session as a logged control change, so a set replays.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use midir::{MidiInput, MidiInputConnection};
use niminal_daemon::ControlSignal;

use crate::audio::{Shared, lock};

/// A control-change message: `1011nnnn 0ccccccc 0vvvvvvv`.
pub fn parse_midi(bytes: &[u8]) -> Option<ControlSignal<'static>> {
    match bytes {
        [status @ 0xB0..=0xBF, cc, value] if *cc < 128 && *value < 128 => {
            Some(ControlSignal::MidiCc { cc: *cc, channel: (status & 0x0F) + 1, value: *value })
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
            let Some(element) = tail.get(..size) else { return };
            parse_osc(element, out);
            rest = &tail[size..];
        }
        return;
    }
    let Some((address, rest)) = padded_string(packet) else { return };
    let Some((tags, args)) = padded_string(rest) else { return };
    let value = match (tags.strip_prefix(',').and_then(|t| t.chars().next()), args) {
        (Some('f'), [a, b, c, d, ..]) => f64::from(f32::from_be_bytes([*a, *b, *c, *d])),
        (Some('i'), [a, b, c, d, ..]) => f64::from(i32::from_be_bytes([*a, *b, *c, *d])),
        (Some('d'), [a, b, c, d, e, f, g, h, ..]) => f64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h]),
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

/// Listens for OSC over UDP on this machine until dropped.
pub struct OscInput {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OscInput {
    pub fn start(daemon: Shared, port: u16) -> Result<OscInput, String> {
        let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], port)))
            .map_err(|e| format!("can't listen for OSC on 127.0.0.1:{port}: {e}"))?;
        socket.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            let mut buffer = vec![0u8; 65_536];
            let mut messages = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                let Ok(length) = socket.recv(&mut buffer) else { continue };
                messages.clear();
                parse_osc(&buffer[..length], &mut messages);
                if messages.is_empty() { continue; }
                let mut daemon = lock(&daemon);
                for (address, value) in &messages {
                    daemon.session_mut().control_input(&ControlSignal::Osc { address, value: *value });
                }
            }
        });
        Ok(OscInput { port, stop, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for OscInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Open every MIDI input whose name contains `filter` (all of them if it is empty).
/// Returns the connections, which stay open until dropped, and their names.
pub fn start_midi(daemon: &Shared, filter: &str) -> Result<(Vec<MidiInputConnection<()>>, Vec<String>), String> {
    let probe = MidiInput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
    let mut connections = Vec::new();
    let mut names = Vec::new();
    for port in probe.ports() {
        let Ok(name) = probe.port_name(&port) else { continue };
        if !name.contains(filter) { continue; }
        let input = MidiInput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
        let daemon = daemon.clone();
        let connection = input
            .connect(&port, "niminal-in", move |_, bytes, _| {
                if let Some(signal) = parse_midi(bytes) {
                    lock(&daemon).session_mut().control_input(&signal);
                }
            }, ())
            .map_err(|e| format!("can't open MIDI input `{name}`: {e}"))?;
        connections.push(connection);
        names.push(name);
    }
    if connections.is_empty() {
        return Err(if filter.is_empty() { "no MIDI inputs found".into() } else { format!("no MIDI input matches `{filter}`") });
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
        assert_eq!(parse_midi(&[0xB1, 7, 100]), Some(ControlSignal::MidiCc { cc: 7, channel: 2, value: 100 }));
        assert_eq!(parse_midi(&[0x90, 60, 100]), None);
        assert_eq!(parse_midi(&[0xB0, 7]), None);
    }

    #[test]
    fn osc_messages_and_bundles_parse() {
        let mut out = Vec::new();
        parse_osc(&message("/fx/width", ",f", &0.5f32.to_be_bytes()), &mut out);
        parse_osc(&message("/a", ",i", &3i32.to_be_bytes()), &mut out);
        parse_osc(&message("/s", ",s", b"hi\0\0"), &mut out);
        assert_eq!(out, vec![("/fx/width".to_string(), 0.5), ("/a".to_string(), 3.0)]);

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
