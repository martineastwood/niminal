//! Same-process A/B benchmark against the pre-native shelf. Run with:
//! cargo run --release -p niminal-lang --example bench_blocks -- [blocks] [trials]
use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};
use std::{hint::black_box, path::Path, time::Instant};

const REFERENCE: &str = include_str!("../tests/fixtures/blocks_dsp_reference.nml");
const CASES: &[(&str, &str)] = &[
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
];
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let blocks = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4096);
    let trials = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7usize)
        .max(1);
    println!(
        "48kHz, {} frames/block, {blocks} blocks/trial, {trials} trials; median ns/frame; compilation and voice creation excluded",
        block_size()
    );
    println!("case,reference_ns,native_ns,speedup");
    for (name, body) in CASES {
        let mut voices = Vec::new();
        for prefix in [REFERENCE, "use std/blocks"] {
            let source = format!("{prefix}\ninstr a() {{ {body} }}");
            let expanded = expand(&source, Path::new(".")).map_err(|e| e.message)?;
            let program = compile(&expanded.text).map_err(|errors| {
                errors
                    .iter()
                    .map(|e| e.render(name, &expanded.text))
                    .collect::<String>()
            })?;
            voices.push(Voice::new(program.instruments[0].graph.clone(), 48000.0));
        }
        let mut out = vec![0.0; block_size()];
        for voice in &mut voices {
            for _ in 0..256 {
                voice.process(&mut out);
            }
        }
        let mut times = [Vec::new(), Vec::new()];
        for trial in 0..trials {
            // Alternate order to reduce warmup/thermal bias.
            for side in [trial % 2, 1 - trial % 2] {
                let start = Instant::now();
                for _ in 0..blocks {
                    voices[side].process(black_box(&mut out));
                }
                times[side]
                    .push(start.elapsed().as_secs_f64() * 1e9 / (blocks * block_size()) as f64);
                black_box(&out);
            }
        }
        for t in &mut times {
            t.sort_by(f64::total_cmp);
        }
        let old = times[0][trials / 2];
        let new = times[1][trials / 2];
        println!("{name},{old:.1},{new:.1},{:.2}x", old / new);
    }
    Ok(())
}
