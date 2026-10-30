use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

fn render(expression: &str, kernel: bool) -> Vec<f32> {
    let source = if kernel {
        format!(
            "fn cloud(src: [num]) {{ state unused = 0\n {expression} }}\ninstr a() {{ cloud(harmonics([1, 0.3], size: 256)) }}"
        )
    } else {
        format!("instr a() {{ src = harmonics([1, 0.3], size: 256)\n {expression} }}")
    };
    let program = compile(&format!("config {{ channels: stereo }}\n{source}"))
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.0);
    let mut output = Vec::new();
    let mut buses = niminal_engine::Buses::new(&[]);
    let mut block = vec![vec![0.0; block_size()]; 2];
    for _ in 0..100 {
        voice.process_blocks(&mut block, block_size(), &mut buses);
        for (left, right) in block[0].iter().zip(&block[1]) {
            output.extend([*left, *right]);
        }
    }
    output
}

#[test]
fn grain_controls_work_identically_in_graph_and_kernel() {
    let expression = "granular(src, density: 67hz, duration: 71ms, position: phasor(0.7hz), rate: -0.8, spray: 20ms, pitch_scatter: 12, jitter: 0.7, shape: 0.2, sweep: -7, pan: 0.2, spread: 0.8, fm: osc(sine, 133hz), fm_depth: 0.4, source2: src, mix: 0.3, position2: phasor(0.3hz), pitch2: 7, gain: 0.125 + phasor(1hz), probability: 0.6, window: [0, 0.2, 1, 0.6, 0], seed: 42)";
    let graph = render(expression, false);
    assert!(graph.iter().all(|v| v.is_finite()));
    assert!(graph.iter().any(|v| v.abs() > 0.01));
    assert_eq!(graph, render(expression, true));
}

#[test]
fn silent_density_and_external_trigger() {
    assert!(
        render("granular([1], density: 0hz)", false)
            .iter()
            .all(|v| *v == 0.0)
    );
    assert!(
        render("granular([1], density: 0hz, trigger: impulse(10hz))", false)
            .iter()
            .any(|v| *v > 0.5)
    );
}

#[test]
fn rejects_wrong_units_and_nonconstant_seed() {
    for expression in [
        "granular([1], duration: 20)",
        "granular([1], density: 20)",
        "granular([1], spray: 0.1)",
        "granular([1], seed: phasor(1hz))",
        "granular([1], window: [1])",
        "granular([1], fm: 10hz)",
        "granular([1], probability: 50ms)",
        "granular([1], pitch2: 440hz)",
        "granular([1], position2: 10ms)",
        "granular([1], gain: 10hz)",
    ] {
        assert!(
            compile(&format!("instr a() {{ {expression} }}")).is_err(),
            "{expression}"
        );
    }
}

#[test]
fn stereo_source_keeps_both_channels_in_graphs_and_kernels() {
    let path = std::env::temp_dir().join(format!("niminal-granular-{}.wav", std::process::id()));
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 2,
            sample_rate: 24000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..1024 {
        writer.write_sample(0.5_f32).unwrap();
        writer.write_sample(-0.25_f32).unwrap();
    }
    writer.finalize().unwrap();
    for kernel in [false, true] {
        let body = if kernel {
            "fn cloud(src: [[num]]) { state unused = 0\n granular(src) }\ninstr a() { cloud(s.data) }"
        } else {
            "instr a() { granular(s.data) }"
        };
        let source = format!(
            "config {{ channels: stereo }}\ns = sample(\"{}\")\n{body}",
            path.display()
        );
        let program = compile(&source).unwrap_or_else(|e| panic!("{e:?}"));
        let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.0);
        let mut buses = niminal_engine::Buses::new(&[]);
        let mut out = vec![vec![0.0; block_size()]; 2];
        voice.process_blocks(&mut out, block_size(), &mut buses);
        assert!(out[0].iter().any(|x| *x > 0.0));
        for (l, r) in out[0].iter().zip(&out[1]) {
            assert!((l + r * 2.0).abs() < 1e-6);
        }
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn released_cloud_finishes_even_when_its_envelope_is_after_the_reverb() {
    let source = "instr a() { granular([1, -1, 0.5], density: 55hz, duration: 240ms).reverb(room: 0.7) * env[1 | 20ms 0] }";
    let program = compile(source).unwrap();
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.0);
    let mut block = vec![0.0; block_size()];
    for _ in 0..100 {
        voice.process(&mut block);
    }
    voice.release();
    for _ in 0..(48000 * 20 / block_size()) {
        voice.process(&mut block);
        if voice.is_finished() {
            return;
        }
    }
    panic!("a silent released grain cloud kept its reverb and voice alive");
}

