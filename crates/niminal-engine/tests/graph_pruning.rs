use niminal_engine::ops::{Custom, Instr, Kernel, Mul, Osc, Tap, TapOut, Wave};
use niminal_engine::{Buses, GraphBuilder, Src, Voice};
use std::sync::Arc;

#[test]
fn unused_nodes_are_removed_and_multi_output_slots_are_remapped() {
    let mut builder = GraphBuilder::new();
    builder
        .add(Osc::new(Wave::Saw), &[("freq", Src::Const(123.0))])
        .unwrap();
    let outputs = builder
        .add_outputs(
            Custom::new(Arc::new(Kernel {
                name: "pair".into(),
                init: vec![
                    Instr::Const { dst: 0, value: 2.0 },
                    Instr::Const { dst: 1, value: 3.0 },
                ],
                registers: 2,
                outputs: vec![("a".into(), 0), ("b".into(), 1)],
                ..Default::default()
            })),
            &[],
        )
        .unwrap();
    let result = builder
        .add(Mul, &[("a", outputs[1]), ("b", Src::Const(4.0))])
        .unwrap();
    let graph = Arc::new(builder.build(result));
    assert_eq!(graph.node_count(), 2);
    let mut voice = Voice::new(graph, 48_000.0);
    let mut out = [0.0; 32];
    voice.process(&mut out);
    assert_eq!(out, [12.0; 32]);
}

#[test]
fn sends_are_roots_even_when_the_audio_output_is_constant() {
    let mut builder = GraphBuilder::new();
    builder
        .add(Osc::new(Wave::Saw), &[("freq", Src::Const(123.0))])
        .unwrap();
    let send = builder
        .add(Mul, &[("a", Src::Const(2.0)), ("b", Src::Const(3.0))])
        .unwrap();
    builder.send(0, send);
    let graph = Arc::new(builder.build(Src::Const(1.0)));
    assert_eq!(graph.node_count(), 1);
    let mut voice = Voice::new(graph, 48_000.0);
    let mut buses = Buses::new(&[1]);
    let mut out = [0.0; 32];
    voice.process_routed(&mut out, &mut buses);
    assert_eq!(out, [1.0; 32]);
    assert_eq!(buses.get(0, 0, 32), &[6.0; 32]);
}

#[test]
fn osc_taps_are_kept_without_an_audio_consumer() {
    let tap = Tap::new();
    let mut builder = GraphBuilder::new();
    builder
        .add(TapOut::new(tap.clone()), &[("x", Src::Const(0.25))])
        .unwrap();
    let graph = Arc::new(builder.build(Src::Const(0.0)));
    assert_eq!(graph.node_count(), 1);
    let mut voice = Voice::new(graph, 48_000.0);
    voice.process(&mut [0.0; 32]);
    assert_eq!(tap.read(), 0.25);
}

#[test]
fn nested_osc_taps_are_observable_even_when_the_kernel_output_is_unused() {
    use niminal_engine::ops::{Native, NativeInput};
    let tap = Tap::new();
    let kernel = Arc::new(Kernel {
        name: "tap".into(),
        code: vec![Instr::Native { dst: 0, index: 0 }],
        registers: 1,
        outputs: vec![("out".into(), 0)],
        natives: vec![Native {
            op: Box::new(TapOut::new(tap.clone())),
            inputs: vec![NativeInput::Const(0.25)],
        }],
        ..Default::default()
    });
    let mut builder = GraphBuilder::new();
    builder.add(Custom::new(kernel), &[]).unwrap();
    let graph = Arc::new(builder.build(Src::Const(0.0)));
    assert_eq!(graph.node_count(), 1);
    let mut voice = Voice::new(graph, 48_000.0);
    voice.process(&mut [0.0; 32]);
    assert_eq!(tap.read(), 0.25);
}
