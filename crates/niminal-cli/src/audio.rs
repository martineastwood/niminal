//! Driving a session's clock: from a real audio device, or from a timer when
//! there is none.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use niminal_daemon::{Daemon, RealtimeConfig, RealtimeRenderer};
use niminal_engine::{BLOCK, MAX_CHANNELS};
use rtrb::{Consumer, Producer, RingBuffer};

/// Control and event preparation share the daemon. DSP owns its state and
/// communicates through bounded queues; neither rendering nor playback locks it.
pub type Shared = Arc<Mutex<Daemon>>;

/// Lock the daemon, carrying on if another thread panicked while holding it.
pub fn lock(daemon: &Shared) -> MutexGuard<'_, Daemon> {
    daemon.lock().unwrap_or_else(|e| e.into_inner())
}

/// The default output device, set up for `channels` channels.
pub struct Output {
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
}

impl Output {
    /// Open the default output device. If `sample_rate` is `None` the device's
    /// own rate is used.
    pub fn open(channels: u16, sample_rate: Option<u32>) -> Result<Output, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("there is no audio output device")?;
        let default = device.default_output_config().map_err(|e| format!("can't query the audio device: {e}"))?;
        // Request a callback comfortably smaller than the output queue. When
        // the backend cannot report limits, retain its default buffer size.
        let buffer_size = match default.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => cpal::BufferSize::Fixed(256u32.clamp(*min, *max)),
            cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
        };
        let config = cpal::StreamConfig {
            channels,
            sample_rate: sample_rate.unwrap_or_else(|| default.sample_rate()),
            buffer_size,
        };
        let queue_frames = match buffer_size {
            cpal::BufferSize::Fixed(frames) => QUEUE_FRAMES.max(frames as usize * 4),
            cpal::BufferSize::Default => QUEUE_FRAMES,
        };
        Ok(Output { name: device.to_string(), device, config, health: Arc::default(), queue_frames })
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
    pub fn start(self, daemon: Shared) -> Result<Running, String> {
        let channels = usize::from(self.config.channels);
        let device_health = self.health.clone();
        let (mut playback, worker) = RenderWorker::start(daemon, channels, self.queue_frames, self.health.clone())?;
        let stream = self
            .device
            .build_output_stream::<f32, _, _>(
                self.config,
                move |data: &mut [f32], _| playback.read(data),
                move |_e| { device_health.errors.fetch_add(1, Ordering::Relaxed); },
                None,
            )
            .map_err(|e| format!("can't open the audio stream: {e}"))?;
        stream.play().map_err(|e| format!("can't start the audio stream: {e}"))?;
        Ok(Running { _stream: stream, _worker: worker })
    }
}

pub struct Running {
    _stream: cpal::Stream,
    _worker: RenderWorker,
}

/// About 21ms at 48kHz. This bounds control latency and lets the worker run
/// independently of device callbacks. Frames are timestamped so a late worker
/// cannot replay old audio after an underrun.
const QUEUE_FRAMES: usize = 1024;

struct Frame {
    at: u64,
    epoch: u64,
    samples: [f32; MAX_CHANNELS],
}

struct Playback {
    queue: Consumer<Frame>,
    channels: usize,
    clock: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    health: Arc<Health>,
    next: u64,
    waiting: Option<Frame>,
    queue_frames: usize,
}

impl Playback {
    /// The complete device callback: bounded work, no allocation, no locks,
    /// and silence on starvation. The device clock advances even during silence.
    fn read(&mut self, data: &mut [f32]) {
        data.fill(0.0);
        let mut epoch = self.epoch.load(Ordering::Acquire);
        let mut missed = false;
        let mut stale_budget = self.queue_frames;
        for (i, output) in data.chunks_exact_mut(self.channels).enumerate() {
            // A panic arriving during a large callback takes effect within one
            // engine block rather than waiting for the next device callback.
            if i % BLOCK == 0 { epoch = self.epoch.load(Ordering::Acquire); }
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
        if missed {
            self.health.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct RenderWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    planner: Option<JoinHandle<()>>,
}

fn start_planner(daemon: Shared, stop: Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name("niminal-plan".into()).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Ok(mut d) = daemon.try_lock() {
                d.session_mut().prepare_realtime();
            }
            thread::sleep(Duration::from_micros(500));
        }
    })
}

