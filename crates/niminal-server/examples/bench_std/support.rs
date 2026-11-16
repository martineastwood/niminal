use niminal_engine::{Voice, block_size};
use niminal_lang::ast::{FnResult, Item};
use niminal_lang::parse_composition;
use niminal_lang::{Layout, compile, expand};
use niminal_score::Time;
use niminal_server::{At, Op, Session};
use std::{hint::black_box, path::Path, time::Instant};

pub const RATE: f32 = 48_000.0;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

pub fn at_120bpm(text: &str) -> String {
    text.replace("3/4beat", "375ms")
        .replace("1/2beat", "250ms")
        .replace("1/4beat", "125ms")
        .replace("1bar", "2sec")
}

/// ns per frame of a voice playing a held note.
pub fn time_voice(source: &str, name: &str, seconds: f64, trials: usize) -> Result<f64, String> {
    let expanded = expand(source, Path::new(".")).map_err(|e| e.message)?;
    let program = compile(&expanded.text).map_err(|errors| {
        errors
            .iter()
            .map(|e| e.render(name, &expanded.text))
            .collect::<String>()
    })?;
    let instrument = program
        .instruments
        .iter()
        .find(|i| i.name == name)
        .ok_or("instrument not found")?;
    let mut voice = Voice::new(instrument.graph.clone(), RATE);
    voice.set_param_by_name("freq", 220.0);
    voice.start_note();
    let mut out = vec![0.0; block_size()];
    let blocks = (seconds * f64::from(RATE)) as usize / block_size();
    for _ in 0..blocks / 2 {
        voice.process(&mut out);
    }
    let times = (0..trials)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..blocks {
                voice.process(black_box(&mut out));
            }
            black_box(&out);
            start.elapsed().as_secs_f64() * 1e9 / (blocks * block_size()) as f64
        })
        .collect();
    Ok(median(times))
}

/// ns per frame of an effect, fed by a saw with noise through a session.
pub fn time_effect(
    helpers_and_source: &str,
    name: &str,
    seconds: f64,
    trials: usize,
) -> Result<f64, String> {
    let source = format!(
        "{helpers_and_source}\ninstr src() {{ osc(saw, 220hz) * 0.4 + noise() * 0.2 }}\nnode n {{ instrument = src\n output = {name} }}"
    );
    let mut session = Session::new(RATE, Layout::Mono);
    let events = vec![
        Op::Define {
            source,
            at: At::Now,
            dir: None,
            tag: None,
        },
        Op::Note {
            target: "n".into(),
            to: None,
            at: At::Now,
            args: Default::default(),
            dur: Some(Time::Seconds(100_000.0)),
            id: None,
            tag: None,
        },
    ];
    session
        .send_events(1, events)
        .map_err(|e| format!("{e:?}"))?;
    let frames = (seconds * f64::from(RATE)) as usize;
    let mut sink = |audio: &[Vec<f32>], n| {
        black_box((audio, n));
    };
    session.render(frames / 2, &mut sink);
    let times = (0..trials)
        .map(|_| {
            let start = Instant::now();
            session.render(frames, &mut sink);
            start.elapsed().as_secs_f64() * 1e9 / frames as f64
        })
        .collect();
    Ok(median(times))
}

/// Render a repeating record-then-play sequence to exercise the reel's grain readers.
const REEL: &str = "r = splice_reel(osc(saw, 220hz), record: phasor(8hz) < 0.5)
 r.left + r.right";

pub fn time_block(body: &str, seconds: f64, trials: usize) -> Result<f64, String> {
    let source = format!("use std/sources\nuse std/modulation\nuse std/gates\nuse std/random\nuse std/sequencing\nuse std/filters\nuse std/shaping\nuse std/granular\nuse std/delays\nuse std/physical\nuse std/spectral\ninstr a(freq: hz = 220hz) {{ {body} }}");
    time_voice(&source, "a", seconds, trials)
}

/// Supply representative inputs for the block parameters that have no defaults.
pub fn block_calls() -> Result<Vec<(String, String)>, String> {
    let text = [
        include_str!("../../../niminal-lang/std/sources.nml"),
        include_str!("../../../niminal-lang/std/modulation.nml"),
        include_str!("../../../niminal-lang/std/gates.nml"),
        include_str!("../../../niminal-lang/std/random.nml"),
        include_str!("../../../niminal-lang/std/sequencing.nml"),
        include_str!("../../../niminal-lang/std/filters.nml"),
        include_str!("../../../niminal-lang/std/shaping.nml"),
        include_str!("../../../niminal-lang/std/granular.nml"),
        include_str!("../../../niminal-lang/std/delays.nml"),
        include_str!("../../../niminal-lang/std/physical.nml"),
        include_str!("../../../niminal-lang/std/spectral.nml"),
    ]
    .join("\n")
    .lines()
    .filter(|line| !line.starts_with("use "))
    .collect::<Vec<_>>()
    .join("\n");
    let items = parse_composition(&text).map_err(|e| e.render("the building blocks", &text))?;
    Ok(items
        .into_iter()
        .filter_map(|item| {
            let Item::Fn(def) = item else { return None };
            let name = def.name.name;
            // the frame helpers take arrays of spectrum frames, which a generic call cannot supply
            if matches!(
                name.as_str(),
                "spectral_envelope" | "envelope_transfer" | "phase_frequency" | "phase_accumulate"
            ) {
                return None;
            }
            if name == "splice_reel" {
                return Some((name, REEL.into()));
            }
            let args = def
                .params
                .iter()
                .filter(|p| p.default.is_none())
                .map(|p| {
                    let param = p.name.name.as_str();
                    let value = match param {
                        "count" => "4",
                        "source" if name == "texture" => "harmonics([1, 0.4, 0.2], size: 48000)",
                        "signals" | "source" | "inputs" => {
                            "[osc(saw, 220hz), osc(saw, 330hz), osc(saw, 440hz)]"
                        }
                        "levels" => "[0.5, 0.5, 0.5]",
                        "values" => "[0, 3, 7, 12]",
                        "freq" => "220hz",
                        "clock" | "trigger" | "data" => "impulse(7hz)",
                        "gate" => "osc(square, 3hz)",
                        "note" => "60",
                        "index" => "1",
                        _ => "osc(saw, 220hz)",
                    };
                    format!("{param}: {value}")
                })
                .collect::<Vec<_>>();
            let call = format!("{name}({})", args.join(", "));
            let body = match def.result {
                FnResult::Named(outputs) => {
                    let sum: Vec<_> = outputs
                        .into_iter()
                        .map(|o| format!("r.{}", o.name))
                        .collect();
                    format!("r = {call}\n {}", sum.join(" + "))
                }
                _ => call,
            };
            Some((name, body))
        })
        .collect())
}
