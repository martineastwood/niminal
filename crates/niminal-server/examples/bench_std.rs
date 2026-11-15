//! Real-time share of every std block and preset: how much of one core each needs to play at 48 kHz.
//!
//! ```text
//! cargo run --release -p niminal-server --example bench_std -- [seconds-per-trial] [trials] [filter]
//! ```
//!
//! Prints `kind,name,ns_per_frame,percent_of_core` sorted slowest first, one line per item. Instruments
//! and drums are a single voice holding a note at A3 with every control at its default; effects are
//! driven through a server session (saw plus noise in, default controls); blocks are called with their
//! defaults on representative inputs. The reel alternates recording and playback. Compilation is excluded. Numbers depend on
//! the machine: compare runs on one machine, and see `docs/kernel-performance.md`.
#[path = "bench_std/support.rs"]
mod support;

use niminal_lang::presets::{PresetKind, all};
use support::{RATE, at_120bpm, block_calls, time_block, time_effect, time_voice};

fn main() -> Result<(), String> {
    let arg = |n: usize| std::env::args().nth(n);
    let seconds: f64 = arg(1).and_then(|s| s.parse().ok()).unwrap_or(0.5);
    if !seconds.is_finite() || seconds < niminal_engine::block_size() as f64 / f64::from(RATE) {
        return Err("seconds per trial must cover at least one audio block".into());
    }
    let trials: usize = arg(2).and_then(|s| s.parse().ok()).unwrap_or(3usize).max(1);
    let filter = arg(3).unwrap_or_default();
    let mut rows: Vec<(String, String, Result<f64, String>)> = Vec::new();
    for p in all().iter().filter(|p| p.id.contains(&filter)) {
        let source = at_120bpm(&format!("{}\n{}", p.helpers, p.source));
        let result = match p.kind {
            PresetKind::Instrument => time_voice(&source, &p.id, seconds, trials),
            PresetKind::Effect => time_effect(&source, &p.id, seconds, trials),
        };
        rows.push((p.kind.keyword().into(), p.id.clone(), result));
    }
    for (name, body) in block_calls()?
        .into_iter()
        .filter(|(n, _)| n.contains(&filter))
    {
        rows.push(("block".into(), name, time_block(&body, seconds, trials)));
    }
    if rows.is_empty() {
        return Err(format!("no workloads matched {filter:?}"));
    }
    let share = |ns: f64| ns * f64::from(RATE) / 1e9 * 100.0;
    rows.sort_by(|a, b| {
        let key = |r: &Result<f64, String>| r.as_ref().copied().unwrap_or(-1.0);
        key(&b.2).total_cmp(&key(&a.2))
    });
    println!("kind,name,ns_per_frame,percent_of_core");
    let mut failed = false;
    for (kind, name, result) in rows {
        match result {
            Ok(ns) => println!("{kind},{name},{ns:.1},{:.2}", share(ns)),
            Err(why) => {
                failed = true;
                eprintln!("{kind} {name} failed\n{why}");
            }
        }
    }
    if failed {
        return Err("one or more workloads failed".into());
    }
    Ok(())
}
