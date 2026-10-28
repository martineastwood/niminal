use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use clap::{Args, Parser, Subcommand};
use niminal_cli::audio::{NullClock, Output, Shared};
use niminal_cli::render::{DEFAULT_SAMPLE_RATE, write_wav};
use niminal_cli::replay::{replay, slice};
use niminal_lang::Layout;
use niminal_server::{EngineServer, Session, from_lines, to_lines};
use niminal_ws::{Server, ServerConfig};

#[derive(Parser)]
#[command(name = "niminal", version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("NIMINAL_BUILD"), ")"))]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct ReplayArgs {
    /// A log a server recorded with `--log`.
    log: PathBuf,
    /// The WAV file to write.
    #[arg(long)]
    out: PathBuf,
    /// The channels of the server that recorded the log.
    #[arg(long, default_value_t = 2)]
    channels: u16,
    /// Turn off the master limiter.
    #[arg(long)]
    no_limiter: bool,
    /// The `--control-step` the server that recorded the log ran with.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_CONTROL_STEP)]
    control_step: usize,
    /// The sample rate of the server that recorded the log.
    #[arg(long, default_value_t = DEFAULT_SAMPLE_RATE)]
    sample_rate: u32,
    /// Keep the audio from this many seconds into the session.
    #[arg(long, default_value_t = 0.0)]
    from: f64,
    /// Keep the audio up to this many seconds into the session (default: to the end).
    #[arg(long)]
    to: Option<f64>,
}

#[derive(Args)]
struct ServerArgs {
    /// The generic engine events API port.
    #[arg(long, default_value_t = 7401)]
    port: u16,
    /// Output channels: 1, 2 (stereo), 4, 6 (5.1) or 8 (7.1).
    #[arg(long, default_value_t = 2)]
    channels: u16,
    #[arg(long)]
    sample_rate: Option<u32>,
    /// Samples per DSP block, a power of two from 16 to 1024. Control changes and bus sends
    /// land on block boundaries; smaller blocks are tighter, larger ones cheaper.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_BLOCK)]
    block_size: usize,
    /// Samples between the runs of a `control { ... }` scope, from 1 to 1024, whatever the block
    /// size: a smaller step tracks fast modulation more closely and costs more.
    #[arg(long, default_value_t = niminal_engine::DEFAULT_CONTROL_STEP)]
    control_step: usize,
    /// Frames in each sound card callback: smaller is lower latency but less forgiving. By default 256,
    /// or what the device allows nearest to it. Not the same as `--block-size`.
    #[arg(long)]
    buffer_size: Option<u32>,
    /// The sound card for both output and input, by part of its name. `--out-device` and
    /// `--in-device` choose one side; `niminal devices` lists them.
    #[arg(long)]
    device: Option<String>,
    /// The output device: an id such as `output:1`, or part of its name.
    #[arg(long)]
    out_device: Option<String>,
    /// The input device: an id such as `input:0`, or part of its name.
    #[arg(long, requires = "audio_input")]
    in_device: Option<String>,
    /// Use only the first N channels of the input device.
    #[arg(long, requires = "audio_input")]
    input_channels: Option<usize>,
    /// Most notes that may sound at once; past it the oldest are ended early.
    #[arg(long, default_value_t = niminal_server::RealtimeConfig::default().max_voices)]
    max_voices: usize,
    /// Say more about what the server is doing: `-v` for the settings it starts with and for clients
    /// coming, going and failing, `-vv` for every request and a status line every few seconds.
    /// None of it runs on the audio thread.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Run without a sound card, at real speed, throwing the audio away.
    #[arg(long)]
    no_audio: bool,
    /// Record everything sent to the session here, to replay later. By default
    /// it goes in `~/.niminal/sessions`.
    #[arg(long)]
    log: Option<PathBuf>,
    /// Don't record the session.
    #[arg(long, conflicts_with = "log")]
    no_log: bool,
    /// Require clients to give this token when they say hello.
    #[arg(long)]
    token: Option<String>,
    /// The address to listen on. Only local addresses are accepted for now.
    #[arg(long, default_value = "127.0.0.1")]
    listen: IpAddr,
    /// Listen to the default audio input device, for fx declarations that read `input(1)`.
    #[arg(long)]
    audio_input: bool,
    /// Threads that render audio together, counting the render thread itself: 1 renders on one core.
    /// By default the cores there are, up to 4. The sound is the same whatever the number.
    #[arg(long)]
    render_threads: Option<usize>,
    /// When to share a node's voices among the render threads: `auto` (when it saves more time than it
    /// costs, as measured on this machine), `always`, `never`, or a number of microseconds of work a
    /// block must have before it is shared. The sound is the same whichever you choose.
    #[arg(long, default_value = "auto")]
    render_fork: niminal_server::Policy,
}