impl RenderWorker {
    fn start(daemon: Shared, channels: usize, queue_frames: usize, health: Arc<Health>) -> Result<(Playback, Self), String> {
        let (producer, consumer) = RingBuffer::new(queue_frames);
        let (next, epoch, mut renderer) = {
            let mut d = lock(&daemon);
            let next = d.session().clock();
            let epoch = d.session().panic_epoch();
            let renderer = d.session_mut().start_realtime(RealtimeConfig::default());
            (next, epoch, renderer)
        };
        let clock = Arc::new(AtomicU64::new(next));
        let playback = Playback { queue: consumer, channels, clock: clock.clone(), epoch: epoch.clone(),
            health: health.clone(), next, waiting: None, queue_frames };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let planner = start_planner(daemon, stop.clone()).map_err(|e| format!("can't start event preparation: {e}"))?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("niminal-render".into()).spawn(move || {
            let mut producer = producer;
            // Prime the queue before starting the sound card.
            fill_queue(&mut renderer, &mut producer, queue_frames, next, &epoch, &health);
            let _ = ready_tx.send(());
            while !flag.load(Ordering::Relaxed) {
                let free = producer.slots();
                if free > 0 {
                    fill_queue(&mut renderer, &mut producer, free.min(BLOCK * 8), clock.load(Ordering::Acquire), &epoch, &health);
                } else {
                    thread::sleep(Duration::from_micros(250));
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
        let worker = Self { stop, thread: Some(thread), planner: Some(planner) };
        ready_rx.recv().map_err(|_| "the audio render worker stopped before starting".to_string())?;
        Ok((playback, worker))
    }
}

fn fill_queue(renderer: &mut RealtimeRenderer, producer: &mut Producer<Frame>, frames: usize, played: u64,
              epoch: &AtomicU64, health: &Health) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Catch up in bounded slices. Audio missed by the device stays missed;
        // events and DSP still advance to preserve the session's timeline.
        let behind = played.saturating_sub(renderer.clock()).min((BLOCK * 8) as u64) as usize;
        renderer.render(behind, &mut |_, _| {});
        if renderer.clock() < played { return; }
        let mut at = renderer.clock();
        let epoch = epoch.load(Ordering::Acquire);
        renderer.render(frames, &mut |block, n| {
            for i in 0..n {
                let mut samples = [0.0; MAX_CHANNELS];
                for (sample, channel) in samples.iter_mut().zip(block) { *sample = channel[i]; }
                // The consumer can only free slots, so the space checked by the
                // producer remains available for this entire render.
                let _ = producer.push(Frame { at, epoch, samples });
                at += 1;
            }
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
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
        if let Some(planner) = self.planner.take() { let _ = planner.join(); }
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
            (f64::from(d.session().sample_rate()), d.session_mut().start_realtime(RealtimeConfig::default()))
        };
        let planner = start_planner(daemon, stop.clone()).expect("start event preparation");
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
        NullClock { stop, thread: Some(thread), planner: Some(planner) }
    }
}

impl Drop for NullClock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(planner) = self.planner.take() { let _ = planner.join(); }
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
            COUNTING.with(|on| { if on.get() { ALLOCATIONS.with(|n| n.set(n.get() + 1)); } });
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            COUNTING.with(|on| { if on.get() { ALLOCATIONS.with(|n| n.set(n.get() + 1)); } });
            unsafe { System.realloc(ptr, layout, size) }
        }
    }

    fn playback() -> (Producer<Frame>, Playback) {
        let (tx, rx) = RingBuffer::new(QUEUE_FRAMES);
        (tx, Playback { queue: rx, channels: 2, clock: Arc::new(AtomicU64::new(0)),
            epoch: Arc::new(AtomicU64::new(0)), health: Arc::default(), next: 0, waiting: None, queue_frames: QUEUE_FRAMES })
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
        for at in 0..16 { tx.push(frame(at, 0, at as f32, -(at as f32))).ok().unwrap(); }
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
        for at in 0..8 { tx.push(frame(at, 0, at as f32, 0.5)).ok().unwrap(); }
        playback.read(&mut audio);
        assert_eq!(audio, [4.0, 0.5, 5.0, 0.5, 6.0, 0.5, 7.0, 0.5]);
        assert_eq!(playback.clock.load(Ordering::Acquire), 8);
    }

    #[test]
    fn panic_discards_audio_already_waiting_in_the_output_queue() {
        let (mut tx, mut playback) = playback();
        for at in 0..4 { tx.push(frame(at, 0, 0.5, 0.5)).ok().unwrap(); }
        playback.epoch.store(1, Ordering::Release);
        tx.push(frame(4, 1, 0.25, 0.75)).ok().unwrap();
        let mut audio = [1.0; 10];
        playback.read(&mut audio);
        assert_eq!(&audio[..8], &[0.0; 8]);
        assert_eq!(&audio[8..], &[0.25, 0.75]);
    }

    #[test]
    fn a_control_lock_cannot_block_the_callback_or_change_its_audio() {
        let mut session = niminal_daemon::Session::new(48_000.0, niminal_lang::Layout::Stereo).without_limiter();
        session.eval("instr tone(freq: hz) { osc(sine, freq) }\ntone(freq: 440hz) for 1beat", Some("now")).unwrap();
        let mut reference = niminal_daemon::Session::new(48_000.0, niminal_lang::Layout::Stereo).without_limiter();
        reference.eval("instr tone(freq: hz) { osc(sine, freq) }\ntone(freq: 440hz) for 1beat", Some("now")).unwrap();
        let expected = reference.process(QUEUE_FRAMES * 4);
        let daemon = Arc::new(Mutex::new(Daemon::new(session)));
        let (mut playback, worker) = RenderWorker::start(daemon.clone(), 2, QUEUE_FRAMES, Arc::default()).unwrap();
        let guard = lock(&daemon);
        let mut audio = vec![0.0; QUEUE_FRAMES * 2];
        for batch in 0..4 {
            if batch > 0 { thread::sleep(Duration::from_millis(5)); }
            playback.read(&mut audio);
            for (i, pair) in audio.as_chunks::<2>().0.iter().enumerate() {
                let at = batch * QUEUE_FRAMES + i;
                assert_eq!(*pair, [expected[0][at], expected[1][at]]);
            }
        }
        assert_eq!(playback.health.underruns.load(Ordering::Relaxed), 0);
        drop(guard);
        drop(worker);
    }
}
