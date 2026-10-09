mod common;
use common::*;
use niminal_engine::ops::{BinOp, Custom, Fn1, Func, Instr, Kernel, Osc, Wave};
use niminal_engine::{GraphBuilder, Port, Src, Voice};
use std::sync::Arc;

/// y = x * (1 - a) + y * a, with a = 0.9
fn one_pole() -> Arc<Kernel> {
    Arc::new(Kernel {
        name: "one_pole".into(),
        ports: vec![Port::named("x", None)],
        code: vec![
            Instr::Input { dst: 0, port: 0 },
            Instr::LoadState { dst: 1, slot: 0 },
            Instr::Const { dst: 2, value: 0.9 },
            Instr::Const { dst: 3, value: 0.1 },
            Instr::Bin { dst: 4, op: BinOp::Mul, a: 0, b: 3 },
            Instr::Bin { dst: 5, op: BinOp::Mul, a: 1, b: 2 },
            Instr::Bin { dst: 6, op: BinOp::Add, a: 4, b: 5 },
            Instr::StoreState { slot: 0, src: 6 },
        ],
        registers: 7,
        state_init: vec![0.0],
        output: 6,
    })
}

fn graph_with(kernel: Arc<Kernel>) -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    let o = g.add(Osc::new(Wave::Saw), &[("freq", Src::Const(300.0))]).unwrap();
    let f = g.add(Custom::new(kernel), &[("x", o)]).unwrap();
    Arc::new(g.build(f))
}

#[test]
fn state_carries_from_sample_to_sample() {
    let out = render(&graph_with(one_pole()), 2000, None, 32);
    let raw = render(&{
        let mut g = GraphBuilder::new();
        let o = g.add(Osc::new(Wave::Saw), &[("freq", Src::Const(300.0))]).unwrap();
        Arc::new(g.build(o))
    }, 2000, None, 32);

    let mut y = 0.0f64;
    for (i, (got, x)) in out.iter().zip(&raw).enumerate() {
        y = f64::from(*x) * 0.1 + y * 0.9;
        assert!((f64::from(*got) - y).abs() < 1e-6, "sample {i}");
    }
}

#[test]
fn each_voice_and_each_call_site_has_its_own_state() {
    let g = graph_with(one_pole());
    let a = render(&g, 500, None, 32);
    let b = render(&g, 500, None, 32);
    assert_eq!(a, b, "a second voice starts from the initial state");

    // two call sites in one graph do not share state
    let mut gb = GraphBuilder::new();
    let o = gb.add(Osc::new(Wave::Saw), &[("freq", Src::Const(300.0))]).unwrap();
    let k = one_pole();
    let first = gb.add(Custom::new(k.clone()), &[("x", o)]).unwrap();
    let second = gb.add(Custom::new(k), &[("x", first)]).unwrap();
    let twice = render(&Arc::new(gb.build(second)), 500, None, 32);
    assert_ne!(twice, a);
    assert!(twice.iter().all(|s| s.is_finite()));
}

#[test]
fn block_splitting_does_not_change_custom_opcode_output() {
    let g = graph_with(one_pole());
    assert_eq!(render(&g, 3000, None, 32), render(&g, 3000, None, 9));
}

#[test]
fn ports_are_named_like_any_other_opcode() {
    let mut g = GraphBuilder::new();
    let e = g.add(Custom::new(one_pole()), &[]).unwrap_err();
    assert_eq!(e.to_string(), "`one_pole` needs an argument `x`");
    let e = g.add(Custom::new(one_pole()), &[("y", Src::Const(1.0))]).unwrap_err();
    assert_eq!(e.to_string(), "`one_pole` has no argument named `y`");
}

#[test]
fn func_applies_math_per_sample() {
    let mut g = GraphBuilder::new();
    let o = g.add(Osc::new(Wave::Sine), &[("freq", Src::Const(100.0))]).unwrap();
    let t = g.add(Func(Fn1::Tanh), &[("x", o)]).unwrap();
    let out = render(&Arc::new(g.build(t)), 480, None, 32);
    let mut v = Voice::new(
        {
            let mut g = GraphBuilder::new();
            let o = g.add(Osc::new(Wave::Sine), &[("freq", Src::Const(100.0))]).unwrap();
            Arc::new(g.build(o))
        },
        SR,
    );
    let mut raw = vec![0.0; 480];
    for c in raw.chunks_mut(32) {
        v.process(c);
    }
    for (a, b) in out.iter().zip(&raw) {
        assert!((f64::from(*a) - f64::from(*b).tanh()).abs() < 1e-6);
    }
}