#[derive(Args)]
struct PresetsArgs {
    /// Print this preset's source instead of the list.
    name: Option<String>,
    /// Only instruments (`instr`) or effects (`fx`).
    #[arg(long, value_parser = ["instr", "fx"])]
    kind: Option<String>,
    /// The whole catalog as JSON.
    #[arg(long, conflicts_with_all = ["name", "kind"])]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Re-render a session to a WAV file from the log a server recorded.
    Replay(ReplayArgs),
    /// Run the audio server: schedule generic events from any client.
    Server(ServerArgs),
    /// Print the built-in opcode reference as Markdown, for the docs site.
    Docs,
    /// List the ready-made instruments and effects, or print one to paste into a file.
    Presets(PresetsArgs),
    /// List the sound cards, with the ids `--out-device` and `--in-device` take.
    Devices,
}

/// `niminal presets`: what comes with niminal, and how to take it.
fn run_presets(args: &PresetsArgs) -> Result<(), String> {
    use niminal_lang::presets;
    if args.json {
        print!("{}", presets::json());
        return Ok(());
    }
    if let Some(name) = &args.name {
        let preset = presets::find(name)
            .ok_or_else(|| format!("error: no preset `{name}`; `niminal presets` lists them"))?;
        println!(
            "// take it by name: use {}.{{{}}}\n",
            preset.shelf, preset.id
        );
        if !preset.helpers.is_empty() {
            println!("{}", preset.helpers);
        }
        print!("{}", preset.source);
        return Ok(());
    }
    let mut folder = "";
    let shown: Vec<_> = presets::all()
        .iter()
        .filter(|p| args.kind.as_deref().is_none_or(|k| p.kind.keyword() == k))
        .collect();
    for p in &shown {
        if p.folder != folder {
            folder = &p.folder;
            println!("\n{folder}");
        }
        println!("  {:<16} {}", p.id, p.description);
    }
    if let Some(p) = shown.first() {
        println!(
            "\ntake one by name: use {}.{{{}}}  (niminal presets <name> prints its source)",
            p.shelf, p.id
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Replay(args) => niminal_engine::set_control_step(args.control_step)
            .map_err(|e| format!("error: {e}"))
            .and_then(|()| run_replay(&args)),
        Command::Server(mut args) => {
            install_crash_log();
            if args.log.is_none() && !args.no_log {
                args.log = state_dir().map(|state| {
                    let dir = state.join("sessions");
                    prune_sessions(&dir, 20);
                    let now = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs());
                    dir.join(format!("{now}.log"))
                });
            }
            run_server(args)
        }
        Command::Docs => {
            print!("{}", opcode_reference());
            Ok(())
        }
        Command::Presets(args) => run_presets(&args),
        Command::Devices => {
            print_devices();
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprint!("{message}");
            if !message.ends_with('\n') {
                eprintln!();
            }
            ExitCode::FAILURE
        }
    }
}

/// `~/.niminal`, where sessions are logged and crashes recorded.
fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let dir = PathBuf::from(home).join(".niminal");
    std::fs::create_dir_all(dir.join("sessions")).ok()?;
    Some(dir)
}

