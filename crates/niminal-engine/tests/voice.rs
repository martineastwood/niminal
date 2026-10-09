mod common;
use common::*;
use niminal_engine::ops::{Env, FilterMode, Gain, Mul, Osc, Svf, Wave};
use niminal_engine::{GraphBuilder, Src};
use std::sync::Arc;

/// The spec's first example, wired by hand:
/// osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level
fn saw_lead() -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    let freq = g.param("freq", 220.0).unwrap();
    let amp = g.param("amp", 0.5).unwrap();
    let level = g.add(Env::adsr(0.005, 0.2, 0.6, 0.3), &[]).unwrap();
    let osc = g.add(Osc::new(Wave::Saw), &[("freq", freq)]).unwrap();
    let lpf = g
        .add(Svf::new(FilterMode::Low), &[("x", osc), ("cutoff", Src::Const(2000.0)), ("res", Src::Const(0.2))])
        .unwrap();
    let gained = g.add(Gain, &[("x", lpf), ("gain", amp)]).unwrap();
    let out = g.add(Mul, &[("a", gained), ("b", level)]).unwrap();
    Arc::new(g.build(out))
}

#[test]
fn saw_lead_sounds_then_decays_after_note_off() {
    let g = saw_lead();
    let note_len = SR as usize / 2;
    let out = render(&g, SR as usize, Some(note_len), 32);

    let held = rms(&out[note_len - 4800..note_len]);
    let tail_end = rms(&out[out.len() - 4800..]);
    assert!(held > 0.05, "audible while held: {held}");
    assert!(tail_end < 1e-4, "silent after the release: {tail_end}");
    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
}

#[test]
fn note_off_splitting_is_sample_accurate_regardless_of_block_size() {
    let g = saw_lead();
    let a = render(&g, 30_000, Some(12_345), 32);
    let b = render(&g, 30_000, Some(12_345), 5);
    assert_eq!(a, b);
}
