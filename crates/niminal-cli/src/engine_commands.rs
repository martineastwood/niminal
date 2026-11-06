//! The commands that run and inspect the engine: `niminal server`, `replay`, `devices` and `docs`.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use clap::Args;
use niminal_host::host::{Host, HostConfig};
use niminal_host::render::{DEFAULT_SAMPLE_RATE, write_wav};
use niminal_host::replay::{replay, slice};
use niminal_server::from_lines;
use niminal_ws::{Server, ServerConfig};

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("error: can't read {}: {e}", path.display()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[derive(Args)]
pub struct ReplayArgs {
    /// A log a server recorded with `--log`.
    pub log: PathBuf,
    /// The WAV file to write.
    #[arg(long)]
    pub out: PathBuf,
    /// The channels of the server that recorded the log.
    #[arg(long, default_value_t = 2)]
    pub channels: u16,
    /// Turn off the master limiter.
    #[arg(long)]
    pub no_limiter: bool,
    /// The `--block-size` the server that recorded the log ran with.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_BLOCK)]
    pub block_size: usize,
    /// The `--control-step` the server that recorded the log ran with.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_CONTROL_STEP)]
    pub control_step: usize,
    /// The sample rate of the server that recorded the log.
    #[arg(long, default_value_t = DEFAULT_SAMPLE_RATE)]
    pub sample_rate: u32,
    /// Keep the audio from this many seconds into the session.
    #[arg(long, default_value_t = 0.0)]
    pub from: f64,
    /// Keep the audio up to this many seconds into the session (default: to the end).
    #[arg(long)]
    pub to: Option<f64>,
}

#[derive(Args)]
pub struct ServerArgs {
    /// The generic engine events API port.
    #[arg(long, default_value_t = 7401)]
    pub port: u16,
    /// Output channels: 1, 2 (stereo), 4, 6 (5.1) or 8 (7.1).
    #[arg(long, default_value_t = 2)]
    pub channels: u16,
    #[arg(long)]
    pub sample_rate: Option<u32>,
    /// Samples per DSP block, a power of two from 16 to 1024. Control changes and bus sends
    /// land on block boundaries; smaller blocks are tighter, larger ones cheaper.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_BLOCK)]
    pub block_size: usize,
    /// Samples between the runs of a `control { ... }` scope, from 1 to 1024, whatever the block
    /// size: a smaller step tracks fast modulation more closely and costs more.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_CONTROL_STEP)]
    pub control_step: usize,
    /// Frames in each sound card callback: smaller is lower latency but less forgiving. By default 256,
    /// or what the device allows nearest to it. Not the same as `--block-size`.
    #[arg(long)]
    pub buffer_size: Option<u32>,
    /// The sound card for both output and input, by part of its name. `--out-device` and
    /// `--in-device` choose one side; `niminal devices` lists them.
    #[arg(long)]
    pub device: Option<String>,
    /// The output device: an id such as `output:1`, or part of its name.
    #[arg(long)]
    pub out_device: Option<String>,
    /// The input device: an id such as `input:0`, or part of its name.
    #[arg(long, requires = "audio_input")]
    pub in_device: Option<String>,
    /// Use only the first N channels of the input device.
    #[arg(long, requires = "audio_input")]
    pub input_channels: Option<usize>,
    /// Most notes that may sound at once; past it the oldest are ended early.
    #[arg(long, default_value_t = niminal_server::RealtimeConfig::default().max_voices)]
    pub max_voices: usize,
    /// Say more about what the server is doing: `-v` for the settings it starts with and for clients
    /// coming, going and failing, `-vv` for every request and a status line every few seconds.
    /// None of it runs on the audio thread.
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Run without a sound card, at real speed, throwing the audio away.
    #[arg(long)]
    pub no_audio: bool,
    /// Record everything sent to the session here, to replay later. By default
    /// it goes in `~/.niminal/sessions`.
    #[arg(long)]
    pub log: Option<PathBuf>,
    /// Don't record the session.
    #[arg(long, conflicts_with = "log")]
    pub no_log: bool,
    /// Require clients to give this token when they say hello.
    #[arg(long)]
    pub token: Option<String>,
    /// The address to listen on. Only local addresses are accepted for now.
    #[arg(long, default_value = "127.0.0.1")]
    pub listen: IpAddr,
    /// Listen to the default audio input device, for fx declarations that read `input(1)`.
    #[arg(long)]
    pub audio_input: bool,
    /// Threads that render audio together, counting the render thread itself: 1 renders on one core.
    /// By default the cores there are, up to 4. The sound is the same whatever the number.
    #[arg(long)]
    pub render_threads: Option<usize>,
    /// When to share a node's voices among the render threads: `auto` (when it saves more time than it
    /// costs, as measured on this machine), `always`, `never`, or a number of microseconds of work a
    /// block must have before it is shared. The sound is the same whichever you choose.
    #[arg(long, default_value = "auto")]
    pub render_fork: niminal_server::Policy,
}

pub fn run_replay(args: &ReplayArgs) -> Result<(), String> {
    let (log, out) = (&args.log, &args.out);
    let entries = from_lines(&read(log)?).map_err(|e| format!("error: {}: {e}", log.display()))?;
    let audio = replay(&entries, args.channels, args.sample_rate, !args.no_limiter)?;
    let audio = slice(audio, args.from, args.to, args.sample_rate);
    if audio[0].is_empty() {
        return Err("error: that part of the session made no sound".into());
    }
    write_wav(out, &audio, args.sample_rate).map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!(
        "replayed {} input{}: wrote {} ({:.2}s, {} channel{})",
        entries.len(),
        plural(entries.len()),
        out.display(),
        audio[0].len() as f64 / f64::from(args.sample_rate),
        audio.len(),
        plural(audio.len())
    );
    Ok(())
}

pub fn print_devices() {
    let devices = niminal_host::audio::devices();
    for (heading, key) in [("Outputs", "outputs"), ("Inputs", "inputs")] {
        println!("{heading}:");
        for device in devices[key].as_array().into_iter().flatten() {
            println!(
                "  {:<10} {}",
                device["id"].as_str().unwrap_or("?"),
                device["name"].as_str().unwrap_or("?")
            );
        }
    }
}

pub fn run_server(args: ServerArgs) -> Result<(), String> {
    let ServerArgs {
        port,
        channels,
        sample_rate,
        block_size,
        control_step,
        buffer_size,
        device,
        out_device,
        in_device,
        input_channels,
        max_voices,
        verbose,
        no_audio,
        log,
        token,
        listen,
        audio_input,
        render_threads,
        render_fork,
        ..
    } = args;
    let mut host = Host::start(HostConfig {
        channels,
        sample_rate,
        block_size,
        control_step,
        buffer_size,
        out_device: out_device.or(device.clone()),
        in_device: in_device.or(device),
        input_channels,
        audio_input,
        max_voices,
        render_threads,
        render_fork,
        no_audio,
        verbose,
        log,
    })?;
    let engine_server = Server::start(
        host.engine(),
        ServerConfig {
            listen,
            port,
            token,
        },
    )?;
    println!(
        "niminal engine listening on ws://{listen}:{} ({}, {} Hz, {})",
        engine_server.port(),
        host.layout(),
        host.sample_rate(),
        host.output_description()
    );
    let stop = crate::stop_requested()?;
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(250));
        host.tick();
    }
    // asked to stop: fade out rather than cutting the speakers off mid-wave
    host.fade_out();
    Ok(())
}

/// The built-in opcodes as a docs page, straight from the registry so it can't drift.
pub fn opcode_reference() -> String {
    let mut ops = niminal_lang::builtin_ops();
    ops.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from(
        "---\ntitle: Opcodes\ndescription: Every built-in opcode, generated from the compiler.\n---\n\n\
         Generated by `niminal docs`. Call an opcode as `lpf(x, cutoff: 1khz)` or as a method, `x.lpf(cutoff: 1khz)`, \
         where the receiver is the first argument. Its leading required arguments may be given in order, \
         `x.lpf(1khz)`; the rest are named. A `?` marks an optional argument.\n\n",
    );
    out.push_str(&format!(
        "Waveforms: {}.\n\n",
        niminal_lang::waves()
            .map(|w| format!("`{w}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for op in &ops {
        out.push_str(&format!(
            "## {}\n\n{}\n\n```niminal\n{}\n```\n\n",
            op.name,
            op.doc,
            op.signature()
        ));
    }
    out
}
