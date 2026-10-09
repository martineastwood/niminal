mod common;
use common::*;
use niminal_engine::ops::{Curve, Env, Segment};
use niminal_engine::{GraphBuilder, Voice};
use std::sync::Arc;

fn env_graph(env: Env) -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    let e = g.add(env, &[]).unwrap();
    Arc::new(g.build(e))
}

fn ms(n: f32) -> usize {
    (n * SR / 1000.0) as usize
}

#[test]
fn attack_reaches_target_and_holds_at_sustain() {
    // env[0 10ms 1 20ms 0.5 | 30ms 0]
    let g = env_graph(Env::new(
        0.0,
        vec![Segment::linear(0.010, 1.0), Segment::linear(0.020, 0.5), Segment::linear(0.030, 0.0)],
        Some(2),
    ));
    let out = render(&g, ms(100.0), None, 32);
    assert_eq!(out[0], 0.0);
    assert!((out[ms(5.0)] - 0.5).abs() < 0.01, "halfway up the attack");
    assert!((out[ms(10.0)] - 1.0).abs() < 1e-3);
    assert!((out[ms(30.0)] - 0.5).abs() < 1e-3);
    assert!(out[ms(30.0)..].iter().all(|v| (v - 0.5).abs() < 1e-3), "holds while gated");
}

#[test]
fn release_runs_from_current_level_even_mid_attack() {
    let g = env_graph(Env::new(
        0.0,
        vec![Segment::linear(0.100, 1.0), Segment::linear(0.100, 0.0)],
        Some(1),
    ));
    // release 25ms into the 100ms attack, so level is about 0.25
    let out = render(&g, ms(300.0), Some(ms(25.0)), 32);
    let at_release = out[ms(25.0)];
    assert!((at_release - 0.25).abs() < 0.01);
    // never rises past where it was, and ends at zero
    assert!(out[ms(25.0)..].iter().all(|v| *v <= at_release + 1e-4));
    assert!(out[ms(25.0) + ms(100.0) + 10..].iter().all(|v| v.abs() < 1e-4));
    // release takes the full release duration regardless of start level
    assert!(out[ms(25.0) + ms(50.0)] > 0.05);
}

#[test]
fn one_shot_ignores_release_and_voice_finishes_after_note_off() {
    let g = env_graph(Env::new(0.0, vec![Segment::linear(0.010, 1.0), Segment::linear(0.010, 0.0)], None));
    let mut v = Voice::new(g, SR);
    let mut buf = [0.0; 32];
    v.process(&mut buf);
    assert!(!v.is_finished(), "not released yet");
    v.release();
    for _ in 0..40 {
        v.process(&mut buf);
    }
    assert!(v.is_finished());
}

#[test]
fn voice_outlives_note_until_release_tail_ends() {
    let g = env_graph(Env::adsr(0.001, 0.01, 0.5, 0.1));
    let mut v = Voice::new(g, SR);
    let mut buf = [0.0; 32];
    for _ in 0..10 {
        v.process(&mut buf);
    }
    v.release();
    v.process(&mut buf);
    assert!(!v.is_finished(), "release tail still sounding");
    for _ in 0..(ms(100.0) / 32 + 2) {
        v.process(&mut buf);
    }
    assert!(v.is_finished());
}

#[test]
fn exp_decay_is_fast_then_slow_and_log_attack_is_fast_then_slow() {
    let decay = env_graph(Env::new(1.0, vec![Segment::new(0.1, 0.0, Curve::Exp)], None));
    let d = render(&decay, ms(100.0), None, 32);
    assert!(d[ms(50.0)] < 0.1, "exp decay is well down at halfway: {}", d[ms(50.0)]);
    assert!(d.windows(2).all(|w| w[1] <= w[0] + 1e-6), "monotone");

    let attack = env_graph(Env::new(0.0, vec![Segment::new(0.1, 1.0, Curve::Log)], None));
    let a = render(&attack, ms(100.0), None, 32);
    assert!(a[ms(50.0)] > 0.9, "log attack is mostly there at halfway: {}", a[ms(50.0)]);

    let grow = env_graph(Env::new(0.0, vec![Segment::new(0.1, 1.0, Curve::Exp)], None));
    let g = render(&grow, ms(100.0), None, 32);
    assert!(g[ms(50.0)] < 0.1, "rising exp starts slowly");
}

#[test]
fn zero_length_segments_jump() {
    let g = env_graph(Env::new(0.0, vec![Segment::linear(0.0, 0.8), Segment::linear(0.010, 0.0)], None));
    let out = render(&g, 64, None, 32);
    assert!((out[0] - 0.8).abs() < 1e-6);
}

#[test]
fn block_splitting_does_not_change_output() {
    let g = env_graph(Env::adsr(0.003, 0.02, 0.4, 0.05));
    let a = render(&g, 6000, Some(2500), 32);
    let b = render(&g, 6000, Some(2500), 7);
    assert_eq!(a, b);
}
