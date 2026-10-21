use niminal_engine::Voice;
use niminal_lang::compile;

fn voice(source: &str, name: &str) -> Voice {
    let program = compile(source).unwrap_or_else(|errors| panic!("{errors:?}"));
    let instrument = program.instruments.iter().find(|i| i.name == name).unwrap();
    Voice::new(instrument.graph.clone(), 48_000.0)
}
fn render(voice: &mut Voice, frames: usize, block: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames];
    for chunk in output.chunks_mut(block) {
        voice.process(chunk);
    }
    output
}

#[test]
fn structural_parameters_specialize_nested_instruments() {
    let source = "
    const wave = [1, 0.5, 0.25]
    fn sine(f: hz) { osc(sine, f) }
    instr base(freq: hz = 440hz, mode: (soft | hard) = soft, partials: const [num] = wave, make: fn = sine) {
        if mode == soft { make(freq) * partials.sum } else { partials[0] }
    }
    instr patch() { base(mode: hard, partials: [0.25, 0.1]) }
    ";
    let p = compile(source).unwrap();
    assert_eq!(
        p.instruments[0]
            .params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["freq", "gain"]
    );
    let out = render(&mut voice(source, "patch"), 128, 32);
    assert!(out.iter().all(|v| (*v - 0.25).abs() < 1e-6));
}

#[test]
fn spawned_layers_overlap_capture_inputs_and_finish() {
    let source = "instr a(trig: num = 0, value: num = 0.25) { spawn(trig, dur: 1ms, max: 4) { value * env[1 | 1ms 0] } }";
    let mut v = voice(source, "a");
    v.set_param(0, 1.0);
    assert_eq!(render(&mut v, 1, 1)[0], 0.25);
    v.set_param(0, 0.0);
    v.set_param(1, 0.5);
    assert_eq!(
        render(&mut v, 1, 1)[0],
        0.25,
        "existing layer keeps its captured input"
    );
    v.set_param(0, 1.0);
    assert_eq!(render(&mut v, 1, 1)[0], 0.75, "layers overlap");
    assert!(
        render(&mut v, 40, 7).iter().all(|x| *x == 0.75),
        "holding a trigger does not spawn repeatedly"
    );
    render(&mut v, 120, 32);
    assert!(render(&mut v, 16, 16).iter().all(|x| *x == 0.0));
    v.release();
    render(&mut v, 512, 32);
    assert!(v.is_finished());
}

#[test]
fn spawned_instrument_has_its_own_time_and_gate() {
    let source = "instr grain() { (note.time < 1ms) * env[1 | 1ms 0] }\ninstr a() { spawn(1, dur: 2ms) { grain() } }";
    let out = render(&mut voice(source, "a"), 200, 17);
    assert!(out[0] > 0.9);
    assert_eq!(out[60], 0.0);
}

#[test]
fn scopes_are_independent_of_render_block_size() {
    for source in [
        "instr a() { spawn(osc(square, 321hz), dur: 10ms, max: 8) { osc(sine, 440hz) * env[1 | 2ms 0] } }",
        "instr a() { oversample(4) { osc(sine, 3khz).tanh } }",
    ] {
        assert_eq!(
            render(&mut voice(source, "a"), 4096, 32),
            render(&mut voice(source, "a"), 4096, 7)
        );
    }
}

#[test]
fn oversampling_uses_the_local_sample_rate() {
    let source = "instr a() { oversample(4) { osc(sine, 440hz) } }";
    let out = render(&mut voice(source, "a"), 48000, 32);
    let crossings = out[1000..]
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    assert!(
        (crossings as i32 - 431).abs() <= 1,
        "pitch stays 440hz: {crossings}"
    );
}

#[test]
fn invalid_scope_settings_are_diagnostics() {
    for source in [
        "instr a() { oversample(3) { 0.5 } }",
        "instr a() { spawn(1, dur: 1ms, max: 0) { 0.5 } }",
        "instr a() { spawn(1, dur: -1ms) { 0.5 } }",
        "instr a() { spawn(1, dur: 1hz) { 0.5 } }",
    ] {
        assert!(compile(source).is_err(), "{source}");
    }
}

fn magnitude(samples: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * hz * i as f64 / 48000.0;
        re += f64::from(x) * phase.cos();
        im += f64::from(x) * phase.sin();
    }
    (re * re + im * im).sqrt() / samples.len() as f64
}

#[test]
fn oversampling_suppresses_folded_distortion_harmonics() {
    let plain = render(
        &mut voice("instr a() { (osc(sine, 9khz) * 4).tanh }", "a"),
        6000,
        32,
    );
    let filtered = render(
        &mut voice(
            "instr a() { oversample(4) { (osc(sine, 9khz) * 4).tanh } }",
            "a",
        ),
        6000,
        32,
    );
    let before = magnitude(&plain[1200..], 21000.0);
    let after = magnitude(&filtered[1200..], 21000.0);
    assert!(
        after < before * 0.1,
        "at least 20db less third-harmonic alias: {before} -> {after}"
    );
    assert!(magnitude(&filtered[1200..], 9000.0) > 0.3);
}

