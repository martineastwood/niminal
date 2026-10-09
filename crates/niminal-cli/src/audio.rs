//! Driving a session's clock: from a real audio device, or from a timer when
//! there is none.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use niminal_daemon::Daemon;

/// A shared daemon. Whoever holds the lock may touch the session; the audio
/// callback takes it briefly for each buffer.
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
}

impl Output {
    /// Open the default output device. If `sample_rate` is `None` the device's
    /// own rate is used.
    pub fn open(channels: u16, sample_rate: Option<u32>) -> Result<Output, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("there is no audio output device")?;
        let default = device.default_output_config().map_err(|e| format!("can't query the audio device: {e}"))?;
        let config = cpal::StreamConfig {
            channels,
            sample_rate: sample_rate.unwrap_or_else(|| default.sample_rate()),
            buffer_size: cpal::BufferSize::Default,
        };
        Ok(Output { name: device.to_string(), device, config })
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Start playing the daemon's session. Audio stops when the result is dropped.
    pub fn start(self, daemon: Shared) -> Result<Running, String> {
        let channels = usize::from(self.config.channels);
        let stream = self
            .device
            .build_output_stream::<f32, _, _>(
                self.config,
                move |data: &mut [f32], _| {
                    let frames = data.len() / channels;
                    let mut written = 0;
                    lock(&daemon).session_mut().render(frames, &mut |block, n| {
                        for i in 0..n {
                            for (c, channel) in block.iter().enumerate().take(channels) {
                                data[(written + i) * channels + c] = channel[i];
                            }
                        }
                        written += n;
                    });
                    // a device asking for more than whole frames gets silence for the rest
                    data[written * channels..].fill(0.0);
                },
                |e| eprintln!("audio error: {e}"),
                None,
            )
            .map_err(|e| format!("can't open the audio stream: {e}"))?;
        stream.play().map_err(|e| format!("can't start the audio stream: {e}"))?;
        Ok(Running { _stream: stream })
    }
}

pub struct Running {
    _stream: cpal::Stream,
}

/// A clock for running without a sound card: advances the session at (a
/// multiple of) real time and throws the audio away.
pub struct NullClock {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NullClock {
    pub fn start(daemon: Shared, speed: f64) -> NullClock {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            let started = Instant::now();
            let rate = f64::from(lock(&daemon).session().sample_rate());
            while !flag.load(Ordering::Relaxed) {
                let due = (started.elapsed().as_secs_f64() * speed * rate) as u64;
                {
                    let mut d = lock(&daemon);
                    let now = d.session().clock();
                    if due > now {
                        // in slices, so that a long stall can't make one huge buffer
                        let mut left = (due - now) as usize;
                        while left > 0 {
                            let n = left.min(4096);
                            d.session_mut().render(n, &mut |_, _| {});
                            left -= n;
                        }
                    }
                }
                thread::sleep(Duration::from_millis(2));
            }
        });
        NullClock { stop, thread: Some(thread) }
    }
}

impl Drop for NullClock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
