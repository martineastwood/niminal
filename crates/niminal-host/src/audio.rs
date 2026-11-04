//! Driving a session's clock: from a real audio device, or from a timer when
//! there is none.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use niminal_engine::MAX_CHANNELS;
use niminal_server::{AudioInput, EngineServer, RealtimeConfig, RealtimeRenderer};
use rtrb::RingBuffer;

/// Control and event preparation share the server. DSP owns its state and
/// communicates through bounded queues; neither rendering nor playback locks it.
pub type Shared = Arc<Mutex<EngineServer>>;

/// Lock the server, carrying on if another thread panicked while holding it.
pub fn lock(server: &Shared) -> MutexGuard<'_, EngineServer> {
    server.lock().unwrap_or_else(|e| e.into_inner())
}

/// The default output device, set up for `channels` channels.
pub struct Output {
    selection: Option<String>,
    device: cpal::Device,
    config: cpal::StreamConfig,
    name: String,
    health: Arc<Health>,
    requested_buffer: Option<u32>,
}

/// What has gone wrong with the audio so far, for the server to report without
/// flooding the terminal from the audio thread.
#[derive(Default)]
pub struct Health {
    /// Buffers replaced with silence because the engine panicked.
    pub panics: AtomicUsize,
    /// Errors the audio device reported (underruns and the like).
    pub errors: AtomicUsize,
    /// Device callbacks whose rendering took longer than the buffer lasts.
    pub underruns: AtomicUsize,
    /// Times event preparation panicked and was restarted.
    pub planner_panics: AtomicUsize,
    /// Errors the audio input device reported.
    pub input_errors: AtomicUsize,
    /// Set when the output stream reported an error: the device may be gone.
    pub lost: AtomicBool,
}

