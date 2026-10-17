//! Signals sent out over OSC: `out = it.osc_out("/vj/level", port: 9000)`. The audio thread keeps the
//! latest value of each; this thread reads them and sends the ones that moved, as often as each
//! asked for. Nothing here is on the audio thread or logged: a replay does not send.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use niminal_lang::OscTap;

use crate::audio::{Shared, lock};
use crate::worker::Worker;

/// An OSC message with one float argument.
pub fn encode_osc(address: &str, value: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(address.len() + 12);
    for text in [address, ",f"] {
        out.extend(text.as_bytes());
        out.push(0);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out.extend(value.to_be_bytes());
    out
}

/// What was last sent for one tap.
struct Sent {
    key: (String, String, u16),
    to: Option<SocketAddr>,
    at: Option<Instant>,
    bits: u32,
}

/// Works out which taps are due and sends them.
pub struct OscSender {
    socket: UdpSocket,
    sent: Vec<Sent>,
}

impl OscSender {
    pub fn new() -> std::io::Result<OscSender> {
        Ok(OscSender { socket: UdpSocket::bind("0.0.0.0:0")?, sent: Vec::new() })
    }

    /// Send every tap whose value has moved since it was last sent, if its rate allows it. Returns how
    /// many messages went.
    pub fn poll(&mut self, taps: &[OscTap], now: Instant) -> usize {
        self.sent.retain(|s| taps.iter().any(|t| key(t) == s.key));
        let mut count = 0;
        for tap in taps {
            let index = match self.sent.iter().position(|s| s.key == key(tap)) {
                Some(i) => i,
                None => {
                    let to = (tap.host.as_str(), tap.port).to_socket_addrs().ok().and_then(|mut a| a.next());
                    self.sent.push(Sent { key: key(tap), to, at: None, bits: 0 });
                    self.sent.len() - 1
                }
            };
            let state = &mut self.sent[index];
            let Some(to) = state.to else { continue };
            let value = tap.value.read();
            let due = state.at.is_none_or(|at| now.duration_since(at).as_secs_f64() >= 1.0 / tap.rate);
            if due && (state.at.is_none() || value.to_bits() != state.bits) && value.is_finite() {
                let _ = self.socket.send_to(&encode_osc(&tap.address, value), to);
                state.at = Some(now);
                state.bits = value.to_bits();
                count += 1;
            }
        }
        count
    }
}

fn key(tap: &OscTap) -> (String, String, u16) {
    (tap.address.clone(), tap.host.clone(), tap.port)
}

/// Sends the running project's `osc_out` signals until dropped.
pub struct OscOutput {
    _worker: Worker,
}

impl OscOutput {
    pub fn start(daemon: Shared) -> Result<OscOutput, String> {
        let mut sender = OscSender::new().map_err(|e| format!("can't send OSC: {e}"))?;
        Ok(OscOutput {
            _worker: Worker::spawn(move |stop| {
                while !stop.load(Ordering::Relaxed) {
                    let taps = lock(&daemon).session().program().taps.clone();
                    if !taps.is_empty() {
                        sender.poll(&taps, Instant::now());
                    }
                    // most projects send nothing, so look less often until one does
                    thread::sleep(Duration::from_millis(if taps.is_empty() { 50 } else { 4 }));
                }
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_osc;
    use niminal_engine::ops::Tap;

    fn tap(address: &str, port: u16, rate: f64) -> OscTap {
        OscTap { address: address.into(), host: "127.0.0.1".into(), port, rate, value: Tap::new() }
    }

    #[test]
    fn a_message_is_an_address_a_type_tag_and_a_float() {
        let bytes = encode_osc("/vj/level", 0.5);
        assert_eq!(bytes.len() % 4, 0);
        let mut parsed = Vec::new();
        parse_osc(&bytes, &mut parsed);
        assert_eq!(parsed, vec![("/vj/level".to_string(), 0.5)]);
        assert_eq!(&encode_osc("/a", 1.0)[..8], b"/a\0\0,f\0\0");
    }

    #[test]
    fn values_are_sent_when_they_move_and_no_faster_than_asked() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let port = receiver.local_addr().unwrap().port();
        let taps = vec![tap("/x", port, 10.0)];
        let mut sender = OscSender::new().unwrap();
        let start = Instant::now();
        let get = || {
            let mut buffer = [0u8; 64];
            let n = receiver.recv(&mut buffer).unwrap();
            let mut out = Vec::new();
            parse_osc(&buffer[..n], &mut out);
            out
        };
        // the first value goes at once, even if it is nothing
        assert_eq!(sender.poll(&taps, start), 1);
        assert_eq!(get(), vec![("/x".to_string(), 0.0)]);
        // unchanged: nothing, however long it has been
        assert_eq!(sender.poll(&taps, start + Duration::from_secs(5)), 0);
        // changed, but sooner than the rate allows (10 a second: every 100ms)
        let mut tapped = niminal_engine::ops::TapOut::new(taps[0].value.clone());
        use niminal_engine::Opcode;
        let ctx = niminal_engine::ProcessCtx::new(48_000.0, true);
        tapped.process(&ctx, &[&[0.25, 0.75]], &mut [0.0, 0.0]);
        assert_eq!(sender.poll(&taps, start + Duration::from_secs(5) + Duration::from_millis(50)), 1, "the rate counts from the last time it was sent");
        assert_eq!(get(), vec![("/x".to_string(), 0.75)]);
        tapped.process(&ctx, &[&[0.5]], &mut [0.0]);
        assert_eq!(sender.poll(&taps, start + Duration::from_secs(5) + Duration::from_millis(60)), 0, "too soon");
        assert_eq!(sender.poll(&taps, start + Duration::from_secs(5) + Duration::from_millis(160)), 1);
        assert_eq!(get(), vec![("/x".to_string(), 0.5)]);
    }

    #[test]
    fn a_tap_that_goes_away_is_forgotten() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = receiver.local_addr().unwrap().port();
        let mut sender = OscSender::new().unwrap();
        let now = Instant::now();
        let taps = vec![tap("/a", port, 30.0)];
        assert_eq!(sender.poll(&taps, now), 1);
        assert_eq!(sender.poll(&[], now), 0);
        assert!(sender.sent.is_empty());
        // and a tap of the same name that comes back is new, so it sends again
        assert_eq!(sender.poll(&taps, now), 1);
    }
}
