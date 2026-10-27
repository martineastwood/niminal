//! Sending MIDI clock. The sequencer says which sample each message belongs to; this holds each
//! until the sound card is playing that sample, so the gear and the sound here stay together.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use midir::{MidiOutput, MidiOutputConnection};
use niminal_live::{Live, Realtime};
use niminal_ws::worker::Worker;

/// A tick later than this is dropped rather than sent in a burst with the ones after it.
const LATE_MICROS: u64 = 50_000;

pub struct ClockOutput {
    tx: Sender<(u64, Realtime)>,
    _worker: Worker,
}

impl ClockOutput {
    /// Send to the output whose name contains `name`; if there is none, make one called that for other
    /// software to listen to (where the system allows).
    pub fn open(name: &str) -> Result<ClockOutput, String> {
        let connection = connect(name)?;
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(move |stop| run(connection, &rx, stop));
        Ok(ClockOutput {
            tx,
            _worker: worker,
        })
    }

    /// Send `message` at Unix time `wall_micros`.
    pub fn send(&self, wall_micros: u64, message: Realtime) {
        let _ = self.tx.send((wall_micros, message));
    }
}

/// Pass on what `live` has to send since this was last called.
pub fn pump_clock_out(live: &mut Live, out: &ClockOutput) {
    for m in live.take_clock_out() {
        if let Some(wall) = live.wall_micros_of(m.at) {
            out.send(wall, m.message);
        }
    }
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

fn connect(name: &str) -> Result<MidiOutputConnection, String> {
    let output = MidiOutput::new("niminal").map_err(|e| format!("can't use MIDI: {e}"))?;
    let found = output
        .ports()
        .into_iter()
        .find(|p| output.port_name(p).is_ok_and(|n| n.contains(name)));
    match found {
        Some(port) => output
            .connect(&port, "niminal-clock")
            .map_err(|e| format!("can't open MIDI output `{name}`: {e}")),
        None => {
            #[cfg(unix)]
            {
                use midir::os::unix::VirtualOutput;
                output
                    .create_virtual(name)
                    .map_err(|e| format!("can't make a MIDI output called `{name}`: {e}"))
            }
            #[cfg(not(unix))]
            {
                Err(format!("no MIDI output matches `{name}`"))
            }
        }
    }
}

fn run(
    mut connection: MidiOutputConnection,
    rx: &Receiver<(u64, Realtime)>,
    stop: &AtomicBool,
) {
    let mut waiting: VecDeque<(u64, Realtime)> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        loop {
            match rx.try_recv() {
                Ok(m) => waiting.push_back(m),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let now = now_micros();
        while let Some(&(due, message)) = waiting.front() {
            if due > now {
                break;
            }
            waiting.pop_front();
            if message != Realtime::Tick || now - due <= LATE_MICROS {
                let _ = connection.send(&[message.byte()]);
            }
        }
        match waiting.front() {
            Some(&(due, _)) if due - now > 2_000 => {
                thread::sleep(Duration::from_micros((due - now - 1_000).min(5_000)));
            }
            Some(_) => thread::sleep(Duration::from_micros(100)),
            None => thread::sleep(Duration::from_millis(2)),
        }
    }
    // closing the output: a stop that is still waiting goes now, so that the gear is not left running
    waiting.extend(rx.try_iter());
    if waiting.iter().any(|&(_, m)| m == Realtime::Stop) {
        let _ = connection.send(&[Realtime::Stop.byte()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midir::{Ignore, MidiInput};
    use std::sync::{Arc, Mutex};

    #[test]
    fn messages_leave_when_their_time_comes() {
        let name = format!("niminal-clock-test-{}", std::process::id());
        let Ok(out) = ClockOutput::open(&name) else {
            eprintln!("no virtual MIDI ports here; skipping");
            return;
        };
        let mut input = MidiInput::new("test").unwrap();
        input.ignore(Ignore::None);
        let Some(port) = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).is_ok_and(|n| n.contains(&name)))
        else {
            eprintln!("the virtual port is not visible; skipping");
            return;
        };
        let got: Arc<Mutex<Vec<(u8, u64)>>> = Arc::default();
        let sink = got.clone();
        let _connection = input
            .connect(
                &port,
                "test-in",
                move |_, bytes, _| sink.lock().unwrap().push((bytes[0], now_micros())),
                (),
            )
            .unwrap();
        let t0 = now_micros() + 100_000;
        out.send(t0, Realtime::Start);
        for i in 0..5 {
            out.send(t0 + i * 20_833, Realtime::Tick);
        }
        thread::sleep(Duration::from_millis(400));
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 6, "{got:?}");
        assert_eq!(got[0].0, 0xFA);
        assert!(got[1..].iter().all(|m| m.0 == 0xF8));
        for (i, (_, arrived)) in got[1..].iter().enumerate() {
            let want = t0 + i as u64 * 20_833;
            let off = arrived.abs_diff(want);
            assert!(off < 5_000, "tick {i} was {off}µs from its time");
        }
        assert!(got[0].1 >= t0, "start went before its time");
    }
}
