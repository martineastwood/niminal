#![allow(clippy::unwrap_used)]
use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};

fn render(expression: &str, kernel: bool) -> Vec<f32> {
    let body = if kernel {
        format!("fn voice() {{ state unused = 0\n{expression} }}\ninstr a() {{voice()}}")
    } else {
        format!("instr a() {{ {expression} }}")
    };
    let program = compile(&body).unwrap_or_else(|e| panic!("{body}: {e:?}"));
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.);
    voice.start_note();
    let mut out = vec![0.; 48000];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    out
}

#[test]
fn physical_sources_match_in_graphs_and_kernels_with_live_controls() {
    for expression in [
        "bowed(220hz, pressure: 0.7, position: phasor(0.4hz), velocity: 0.6, brightness: phasor(0.2hz))",
        "reed(220hz, pressure: 0.65, stiffness: phasor(0.3hz), breath: 0.2, brightness: phasor(0.2hz))",
    ] {
        let graph = render(expression, false);
        assert!(graph.iter().all(|x| x.is_finite() && x.abs() < 2.));
        assert!(graph.iter().any(|x| x.abs() > 0.01));
        assert_eq!(graph, render(expression, true));
    }
}

#[test]
fn physical_frequency_and_controls_require_the_right_units() {
    for expression in [
        "bowed(220)",
        "reed(1sec)",
        "bowed(220hz, position: 10ms)",
        "reed(220hz, pressure: -6db)",
        "reed(220hz, stiffness: 440hz)",
    ] {
        assert!(
            compile(&format!("instr a() {{{expression}}}")).is_err(),
            "{expression}"
        );
    }
}

#[test]
fn physical_presets_have_release_tails_that_finish() {
    let source = "use std/instruments.{bowed_string, reed_wind}";
    let expanded = expand(source, std::path::Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap();
    for instrument in &program.instruments {
        let mut voice = Voice::new(instrument.graph.clone(), 48000.);
        voice.set_param_by_name("freq", 220.);
        voice.start_note();
        let mut block = vec![0.; block_size()];
        for _ in 0..600 {
            voice.process(&mut block);
        }
        assert!(
            block.iter().any(|x| x.abs() > 0.001),
            "{} is silent",
            instrument.name
        );
        voice.release();
        for _ in 0..1000 {
            voice.process(&mut block);
        }
        assert!(
            block.iter().all(|x| x.abs() < 1e-6),
            "{} did not end",
            instrument.name
        );
    }
}

#[test]
fn waveguides_expand_over_frequency_and_control_arrays() {
    for expression in [
        "bowed([220hz,330hz], position: [0.25,0.5])",
        "reed([220hz,330hz], stiffness: [0.4,0.6])",
    ] {
        let mut sounds = Vec::new();
        for state in ["", "state unused = 0\n"] {
            let source = format!(
                "config {{channels: stereo}}\nfn model() {{ {state}{expression} }}\ninstr a() {{model()}}"
            );
            let program = compile(&source).unwrap_or_else(|e| panic!("{source}: {e:?}"));
            let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.);
            let mut buses = niminal_engine::Buses::new(&[]);
            let mut out = vec![vec![0.; block_size()]; 2];
            let mut flat = Vec::new();
            for _ in 0..800 {
                voice.process_blocks(&mut out, block_size(), &mut buses);
                for (&left, &right) in out[0].iter().zip(&out[1]) {
                    flat.extend([left, right]);
                }
            }
            assert!(flat.iter().all(|x| x.is_finite() && x.abs() < 2.));
            assert!(
                flat.as_chunks::<2>()
                    .0
                    .iter()
                    .any(|x| (x[0] - x[1]).abs() > 0.01)
            );
            sounds.push(flat);
        }
        assert_eq!(sounds[0], sounds[1]);
    }
}