impl Output {
    /// Open the default output device. If `sample_rate` is `None` the device's
    /// own rate is used.
    pub fn open(channels: u16, sample_rate: Option<u32>) -> Result<Output, String> {
        Self::open_selected(channels, sample_rate, None, None)
    }
    pub fn open_selected(
        channels: u16,
        sample_rate: Option<u32>,
        id: Option<&str>,
        requested_buffer: Option<u32>,
    ) -> Result<Output, String> {
        let device = select_device(false, id)?;
        let default = device
            .default_output_config()
            .map_err(|e| format!("can't query the audio device: {e}"))?;
        // Request a small callback. When the backend cannot report limits, retain its default buffer size.
        let buffer_size = match default.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => {
                cpal::BufferSize::Fixed(256u32.clamp(*min, *max))
            }
            cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
        };
        let buffer_size = requested_buffer
            .map(cpal::BufferSize::Fixed)
            .unwrap_or(buffer_size);
        let requested_rate = sample_rate.unwrap_or_else(|| default.sample_rate());
        let supported = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .any(|c| {
                c.channels() == channels
                    && c.sample_format() == cpal::SampleFormat::F32
                    && requested_rate >= c.min_sample_rate()
                    && requested_rate <= c.max_sample_rate()
                    && match (buffer_size, c.buffer_size()) {
                        (
                            cpal::BufferSize::Fixed(n),
                            cpal::SupportedBufferSize::Range { min, max },
                        ) => n >= *min && n <= *max,
                        _ => true,
                    }
            });
        if !supported {
            return Err("output device does not support the requested float audio format".into());
        }
        let config = cpal::StreamConfig {
            channels,
            sample_rate: sample_rate.unwrap_or_else(|| default.sample_rate()),
            buffer_size,
        };
        Ok(Output {
            selection: id.map(str::to_string),
            name: device.to_string(),
            device,
            config,
            health: Arc::default(),
            requested_buffer,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The device callback size in frames, if one was asked for or the device reports one.
    pub fn buffer_frames(&self) -> Option<u32> {
        match self.config.buffer_size {
            cpal::BufferSize::Fixed(frames) => Some(frames),
            cpal::BufferSize::Default => None,
        }
    }

    pub fn health(&self) -> Arc<Health> {
        self.health.clone()
    }

    /// Start playing the server's session. Audio stops when the result is dropped.
    pub fn start(
        self,
        server: Shared,
        input: Option<AudioInput>,
        config: RealtimeConfig,
    ) -> Result<Running, String> {
        let (playback, helpers) = Playback::start(
            server,
            usize::from(self.config.channels),
            self.config.sample_rate,
            self.health.clone(),
            input,
            config,
        )?;
        let playback = Arc::new(Mutex::new(playback));
        let stream = build_stream(&self.device, self.config, &playback, &self.health)?;
        Ok(Running {
            selection: self.selection,
            requested_buffer: self.requested_buffer,
            stream: Some(stream),
            playback,
            health: self.health,
            channels: self.config.channels,
            sample_rate: self.config.sample_rate,
            _helpers: helpers,
        })
    }
}

/// The output stream, reading from `playback` and flagging the device as lost when it reports an error.
fn build_stream(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    playback: &Arc<Mutex<Playback>>,
    health: &Arc<Health>,
) -> Result<cpal::Stream, String> {
    let (playback, lost) = (playback.clone(), health.clone());
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                // only this callback takes the lock, so it is never contended; a failed try is silence
                match playback.try_lock() {
                    Ok(mut playback) => {
                        if let Some(probe) = &playback.probe {
                            let stamp = info.timestamp();
                            let latency = stamp.playback.duration_since(stamp.callback);
                            probe.set_latency_micros(latency.as_micros() as u64);
                        }
                        playback.read(data)
                    }
                    Err(_) => data.fill(0.0),
                }
            },
            move |_e| {
                lost.errors.fetch_add(1, Ordering::Relaxed);
                lost.lost.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| format!("can't open the audio stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("can't start the audio stream: {e}"))?;
    Ok(stream)
}

/// The default input device, listened to for nodes that read `input(..)`.
pub struct Listening {
    _stream: cpal::Stream,
    channels: usize,
    name: String,
}

impl Listening {
    /// Open the default input device at `sample_rate`. What it hears is put in the ring that is returned
    /// with it, which holds `ring_frames` frames of as many channels as the device has (at most
    /// [`MAX_CHANNELS`]).
    pub fn open(
        sample_rate: u32,
        ring_frames: usize,
        health: Arc<Health>,
    ) -> Result<(Listening, AudioInput), String> {
        Self::open_selected(sample_rate, ring_frames, health, None, None)
    }
    /// Like [`Listening::open`] on the device `id`, keeping only the first `channels` channels it has.
    pub fn open_selected(
        sample_rate: u32,
        ring_frames: usize,
        health: Arc<Health>,
        id: Option<&str>,
        channels: Option<usize>,
    ) -> Result<(Listening, AudioInput), String> {
        let device = select_device(true, id)?;
        let default = device
            .default_input_config()
            .map_err(|e| format!("can't query the audio input: {e}"))?;
        let all = usize::from(default.channels());
        let kept = all
            .min(MAX_CHANNELS)
            .min(channels.unwrap_or(usize::MAX))
            .max(1);
        let (mut producer, ring) = AudioInput::ring(kept, ring_frames);
        let config = cpal::StreamConfig {
            channels: default.channels(),
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = device
            .build_input_stream::<f32, _, _>(
                config,
                move |data: &[f32], _| {
                    for frame in data.chunks_exact(all) {
                        // a full ring means the render side is behind: drop the frame whole
                        if producer.slots() >= kept {
                            for sample in &frame[..kept] {
                                let _ = producer.push(*sample);
                            }
                        }
                    }
                },
                move |_e| {
                    health.input_errors.fetch_add(1, Ordering::Relaxed);
                },
                None,
            )
            .map_err(|e| format!("can't open the audio input at {sample_rate} Hz: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("can't start the audio input: {e}"))?;
        Ok((
            Listening {
                _stream: stream,
                channels: kept,
                name: device.to_string(),
            },
            ring,
        ))
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct Running {
    selection: Option<String>,
    requested_buffer: Option<u32>,
    stream: Option<cpal::Stream>,
    playback: Arc<Mutex<Playback>>,
    health: Arc<Health>,
    channels: u16,
    sample_rate: u32,
    _helpers: Helpers,
}

impl Running {
    /// Whether the output stream has reported an error since it was last opened.
    pub fn device_lost(&self) -> bool {
        self.health.lost.load(Ordering::Acquire)
    }

    /// Open the default output device again, at the rate the session runs at, and carry on playing from
    /// the same renderer. The session's clock waits while there is no device.
    pub fn reopen(&mut self) -> Result<String, String> {
        self.stream = None;
        let output = Output::open_selected(
            self.channels,
            Some(self.sample_rate),
            self.selection.as_deref(),
            self.requested_buffer,
        )?;
        let stream = build_stream(&output.device, output.config, &self.playback, &self.health)?;
        self.health.lost.store(false, Ordering::Release);
        self.stream = Some(stream);
        Ok(output.name)
    }
}

/// The sound card's callback renders the audio itself, as the session's clock advances: there is no
/// render thread and no queue between the engine and the device. Notes are prepared ahead of it by
/// the planner, so the callback only mixes.
struct Playback {
    renderer: RealtimeRenderer,
    channels: usize,
    sample_rate: f64,
    clock: Arc<AtomicU64>,
    health: Arc<Health>,
    probe: Option<Arc<niminal_server::ClockProbe>>,
}

/// What runs beside the callback: the planner, and the sender of `midi_out` notes.
struct Helpers {
    stop: Arc<AtomicBool>,
    planner: Option<JoinHandle<()>>,
    /// Sends the notes of `midi_out` instruments as the sound card plays them.
    _midi: Option<crate::midi_out::MidiOutput>,
}

impl Playback {
    fn start(
        server: Shared,
        channels: usize,
        sample_rate: u32,
        health: Arc<Health>,
        input: Option<AudioInput>,
        config: RealtimeConfig,
    ) -> Result<(Playback, Helpers), String> {
        let (probe, mut renderer) = {
            let mut d = lock(&server);
            let probe = d.session().clock_probe();
            let mut renderer = d.session_mut().start_realtime(config);
            if let Some(input) = input {
                renderer.set_audio_input(input);
            }
            (probe, renderer)
        };
        let clock = Arc::new(AtomicU64::new(renderer.clock()));
        let (midi_tx, midi_rx) = RingBuffer::new(1024);
        let midi = crate::midi_out::MidiOutput::start(clock.clone(), midi_rx);
        renderer.set_midi_output(midi_tx, Some(midi.wake_handle()));
        let stop = Arc::new(AtomicBool::new(false));
        let planner = start_planner(server, stop.clone(), health.clone())
            .map_err(|e| format!("can't start event preparation: {e}"))?;
        let playback = Playback {
            renderer,
            channels,
            sample_rate: f64::from(sample_rate),
            clock,
            health,
            probe: Some(probe),
        };
        let helpers = Helpers {
            stop,
            planner: Some(planner),
            _midi: Some(midi),
        };
        Ok((playback, helpers))
    }

    /// The complete device callback: bounded work, no allocation, no locks, and silence if the engine
    /// panics. The session's clock advances by exactly the frames asked for.
    fn read(&mut self, data: &mut [f32]) {
        data.fill(0.0);
        if let Some(probe) = &self.probe {
            probe.record(self.renderer.clock());
        }
        let started = Instant::now();
        let channels = self.channels;
        let frames = data.len() / channels;
        let renderer = &mut self.renderer;
        let mut written = 0;
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            renderer.render(frames, &mut |block, n| {
                for (c, plane) in block.iter().take(channels).enumerate() {
                    for (i, sample) in plane[..n].iter().enumerate() {
                        data[(written + i) * channels + c] = *sample;
                    }
                }
                written += n;
            });
        }));
        if rendered.is_err() {
            data.fill(0.0);
            self.renderer.request_panic();
            self.health.panics.fetch_add(1, Ordering::Relaxed);
        }
        self.clock.store(self.renderer.clock(), Ordering::Release);
        if started.elapsed().as_secs_f64() * self.sample_rate > frames as f64 {
            self.health.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many windows the planner makes while it holds the server, so that a change after a big edit
/// (when every window is made again) waits for a few windows rather than for all of them.
const PLAN_STEP: usize = 1;

fn start_planner(
    server: Shared,
    stop: Arc<AtomicBool>,
    health: Arc<Health>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("niminal-plan".into())
        .spawn(move || {
            if let Ok(d) = server.lock() {
                d.session().set_planner_thread(thread::current());
            }
            while !stop.load(Ordering::Relaxed) {
                let locked = match server.try_lock() {
                    Ok(d) => Some(d),
                    // a panic elsewhere must not end preparation
                    Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
                    Err(std::sync::TryLockError::WouldBlock) => None,
                };
                match locked {
                    Some(mut d) => {
                        // A panic here must not end preparation, or the music stops without a word: the
                        // renderer is told to start clean and the next round plans again from the session.
                        let planned =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                d.session_mut().prepare_realtime_step(PLAN_STEP)
                            }));
                        if planned.is_err() {
                            health.planner_panics.fetch_add(1, Ordering::Relaxed);
                            d.session().restart_realtime();
                        }
                        drop(d);
                        if planned.is_ok_and(|more| more) {
                            // Some windows are still to plan, but whoever is waiting for the server goes first.
                            thread::sleep(Duration::from_micros(100));
                        } else {
                            // A window is up to 40ms of sound and the plan reaches 170ms ahead, so there is no
                            // hurry. A change that lands wakes this thread at once.
                            thread::park_timeout(Duration::from_millis(30));
                        }
                    }
                    // someone is evaluating: look again in a moment
                    None => thread::sleep(Duration::from_micros(200)),
                }
            }
        })
}

/// Without this macOS may park a thread that has deadlines for tens of milliseconds.
pub fn prefer_low_latency_scheduling() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets the scheduling class of the calling thread; nothing else is touched.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

impl Drop for Helpers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(planner) = self.planner.take() {
            planner.thread().unpark();
            let _ = planner.join();
        }
    }
}

/// A clock for running without a sound card: advances the session at (a
/// multiple of) real time and throws the audio away.
pub struct NullClock {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    planner: Option<JoinHandle<()>>,
}

impl NullClock {
    pub fn start(server: Shared, speed: f64, config: RealtimeConfig) -> NullClock {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (rate, mut renderer) = {
            let mut d = lock(&server);
            (
                f64::from(d.session().sample_rate()),
                d.session_mut().start_realtime(config),
            )
        };
        let planner =
            start_planner(server, stop.clone(), Arc::default()).expect("start event preparation");
        let thread = thread::spawn(move || {
            let started = Instant::now();
            let initial = renderer.clock();
            while !flag.load(Ordering::Relaxed) {
                let due = initial + (started.elapsed().as_secs_f64() * speed * rate) as u64;
                let n = due.saturating_sub(renderer.clock()).min(4096) as usize;
                renderer.render(n, &mut |_, _| {});
                thread::sleep(Duration::from_millis(2));
            }
        });
        NullClock {
            stop,
            thread: Some(thread),
            planner: Some(planner),
        }
    }
}

impl Drop for NullClock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(planner) = self.planner.take() {
            let _ = planner.join();
        }
    }
}

fn select_device(input: bool, id: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    if let Some(id) = id.filter(|id| *id != "default") {
        let kind = if input { "input:" } else { "output:" };
        let mut devices = if input {
            host.input_devices()
        } else {
            host.output_devices()
        }
        .map_err(|e| e.to_string())?;
        // an id from `audio.devices` such as `output:1`, else part of a device's name
        if let Some(index) = id.strip_prefix(kind).and_then(|i| i.parse::<usize>().ok()) {
            return devices
                .nth(index)
                .ok_or_else(|| "audio device is no longer available".into());
        }
        let wanted = id.to_lowercase();
        devices
            .find(|d| d.to_string().to_lowercase().contains(&wanted))
            .ok_or_else(|| {
                format!(
                    "no audio {} device matches `{id}`; `niminal devices` lists them",
                    if input { "input" } else { "output" }
                )
            })
    } else {
        if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
        .ok_or_else(|| "no default audio device".into())
    }
}
pub fn devices() -> serde_json::Value {
    let host = cpal::default_host();
    let list = |input: bool| -> Vec<serde_json::Value> {
        let devices = if input {
            host.input_devices()
        } else {
            host.output_devices()
        };
        devices.map(|ds|ds.enumerate().map(|(index,device)|{
            let configs=if input{device.supported_input_configs().map(|c|c.collect::<Vec<_>>())}else{device.supported_output_configs().map(|c|c.collect::<Vec<_>>())};
            let configs=configs.map(|cs|cs.into_iter().map(|c|serde_json::json!({"channels":c.channels(),"min_sample_rate":c.min_sample_rate(),"max_sample_rate":c.max_sample_rate(),"format":format!("{:?}",c.sample_format()),"buffer_size":format!("{:?}",c.buffer_size())})).collect::<Vec<_>>()).unwrap_or_default();
            serde_json::json!({"id":format!("{}:{index}",if input{"input"}else{"output"}),"name":device.to_string(),"configs":configs})
        }).collect()).unwrap_or_default()
    };
    serde_json::json!({"inputs":list(true),"outputs":list(false)})
}
pub fn validate_input(id: &str, rate: u32) -> Result<(), String> {
    let device = select_device(true, Some(id))?;
    let supported = device
        .supported_input_configs()
        .map_err(|e| e.to_string())?
        .any(|c| {
            c.sample_format() == cpal::SampleFormat::F32
                && rate >= c.min_sample_rate()
                && rate <= c.max_sample_rate()
        });
    if supported {
        Ok(())
    } else {
        Err("input device does not support the requested sample rate".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct CountingAllocator;
    thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }
    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            COUNTING.with(|on| {
                if on.get() {
                    ALLOCATIONS.with(|n| n.set(n.get() + 1));
                }
            });
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            COUNTING.with(|on| {
                if on.get() {
                    ALLOCATIONS.with(|n| n.set(n.get() + 1));
                }
            });
            unsafe { System.realloc(ptr, layout, size) }
        }
    }

