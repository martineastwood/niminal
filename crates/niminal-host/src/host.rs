//! The engine and the audio it plays through, hosted in this process: the sound card, the render threads, the
//! session and everything that keeps them healthy. `niminal server` puts a WebSocket in front of one; an app
//! that embeds the engine talks to it directly.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use niminal_lang::Layout;
use niminal_server::{EngineServer, Policy, RealtimeConfig, Session, to_lines};

use crate::audio::{Health, Listening, NullClock, Output, Running, Shared, lock};
use crate::osc_out::OscOutput;
use crate::render::DEFAULT_SAMPLE_RATE;

/// How the engine is set up. The default is stereo to the default sound card.
pub struct HostConfig {
    /// Output channels: 1, 2 (stereo), 4, 6 (5.1) or 8 (7.1).
    pub channels: u16,
    /// The session's rate; the sound card's own when `None`.
    pub sample_rate: Option<u32>,
    /// Samples per DSP block.
    pub block_size: usize,
    /// Samples between the runs of a `control { ... }` scope.
    pub control_step: usize,
    /// Frames in each sound card callback.
    pub buffer_size: Option<u32>,
    /// The output device: an id or part of its name.
    pub out_device: Option<String>,
    /// The input device, for `audio_input`.
    pub in_device: Option<String>,
    /// Use only the first N channels of the input device.
    pub input_channels: Option<usize>,
    /// Listen to an audio input device, for fx declarations that read `input(..)`.
    pub audio_input: bool,
    /// Most notes that may sound at once.
    pub max_voices: usize,
    /// Threads that render audio together; by default the cores there are, up to 4.
    pub render_threads: Option<usize>,
    /// When to share a node's voices among the render threads.
    pub render_fork: Policy,
    /// Run at real speed with no sound card, throwing the audio away.
    pub no_audio: bool,
    /// 1 for the settings and what clients do, 2 for a status line every few seconds.
    pub verbose: u8,
    /// Record everything sent to the session here, to replay later.
    pub log: Option<PathBuf>,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            channels: 2,
            sample_rate: None,
            block_size: niminal_engine::DEFAULT_BLOCK,
            control_step: niminal_engine::DEFAULT_CONTROL_STEP,
            buffer_size: None,
            out_device: None,
            in_device: None,
            input_channels: None,
            audio_input: false,
            max_voices: RealtimeConfig::default().max_voices,
            render_threads: None,
            render_fork: Policy::default(),
            no_audio: false,
            verbose: 0,
            log: None,
        }
    }
}

/// A running engine. Keep it alive for as long as there should be sound, and call [`Host::tick`] a few times a
/// second.
pub struct Host {
    engine: Shared,
    layout: Layout,
    rate: u32,
    where_to: String,
    audio: Option<Running>,
    listening: Option<Listening>,
    virtual_clock: Option<NullClock>,
    health: Option<Arc<Health>>,
    verbose: u8,
    log: Option<PathBuf>,
    written: (usize, u64),
    reported: (usize, usize, usize, usize, usize),
    last_reopen: Instant,
    last_report: Instant,
    last_stats: Instant,
    /// Held so that signals the project sends over OSC go out whenever the engine runs.
    #[allow(dead_code)]
    osc_out: OscOutput,
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `~/.niminal`, where sessions are logged and crashes recorded.
fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let dir = PathBuf::from(home).join(".niminal");
    std::fs::create_dir_all(dir.join("sessions")).ok()?;
    Some(dir)
}

/// The newest session logs are kept, the rest removed.
fn prune_sessions(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    logs.sort();
    for old in logs.iter().take(logs.len().saturating_sub(keep)) {
        let _ = std::fs::remove_file(old);
    }
}

/// A new file in `~/.niminal/sessions` to record this session in, after removing all but the newest few.
pub fn new_session_log() -> Option<PathBuf> {
    let dir = state_dir()?.join("sessions");
    prune_sessions(&dir, 20);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Some(dir.join(format!("{now}.log")))
}