#[test]
fn source_window_and_second_source_are_distinct_named_tables() {
    for kernel in [false, true] {
        let out = render(
            "granular([1], source2: [-1], mix: 1, window: [1, 1], density: 0hz, trigger: 1, pan: -1)",
            kernel,
        );
        for frame in out[..512].as_chunks::<2>().0 {
            assert_eq!(*frame, [-1.0, 0.0]);
        }
        assert!(
            render("granular([1], window: [0, 0])", kernel)
                .iter()
                .all(|x| *x == 0.0)
        );
    }
}

#[test]
fn probability_zero_and_captured_decibel_gain_work_in_both_compilers() {
    for kernel in [false, true] {
        assert!(
            render(
                "granular([1], probability: 0, trigger: impulse(400hz))",
                kernel
            )
            .iter()
            .all(|v| *v == 0.0)
        );
        let out = render(
            "granular([1], density: 0hz, trigger: 1, window: [1, 1], pan: -1, gain: -6.0206db)",
            kernel,
        );
        for frame in out[..512].as_chunks::<2>().0 {
            assert!((frame[0] - 0.5).abs() < 1e-6);
            assert_eq!(frame[1], 0.0);
        }
    }
}

#[test]
fn second_source_defaults_to_the_primary_position() {
    for kernel in [false, true] {
        let base =
            "granular(src, source2: src, mix: 0.7, position: phasor(0.7hz), pitch2: -7, seed: 21";
        assert_eq!(
            render(&format!("{base})"), kernel),
            render(&format!("{base}, position2: phasor(0.7hz))"), kernel)
        );
    }
}

#[test]
fn recompiling_a_stateful_function_preserves_its_running_cloud() {
    let source = "fn cloud(src: [num]) { state unused = 0\n granular(src, density: 67hz, duration: 71ms, probability: 0.45, seed: 42) }\ninstr a() { cloud(harmonics([1, 0.3], size: 256)) }";
    let original = compile(source).unwrap();
    let mut old = Voice::new(original.instruments[0].graph.clone(), 48000.0);
    let mut expected = Voice::new(original.instruments[0].graph.clone(), 48000.0);
    let mut a = vec![0.0; block_size()];
    let mut b = a.clone();
    for _ in 0..100 {
        old.process(&mut a);
        expected.process(&mut b);
    }
    let replacement = compile(source).unwrap();
    let mut voice = Voice::new(replacement.instruments[0].graph.clone(), 48000.0);
    voice.carry_state(&mut old);
    for _ in 0..100 {
        voice.process(&mut a);
        expected.process(&mut b);
        assert_eq!(a, b);
    }
}

