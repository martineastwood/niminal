//! The sequencer and an engine in one process with a clock of their own, for rendering to a file and for
//! tests: the same requests as when the engine is across a WebSocket, but time only moves when asked to.
//!
//! This is a harness, not a client: it holds the engine's `Session` to step its clock and read its output, which
//! the wire protocol doesn't offer. Only `nimlive render` and tests use it. The sequencer itself ([`crate::Live`])
//! reaches the engine only through an [`EngineLink`](crate::EngineLink), as any other frontend would.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};

use niminal_daemon::{EngineServer, Session};
use niminal_lang::Layout;

use crate::link::InProcess;
use crate::live::Live;
use crate::quantize::QuantizeDefaults;

pub struct Offline {
    live: Live,
    engine: Arc<Mutex<EngineServer>>,
    layout: Layout,
    client: niminal_daemon::ClientId,
}

impl Offline {
    /// An engine that outputs `layout` at `sample_rate`, with the limiter on, and a sequencer for it.
    pub fn new(sample_rate: f32, layout: Layout) -> Offline {
        Offline::with_session(Session::new(sample_rate, layout), layout)
    }

    /// Turn the master limiter off. Only for offline use, and before anything is sent.
    pub fn without_limiter(self) -> Offline {
        let rate = self.live.sample_rate();
        let layout = self.layout;
        Offline::with_session(Session::new(rate, layout).without_limiter(), layout)
    }

    fn with_session(session: Session, layout: Layout) -> Offline {
        let sample_rate = session.sample_rate();
        let engine = Arc::new(Mutex::new(EngineServer::new(session)));
        let link = InProcess::new(engine.clone());
        let client = link.client_id();
        let live = Live::new(Box::new(link), sample_rate, layout);
        Offline {
            live,
            engine,
            layout,
            client,
        }
    }

    pub fn with_defaults(mut self, defaults: QuantizeDefaults) -> Offline {
        self.live = self.live.with_defaults(defaults);
        self
    }

    /// The engine, for what the sequencer doesn't own: its clock, its log, its meters.
    pub fn engine(&self) -> MutexGuard<'_, EngineServer> {
        self.engine.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the engine was sent, with when, as a log that replays to the same audio.
    pub fn log(&self) -> Vec<niminal_daemon::LogEntry> {
        self.engine().session().log().to_vec()
    }

    /// Changes when entries are appended to the engine's log or when actual control timing is acknowledged.
    pub fn log_version(&self) -> (usize, u64) {
        self.engine().session().log_version()
    }

    /// The notes that went to `midi_out` instruments since this was last called, in order.
    pub fn take_midi(&mut self) -> Vec<niminal_daemon::MidiEvent> {
        self.engine().session_mut().take_midi()
    }

    /// Each control's name, value, where it is going, and unit.
    pub fn control_values(&self) -> Vec<(String, f32, f32, Option<&'static str>)> {
        self.engine()
            .session()
            .control_values()
            .into_iter()
            .filter_map(|(name, value, target, unit)| {
                self.live
                    .control_name(&name, self.client)
                    .map(|name| (name, value, target, unit))
            })
            .collect()
    }

    /// The loudest sample on each output channel since this was last called.
    pub fn take_peaks(&mut self) -> Vec<f32> {
        self.engine().session_mut().take_peaks()
    }

    /// Voices silenced for producing NaN or infinity since this was last called.
    pub fn take_silenced(&mut self) -> usize {
        self.engine().session_mut().take_silenced()
    }

    /// The audio that tracks reading `input(..)` hear, a buffer for each channel starting at sample 0.
    pub fn set_audio_input(&mut self, channels: Vec<Vec<f32>>) {
        self.engine().session_mut().set_audio_input(channels);
    }

    /// Start keeping what each track and bus puts out, for stems. Only for renders: it grows without bound.
    pub fn capture_stems(&mut self) {
        self.engine().session_mut().capture_stems();
    }

