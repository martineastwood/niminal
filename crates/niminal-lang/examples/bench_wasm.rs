//! Compare this executable with NIMINAL_JIT=0/1 (the native code generator, on by default) or NIMINAL_WASM=0/1 (`wasm-kernel`).
//! cargo build --release -p niminal-lang --example bench_wasm --features wasm-kernel
use niminal_engine::{Buses, Voice, block_size};
use niminal_lang::{compile, expand};
use std::{hint::black_box, io::Write, path::Path, time::Instant};

const REFERENCE: &str = include_str!("../tests/fixtures/blocks_dsp_reference.nml");
const REEL_REFERENCE: &str = include_str!("../tests/fixtures/splice_reel_reference.nml");
const BLOCKS: &[(&str, &str)] = &[
    (
        "contour",
        "c = contour(trigger: impulse(7hz), rise: 10ms, fall: 80ms)\n c.value + c.eor + c.eoc",
    ),
    ("slew", "slew(osc(square, 3hz), rise: 10ms, fall: 100ms)"),
    (
        "filter steady",
        "f = osc(saw, 220hz).multimode_filter(cutoff: 2khz, res: 0.7, drive: 0.3)\n f.low + f.band + f.high + f.notch",
    ),
    (
        "filter modulated",
        "f = osc(saw, 220hz).multimode_filter(cutoff: 2khz + osc(sine, 3hz) * 1500hz, res: 0.7, drive: 0.3)\n f.low + f.band + f.high + f.notch",
    ),
    (
        "filter stepped",
        "f = osc(saw, 220hz).multimode_filter(cutoff: 2khz + (phasor(3hz) < 0.5) * 1500hz, res: 0.7, drive: 0.3)\n f.low + f.band + f.high + f.notch",
    ),
    (
        "frequency shifter",
        "f = osc(saw, 220hz).freq_shifter(shift: 123hz)\n f.up + f.down",
    ),
    (
        "reverse",
        "osc(saw, 220hz).freeze_reverse(length: 100ms, reverse: 1)",
    ),
    (
        "tap delay 4",
        "t = osc(saw, 220hz).tap_delay(time: 50ms, taps: 4, shifts: [0, 7, 12], feedback: 0.5, filter: lowpass)\n t.left + t.right",
    ),
    (
        "tap delay 16",
        "t = osc(saw, 220hz).tap_delay(time: 50ms, taps: 16, shifts: [0, 7, 12], feedback: 0.5, filter: lowpass)\n t.left + t.right",
    ),
    (
        "splice reel",
        "r = splice_reel(osc(saw, 220hz), record: phasor(8hz) < 0.5, grain_size: 0.7, morph: 0.95, speed: 1.37)\n r.left + r.right",
    ),
];
fn render(voice: &mut Voice, out: &mut [Vec<f32>], buses: &mut Buses) {
    let frames = out[0].len();
    voice.process_blocks(out, frames, buses);
    buses.clock.sample += frames as u64;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let blocks = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4096usize)
        .max(1);
    let trials = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7usize)
        .max(1);
    let mut cases: Vec<(String, String)> = BLOCKS
        .iter()
        .map(|(name, body)| {
            let source = if *name == "splice reel" {
                format!(
                    "{REEL_REFERENCE}\ninstr a() {{ {} }}",
                    body.replacen("splice_reel(", "splice_reel_reference(", 1)
                )
            } else {
                format!("{REFERENCE}\ninstr a() {{ {body} }}")
            };
            (format!("frozen {name}"), source)
        })
        .collect();
    cases.push(("scalar cascade".into(), "fn cascade(x) { state a = 0\na = x * 0.01 + a * 0.99\nfor i in 0..<32 { x = x * 0.98 + a * 0.02 }\nx }\ninstr a() { cascade(osc(saw, 220hz)) }".into()));
    for (name, body) in [
        ("electric piano", "use std/instruments.{electric_piano}"),
        ("macro voice", "use std/instruments.{macro_voice}"),
        ("harmonic drone", "use std/instruments.{harmonic_drone}"),
        ("kick 808", "use std/drums.{kick_808}"),
        (
            "native overdrive path",
            "instr a() { x = osc(saw, 220hz)\noversample(4) { x.distortion(drive: 5.8, tone: 0.5, mix: 1) } }",
        ),
    ] {
        cases.push((name.into(), body.into()));
    }
    let mut audio = std::env::var("NIMINAL_BENCH_AUDIO")
        .ok()
        .map(std::fs::File::create)
        .transpose()?
        .map(std::io::BufWriter::new);
    println!(
        "case,compile_ms,median_ns_frame,p99_ns_block,max_ns_block,audio_hash,first_compile_ms"
    );
    for (name, source) in cases {
        let mut compiled = None;
        let mut compile_times = Vec::with_capacity(3);
        for _ in 0..3 {
            let start = Instant::now();
            let expanded = expand(&source, Path::new(".")).map_err(|e| e.message)?;
            let program = compile(&expanded.text).map_err(|errors| {
                errors
                    .iter()
                    .map(|e| e.render(&name, &expanded.text))
                    .collect::<String>()
            })?;
            compile_times.push(start.elapsed().as_secs_f64() * 1000.0);
            compiled = Some(program);
        }
        let first_compile_ms = compile_times[0];
        compile_times.sort_by(f64::total_cmp);
        let compile_ms = compile_times[1];
        let program = compiled.ok_or("no compiled program")?;
        let preset_name = name.replace(' ', "_");
        let graph = program
            .instruments
            .iter()
            .find(|i| i.name == "a" || i.name == preset_name)
            .ok_or("missing benchmark instrument")?
            .graph
            .clone();
        let mut voice = Voice::new(graph.clone(), 48_000.0);
        voice.set_param_by_name("freq", 220.0);
        voice.start_note();
        let mut out = vec![vec![0.0; block_size()]];
        voice.set_realtime();
        let mut buses = Buses::new(&[]);
        for _ in 0..256 {
            render(&mut voice, &mut out, &mut buses);
        }
        let mut times = Vec::with_capacity(trials);
        for _ in 0..trials {
            let start = Instant::now();
            for _ in 0..blocks {
                render(&mut voice, black_box(&mut out), &mut buses);
            }
            times.push(start.elapsed().as_secs_f64() * 1e9 / (blocks * block_size()) as f64);
            black_box(&out);
        }
        times.sort_by(f64::total_cmp);
        let mut tails = Vec::with_capacity(4096);
        for _ in 0..4096 {
            let start = Instant::now();
            render(&mut voice, black_box(&mut out), &mut buses);
            tails.push(start.elapsed().as_nanos());
        }
        tails.sort_unstable();
        // A fresh voice makes verification independent of benchmark duration and trial count.
        let mut voice = Voice::new(graph, 48_000.0);
        voice.set_param_by_name("freq", 220.0);
        voice.start_note();
        voice.set_realtime();
        let mut buses = Buses::new(&[]);
        let mut hash = 0xcbf29ce484222325u64;
        for _ in 0..4096 {
            render(&mut voice, &mut out, &mut buses);
            for sample in out.iter().flatten() {
                for byte in sample.to_bits().to_le_bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                    if let Some(audio) = &mut audio {
                        audio.write_all(&[byte])?;
                    }
                }
            }
        }
        println!(
            "{name},{compile_ms:.3},{:.1},{},{},{hash:016x},{first_compile_ms:.3}",
            times[trials / 2],
            tails[tails.len() * 99 / 100],
            tails[tails.len() - 1]
        );
    }
    if let Some(audio) = &mut audio {
        audio.flush()?;
    }
    Ok(())
}
