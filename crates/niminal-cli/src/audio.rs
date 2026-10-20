//! Driving a session's clock: from a real audio device, or from a timer when
//! there is none.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use niminal_daemon::{AudioInput, EngineServer, RealtimeConfig, RealtimeRenderer};
use niminal_engine::{BLOCK, MAX_CHANNELS};
use rtrb::{Consumer, Producer, RingBuffer};

/// Control and event preparation share the daemon. DSP owns its state and
/// communicates through bounded queues; neither rendering nor playback locks it.
pub type Shared = Arc<Mutex<EngineServer>>;

/// Lock the daemon, carrying on if another thread panicked while holding it.
pub fn lock(daemon: &Shared) -> MutexGuard<'_, EngineServer> {
    daemon.lock().unwrap_or_else(|e| e.into_inner())
}

/// The default output device, set up for `channels` channels.
pub struct Output {
    selection: Option<String>,
    device: cpal::Device,
    config: cpal::StreamConfig,
    name: String,
    health: Arc<Health>,
    queue_frames: usize,
}

/// What has gone wrong with the audio so far, for the daemon to report without
/// flooding the terminal from the audio thread.
#[derive(Default)]
pub struct Health {
    /// Buffers replaced with silence because the engine panicked.
    pub panics: AtomicUsize,
    /// Errors the audio device reported (underruns and the like).
    pub errors: AtomicUsize,
    /// Device callbacks that exhausted the render queue.
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
        // Request a callback comfortably smaller than the output queue. When
        // the backend cannot report limits, retain its default buffer size.
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
        let queue_frames = match buffer_size {
            cpal::BufferSize::Fixed(frames) => QUEUE_FRAMES.max(frames as usize * 4),
            cpal::BufferSize::Default => QUEUE_FRAMES,
        };
        Ok(Output {
            selection: id.map(str::to_string),
            name: device.to_string(),
            device,
            config,
            health: Arc::default(),
            queue_frames,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn health(&self) -> Arc<Health> {
        self.health.clone()
    }

    /// Start playing the daemon's session. Audio stops when the result is dropped.
    pub fn start(self, daemon: Shared, input: Option<AudioInput>) -> Result<Running, String> {
        let channels = usize::from(self.config.channels);
        let (playback, worker) = RenderWorker::start(
            daemon,
            channels,
            self.queue_frames,
            self.health.clone(),
            input,
        )?;
        let playback = Arc::new(Mutex::new(playback));
        let stream = build_stream(&self.device, self.config, &playback, &self.health)?;
        Ok(Running {
            selection: self.selection,
            stream: Some(stream),
            playback,
            health: self.health,
            channels: self.config.channels,
            sample_rate: self.config.sample_rate,
            _worker: worker,
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

/// The default input device, listened to for tracks that read `input(..)`.
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
        Self::open_selected(sample_rate, ring_frames, health, None)
    }
    pub fn open_selected(
        sample_rate: u32,
        ring_frames: usize,
        health: Arc<Health>,
        id: Option<&str>,
    ) -> Result<(Listening, AudioInput), String> {
        let device = select_device(true, id)?;
        let default = device
            .default_input_config()
            .map_err(|e| format!("can't query the audio input: {e}"))?;
        let all = usize::from(default.channels());
        let kept = all.min(MAX_CHANNELS);
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
    stream: Option<cpal::Stream>,
    playback: Arc<Mutex<Playback>>,
    health: Arc<Health>,
    channels: u16,
    sample_rate: u32,
    _worker: RenderWorker,
}

impl Running {
    /// Whether the output stream has reported an error since it was last opened.
    pub fn device_lost(&self) -> bool {
        self.health.lost.load(Ordering::Acquire)
    }

    /// Open the default output device again, at the rate the session runs at, and carry on playing from
    /// the same queue. The session's clock waits while there is no device.
    pub fn reopen(&mut self) -> Result<String, String> {
        self.stream = None;
        let output = Output::open_selected(
            self.channels,
            Some(self.sample_rate),
            self.selection.as_deref(),
            None,
        )?;
        let stream = build_stream(&output.device, output.config, &self.playback, &self.health)?;
        self.health.lost.store(false, Ordering::Release);
        self.stream = Some(stream);
        Ok(output.name)
    }
}

/// About 43ms at 48kHz. This bounds control latency and lets the worker run
/// independently of device callbacks. Frames are timestamped so a late worker
/// cannot replay old audio after an underrun.
const QUEUE_FRAMES: usize = 2048;

struct Frame {
    at: u64,
    epoch: u64,
    samples: [f32; MAX_CHANNELS],
}

struct Playback {
    queue: Consumer<Frame>,
    render_thread: Option<thread::Thread>,
    channels: usize,
    clock: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    health: Arc<Health>,
    next: u64,
    waiting: Option<Frame>,
    queue_frames: usize,
    probe: Option<Arc<niminal_daemon::ClockProbe>>,
}

impl Playback {
    /// The complete device callback: bounded work, no allocation, no locks,
    /// and silence on starvation. The device clock advances even during silence.
    fn read(&mut self, data: &mut [f32]) {
        data.fill(0.0);
        if let Some(probe) = &self.probe {
            probe.record(self.next);
        }
        // Usually the queue contains exactly the next stretch of audio. Read it in place and
        // release the slots once per callback, avoiding a ring update and a Frame copy per sample.
        let frames = data.len() / self.channels;
        if self.waiting.is_none()
            && let Ok(chunk) = self.queue.read_chunk(frames)
        {
            let (first, second) = chunk.as_slices();
            if first
                .iter()
                .chain(second)
                .enumerate()
                .all(|(i, f)| f.at == self.next + i as u64)
            {
                let mut epoch = self.epoch.load(Ordering::Acquire);
                for (i, (frame, output)) in first
                    .iter()
                    .chain(second)
                    .zip(data.chunks_exact_mut(self.channels))
                    .enumerate()
                {
                    if i % BLOCK == 0 {
                        epoch = self.epoch.load(Ordering::Acquire);
                    }
                    if frame.epoch == epoch {
                        output.copy_from_slice(&frame.samples[..self.channels]);
                    }
                }
                chunk.commit_all();
                self.next += frames as u64;
                self.clock.store(self.next, Ordering::Release);
                if let Some(renderer) = &self.render_thread {
                    renderer.unpark();
                }
                return;
            }
        }
        let mut epoch = self.epoch.load(Ordering::Acquire);
        let mut missed = false;
        let mut stale_budget = self.queue_frames;
        for (i, output) in data.chunks_exact_mut(self.channels).enumerate() {
            // A panic arriving during a large callback takes effect within one
            // engine block rather than waiting for the next device callback.
            if i % BLOCK == 0 {
                epoch = self.epoch.load(Ordering::Acquire);
            }
            loop {
                if self.waiting.is_none() {
                    self.waiting = self.queue.pop().ok();
                }
                match self.waiting.as_ref() {
                    Some(frame) if frame.at < self.next && stale_budget > 0 => {
                        self.waiting = None;
                        stale_budget -= 1;
                    }
                    _ => break,
                }
            }
            if self.waiting.as_ref().is_some_and(|f| f.at == self.next) {
                if let Some(frame) = self.waiting.take()
                    && frame.epoch == epoch
                {
                    output.copy_from_slice(&frame.samples[..self.channels]);
                }
            } else {
                missed = true;
            }
            self.next += 1;
        }
        self.clock.store(self.next, Ordering::Release);
        if let Some(renderer) = &self.render_thread {
            renderer.unpark();
        }
        if missed {
            self.health.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct RenderWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    planner: Option<JoinHandle<()>>,
    /// Sends the notes of `midi_out` instruments as the sound card plays them.
    _midi: Option<crate::midi_out::MidiOutput>,
}

/// How many windows the planner makes while it holds the daemon, so that a change after a big edit
/// (when every window is made again) waits for a few windows rather than for all of them.
const PLAN_STEP: usize = 1;

fn start_planner(
    daemon: Shared,
    stop: Arc<AtomicBool>,
    health: Arc<Health>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("niminal-plan".into())
        .spawn(move || {
            if let Ok(d) = daemon.lock() {
                d.session().set_planner_thread(thread::current());
            }
            while !stop.load(Ordering::Relaxed) {
                let locked = match daemon.try_lock() {
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
                            // Some windows are still to plan, but whoever is waiting for the daemon goes first.
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

impl RenderWorker {
    fn start(
        daemon: Shared,
        channels: usize,
        queue_frames: usize,
        health: Arc<Health>,
        input: Option<AudioInput>,
    ) -> Result<(Playback, Self), String> {
        let (producer, consumer) = RingBuffer::new(queue_frames);
        let (next, epoch, mut renderer, probe) = {
            let mut d = lock(&daemon);
            let probe = d.session().clock_probe();
            let next = d.session().clock();
            let epoch = d.session().panic_epoch();
            let mut renderer = d.session_mut().start_realtime(RealtimeConfig::default());
            if let Some(input) = input {
                renderer.set_audio_input(input);
            }
            (next, epoch, renderer, probe)
        };
        let clock = Arc::new(AtomicU64::new(next));
        let (midi_tx, midi_rx) = RingBuffer::new(1024);
        let midi = crate::midi_out::MidiOutput::start(clock.clone(), midi_rx);
        renderer.set_midi_output(midi_tx, Some(midi.wake_handle()));
        let mut playback = Playback {
            queue: consumer,
            render_thread: None,
            channels,
            clock: clock.clone(),
            epoch: epoch.clone(),
            health: health.clone(),
            next,
            waiting: None,
            queue_frames,
            probe: Some(probe),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let planner = start_planner(daemon, stop.clone(), health.clone())
            .map_err(|e| format!("can't start event preparation: {e}"))?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("niminal-render".into())
            .spawn(move || {
                let mut producer = producer;
                prefer_low_latency_scheduling();
                // Prime the queue before starting the sound card.
                fill_queue(
                    &mut renderer,
                    &mut producer,
                    queue_frames,
                    next,
                    &epoch,
                    &health,
                );
                let _ = ready_tx.send(());
                // Top the queue up in chunks, and sleep until the sound card has used enough for the next one.
                let chunk = BLOCK * 8;
                while !flag.load(Ordering::Relaxed) {
                    let free = producer.slots();
                    if free >= chunk {
                        fill_queue(
                            &mut renderer,
                            &mut producer,
                            chunk,
                            clock.load(Ordering::Acquire),
                            &epoch,
                            &health,
                        );
                    } else {
                        // The callback wakes us after consuming audio. The timeout also lets shutdown
                        // and device loss make progress when there are no callbacks.
                        thread::park_timeout(Duration::from_millis(20));
                    }
                }
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                stop.store(true, Ordering::Relaxed);
                let _ = planner.join();
                return Err(format!("can't start the audio render worker: {error}"));
            }
        };
        playback.render_thread = Some(thread.thread().clone());
        let worker = Self {
            stop,
            thread: Some(thread),
            planner: Some(planner),
            _midi: Some(midi),
        };
        ready_rx
            .recv()
            .map_err(|_| "the audio render worker stopped before starting".to_string())?;
        Ok((playback, worker))
    }
}

/// Without this macOS may park the render thread for tens of milliseconds, which dries the queue.
fn prefer_low_latency_scheduling() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets the scheduling class of the calling thread; nothing else is touched.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

fn fill_queue(
    renderer: &mut RealtimeRenderer,
    producer: &mut Producer<Frame>,
    frames: usize,
    played: u64,
    epoch: &AtomicU64,
    health: &Health,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Catch up in bounded slices. Audio missed by the device stays missed;
        // events and DSP still advance to preserve the session's timeline.
        let behind = played
            .saturating_sub(renderer.clock())
            .min((BLOCK * 8) as u64) as usize;
        if behind > 0 {
            renderer.render(behind, &mut |_, _| {});
        }
        if renderer.clock() < played {
            return;
        }
        let mut at = renderer.clock();
        let epoch = epoch.load(Ordering::Acquire);
        renderer.render(frames, &mut |block, n| {
            // Publish one DSP block at a time instead of synchronizing the ring for every sample.
            // The consumer only frees slots, so the caller's capacity check still holds here.
            if let Ok(chunk) = producer.write_chunk_uninit(n) {
                chunk.fill_from_iter((0..n).map(|i| {
                    let mut samples = [0.0; MAX_CHANNELS];
                    for (sample, channel) in samples.iter_mut().zip(block) {
                        *sample = channel[i];
                    }
                    Frame {
                        at: at + i as u64,
                        epoch,
                        samples,
                    }
                }));
            }
            at += n as u64;
        });
    }));
    if result.is_err() {
        renderer.request_panic();
        health.panics.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        if let Some(planner) = self.planner.take() {
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
    pub fn start(daemon: Shared, speed: f64) -> NullClock {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (rate, mut renderer) = {
            let mut d = lock(&daemon);
            (
                f64::from(d.session().sample_rate()),
                d.session_mut().start_realtime(RealtimeConfig::default()),
            )
        };
        let planner =
            start_planner(daemon, stop.clone(), Arc::default()).expect("start event preparation");
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
        let index = id
            .strip_prefix(kind)
            .and_then(|i| i.parse::<usize>().ok())
            .ok_or("invalid device id")?;
        let mut devices = if input {
            host.input_devices()
        } else {
            host.output_devices()
        }
        .map_err(|e| e.to_string())?;
        devices
            .nth(index)
            .ok_or_else(|| "audio device is no longer available".into())
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

    fn playback() -> (Producer<Frame>, Playback) {
        let (tx, rx) = RingBuffer::new(QUEUE_FRAMES);
        (
            tx,
            Playback {
                queue: rx,
                render_thread: None,
                channels: 2,
                clock: Arc::new(AtomicU64::new(0)),
                epoch: Arc::new(AtomicU64::new(0)),
                health: Arc::default(),
                next: 0,
                waiting: None,
                queue_frames: QUEUE_FRAMES,
                probe: None,
            },
        )
    }

    fn frame(at: u64, epoch: u64, left: f32, right: f32) -> Frame {
        let mut samples = [0.0; MAX_CHANNELS];
        samples[0] = left;
        samples[1] = right;
        Frame { at, epoch, samples }
    }

    #[test]
    fn the_callback_allocates_nothing_and_interleaves_every_channel() {
        let (mut tx, mut playback) = playback();
        for at in 0..16 {
            tx.push(frame(at, 0, at as f32, -(at as f32))).ok().unwrap();
        }
        let mut audio = [0.0; 32];
        ALLOCATIONS.with(|n| n.set(0));
        COUNTING.with(|on| on.set(true));
        playback.read(&mut audio);
        COUNTING.with(|on| on.set(false));
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        for (i, pair) in audio.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(*pair, [i as f32, -(i as f32)]);
        }
        assert_eq!(playback.clock.load(Ordering::Acquire), 16);
    }

    #[test]
    fn starvation_is_silent_and_old_audio_is_not_replayed() {
        let (mut tx, mut playback) = playback();
        let mut audio = [1.0; 8];
        playback.read(&mut audio);
        assert_eq!(audio, [0.0; 8]);
        assert_eq!(playback.health.underruns.load(Ordering::Relaxed), 1);
        for at in 0..8 {
            tx.push(frame(at, 0, at as f32, 0.5)).ok().unwrap();
        }
        playback.read(&mut audio);
        assert_eq!(audio, [4.0, 0.5, 5.0, 0.5, 6.0, 0.5, 7.0, 0.5]);
        assert_eq!(playback.clock.load(Ordering::Acquire), 8);
    }

    #[test]
    fn panic_discards_audio_already_waiting_in_the_output_queue() {
        let (mut tx, mut playback) = playback();
        for at in 0..4 {
            tx.push(frame(at, 0, 0.5, 0.5)).ok().unwrap();
        }
        playback.epoch.store(1, Ordering::Release);
        tx.push(frame(4, 1, 0.25, 0.75)).ok().unwrap();
        let mut audio = [1.0; 10];
        playback.read(&mut audio);
        assert_eq!(&audio[..8], &[0.0; 8]);
        assert_eq!(&audio[8..], &[0.25, 0.75]);
    }

    #[test]
    fn preparation_carries_on_after_another_thread_panics_holding_the_daemon() {
        let mut session =
            niminal_daemon::Session::new(48_000.0, niminal_lang::Layout::Stereo).without_limiter();
        play_tone(&mut session);
        let daemon: Shared = Arc::new(Mutex::new(EngineServer::new(session)));
        let poisoner = daemon.clone();
        let _ = thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("a client thread failed");
        })
        .join();
        assert!(daemon.is_poisoned());
        let (mut playback, _worker) =
            RenderWorker::start(daemon, 2, QUEUE_FRAMES, Arc::default(), None).unwrap();
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

    fn play_tone(session: &mut niminal_daemon::Session) {
        use niminal_daemon::{At, Op};
        use niminal_score::{Time, Value};
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
        let mut session =
            niminal_daemon::Session::new(48_000.0, niminal_lang::Layout::Stereo).without_limiter();
        play_tone(&mut session);
        let mut reference =
            niminal_daemon::Session::new(48_000.0, niminal_lang::Layout::Stereo).without_limiter();
        play_tone(&mut reference);
        let expected = reference.process(QUEUE_FRAMES * 4);
        let daemon = Arc::new(Mutex::new(EngineServer::new(session)));
        let (mut playback, worker) =
            RenderWorker::start(daemon.clone(), 2, QUEUE_FRAMES, Arc::default(), None).unwrap();
        let guard = lock(&daemon);
        let mut audio = vec![0.0; QUEUE_FRAMES * 2];
        for batch in 0..4 {
            if batch > 0 {
                thread::sleep(Duration::from_millis(5));
            }
            playback.read(&mut audio);
            for (i, pair) in audio.as_chunks::<2>().0.iter().enumerate() {
                let at = batch * QUEUE_FRAMES + i;
                assert_eq!(
                    *pair,
                    [expected[0][at], expected[1][at]],
                    "batch {batch} i {i}"
                );
            }
        }
        assert_eq!(playback.health.underruns.load(Ordering::Relaxed), 0);
        drop(guard);
        drop(worker);
    }
}
