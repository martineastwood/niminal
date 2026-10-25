//! Oscillator controls through both graph and sample-kernel compilation.
use niminal_engine::{Buses, Voice, block_size};
use niminal_lang::compile;

fn render(source: &str, frames: usize) -> Vec<Vec<f32>> {
    let p = compile(source).unwrap_or_else(|errors| panic!("{errors:?}"));
    let mut voice = Voice::new(p.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut result = vec![Vec::new(); p.instruments[0].graph.channels()];
    for start in (0..frames).step_by(block_size()) {
        let n = block_size().min(frames - start);
        let mut out = vec![vec![0.0; block_size()]; result.len()];
        voice.process_blocks(&mut out, n, &mut buses);
        for (result, out) in result.iter_mut().zip(out) {
            result.extend_from_slice(&out[..n]);
        }
    }
    result
}

#[test]
fn all_controls_work_in_graphs_and_stateful_functions() {
    for expression in [
        "osc(pulse, 2317hz, width: 0.23, phase: 0.17, sync: 937hz)",
        "osc(sine, 220hz, phase: 0.25, phase_mod: osc(sine, 7hz) * 0.1)",
        "phasor(-2317hz, phase: 0.17, sync: 937hz)",
    ] {
        let graph = render(&format!("instr tone() {{ {expression} }}"), 4096);
        let kernel = render(
            &format!("fn voice() {{ state unused = 0\n{expression} }}\ninstr tone() {{ voice() }}"),
            4096,
        );
        let worst = graph
            .iter()
            .flatten()
            .zip(kernel.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            worst < 1e-6,
            "{expression}: graph/kernel difference {worst}"
        );
        assert!(graph[0].iter().all(|x| x.is_finite()));
    }
}

#[test]
fn pulse_width_can_be_modulated_at_audio_rate() {
    let source = "instr tone() { osc(pulse, 220hz, width: 0.5 + osc(sine, 0.5hz) * 0.4) }";
    let out = render(source, 48000);
    assert!(out[0].iter().all(|x| x.is_finite() && x.abs() <= 2.0));
    let average =
        |from: usize, to: usize| out[0][from..to].iter().sum::<f32>() / (to - from) as f32;
    assert!(average(18000, 30000) > average(0, 12000) + 0.2);
}

#[test]
fn phase_applies_to_wavetables_and_sync_supports_banks() {
    let out = render(
        "const wave = harmonics([1], size: 64)\ninstr tone() { osc(wave, 0hz, phase: 0.25) }",
        64,
    );
    assert!(out[0].iter().all(|x| (*x - 1.0).abs() < 1e-4));
    let source = "const a = harmonics([1], size: 64)\nconst b = harmonics([1, 0.5], size: 64)\ninstr tone() { osc(bank([a, b]), 337hz, position: 0.4, phase: 0.1, sync: 150hz) }";
    let out = render(source, 4096);
    assert!(out[0].iter().all(|x| x.is_finite()));
    assert!(out[0].iter().any(|x| x.abs() > 0.5));
}

#[test]
fn oscillator_controls_check_units() {
    for (expression, parameter) in [
        ("osc(pulse, 220hz, width: 20ms)", "width"),
        ("osc(saw, 220hz, sync: 1)", "sync"),
        ("osc(sine, 220hz, phase: 90deg)", "phase"),
        ("phasor(220hz, sync: 1)", "sync"),
    ] {
        let src = format!("instr tone() {{ {expression} }}");
        let errors = compile(&src).err().expect("expected an invalid unit");
        assert!(
            errors.iter().any(|e| e.message.contains(parameter)),
            "{errors:?}"
        );
    }
}
