//! `niminal play FILE`: the engine and the sequencer in this process, playing a file through the sound card and
//! playing it again each time it, or a module it uses, changes.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use clap::Args;
use niminal_host::host::{Host, HostConfig};
use niminal_sequencer::{Begin, InProcess, Live, LiveServer, ParsedEval};
use niminal_sequencer::input::{LiveShared, OscInput};
use niminal_server::Problem;
use niminal_ws::lock;

#[derive(Args)]
pub struct PlayArgs {
    /// The `.nml` file to play. It is evaluated again, with every module it uses, whenever any of them changes.
    file: PathBuf,
    /// Output channels: 1, 2 (stereo), 4, 6 (5.1) or 8 (7.1).
    #[arg(long, default_value_t = 2)]
    channels: u16,
    /// The output device: an id such as `output:1`, or part of its name (`niminal devices` lists them).
    #[arg(long)]
    out_device: Option<String>,
    /// Frames in each sound card callback: smaller is lower latency but less forgiving.
    #[arg(long)]
    buffer_size: Option<u32>,
    /// The session's sample rate; the sound card's own by default.
    #[arg(long)]
    sample_rate: Option<u32>,
    /// Run at real speed with no sound card, throwing the audio away.
    #[arg(long)]
    no_audio: bool,
    /// Take MIDI control changes from every input whose name contains this
    /// (give an empty name for all inputs), for `ctl x = midi.cc(n)`.
    #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
    midi: Option<String>,
    /// Take OSC over UDP on this local port, for `ctl x = osc_in("/address")`.
    #[arg(long, value_name = "PORT")]
    osc_port: Option<u16>,
    /// Send MIDI clock (24 a beat, with start and stop) to the output whose name contains this,
    /// making an output of that name if there is none.
    #[arg(long, value_name = "NAME")]
    clock_out: Option<String>,
    /// Follow the MIDI clock of the input whose name contains this: its tempo, and the bar line at its start.
    #[arg(long, value_name = "NAME")]
    clock_in: Option<String>,
}

pub fn run_play(args: PlayArgs) -> Result<(), String> {
    let PlayArgs { file, channels, out_device, buffer_size, sample_rate, no_audio, midi, osc_port, clock_out, clock_in } = args;
    if !file.is_file() {
        return Err(format!("error: can't read {}", file.display()));
    }
    let mut host = Host::start(HostConfig {
        channels,
        sample_rate,
        buffer_size,
        out_device,
        no_audio,
        log: niminal_host::host::new_session_log(),
        ..Default::default()
    })?;
    let live = Live::new(InProcess::new(host.engine()), host.sample_rate() as f32, host.layout())
        .with_clock_sync(Duration::from_millis(100));
    let live: LiveShared = Arc::new(Mutex::new(LiveServer::new(live)));

    let _midi = midi
        .as_deref()
        .map(|filter| {
            let live = live.clone();
            niminal_sequencer::input::open_midi(filter, move |signal| {
                lock(&live).live_mut().control_input(signal);
            })
        })
        .transpose()?;
    let _osc: Option<OscInput> = osc_port.map(|port| OscInput::start(live.clone(), port)).transpose()?;
    let clock_out = clock_out.as_deref().map(niminal_sequencer::clock::ClockOutput::open).transpose()?;
    if clock_out.is_some() {
        lock(&live).live_mut().set_clock_out(true);
    }
    let _clock_in = clock_in
        .as_deref()
        .map(|filter| {
            lock(&live).live_mut().set_clock_in(true);
            let live = live.clone();
            niminal_sequencer::input::open_clock_in(filter, move |message, arrived| {
                lock(&live).live_mut().midi_clock(message, arrived);
            })
        })
        .transpose()?;

    let mut watched = load(&live, &file, Some("now"));
    println!(
        "playing {} on {} at {} Hz; save it to hear the change, Ctrl+C to stop",
        file.display(),
        host.output_description(),
        host.sample_rate()
    );
    // Events go to the engine every 25ms; the files are looked at a tenth as often, as the engine is looked after.
    let stop = crate::stop_requested()?;
    let mut ticks = 0u32;
    while !stop.load(Ordering::Acquire) {
        {
            let mut server = lock(&live);
            server.advance();
            if let Some(out) = &clock_out {
                niminal_sequencer::clock::pump_clock_out(server.live_mut(), out);
            }
            // what the sequencer has to say (a refused change, a note about a control) is told here
            server.notifications();
            for notice in server.live_mut().take_notices() {
                eprintln!("{notice}");
            }
        }
        ticks = ticks.wrapping_add(1);
        if ticks.is_multiple_of(10) {
            host.tick();
            let modified = newest(&watched.0);
            if modified.is_some() && modified != watched.1 {
                watched = load(&live, &file, None);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // asked to stop: fade out rather than cutting the speakers off mid-wave
    host.fade_out();
    Ok(())
}

/// The latest modification time among `files`.
fn newest(files: &[PathBuf]) -> Option<SystemTime> {
    files.iter().filter_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()).max()
}

/// Evaluate the file, reporting what happened. Returns the files it is made of and when they were last changed,
/// to notice when any changes again.
fn load(live: &LiveShared, path: &Path, quantize: Option<&str>) -> (Vec<PathBuf>, Option<SystemTime>) {
    let mut files = vec![path.to_path_buf()];
    let name = path.display().to_string();
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("error: can't read {name}: {e}");
            let modified = newest(&files);
            return (files, modified);
        }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let parsed = ParsedEval::from_dir(&source, dir, quantize);
    files.extend(parsed.module_files());
    let modified = newest(&files);
    let begun = {
        let mut server = lock(live);
        server.live_mut().set_sample_dir(dir);
        server.live_mut().begin_parsed(parsed)
    };
    let result = match begun {
        Begin::Done(result) => result,
        Begin::Job(job) => {
            let mut done = job.compile();
            loop {
                let retry = {
                    let mut server = lock(live);
                    match server.live().retry_eval(&done) {
                        Some(job) => job,
                        None => break server.live_mut().finish_eval(done),
                    }
                };
                done = retry.compile();
            }
        }
    };
    match result {
        Ok(accepted) => match accepted.id {
            Some(_) => println!("{name}: lands at bar {} beat {:.2}", accepted.position.0, accepted.position.1),
            None => println!("{name}: nothing to do"),
        },
        Err(problems) => eprint!("{}", render_problems(&problems, &source, &name)),
    }
    (files, modified)
}

/// Problems in `source` as text, in the same style as compile errors.
fn render_problems(problems: &[Problem], source: &str, name: &str) -> String {
    problems
        .iter()
        .map(|p| {
            let mut out = format!("error: {}\n", p.message);
            if let (Some(line), Some(column)) = (p.line, p.column) {
                let text = source.lines().nth(line.saturating_sub(1)).unwrap_or("");
                let gutter = line.to_string();
                let pad = " ".repeat(gutter.len());
                out.push_str(&format!(
                    "{pad}--> {name}:{line}:{column}\n{pad} |\n{gutter} | {text}\n{pad} | {}^\n",
                    " ".repeat(column.saturating_sub(1))
                ));
            }
            if let Some(help) = &p.help {
                out.push_str(&format!("  = help: {help}\n"));
            }
            if let Some(key) = &p.in_definition {
                out.push_str(&format!("  = note: this is in the earlier definition `{key}`\n"));
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}
