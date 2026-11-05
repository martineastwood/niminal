//! The shelf instruments and drums stay finite whatever their controls are set to,
//! including values far outside the range they declare: a user can type anything, and automation
//! can overshoot.
#![allow(clippy::unwrap_used)]

use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};

fn compiled(shelf: &str) -> niminal_lang::Program {
    let names: Vec<&str> = niminal_lang::presets::of_shelf(shelf)
        .map(|p| p.id.as_str())
        .collect();
    let source = format!("use {shelf}.{{{}}}\n", names.join(", "));
    let expanded = expand(&source, std::path::Path::new(".")).unwrap();
    compile(&expanded.text).unwrap_or_else(|e| panic!("{shelf} does not compile: {e:?}"))
}

/// Plays one note with every control set by `set(min, max)`, and reports anything wrong.
fn problem(
    instrument: &niminal_lang::Instrument,
    note_freq: f32,
    set: &dyn Fn(f64, f64) -> f64,
) -> Option<String> {
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    for param in &instrument.params {
        let (lo, hi) = param.range.unwrap_or((0.0, 1.0));
        voice.set_param_by_name(&param.name, set(lo, hi) as f32);
    }
    voice.set_param_by_name("freq", note_freq);
    voice.start_note();
    let mut out = vec![0.0; 120 * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    if voice.is_poisoned() {
        return Some("produced NaN or infinity".into());
    }
    // loudness is the master limiter's job; what must never happen is a number that isn't one
    (!out.iter().all(|v| v.is_finite())).then(|| "produced NaN or infinity".into())
}

#[test]
fn shelf_instruments_survive_extreme_controls() {
    type Setting = (&'static str, &'static dyn Fn(f64, f64) -> f64);
    let settings: [Setting; 8] = [
        ("minimum", &|lo, _| lo),
        ("maximum", &|_, hi| hi),
        ("zero", &|_, _| 0.0),
        ("past the maximum", &|_, hi| hi * 10.0),
        ("far past the maximum", &|_, hi| hi * 1e6),
        ("below the minimum", &|lo, hi| lo - (hi - lo) * 10.0),
        ("negative", &|_, hi| -hi.abs().max(1.0)),
        ("huge", &|_, _| 1e30),
    ];
    let mut failures = Vec::new();
    for shelf in ["std/instruments", "std/drums"] {
        for instrument in &compiled(shelf).instruments {
            for freq in [0.0, 1.0, 55.0, 880.0, 20_000.0, 1e6] {
                for (label, set) in &settings {
                    if let Some(why) = problem(instrument, freq, set) {
                        failures.push(format!(
                            "{} at {freq} Hz, controls {label}: {why}",
                            instrument.name
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