impl Host {
    pub fn start(config: HostConfig) -> Result<Host, String> {
        let HostConfig {
            channels,
            sample_rate,
            block_size,
            control_step,
            buffer_size,
            out_device,
            in_device,
            input_channels,
            audio_input,
            max_voices,
            render_threads,
            render_fork,
            no_audio,
            verbose,
            log,
        } = config;
        niminal_engine::set_block_size(block_size)?;
        niminal_engine::set_control_step(control_step)?;
        if max_voices == 0 {
            return Err("error: --max-voices must be at least 1".into());
        }
        let realtime = RealtimeConfig {
            max_voices,
            ..Default::default()
        };
        let threads = render_threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(4)))
            .max(1);
        let fork: Option<Arc<dyn niminal_engine::Fork>> = (threads > 1).then(|| {
            Arc::new(niminal_server::Pool::new(
                threads - 1,
                render_fork,
                crate::audio::prefer_low_latency_scheduling,
            )) as Arc<dyn niminal_engine::Fork>
        });
        let layout = Layout::standard(channels)?;
        let output = if no_audio {
            None
        } else {
            Some(Output::open_selected(
                channels,
                sample_rate,
                out_device.as_deref(),
                buffer_size,
            )?)
        };
        let rate = output
            .as_ref()
            .map(Output::sample_rate)
            .or(sample_rate)
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        let where_to = output.as_ref().map_or_else(
            || "no audio output".to_string(),
            |o| format!("audio to {}", o.name()),
        );
        let mut session = Session::new(rate as f32, layout);
        session.set_fork(fork.clone());
        let engine: Shared = Arc::new(Mutex::new(EngineServer::new(session)));
        let mut host = Host {
            osc_out: OscOutput::start(engine.clone())?,
            engine,
            layout,
            rate,
            where_to,
            audio: None,
            listening: None,
            virtual_clock: None,
            health: output.as_ref().map(Output::health),
            verbose,
            log,
            written: (0, 0),
            reported: (0, 0, 0, 0, 0),
            last_reopen: Instant::now(),
            last_report: Instant::now(),
            last_stats: Instant::now(),
        };
        // lines for `-v` go through a thread of their own, so a slow terminal never holds up the engine
        if verbose > 0 {
            let (lines, printed) = std::sync::mpsc::channel::<String>();
            std::thread::spawn(move || {
                for line in printed {
                    eprintln!("{line}");
                }
            });
            lock(&host.engine).set_trace(verbose, lines);
            eprintln!(
                "settings: {rate} Hz, block {} samples, device buffer {}, up to {max_voices} voices, {threads} render thread{}",
                niminal_engine::block_size(),
                output
                    .as_ref()
                    .and_then(Output::buffer_frames)
                    .map_or("default".to_string(), |f| format!("{f} frames")),
                plural(threads),
            );
        }
        // the default input device, if asked for, feeds fx declarations that read `input(..)`
        let mut input = None;
        if audio_input {
            if no_audio {
                return Err("error: --audio-input needs audio output; leave out --no-audio".into());
            }
            let (device, ring) = Listening::open_selected(
                rate,
                usize::try_from(rate).unwrap_or(48_000) / 4,
                host.health.clone().unwrap_or_default(),
                in_device.as_deref(),
                input_channels,
            )
            .map_err(|e| format!("error: {e}"))?;
            println!(
                "audio input from {} ({} channel{})",
                device.name(),
                device.channels(),
                plural(device.channels())
            );
            host.listening = Some(device);
            input = Some(ring);
        }
        match output {
            Some(o) => host.audio = Some(o.start(host.engine.clone(), input, realtime)?),
            None => {
                host.virtual_clock = Some(NullClock::start(host.engine.clone(), 1.0, realtime));
            }
        }
        Ok(host)
    }

    /// The engine, to connect clients to.
    pub fn engine(&self) -> Shared {
        self.engine.clone()
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Where the audio goes, in words.
    /// Fade the sound card's output to silence. Call it before the program quits; it takes about 100 ms.
    pub fn fade_out(&self) {
        if let Some(audio) = &self.audio {
            audio.fade_out();
        }
    }

    pub fn output_description(&self) -> &str {
        &self.where_to
    }

    /// Look after the engine: look for a sound card that went away, report what has gone wrong, and write the
    /// session log. Call it every quarter of a second or so.
    pub fn tick(&mut self) {
        let _ = (&self.listening, &self.virtual_clock);
        // the output device went away or failed: look for it again every couple of seconds
        if let Some(running) = &mut self.audio
            && running.device_lost()
            && self.last_reopen.elapsed() >= Duration::from_secs(2)
        {
            self.last_reopen = Instant::now();
            match running.reopen() {
                Ok(name) => eprintln!("audio: back on {name}"),
                Err(e) => eprintln!("audio: the output device is gone ({e}); trying again"),
            }
        }
        self.report_health();
        if self.verbose >= 2 && self.last_stats.elapsed() >= Duration::from_secs(5) {
            self.last_stats = Instant::now();
            let (voices, metrics) = {
                let e = lock(&self.engine);
                (e.session().state().voices, e.session().realtime_metrics())
            };
            let underruns = self.health.as_ref().map_or(0, |h| {
                h.underruns.load(std::sync::atomic::Ordering::Relaxed)
            });
            match metrics {
                Some(m) => eprintln!(
                    "status: {voices} voices, slowest render {} us, {} late events, {} starved frames, {} deadline misses, {underruns} underruns",
                    m.max_render_micros, m.late_events, m.starved_frames, m.deadline_misses
                ),
                None => eprintln!("status: {voices} voices"),
            }
        }
        if let Some(path) = &self.log {
            // format under the lock, write to disk without it
            let text = {
                let mut e = lock(&self.engine);
                e.session_mut().sync_realtime();
                let entries = e.session().log();
                let version = e.session().log_version();
                (version != self.written).then(|| {
                    self.written = version;
                    to_lines(entries)
                })
            };
            if let Some(text) = text {
                let _ = std::fs::write(path, text);
            }
        }
    }

    fn report_health(&mut self) {
        let Some(h) = &self.health else { return };
        if self.last_report.elapsed() < Duration::from_secs(2) {
            return;
        }
        use std::sync::atomic::Ordering::Relaxed;
        let now = (
            h.panics.load(Relaxed),
            h.errors.load(Relaxed),
            h.underruns.load(Relaxed),
            h.planner_panics.load(Relaxed),
            h.input_errors.load(Relaxed),
        );
        let before = self.reported;
        if now.0 > before.0 {
            eprintln!(
                "audio: the engine failed on {} buffer(s) and was silenced for them; see the crash log",
                now.0 - before.0
            );
        }
        if now.1 > before.1 {
            eprintln!(
                "audio: {} device problem(s); check the output device",
                now.1 - before.1
            );
        }
        if now.3 > before.3 {
            eprintln!(
                "audio: event preparation failed {} time(s) and was restarted; see the crash log",
                now.3 - before.3
            );
        }
        if now.4 > before.4 {
            eprintln!("audio: {} input device problem(s)", now.4 - before.4);
        }
        if now.2 > before.2 {
            eprintln!(
                "audio: {} output buffer(s) ran dry and were silenced",
                now.2 - before.2
            );
        }
        if now != before {
            self.reported = now;
            self.last_report = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use niminal_server::{EngineReply, EngineRequest, PROTOCOL_VERSION};

    #[test]
    fn an_engine_hosted_without_a_sound_card_answers_a_client_in_the_same_process() {
        let mut host = Host::start(HostConfig {
            no_audio: true,
            ..Default::default()
        })
        .expect("the engine starts without a sound card");
        assert_eq!(host.sample_rate(), 48_000);
        assert_eq!(host.output_description(), "no audio output");
        let engine = host.engine();
        let client = lock(&engine).connect_local();
        let hello = EngineRequest::Hello {
            protocol: PROTOCOL_VERSION,
            client: Some("test".into()),
            token: None,
        };
        let reply = lock(&engine).request(client, hello).expect("hello");
        assert!(matches!(reply, EngineReply::Hello { sample_rate, channels, .. } if sample_rate == 48_000.0 && channels == 2));
        // the engine's own clock runs and ticking looks after it without a sound card
        std::thread::sleep(Duration::from_millis(300));
        host.tick();
        lock(&engine)
            .request(client, EngineRequest::Status {})
            .expect("status");
    }
}