    fn tone_session() -> niminal_server::Session {
        let mut session =
            niminal_server::Session::new(48_000.0, niminal_lang::Layout::Stereo).without_limiter();
        play_tone(&mut session);
        session
    }

    #[test]
    fn the_callback_allocates_nothing_and_interleaves_every_channel() {
        let mut reference = tone_session();
        let expected = reference.process(512);
        let mut session = tone_session();
        let renderer = session.start_realtime(RealtimeConfig::default());
        session.prepare_realtime();
        let mut playback = Playback {
            renderer,
            channels: 2,
            sample_rate: 48_000.0,
            clock: Arc::new(AtomicU64::new(0)),
            health: Arc::default(),
            probe: None,
        };
        let mut audio = [0.0; 1024];
        ALLOCATIONS.with(|n| n.set(0));
        COUNTING.with(|on| on.set(true));
        playback.read(&mut audio);
        COUNTING.with(|on| on.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        for (i, pair) in audio.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(*pair, [expected[0][i], expected[1][i]], "frame {i}");
        }
        assert_eq!(playback.clock.load(Ordering::Acquire), 512);
    }

    #[test]
    fn preparation_carries_on_after_another_thread_panics_holding_the_server() {
        let server: Shared = Arc::new(Mutex::new(EngineServer::new(tone_session())));
        let poisoner = server.clone();
        let _ = thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("a client thread failed");
        })
        .join();
        assert!(server.is_poisoned());
        let (mut playback, _helpers) = Playback::start(
            server,
            2,
            48_000,
            Arc::default(),
            None,
            RealtimeConfig::default(),
        )
        .unwrap();
        // the planner keeps feeding the renderer, so the tone is still heard
        let mut heard = false;
        for _ in 0..50 {
            thread::sleep(Duration::from_millis(20));
            let mut audio = vec![0.0; 512];
            playback.read(&mut audio);
            heard |= audio.iter().any(|s| *s != 0.0);
        }
        assert!(heard);
    }

    fn play_tone(session: &mut niminal_server::Session) {
        use niminal_score::{Time, Value};
        use niminal_server::{At, Op};
        let define = Op::Define {
            source: "instr tone(freq: hz) { osc(sine, freq) }".into(),
            at: At::Sample(0),
            dir: None,
            tag: None,
        };
        let note = Op::Note {
            to: None,
            target: "tone".into(),
            at: At::Sample(0),
            args: [("freq".to_string(), Value::Hz(440.0))].into(),
            dur: Some(Time::Seconds(0.5)),
            id: None,
            tag: None,
        };
        session.send_events(0, vec![define, note]).unwrap();
    }

    #[test]
    fn a_control_lock_cannot_block_the_callback_or_change_its_audio() {
        const FRAMES: usize = 2048;
        let mut reference = tone_session();
        let expected = reference.process(FRAMES * 4);
        let server = Arc::new(Mutex::new(EngineServer::new(tone_session())));
        let (mut playback, helpers) = Playback::start(
            server.clone(),
            2,
            48_000,
            Arc::default(),
            None,
            RealtimeConfig::default(),
        )
        .unwrap();
        // let the planner prepare the notes before something holds the server
        thread::sleep(Duration::from_millis(200));
        let guard = lock(&server);
        let mut audio = vec![0.0; FRAMES * 2];
        for batch in 0..4 {
            playback.read(&mut audio);
            for (i, pair) in audio.as_chunks::<2>().0.iter().enumerate() {
                let at = batch * FRAMES + i;
                assert_eq!(
                    *pair,
                    [expected[0][at], expected[1][at]],
                    "batch {batch} i {i}"
                );
            }
        }
        drop(guard);
        drop(helpers);
    }
}