#[test]
fn triggered_instrument_reporting_keeps_repeated_events() {
    let source = "instr a(trigger: num = 0) { (0.5 + trigger * 0).osc_out(\"/hit\", port: 9000, on: trigger) }";
    let p = compile(source).unwrap();
    let mut v = Voice::new(p.instruments[0].graph.clone(), 48000.0);
    let mut cursor = 0;
    assert!(p.taps[0].value.read_events(&mut cursor).is_empty());
    for _ in 0..3 {
        v.set_param(0, 1.0);
        render(&mut v, 1, 1);
        v.set_param(0, 0.0);
        render(&mut v, 1, 1);
    }
    assert_eq!(p.taps[0].value.read_events(&mut cursor), [0.5, 0.5, 0.5]);
    assert!(p.taps[0].value.read_events(&mut cursor).is_empty());
}

#[test]
fn spawned_grains_read_shared_buffers_with_their_own_playheads() {
    let source = "const data = [0.25, 0.5, 0.75, 1]\nbuffer take = data\ninstr grain() { take.at(note.time.num * sample_rate.num) * env[1 | 1ms 0] }\ninstr a() { spawn(1, dur: 1ms) { grain(gain: -6db) } }";
    let out = render(&mut voice(source, "a"), 64, 13);
    assert!((out[0] - 0.25 * 10f32.powf(-6.0 / 20.0)).abs() < 1e-5);
    assert!((out[1] / out[0] - 2.0).abs() < 1e-5);
}

#[test]
fn full_spawn_pools_drop_new_triggers_without_interrupting_layers() {
    let source = "instr a(trigger: num = 0, value: num = 0.25) { spawn(trigger, dur: 1sec, max: 1) { value } }";
    let mut v = voice(source, "a");
    v.set_param(0, 1.0);
    render(&mut v, 1, 1);
    v.set_param(0, 0.0);
    render(&mut v, 1, 1);
    v.set_param(1, 0.75);
    v.set_param(0, 1.0);
    assert_eq!(render(&mut v, 8, 8), [0.25; 8]);
}

#[test]
fn oversampled_init_parameters_keep_exact_constant_input_values() {
    let source = "instr a(value: num = 0.5) { oversample(4) { state held = value\nheld } }";
    let out = render(&mut voice(source, "a"), 128, 13);
    assert!(
        out[64..].iter().all(|x| (*x - 0.5).abs() < 1e-6),
        "{:?}",
        &out[64..72]
    );
}

#[test]
fn instruments_can_bind_shared_buffer_references() {
    let source = "const values = [0.25, 0.5]\nbuffer take = zeros(2)\nbuffer alternate = values\ninstr reader(source: const = take) { source[0] }\ninstr patch() { reader(source: alternate) }";
    assert_eq!(render(&mut voice(source, "patch"), 4, 4), [0.25; 4]);
}

#[test]
fn nested_scopes_support_multichannel_results_and_reset_independently() {
    let source = "instr a() { spawn(osc(square, 100hz), dur: 2ms) { oversample(4) { [osc(sine, 440hz), osc(sine, 660hz)] * env[1 | 1ms 0] } }.sum }";
    let a = render(&mut voice(source, "a"), 2048, 32);
    let b = render(&mut voice(source, "a"), 2048, 7);
    assert_eq!(a, b);
    assert!(a.iter().any(|v| v.abs() > 0.1));
}

#[test]
fn latency_compensation_aligns_parallel_paths_inside_and_outside_scopes() {
    for source in [
        "instr a() { x = osc(sine, 1khz)\nx - oversample(4) { x } }",
        "instr a() { spawn(1, dur: 1sec) { x = osc(sine, 1khz)\nx - oversample(4) { x } } }",
    ] {
        let out = render(&mut voice(source, "a"), 4096, 32);
        let rms = (out[1000..].iter().map(|x| x * x).sum::<f32>() / 3096.0).sqrt();
        assert!(
            rms < 0.005,
            "parallel paths cancel after compensation: {rms}"
        );
    }
}

#[test]
fn buffer_parameters_work_through_stateful_functions_and_nested_scopes() {
    let source = "const data = [0.25, 0.5]\nbuffer one = data\nfn reader(source: buffer) { state pos = 0\nx = source[pos]\npos = pos + 1\nx }\ninstr a(source: buffer = one) { spawn(1, dur: 1sec) { oversample(2) { reader(source) } } }";
    let output = render(&mut voice(source, "a"), 128, 17);
    assert!(output.iter().all(|v| v.is_finite()));
    assert!(output[64..].iter().any(|v| v.abs() > 0.1));
}

#[test]
fn resetting_a_nested_fx_clears_its_compensation_history() {
    let source = "fn pair(trigger) { state keep = 0\nreset_on(trigger) { x = osc(sine, 440hz)\nx + oversample(2) { x } } }\ninstr a(trigger: num = 0) { pair(trigger) }";
    let mut running = voice(source, "a");
    render(&mut running, 100, 25);
    running.set_param(0, 1.0);
    let reset = render(&mut running, 64, 16);
    let mut fresh = voice(source, "a");
    fresh.set_param(0, 1.0);
    assert_eq!(reset, render(&mut fresh, 64, 16));
}

#[test]
fn delayed_nested_inputs_preserve_note_start_initialization() {
    let source = "fn normalized(x, freq: init hz) { state length = sample_rate / freq\nx * length / length }\ninstr a(freq: hz = 440hz) { oversample(2) { x = oversample(2) { osc(sine, freq) }\noversample(2) { normalized(x, freq) } } }";
    let output = render(&mut voice(source, "a"), 1024, 32);
    assert!(output.iter().all(|v| v.is_finite()));
    assert!(
        output[128..].iter().any(|v| v.abs() > 0.5),
        "initialization must not see a zero frequency"
    );
}
