//! Native input so MIDI keyboards work in the desktop WebView without Web MIDI support.
use midir::{MidiInput, MidiInputConnection};
use serde::Serialize;
use std::sync::Mutex;
use tauri::Emitter;

#[derive(Default)]
pub struct Midi(Mutex<Option<MidiInputConnection<()>>>);

#[derive(Serialize)]
pub struct Port {
    id: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    On { channel: u8, note: u8, velocity: u8 },
    Off { channel: u8, note: u8 },
    Control { channel: u8, controller: u8, value: u8 },
    Sustain { channel: u8, down: bool },
    Panic { channel: u8 },
}

fn parse(bytes: &[u8]) -> Option<Message> {
    let [status, a, b] = *bytes else { return None; };
    if a > 127 || b > 127 { return None; }
    let channel = status & 15;
    match status & 0xf0 {
        0x90 if b > 0 => Some(Message::On { channel, note: a, velocity: b }),
        0x80 | 0x90 => Some(Message::Off { channel, note: a }),
        0xb0 if a == 64 => Some(Message::Sustain { channel, down: b >= 64 }),
        0xb0 if a == 121 => Some(Message::Sustain { channel, down: false }),
        0xb0 if a == 120 || a == 123 => Some(Message::Panic { channel }),
        0xb0 if a < 120 => Some(Message::Control { channel, controller: a, value: b }),
        _ => None,
    }
}

#[derive(Clone, Serialize)]
struct Event {
    port: String,
    message: Message,
}

#[tauri::command]
pub fn midi_ports() -> Result<Vec<Port>, String> {
    let input = MidiInput::new("niminal tracker").map_err(|e| e.to_string())?;
    input.ports().iter().map(|port| Ok(Port {
        id: port.id(),
        name: input.port_name(port).map_err(|e| e.to_string())?,
    })).collect()
}

#[tauri::command]
pub fn midi_connect(app: tauri::AppHandle, state: tauri::State<'_, Midi>, port: Option<String>) -> Result<(), String> {
    let mut connection = state.0.lock().map_err(|e| e.to_string())?;
    // Dropping the connection stops the callback before another device is opened.
    *connection = None;
    let Some(id) = port else { return Ok(()); };
    let input = MidiInput::new("niminal tracker").map_err(|e| e.to_string())?;
    let port = input.ports().into_iter().find(|p| p.id() == id).ok_or("MIDI input is no longer connected; refresh the inputs")?;
    *connection = Some(input.connect(&port, "tracker notes", move |_, bytes, _| {
        if let Some(message) = parse(bytes) {
            let _ = app.emit("midi", Event { port: id.clone(), message });
        }
    }, ()).map_err(|e| e.to_string())?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_controllers_keep_channel_number_and_seven_bit_value() {
        assert_eq!(parse(&[0xb9, 74, 127]), Some(Message::Control { channel: 9, controller: 74, value: 127 }));
        assert_eq!(parse(&[0xb0, 7, 0]), Some(Message::Control { channel: 0, controller: 7, value: 0 }));
        assert_eq!(parse(&[0xb0, 7, 128]), None);
        assert_eq!(parse(&[0xb0, 122, 0]), None);
    }
    #[test]
    fn sustain_uses_the_midi_threshold_and_reset_controllers_releases_it() {
        for (value, down) in [(0, false), (63, false), (64, true), (127, true)] {
            assert_eq!(parse(&[0xb3, 64, value]), Some(Message::Sustain { channel: 3, down }));
        }
        assert_eq!(parse(&[0xbf, 121, 0]), Some(Message::Sustain { channel: 15, down: false }));
    }
    #[test]
    fn parses_channels_velocity_zero_and_all_notes_off() {
        assert_eq!(parse(&[0x92, 60, 100]), Some(Message::On { channel: 2, note: 60, velocity: 100 }));
        assert_eq!(parse(&[0x92, 60, 0]), Some(Message::Off { channel: 2, note: 60 }));
        assert_eq!(parse(&[0x82, 60, 50]), Some(Message::Off { channel: 2, note: 60 }));
        assert_eq!(parse(&[0xbf, 123, 0]), Some(Message::Panic { channel: 15 }));
        assert_eq!(parse(&[0xb0, 120, 0]), Some(Message::Panic { channel: 0 }));
        for bytes in [&[0xf8][..], &[0xc0, 1], &[0x90, 200, 1], &[0x90, 60], &[0xe0, 1, 2], &[0xb0, 122, 127]] {
            assert_eq!(parse(bytes), None);
        }
    }
}
