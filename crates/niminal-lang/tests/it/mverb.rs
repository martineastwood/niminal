#![allow(clippy::unwrap_used)]
use niminal_engine::{Buses, Voice, block_size};
use niminal_lang::compile;

fn render(expression: &str, kernel: bool) -> Vec<f32> {
    let body = if kernel {
        format!(
            "fn effect(x: [num]) {{ state unused = 0\n {expression} }}\ninstr a() {{ effect([impulse(0hz), 0]) }}"
        )
    } else {
        format!("instr a() {{ x = [impulse(0hz), 0]\n {expression} }}")
    };
    let source = format!("config {{ channels: stereo }}\n{body}");
    let program = compile(&source).unwrap_or_else(|e| panic!("{e:?}"));
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut block = vec![vec![0.0; block_size()]; 2];
    let mut out = Vec::new();
    for _ in 0..800 {
        voice.process_blocks(&mut block, block_size(), &mut buses);
        for (l, r) in block[0].iter().zip(&block[1]) {
            out.extend([*l, *r]);
        }
    }
    out
}

#[test]
fn stereo_tank_matches_in_graphs_and_kernels_with_moving_controls() {
    let expression = "x.mverb(damp: phasor(0.7hz), density: 0.4, bandwidth: 0.8, decay: 0.7, predelay: (10 + phasor(1hz) * 20).ms, size: 0.6, mix: 0.8, early_mix: phasor(0.3hz))";
    let graph = render(expression, false);
    assert!(graph.iter().all(|x| x.is_finite()));
    assert!(graph.iter().any(|x| x.abs() > 0.01));
    assert_eq!(graph, render(expression, true));
}

#[test]
fn mono_and_separate_right_inputs_work_in_both_compilers() {
    for expression in ["x[0].mverb()", "x[0].mverb(right: x[1])"] {
        let graph = render(expression, false);
        assert!(graph.iter().any(|x| x.abs() > 0.001));
        assert_eq!(graph, render(expression, true));
    }
}

#[test]
fn stereo_dry_mix_preserves_channels() {
    for kernel in [false, true] {
        let out = render("x.mverb(mix: 0)", kernel);
        assert!(out[0] > 0.9);
        assert!(out.iter().skip(1).step_by(2).all(|x| *x == 0.0));
    }
}

#[test]
fn rejects_surround_audio_vector_controls_and_wrong_units() {
    for expression in [
        "[1, 0, 0].mverb()",
        "[1, 0].mverb(right: 0)",
        "1.mverb(decay: [0.3, 0.7])",
        "1.mverb(predelay: 0.1)",
        "1.mverb(size: 10ms)",
        "1.mverb(right: [0, 0])",
    ] {
        for state in ["", "state unused = 0\n"] {
            let source = format!("fn effect() {{ {state}{expression} }}\ninstr a() {{ effect() }}");
            assert!(compile(&source).is_err(), "{source}");
        }
    }
}