/// Record panics in `~/.niminal/crash.log` (as well as printing them) so that a
/// crash during a performance can be looked at afterwards.
fn install_crash_log() {
    let log = state_dir().map(|d| d.join("crash.log"));
    std::panic::set_hook(Box::new(move |info| {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let thread = std::thread::current();
        let report = format!(
            "[{now}] niminal {} panicked in thread `{}`: {info}\n{}\n",
            env!("NIMINAL_BUILD"),
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        eprintln!("niminal: internal error: {info}");
        if let Some(path) = &log
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            use std::io::Write;
            let _ = file.write_all(report.as_bytes());
            eprintln!("niminal: details are in {}", path.display());
        }
    }));
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

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("error: can't read {}: {e}", path.display()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn run_replay(args: &ReplayArgs) -> Result<(), String> {
    let (log, out) = (&args.log, &args.out);
    let entries = from_lines(&read(log)?).map_err(|e| format!("error: {}: {e}", log.display()))?;
    let audio = replay(&entries, args.channels, args.sample_rate, !args.no_limiter)?;
    let audio = slice(audio, args.from, args.to, args.sample_rate);
    if audio[0].is_empty() {
        return Err("error: that part of the session made no sound".into());
    }
    write_wav(out, &audio).map_err(|e| format!("error: can't write {}: {e}", out.display()))?;
    println!(
        "replayed {} input{}: wrote {} ({:.2}s, {} channel{})",
        entries.len(),
        plural(entries.len()),
        out.display(),
        audio[0].len() as f64 / f64::from(DEFAULT_SAMPLE_RATE),
        audio.len(),
        plural(audio.len())
    );
    Ok(())
}

fn print_devices() {
    let devices = niminal_cli::audio::devices();
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

fn run_server(args: ServerArgs) -> Result<(), String> {
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
    niminal_engine::set_block_size(block_size)?;
    niminal_engine::set_control_step(control_step)?;
    if max_voices == 0 {
        return Err("error: --max-voices must be at least 1".into());
    }
    let realtime = niminal_server::RealtimeConfig {
        max_voices,
        ..Default::default()
    };
    let out_device = out_device.or(device.clone());
    let in_device = in_device.or(device);
    let threads = render_threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(4)))
        .max(1);
    let fork: Option<Arc<dyn niminal_engine::Fork>> = (threads > 1).then(|| {
        Arc::new(niminal_server::Pool::new(
            threads - 1,
            render_fork,
            niminal_cli::audio::prefer_low_latency_scheduling,
        )) as Arc<dyn niminal_engine::Fork>
    });
    let new_session = |rate: f32, layout: Layout| {
        let mut session = Session::new(rate, layout);
        session.set_fork(fork.clone());
        session
    };
    let log = log.as_deref();
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

    let engine: Shared = Arc::new(Mutex::new(EngineServer::new(new_session(
        rate as f32,
        layout,
    ))));
    // lines for `-v` go through a thread of their own, so a slow terminal never holds up the engine
    if verbose > 0 {
        let (lines, printed) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in printed {
                eprintln!("{line}");
            }
        });
        niminal_cli::audio::lock(&engine).set_trace(verbose, lines);
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
    let mut health = output.as_ref().map(Output::health);
    let audio_control = niminal_server::AudioControl::default();
    audio_control.publish_devices(niminal_cli::audio::devices());
    audio_control.publish_status(serde_json::json!({"state":if no_audio{"virtual"}else{"running"},"sample_rate":rate,"channels":channels,"output":where_to}));
    niminal_cli::audio::lock(&engine).set_audio_control(audio_control.clone());
    // the default input device, if asked for, feeds fx declarations that read `input(..)`
    let (mut listening, mut input) = (None, None);
    if audio_input {
        if no_audio {
            return Err("error: --audio-input needs audio output; leave out --no-audio".into());
        }
        let (device, ring) = niminal_cli::audio::Listening::open_selected(
            rate,
            usize::try_from(rate).unwrap_or(48_000) / 4,
            health.clone().unwrap_or_default(),
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
        listening = Some(device);
        input = Some(ring);
    }
    let _ = &listening;
    let mut audio = match output {
        Some(o) => Some(o.start(engine.clone(), input, realtime)?),
        None => None,
    };
    let mut virtual_clock = no_audio.then(|| NullClock::start(engine.clone(), 1.0, realtime));
    let engine_server = Server::start(
        engine.clone(),
        ServerConfig {
            listen,
            port,
            token: token.clone(),
        },
    )?;
    println!(
        "niminal engine listening on ws://{listen}:{} ({layout}, {rate} Hz, {where_to})",
        engine_server.port()
    );

    // signals the project sends over OSC (`osc_out`) go out whenever the engine runs
    let _osc_out = niminal_cli::osc_out::OscOutput::start(engine.clone())?;

    let mut written = (0, 0);
    let mut reported = (0, 0, 0, 0, 0);
    let mut last_reopen = std::time::Instant::now();
    let mut last_report = std::time::Instant::now();
    let mut last_stats = std::time::Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        if let Some((id, settings)) = audio_control.take_restart() {
            let prepared = (|| -> Result<Option<Output>, String> {
                if settings.no_audio {
                    if settings.input.is_some() {
                        return Err("audio input needs output".into());
                    }
                    Ok(None)
                } else {
                    if let Some(input) = &settings.input {
                        niminal_cli::audio::validate_input(input, settings.sample_rate)?;
                    }
                    Output::open_selected(
                        settings.channels as u16,
                        Some(settings.sample_rate),
                        settings.output.as_deref(),
                        settings.buffer_size,
                    )
                    .map(Some)
                }
            })();
            if verbose > 0 {
                match &prepared {
                    Ok(_) => eprintln!(
                        "audio: restarting with {}",
                        serde_json::to_string(&settings).unwrap_or_default()
                    ),
                    Err(error) => eprintln!("audio: new settings rejected: {error}"),
                }
            }
            match prepared {
                Err(error)=>audio_control.publish_status(serde_json::json!({"state":"configuration_rejected","id":id,"error":error,"previous_audio_running":audio.is_some()})),
                Ok(output)=>{
                    audio_control.publish_status(serde_json::json!({"state":"restarting","id":id}));
                    audio=None;listening=None;virtual_clock=None;
                    niminal_cli::audio::lock(&engine).reset_session(new_session(settings.sample_rate as f32,Layout::standard(settings.channels as u16).unwrap_or_else(|_|Layout::from_count(settings.channels))));
                    let restarted=(||->Result<(),String>{
                        health=output.as_ref().map(Output::health);
                        let mut input=None;
                        if let Some(device)=&settings.input {let (opened,ring)=niminal_cli::audio::Listening::open_selected(settings.sample_rate,settings.sample_rate as usize/4,health.clone().unwrap_or_default(),Some(device),input_channels)?;listening=Some(opened);input=Some(ring);}
                        if let Some(output)=output{audio=Some(output.start(engine.clone(),input,realtime)?);}else{virtual_clock=Some(NullClock::start(engine.clone(),1.0,realtime));}
                        Ok(())
                    })();
                    audio_control.publish_status(match restarted {Ok(())=>serde_json::json!({"state":if settings.no_audio{"virtual"}else{"running"},"id":id,"settings":settings}),Err(error)=>serde_json::json!({"state":"error","id":id,"error":error})});
                    written=(0,0);reported=(0,0,0,0,0);
                }
            }
            audio_control.publish_devices(niminal_cli::audio::devices());
        }
        let _ = (&listening, &virtual_clock);
        // the output device went away or failed: look for it again every couple of seconds
        if let Some(running) = &mut audio
            && running.device_lost()
            && last_reopen.elapsed() >= Duration::from_secs(2)
        {
            last_reopen = std::time::Instant::now();
            match running.reopen() {
                Ok(name) => eprintln!("audio: back on {name}"),
                Err(e) => eprintln!("audio: the output device is gone ({e}); trying again"),
            }
        }
        if let Some(h) = &health
            && last_report.elapsed() >= Duration::from_secs(2)
        {
            use std::sync::atomic::Ordering::Relaxed;
            let now = (
                h.panics.load(Relaxed),
                h.errors.load(Relaxed),
                h.underruns.load(Relaxed),
                h.planner_panics.load(Relaxed),
                h.input_errors.load(Relaxed),
            );
            if now.0 > reported.0 {
                eprintln!(
                    "audio: the engine failed on {} buffer(s) and was silenced for them; see the crash log",
                    now.0 - reported.0
                );
            }
            if now.1 > reported.1 {
                eprintln!(
                    "audio: {} device problem(s); check the output device",
                    now.1 - reported.1
                );
            }
            if now.3 > reported.3 {
                eprintln!(
                    "audio: event preparation failed {} time(s) and was restarted; see the crash log",
                    now.3 - reported.3
                );
            }
            if now.4 > reported.4 {
                eprintln!("audio: {} input device problem(s)", now.4 - reported.4);
            }
            if now.2 > reported.2 {
                eprintln!(
                    "audio: {} output buffer(s) ran dry and were silenced",
                    now.2 - reported.2
                );
            }
            if now != reported {
                reported = now;
                last_report = std::time::Instant::now();
            }
        }
        if verbose >= 2 && last_stats.elapsed() >= Duration::from_secs(5) {
            last_stats = std::time::Instant::now();
            let (voices, metrics) = {
                let e = niminal_cli::audio::lock(&engine);
                (e.session().state().voices, e.session().realtime_metrics())
            };
            let underruns = health.as_ref().map_or(0, |h| {
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
        if let Some(path) = log {
            // format under the lock, write to disk without it
            let text = {
                let mut e = niminal_cli::audio::lock(&engine);
                e.session_mut().sync_realtime();
                let entries = e.session().log();
                let version = e.session().log_version();
                (version != written).then(|| {
                    written = version;
                    to_lines(entries)
                })
            };
            if let Some(text) = text {
                let _ = std::fs::write(path, text);
            }
        }
    }
}

/// The built-in opcodes as a docs page, straight from the registry so it can't drift.
fn opcode_reference() -> String {
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