#[test]
fn parity_controls_and_monitor_records_agree_in_both_compilers() {
    let expression = r#"g = granular(src, source2: src, source3: src, source4: src,
      source_bank: [src, src], select: 0, select2: 1, select3: 0, select4: 1,
      density: 67hz, duration: 71ms, position: phasor(0.7hz), freq: 220hz,
      rate1: 0.9, rate2: -0.8, rate3: 0, rate4: 1.1,
      position2: 0.2, position3: 0.3, position4: 0.4,
      pitch: -5, pitch2: 7, pitch3: -12, pitch4: 12,
      weight1: 0.4, weight2: 0.3, weight3: 0.2, weight4: 0.1,
      spray: 10ms, pitch_scatter: 3, probability: 0.8,
      distribution: -0.7, distribution_table: [0, 0.25, 0.5],
      sustain: 0.25, shape: 0.3, window_amount: 0.8,
      window: [0, 1, 0], attack_window: [0, 1], decay_window: [1, 0],
      pitch_start: -7, pitch_end: 12, sweep_shape: 0.8,
      fm: osc(sine, 133hz), fm_depth: 0.2, fm_window: [0, 1, 0],
      fm_windows: [[0, 1], [1, 0]], fm_window_index: phasor(7hz) * 2,
      train_freq: 333hz, train_gain: 0.08, train_partials: 12, train_chroma: 1.1,
      gain_mask: [0, 2, 1, 0.5, 0], start_mask: [0, 1, 1, 0.5],
      end_mask: [0, 1, 2, 1], fm_mask: [0, 0, 0.4], channel_mask: [0, 1, 0, 0.5],
      mix_mask: [0, 1, 1, 0.2, 0.3, 0.1, 0.1, 0.5, 0.5, 0.3, 0.2, 0.1],
      max_grains: 32, monitor: 1, seed: 42)
    [g.audio[0] + g.sync * 0.125, g.audio[1] + g.phase * 0.125]"#;
    let graph = render(expression, false);
    assert!(graph.iter().all(|x| x.is_finite()));
    assert!(graph.iter().any(|x| x.abs() > 0.01));
    assert_eq!(graph, render(expression, true));
}

#[test]
fn monitor_control_outputs_never_get_mixed_into_audio() {
    for kernel in [false, true] {
        assert_eq!(
            render("granular(src, probability: 0.7, seed: 3)", kernel),
            render(
                "g = granular(src, probability: 0.7, seed: 3, monitor: 1)\ng.audio",
                kernel
            ),
        );
        let indices = render(
            "g = granular([0], density: 1hz, monitor: 1)\n[g.gain_index + g.start_index + g.end_index, g.fm_index + g.channel_index + g.mix_index]",
            kernel,
        );
        assert!(indices.iter().all(|x| *x == 3.0));
    }
}

#[test]
fn a_monitored_clock_synchronizes_another_cloud_in_graphs_and_functions() {
    let expression = "leader = granular([0], density: 137hz, monitor: 1)\ngrain = granular(src, density: 0hz, sync: leader.sync, duration: 11ms)\ngrain";
    assert_eq!(render(expression, false), render(expression, true));
    assert!(render(expression, false).iter().any(|x| x.abs() > 0.01));
}

#[test]
fn multichannel_audio_and_mask_writes_work_inside_stateful_functions() {
    let expression = "g = granular([1], channels: 8, monitor: 1, density: 2hz, duration: 1sec, window: [1, 1], channel: 7.5, gain_mask: [0, 1, 0.5, 1], gain_index: 1)\n[g.audio[0], g.audio[7]]";
    let graph = render(expression, false);
    assert_eq!(graph, render(expression, true));
    assert!(graph.iter().all(|x| *x == 0.5));
}

#[test]
fn inline_table_banks_select_whole_sources() {
    for kernel in [false, true] {
        let expression = "granular([0], source_bank: [[1], [2]], select: 1, density: 0hz, trigger: 1, window: [1, 1], pan: -1)";
        let out = render(expression, kernel);
        for frame in out[..512].as_chunks::<2>().0 {
            assert_eq!(*frame, [2.0, 0.0]);
        }
    }
}

#[test]
fn parity_settings_and_mask_headers_are_checked_when_compiling() {
    for args in [
        "channels: 9",
        "channels: 2.5",
        "max_grains: 0",
        "max_grains: 4097",
        "monitor: 2",
        "max_grains: phasor(1hz)",
        "pan_law: [0, 1]",
        "gain_mask: [0, 4, 1]",
        "mix_mask: [0, 0, 1]",
        "gain_mask: [1, 0, 1, 1]",
        "fm_window: [1]",
        "source3: []",
        "train_freq: 100",
        "sync: 20hz",
    ] {
        assert!(
            compile(&format!("instr a() {{ granular([1], {args}) }}")).is_err(),
            "{args}"
        );
    }
}

#[test]
fn mono_granular_outputs_work_through_the_one_sample_kernel_path() {
    let expression = "g = granular([1], channels: 1, density: 0hz, trigger: 1, duration: 1sec, window: [1, 1])\n[g, g]";
    assert_eq!(render(expression, false), render(expression, true));
    assert!(render(expression, true).iter().all(|x| *x == 1.0));
}