    /// The stems kept so far, no longer kept after this.
    pub fn take_stems(&mut self) -> Option<niminal_engine::Stems> {
        let mut engine = self.engine();
        let mut captured = engine.session_mut().take_stems()?;
        let program = engine.session().program();
        let tracks = self
            .live
            .program()
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let index = if i == 0 {
                    Some(0)
                } else {
                    let name = format!(
                        "__routing_processor_{}_{}",
                        self.client,
                        crate::live::routing_id(&track.name)
                    );
                    program.track_index(&name)
                };
                index
                    .and_then(|i| captured.tracks.get_mut(i))
                    .map(std::mem::take)
                    .unwrap_or_default()
            })
            .collect();
        let buses = self
            .live
            .program()
            .buses
            .iter()
            .map(|name| {
                program
                    .buses
                    .iter()
                    .position(|n| n == &format!("__client_{}::{name}", self.client))
                    .and_then(|i| captured.buses.get_mut(i))
                    .map(std::mem::take)
                    .unwrap_or_default()
            })
            .collect();
        Some(niminal_engine::Stems { tracks, buses })
    }

    /// Changes on an immediate panic.
    pub fn panic_epoch(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.engine().session().panic_epoch()
    }

    /// Hand the engine's mixer to a renderer of its own, which the caller drives. The engine's audio is then
    /// made by that renderer, not by [`Offline::process`].
    pub fn start_realtime(
        &mut self,
        config: niminal_daemon::RealtimeConfig,
    ) -> niminal_daemon::RealtimeRenderer {
        self.engine().session_mut().start_realtime(config)
    }

    /// Let the sequencer send what falls due, and the engine prepare its windows.
    pub fn prepare_realtime(&mut self) {
        self.live.advance();
        self.engine().session_mut().prepare_realtime();
    }

    /// Like [`Offline::prepare_realtime`], but plans at most `max_windows`. Returns whether more is waiting.
    pub fn prepare_realtime_step(&mut self, max_windows: usize) -> bool {
        self.live.advance();
        self.engine()
            .session_mut()
            .prepare_realtime_step(max_windows)
    }

    /// The sample up to which the engine has prepared audio.
    pub fn prepared_until(&self) -> u64 {
        self.engine().session().prepared_until()
    }

    /// Bring the engine up to the renderer's clock.
    pub fn sync_realtime(&mut self) {
        self.engine().session_mut().sync_realtime();
    }

    pub fn realtime_metrics(&self) -> Option<niminal_daemon::RenderMetrics> {
        self.engine().session().realtime_metrics()
    }

    /// Move a control at sample `at` (now if none), over `smooth_ms` (the control's own smoothing if none), by
    /// sending the engine the event directly, as another client would.
    pub fn set_control(
        &mut self,
        name: &str,
        value: f64,
        unit: Option<&str>,
        at: Option<u64>,
        smooth_ms: Option<f64>,
    ) -> Result<(), Vec<niminal_daemon::Problem>> {
        use niminal_daemon::{At, Op};
        let op = Op::Control {
            name: name.into(),
            at: at.map_or(At::Now, At::Sample),
            value,
            unit: unit.map(str::to_owned),
            glide: smooth_ms.map(|ms| niminal_score::Time::Seconds(ms / 1000.0)),
            tag: None,
        };
        self.send_events(vec![op]).map(|_| ())
    }

    /// Send the engine events directly, as a client of its own, the way a program next to the sequencer would.
    pub fn send_events(
        &mut self,
        ops: Vec<niminal_daemon::Op>,
    ) -> Result<niminal_daemon::Sent, Vec<niminal_daemon::Problem>> {
        self.engine().session_mut().send_events(self.client, ops)
    }

    pub fn sequencer(&mut self) -> &mut Live {
        &mut self.live
    }

    /// The engine's clock.
    pub fn clock(&self) -> u64 {
        self.engine().session().clock()
    }

    /// Mix `frames` samples and return them, one buffer per channel. Everything the sequencer has for that
    /// stretch is sent first.
    pub fn process(&mut self, frames: usize) -> Vec<Vec<f32>> {
        let end = self.clock() + frames as u64;
        self.live.advance_to(end);
        let out = self.engine().session_mut().process(frames);
        // what falls due at the end is sent, so that a change made now sees the project as it will be
        self.live.advance();
        out
    }
}

impl Deref for Offline {
    type Target = Live;
    fn deref(&self) -> &Live {
        &self.live
    }
}

impl DerefMut for Offline {
    fn deref_mut(&mut self) -> &mut Live {
        &mut self.live
    }
}
