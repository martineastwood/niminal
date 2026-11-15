//! Broad per-voice ceilings for representative std workloads at 48 kHz.
//! Measure on one thread and use medians so a scheduler interruption does not fail the test.
#[path = "../examples/bench_std/support.rs"]
mod support;

use niminal_lang::presets::{PresetKind, all};
use support::{at_120bpm, block_calls, time_block, time_effect, time_voice};

#[test]
fn heavy_std_workloads_stay_within_their_render_budgets() -> Result<(), String> {
    const SECONDS: f64 = 0.2;
    const TRIALS: usize = 5;
    let blocks = block_calls()?;
    let mut failures = Vec::new();
    for (name, budget) in [
        ("splice_reel", 3_000.0),
        ("row_sequencer", 2_500.0),
        ("low_pass_gate", 2_000.0),
        ("tap_delay", 5_000.0),
        ("vocoder", 7_000.0),
        ("multi_voice", 7_000.0),
        ("electric_piano", 8_000.0),
        ("macro_voice", 8_000.0),
        ("harmonic_drone", 7_000.0),
        ("kick_808", 7_000.0),
        ("overdrive", 9_000.0),
    ] {
        let measured = if let Some((_, body)) = blocks.iter().find(|(id, _)| id == name) {
            time_block(body, SECONDS, TRIALS)
        } else {
            let preset = all()
                .iter()
                .find(|p| p.id == name)
                .ok_or_else(|| format!("missing std workload {name}"))?;
            let source = at_120bpm(&format!("{}\n{}", preset.helpers, preset.source));
            match preset.kind {
                PresetKind::Instrument => time_voice(&source, name, SECONDS, TRIALS),
                PresetKind::Effect => time_effect(&source, name, SECONDS, TRIALS),
            }
        }
        .map_err(|error| format!("{name}: {error}"))?;
        eprintln!("{name}: {measured:.1} ns/frame (budget {budget:.0})");
        if measured > budget {
            failures.push(format!("{name}: {measured:.1} ns/frame > {budget:.0}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
