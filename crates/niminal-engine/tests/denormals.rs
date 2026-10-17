//! Effects that feed back on themselves must settle to exact silence, never lingering in the range of
//! numbers too small to be normal. These render through a bare `Voice`, which has no hardware flush-to-zero
//! (the mixer's guard), so they hold on any machine and in a browser too.

mod common;
use common::*;
use niminal_engine::ops::{Delay, FilterMode, Reverb, Svf};
use niminal_engine::{Graph, GraphBuilder, Opcode, Port, ProcessCtx, Src};
use std::sync::Arc;

/// Emits 1.0 on the first sample, then silence.
#[derive(Clone)]
struct Impulse(bool);

impl Opcode for Impulse {
    fn name(&self) -> &str {
        "impulse"
    }
    fn ports(&self) -> &[Port] {
        &[]
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Impulse(false))
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], out: &mut [f32]) {
        for o in out {
            *o = if self.0 { 0.0 } else { 1.0 };
            self.0 = true;
        }
    }
}

fn after_an_impulse(effect: impl Opcode, ports: &[(&'static str, Src)]) -> Arc<Graph> {
    let mut g = GraphBuilder::new();
    let impulse = g.add(Impulse(false), &[]).unwrap();
    let mut wired = vec![("x", impulse)];
    wired.extend_from_slice(ports);
    let out = g.add(effect, &wired).unwrap();
    Arc::new(g.build(out))
}

/// A minute of what an effect does after one impulse: it must ring, and then be exactly silent without
/// passing through numbers too small to be normal.
fn rings_out_cleanly(name: &str, graph: Arc<Graph>) {
    let out = render(&graph, 60 * SR as usize, None, 32);
    assert!(out.iter().any(|s| s.abs() > 1e-3), "{name} rang at all");
    if let Some(at) = out.iter().position(|s| s.is_subnormal()) {
        panic!("{name} was a number too small to be normal at sample {at}: {:e}", out[at]);
    }
    let last_second = &out[out.len() - SR as usize..];
    assert!(last_second.iter().all(|s| *s == 0.0), "{name} was still not silent after a minute");
}

#[test]
fn a_reverb_settles_to_silence() {
    let reverb = after_an_impulse(Reverb::new(), &[("room", Src::Const(0.5)), ("damp", Src::Const(0.5))]);
    rings_out_cleanly("reverb", reverb);
}

#[test]
fn a_resonant_filter_settles_to_silence() {
    for mode in [FilterMode::Low, FilterMode::High, FilterMode::Band, FilterMode::Notch] {
        let filter = after_an_impulse(Svf::new(mode), &[("cutoff", Src::Const(200.0)), ("res", Src::Const(1.0))]);
        rings_out_cleanly(&format!("{mode:?} filter"), filter);
    }
}

#[test]
fn a_feedback_delay_settles_to_silence() {
    let delay = after_an_impulse(Delay::new(1.0), &[("time", Src::Const(0.001)), ("feedback", Src::Const(0.995))]);
    rings_out_cleanly("delay", delay);
}
