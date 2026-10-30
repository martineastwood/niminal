use super::*;

fn mono(source: Arc<Table>, seed: u32) -> Granular {
    let mut config = GranularConfig {
        seed,
        ..GranularConfig::default()
    };
    config.sources[0] = vec![source];
    Granular::new(config).unwrap()
}
fn source(rate: Option<f32>) -> Arc<Table> {
    Arc::new(Table::from_f64(
        (0..128).map(|i| (i as f64 * 0.1).sin()),
        rate,
    ))
}
fn params(base: [f32; 15]) -> [f32; 19] {
    let mut p = [0.0; 19];
    p[..15].copy_from_slice(&base);
    p[15] = 1.0;
    p[16] = 1.0;
    p[17] = f32::INFINITY;
    p
}

fn controls() -> [f32; 19] {
    params([
        20.0, 0.1, 0.3, 1.0, 0.01, 7.0, 0.6, 0.0, 0.5, -12.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ])
}

#[test]
fn seeded_reset_and_block_processing_are_identical() {
    let mut g = mono(source(None), 7);
    let ctx = ProcessCtx::new(48_000.0, true);
    let p = controls();
    let expected: Vec<_> = (0..10000).map(|_| g.process_one(&ctx, &p)).collect();
    assert!(expected.iter().any(|x| x.abs() > 0.1));
    g.reset();
    let data: Vec<_> = p.iter().map(|v| vec![*v; 10000]).collect();
    let inputs: Vec<_> = data.iter().map(Vec::as_slice).collect();
    let mut actual = vec![0.0; 10000];
    g.process(&ctx, &inputs, &mut actual);
    assert_eq!(actual, expected);
    let mut other = mono(source(None), 8);
    assert!((0..10000).any(|i| other.step(&p, true)[0] != expected[i]));
}

#[test]
fn trigger_is_an_edge_and_grains_capture_controls() {
    let mut g = mono(source(Some(24_000.0)), 0);
    let mut p = params([
        0.0, 0.01, 0.25, -2.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]);
    g.step(&p, true);
    assert_eq!(g.grains[0].step[0], -1.0);
    assert_eq!(g.grains[0].length, 480);
    p[1] = 1.0;
    p[3] = 10.0;
    for _ in 0..100 {
        g.step(&p, true);
    }
    assert_eq!(g.grains.iter().filter(|g| g.length > 0).count(), 1);
    assert_eq!(g.grains[0].step[0], -1.0);
    assert_eq!(g.grains[0].length, 480);
    for _ in 0..500 {
        g.step(&p, true);
    }
    assert_eq!(g.step(&p, true), [0.0; 2]);
    p[7] = 0.0;
    g.step(&p, true);
    p[7] = 1.0;
    g.step(&p, true);
    assert_eq!(g.grains[0].length, 48000);
}

#[test]
fn window_has_silent_endpoints_and_shape_controls_peak() {
    let source = Arc::new(Table::from_f64([1.0], None));
    let mut g = mono(source, 0);
    g.prepare(1000.0);
    let p = params([
        0.0, 0.101, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]);
    let out: Vec<_> = (0..102).map(|_| g.step(&p, true)).collect();
    assert_eq!(out[0], [0.0; 2]);
    assert!((out[25][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert_eq!(out[100], [0.0; 2]);
    assert_eq!(out[101], [0.0; 2]);
}

#[test]
fn density_modulation_changes_the_running_clock() {
    let mut g = mono(source(None), 0);
    g.prepare(1000.0);
    let mut p = params([
        1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]);
    g.step(&p, true);
    p[0] = 1000.0;
    for _ in 0..10 {
        g.step(&p, true);
    }
    assert!(g.grains.iter().filter(|g| g.length > 0).count() >= 9);
}

#[test]
fn compatible_hot_reload_continues_the_cloud() {
    let source = source(None);
    let mut old = mono(source.clone(), 17);
    let mut p = controls();
    p[16] = 0.45;
    for _ in 0..7000 {
        old.step(&p, true);
    }
    let mut expected = old.clone();
    let mut replacement = mono(source, 17);
    replacement.carry_state(&mut old);
    for _ in 0..7000 {
        assert_eq!(replacement.step(&p, true), expected.step(&p, true));
    }
}

fn constant_grain(mix: f32) -> (Granular, [f32; 19]) {
    let mut config = GranularConfig {
        window: Some(Arc::new(Table::from_f64([1.0, 1.0], None))),
        ..GranularConfig::default()
    };
    config.sources[0] = vec![Arc::new(Table::from_f64([1.0], None))];
    config.sources[1] = vec![Arc::new(Table::from_f64([-1.0], Some(24000.0)))];
    let mut g = Granular::new(config).unwrap();
    g.prepare(48000.0);
    (
        g,
        params([
            0.0, 0.01, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.0, -1.0, 0.0, 0.0, 0.0, mix,
        ]),
    )
}

#[test]
fn pan_and_source_blend_are_captured_per_grain() {
    for (mix, expected) in [(0.0, 1.0), (0.25, 0.5), (1.0, -1.0)] {
        let (mut g, mut p) = constant_grain(mix);
        assert_eq!(g.step(&p, true), [expected, 0.0]);
        assert_eq!(&g.grains[0].step[..2], &[1.0, 0.5]);
        p[10] = 1.0;
        p[14] = 0.9;
        assert_eq!(g.step(&p, true), [expected, 0.0]);
    }
}

#[test]
fn spread_assigns_different_equal_power_positions_to_grains() {
    let (mut g, mut p) = constant_grain(0.0);
    p[0] = 48000.0;
    p[10] = 0.0;
    p[11] = 1.0;
    let mut centered = g.clone();
    let mut centered_p = p;
    centered_p[11] = 0.0;
    for _ in 0..64 {
        g.step(&p, true);
        centered.step(&centered_p, true);
    }
    for (wide, center) in g.grains.iter().zip(centered.grains.iter()) {
        assert_eq!(wide.position, center.position);
        assert_eq!(wide.step, center.step);
    }
    let pans: Vec<_> = g
        .grains
        .iter()
        .filter(|g| g.length > 0)
        .map(|g| g.pan)
        .collect();
    assert!(pans.iter().any(|p| p[0] > 0.9));
    assert!(pans.iter().any(|p| p[1] > 0.9));
    for p in pans {
        assert!((p[0] * p[0] + p[1] * p[1] - 1.0).abs() < 1e-6);
    }
}

#[test]
fn custom_window_reads_its_whole_shape_including_endpoints() {
    let (mut g, mut p) = constant_grain(0.0);
    g.prepare(1000.0);
    p[1] = 0.005;
    Arc::make_mut(&mut g.config).window = Some(Arc::new(Table::from_f64([0.0, 1.0, 0.0], None)));
    for expected in [0.0, 0.5, 1.0, 0.5, 0.0, 0.0] {
        assert!((g.step(&p, true)[0] - expected).abs() < 1e-6);
    }
}

#[test]
fn fm_changes_an_existing_grain_while_depth_stays_captured() {
    let (mut g, mut p) = constant_grain(0.0);
    g.sources[0] =
        Some(Source::new(vec![Arc::new(Table::from_f64([0.0, 1.0, 0.0, -1.0], None))]).unwrap());
    p[3] = 1.0;
    p[13] = 1.0;
    assert_eq!(g.step(&p, true), [0.0; 2]);
    p[12] = 1.0;
    p[13] = 0.0;
    assert_eq!(g.step(&p, true), [1.0, 0.0]);
    assert_eq!(g.step(&p, true), [-1.0, 0.0]);
    p[12] = -1.0;
    assert_eq!(g.step(&p, true), [1.0, 0.0]);
    assert_eq!(g.step(&p, true), [1.0, 0.0]);
}

#[test]
fn gain_is_captured_and_zero_gain_does_not_allocate_grains() {
    let (mut g, mut p) = constant_grain(0.0);
    p[15] = -0.25;
    assert_eq!(g.step(&p, true), [-0.25, 0.0]);
    p[15] = 0.0;
    p[16] = 0.0;
    assert_eq!(g.step(&p, true), [-0.25, 0.0]);
    g.reset();
    p[16] = 1.0;
    p[0] = 48000.0;
    for _ in 0..100 {
        assert_eq!(g.step(&p, true), [0.0; 2]);
    }
    assert!(g.grains.iter().all(|g| g.length == 0));
}

#[test]
fn masked_grains_do_not_steal_playing_grains_or_shift_the_clock() {
    let (mut g, mut p) = constant_grain(0.0);
    p[0] = 48000.0;
    p[1] = 10.0;
    for _ in 0..CAPACITY {
        g.step(&p, true);
    }
    let ages: Vec<_> = g.grains.iter().map(|g| g.age).collect();
    let mut unmasked = g.clone();
    let unmasked_p = p;
    p[16] = 0.0;
    assert_eq!(g.step(&p, true), [CAPACITY as f32, 0.0]);
    for (g, age) in g.grains.iter().zip(ages) {
        assert_eq!(g.age, age + 1);
    }
    unmasked.step(&unmasked_p, true);
    assert_eq!(g.countdown, unmasked.countdown);
    assert_eq!(g.rng, unmasked.rng);
    // Masking also rejects external triggers with the internal clock disabled.
    g.reset();
    p[0] = 0.0;
    assert_eq!(g.step(&p, true), [0.0; 2]);
    assert!(g.grains.iter().all(|g| g.length == 0));
}

#[test]
fn probability_is_seeded_and_keeps_the_other_random_choices() {
    let mut g = mono(source(None), 17);
    let mut unmasked = g.clone();
    let mut p = controls();
    p[16] = 0.35;
    let mut decisions = Vec::new();
    for _ in 0..512 {
        g.grains.clear();
        unmasked.grains.clear();
        let mut unmasked_p = p;
        unmasked_p[16] = 1.0;
        g.spawn(&p);
        unmasked.spawn(&unmasked_p);
        let accepted = !g.grains.is_empty();
        decisions.push(accepted);
        if accepted {
            assert_eq!(g.grains[0].position, unmasked.grains[0].position);
            assert_eq!(g.grains[0].step, unmasked.grains[0].step);
            assert_eq!(g.grains[0].pan, unmasked.grains[0].pan);
        }
        assert_eq!(g.rng, unmasked.rng);
    }
    assert!((120..240).contains(&decisions.iter().filter(|d| **d).count()));
    g.reset();
    for expected in decisions {
        g.grains.clear();
        g.spawn(&p);
        assert_eq!(!g.grains.is_empty(), expected);
    }
}

#[test]
fn second_source_has_captured_position_and_tuning_including_fm() {
    let mut config = GranularConfig {
        window: Some(Arc::new(Table::from_f64([1.0, 1.0], None))),
        ..GranularConfig::default()
    };
    config.sources[0] = vec![Arc::new(Table::from_f64(
        (0..8).map(|i| i as f64),
        Some(48000.0),
    ))];
    config.sources[1] = vec![Arc::new(Table::from_f64(
        (0..16).map(|i| i as f64),
        Some(24000.0),
    ))];
    let mut g = Granular::new(config).unwrap();
    let (_, mut p) = constant_grain(0.5);
    p[2] = 0.25;
    p[17] = -0.25;
    p[18] = 12.0;
    p[13] = 1.0;
    assert_eq!(g.step(&p, true), [7.0, 0.0]);
    assert_eq!(&g.grains[0].step[..2], &[1.0, 1.0]);
    assert_eq!(g.grains[0].fm_depth, 1.0);
    p[17] = 0.0;
    p[18] = -12.0;
    assert_eq!(g.step(&p, true), [8.0, 0.0]);
}

#[test]
fn overload_and_nonfinite_controls_stay_bounded() {
    let mut g = mono(source(None), 0);
    let mut p = controls();
    p[0] = 48000.0;
    p[1] = 10.0;
    for _ in 0..1000 {
        assert!(g.step(&p, true).iter().all(|x| x.is_finite()));
    }
    assert_eq!(
        g.grains.iter().filter(|g| g.age < g.length).count(),
        CAPACITY
    );
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        p.fill(value);
        for _ in 0..100 {
            assert!(g.step(&p, true).iter().all(|x| x.is_finite()));
        }
    }
}

fn full_controls() -> Vec<f32> {
    PORTS.iter().map(|p| p.default.unwrap()).collect()
}
fn set(p: &mut [f32], name: &str, value: f32) {
    p[PORTS.iter().position(|p| p.name == name).unwrap()] = value;
}
fn flat_config() -> GranularConfig {
    let mut config = GranularConfig {
        window: Some(Arc::new(Table::from_f64([1.0, 1.0], None))),
        ..GranularConfig::default()
    };
    config.sources[0] = vec![Arc::new(Table::from_f64([1.0], None))];
    config
}

#[test]
fn hard_sync_holds_the_clock_and_soft_sync_works_at_zero_density() {
    let mut g = Granular::new(flat_config()).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 10.0);
    set(&mut p, "sync", 1.0);
    assert_eq!(g.frame(&p, true)[2], 1.0);
    for _ in 0..200 {
        let out = g.frame(&p, true);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.0);
    }
    set(&mut p, "sync", 0.0);
    let pulses: f32 = (0..102).map(|_| g.frame(&p, true)[2]).sum();
    assert_eq!(pulses, 1.0);
    g.reset();
    set(&mut p, "density", 0.0);
    set(&mut p, "sync", 0.25);
    for _ in 0..3 {
        assert_eq!(g.frame(&p, true)[2], 0.0);
    }
    assert_eq!(g.frame(&p, true)[2], 1.0);
    set(&mut p, "sync", 0.5);
    assert_eq!(g.frame(&p, true)[3], 0.5);
    set(&mut p, "sync", -0.25);
    assert_eq!(g.frame(&p, true)[3], 0.25);
    set(&mut p, "sync", 0.75);
    assert_eq!(g.frame(&p, true)[2], 1.0);
}

#[test]
fn displaced_grains_capture_controls_before_waiting_and_are_cancelled_on_release() {
    let mut config = flat_config();
    config.sources[0] = vec![Arc::new(Table::from_f64((0..8).map(|i| i as f64), None))];
    config.distribution_table = Some(Arc::new(Table::from_f64([0.5], None)));
    let mut g = Granular::new(config).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 10.0);
    set(&mut p, "duration", 0.02);
    set(&mut p, "rate", 0.0);
    set(&mut p, "position", 0.25);
    set(&mut p, "distribution", 1.0);
    set(&mut p, "pan", -1.0);
    let out = g.frame(&p, true);
    assert_eq!(out[0], 0.0);
    assert_eq!(out[2], 1.0);
    let mut released = g.clone();
    set(&mut p, "position", 0.75);
    for _ in 1..50 {
        assert_eq!(g.frame(&p, true)[0], 0.0);
    }
    assert_eq!(g.frame(&p, true)[0], 2.0);
    for _ in 0..200 {
        assert_eq!(released.frame(&p, false)[0], 0.0);
    }
    assert!(released.grains.iter().all(|g| g.age >= g.length));
}

#[test]
fn sequential_displacements_keep_a_steady_underlying_pulse() {
    let mut config = flat_config();
    config.distribution_table = Some(Arc::new(Table::from_f64([0.0, 0.5, 0.25], None)));
    let mut g = Granular::new(config).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 10.0);
    set(&mut p, "duration", 0.002);
    set(&mut p, "distribution", -1.0);
    set(&mut p, "pan", -1.0);
    let (mut births, mut starts) = (Vec::new(), Vec::new());
    let mut previous = 0.0;
    for i in 0..320 {
        let frame = g.frame(&p, true);
        if frame[2] > 0.0 {
            births.push(i);
        }
        if frame[0] > 0.0 && previous == 0.0 {
            starts.push(i);
        }
        previous = frame[0];
    }
    assert_eq!(births, [0, 100, 200, 300]);
    assert_eq!(starts, [0, 150, 225, 300]);
}

#[test]
fn rate_sweeps_have_start_end_and_curvature_controls() {
    let mut halfway = Vec::new();
    for shape in [0.0, 0.2, 0.5, 0.8, 1.0] {
        let mut g = Granular::new(flat_config()).unwrap();
        g.prepare(1000.0);
        let mut p = full_controls();
        set(&mut p, "density", 0.0);
        set(&mut p, "trigger", 1.0);
        set(&mut p, "duration", 0.1);
        set(&mut p, "pitch_end", 12.0);
        set(&mut p, "sweep_shape", shape);
        for _ in 0..50 {
            g.frame(&p, true);
        }
        halfway.push(g.grains[0].step[0]);
        for _ in 0..49 {
            g.frame(&p, true);
        }
        let expected = if shape == 0.0 { 1.0 } else { 2.0 };
        let final_step =
            g.grains[0].step[0] * g.grains[0].sweep_decay + g.grains[0].sweep_offset[0];
        assert!((final_step - expected).abs() < 1e-8);
        g.frame(&p, true);
        assert!(g.grains.is_empty());
    }
    assert_eq!(halfway[0], 1.0);
    assert!(halfway[1] < 1.5);
    assert!((halfway[2] - 1.5).abs() < 1e-8);
    assert!(halfway[3] > 1.5);
    assert_eq!(halfway[4], 2.0);
}

#[test]
fn attack_sustain_decay_and_secondary_envelopes_are_independent() {
    let mut config = flat_config();
    config.window = Some(Arc::new(Table::from_f64([0.5, 0.5], None)));
    config.attack_window = Some(Arc::new(Table::from_f64([0.0, 1.0], None)));
    config.decay_window = Some(Arc::new(Table::from_f64([1.0, 0.0], None)));
    let mut g = Granular::new(config).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    set(&mut p, "duration", 0.101);
    set(&mut p, "sustain", 0.5);
    set(&mut p, "window_amount", 0.5);
    set(&mut p, "pan", -1.0);
    let out: Vec<_> = (0..101).map(|_| g.frame(&p, true)[0]).collect();
    assert_eq!(out[0], 0.0);
    assert_eq!(out[25], 0.75);
    assert_eq!(out[50], 0.75);
    assert_eq!(out[75], 0.75);
    assert_eq!(out[100], 0.0);
}

#[test]
fn fm_envelopes_are_relative_and_window_selection_is_captured() {
    let mut config = flat_config();
    config.sources[0] = vec![Arc::new(Table::from_f64((0..2000).map(|i| i as f64), None))];
    config.fm_windows = vec![
        Arc::new(Table::from_f64([0.0, 0.0], None)),
        Arc::new(Table::from_f64([1.0, 1.0], None)),
    ];
    let mut g = Granular::new(config).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    set(&mut p, "fm", 1.0);
    set(&mut p, "fm_depth", 1.0);
    set(&mut p, "fm_window_index", 0.0);
    g.frame(&p, true);
    set(&mut p, "fm_window_index", 1.0);
    g.frame(&p, true);
    assert_eq!(g.grains[0].position[0], 2.0);
    set(&mut p, "trigger", 0.0);
    g.frame(&p, true);
    set(&mut p, "trigger", 1.0);
    g.frame(&p, true);
    assert_eq!(g.grains[0].position[0], 4.0);
    assert_eq!(g.grains[1].position[0], 2.0);
    assert_eq!(g.grains[0].fm_window_id, 0);
    assert_eq!(g.grains[1].fm_window_id, 1);
}

#[test]
fn four_sources_route_between_wrapping_neighbors_with_custom_pan_laws() {
    let mut config = flat_config();
    config.channels = 4;
    config.sources = [1.0, 2.0, 3.0, 4.0].map(|v| vec![Arc::new(Table::from_f64([v], None))]);
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    for (name, weight) in ["weight1", "weight2", "weight3", "weight4"]
        .into_iter()
        .zip([0.1, 0.2, 0.3, 0.4])
    {
        set(&mut p, name, weight);
    }
    set(&mut p, "channel", 3.5);
    let mut g = Granular::new(config.clone()).unwrap();
    let frame = g.frame(&p, true);
    assert!((frame[0] - 1.5).abs() < 1e-6);
    assert_eq!(frame[1], 0.0);
    assert_eq!(frame[2], 0.0);
    assert!((frame[3] - 1.5).abs() < 1e-6);
    config.pan_law = Some(Arc::new(Table::from_f64(
        (0..8).flat_map(|_| [0.0, 0.25, 1.0]),
        None,
    )));
    let mut g = Granular::new(config).unwrap();
    let frame = g.frame(&p, true);
    assert!((frame[0] - 0.75).abs() < 1e-6);
    assert!((frame[3] - 0.75).abs() < 1e-6);
}

#[test]
fn every_reader_can_select_and_capture_a_bank_entry() {
    let mut config = flat_config();
    config.source_bank = vec![
        Arc::new(Table::from_f64([10.0], None)),
        Arc::new(Table::from_f64([20.0], None)),
    ];
    for reader in 0..4 {
        let mut g = Granular::new(config.clone()).unwrap();
        let mut p = full_controls();
        set(&mut p, "density", 0.0);
        set(&mut p, "trigger", 1.0);
        set(&mut p, "pan", -1.0);
        for name in ["weight1", "weight2", "weight3", "weight4"] {
            set(&mut p, name, 0.0);
        }
        set(
            &mut p,
            ["weight1", "weight2", "weight3", "weight4"][reader],
            1.0,
        );
        let select = ["select", "select2", "select3", "select4"][reader];
        set(&mut p, select, 0.0);
        assert_eq!(g.frame(&p, true)[0], 10.0);
        set(&mut p, select, -1.0);
        assert_eq!(g.frame(&p, true)[0], 10.0);
        set(&mut p, "trigger", 0.0);
        g.frame(&p, true);
        set(&mut p, "trigger", 1.0);
        assert_eq!(g.frame(&p, true)[0], 30.0);
    }
}

#[test]
fn masks_loop_and_expose_writable_next_indices_even_when_rejected() {
    let mut config = flat_config();
    config.sources[1] = vec![Arc::new(Table::from_f64([2.0], None))];
    config.masks = [
        vec![1.0, 2.0, 10.0, 20.0, 30.0, 40.0],
        vec![0.0, 1.0, 1.0, 2.0],
        vec![0.0, 0.0, 0.5],
        vec![0.0, 0.0, 2.0],
        vec![0.0, 1.0, 0.0, 1.0],
        vec![0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
    ]
    .map(|v| Some(Arc::new(Table::from_f64(v, None))));
    let mut g = Granular::new(config).unwrap();
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    set(&mut p, "weight1", 1.0);
    set(&mut p, "weight2", 1.0);
    set(&mut p, "fm_depth", 0.5);
    let frame = g.frame(&p, true);
    assert_eq!(frame[4], 2.0);
    assert_eq!(g.grains[0].gain, 20.0);
    assert_eq!(g.grains[0].weights[..2], [1.0, 0.0]);
    assert_eq!(g.grains[0].fm_depth, 1.0);
    set(&mut p, "trigger", 0.0);
    g.frame(&p, true);
    set(&mut p, "trigger", 1.0);
    let frame = g.frame(&p, true);
    assert_eq!(frame[4], 3.0);
    assert_eq!(g.grains[1].gain, 30.0);
    assert_eq!(g.grains[1].weights[..2], [0.0, 1.0]);
    assert_eq!(g.grains[1].step[0], 2.0);
    assert_eq!(g.grains[1].pan[..2], [0.0, 1.0]);
    set(&mut p, "trigger", 0.0);
    g.frame(&p, true);
    set(&mut p, "gain_index", 2.0);
    set(&mut p, "probability", 0.0);
    set(&mut p, "trigger", 1.0);
    let frame = g.frame(&p, true);
    assert_eq!(frame[4], 3.0);
    assert_eq!(frame[2], 1.0);
    assert_eq!(g.grains.iter().filter(|g| g.length > 0).count(), 2);
}

#[test]
fn configurable_pool_steals_the_oldest_not_the_grain_nearest_its_end() {
    let mut config = flat_config();
    config.max_grains = 2;
    let mut g = Granular::new(config).unwrap();
    let mut p = full_controls();
    set(&mut p, "duration", 1.0);
    g.spawn(&p);
    set(&mut p, "duration", 0.1);
    g.spawn(&p);
    set(&mut p, "duration", 0.5);
    g.spawn(&p);
    assert_eq!(g.grains.len(), 2);
    assert_eq!(g.grains[0].birth, 2);
    assert_eq!(g.grains[1].birth, 1);
}

#[test]
fn trainlets_obey_nyquist_and_release_without_leaking_grains() {
    let mut g = Granular::new(GranularConfig::default()).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    set(&mut p, "train_freq", 100.0);
    set(&mut p, "train_partials", 100.0);
    set(&mut p, "train_gain", 0.5);
    set(&mut p, "duration", 0.02);
    let mut sound = vec![g.frame(&p, true)[0]];
    assert_eq!(g.grains[0].train_partials, 5);
    sound.extend((1..20).map(|_| g.frame(&p, true)[0]));
    assert!(sound.iter().any(|v| v.abs() > 0.01));
    assert!(sound.iter().all(|v| v.abs() <= 0.5));
    for _ in 0..100 {
        assert_eq!(g.frame(&p, false)[0], 0.0);
    }
}

#[test]
fn invalid_resources_fail_before_the_audio_thread_runs() {
    let mut c = flat_config();
    c.channels = 9;
    assert!(Granular::new(c).is_err());
    let mut c = flat_config();
    c.max_grains = 0;
    assert!(Granular::new(c).is_err());
    let mut c = flat_config();
    c.pan_law = Some(Arc::new(Table::from_f64([0.0, 1.0], None)));
    assert!(Granular::new(c).is_err());
    let mut c = flat_config();
    c.masks[0] = Some(Arc::new(Table::from_f64([0.0, 4.0, 1.0], None)));
    assert!(Granular::new(c).is_err());
    let mut c = flat_config();
    c.masks[5] = Some(Arc::new(Table::from_f64([0.0, 0.0, 1.0], None)));
    assert!(Granular::new(c).is_err());
}

#[test]
fn reloading_keeps_delayed_grains_mask_cursors_and_trainlets_in_sync() {
    let mut config = flat_config();
    config.masks[0] = Some(Arc::new(Table::from_f64([0.0, 2.0, 1.0, 0.5, 0.25], None)));
    config.distribution_table = Some(Arc::new(Table::from_f64([0.0, 0.25, 0.5], None)));
    let mut old = Granular::new(config.clone()).unwrap();
    old.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 53.0);
    set(&mut p, "distribution", -1.0);
    set(&mut p, "probability", 0.7);
    set(&mut p, "train_gain", 0.2);
    for _ in 0..137 {
        old.frame(&p, true);
    }
    let mut expected = old.clone();
    let mut replacement = Granular::new(config).unwrap();
    replacement.prepare(1000.0);
    replacement.carry_state(&mut old);
    for _ in 0..1000 {
        assert_eq!(replacement.frame(&p, true), expected.frame(&p, true));
    }
}

#[test]
fn a_five_way_mix_mask_can_enable_any_reader_or_trainlet_on_its_own() {
    let mut config = flat_config();
    config.sources = [1.0, 2.0, 3.0, 4.0].map(|v| vec![Arc::new(Table::from_f64([v], None))]);
    config.masks[5] = Some(Arc::new(Table::from_f64(
        [0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        None,
    )));
    let mut g = Granular::new(config).unwrap();
    let p = full_controls();
    g.spawn(&p);
    assert_eq!(g.grains[0].weights, [0.0, 0.0, 1.0, 0.0, 0.0]);
    g.spawn(&p);
    assert_eq!(g.grains[1].weights, [0.0, 0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn readers_can_reverse_or_freeze_independently() {
    let mut config = flat_config();
    let ramp = Arc::new(Table::from_f64((0..8).map(|i| i as f64), None));
    config.sources = std::array::from_fn(|_| vec![ramp.clone()]);
    let mut g = Granular::new(config).unwrap();
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    set(&mut p, "position", 0.25);
    set(&mut p, "rate", 0.5);
    set(&mut p, "rate1", 1.0);
    set(&mut p, "rate2", -1.0);
    set(&mut p, "rate3", 0.0);
    set(&mut p, "rate4", 2.0);
    for name in ["weight1", "weight2", "weight3", "weight4"] {
        set(&mut p, name, 1.0);
    }
    g.frame(&p, true);
    assert_eq!(&g.grains[0].step[..4], &[0.5, -0.5, 0.0, 1.0]);
    assert_eq!(g.grains[0].position, [2.5, 1.5, 2.0, 3.0]);
}

#[test]
fn cycle_frequency_stays_tuned_across_bank_entries_of_different_lengths() {
    let mut config = flat_config();
    config.source_bank = vec![
        Arc::new(Table::from_f64([0.0; 4], None)),
        Arc::new(Table::from_f64([0.0; 8], None)),
    ];
    let mut g = Granular::new(config).unwrap();
    g.prepare(1000.0);
    let mut p = full_controls();
    set(&mut p, "density", 0.0);
    set(&mut p, "trigger", 1.0);
    set(&mut p, "freq", 100.0);
    set(&mut p, "select", 0.0);
    g.frame(&p, true);
    assert!((g.grains[0].step[0] / 4.0 - 0.1).abs() < 1e-10);
    set(&mut p, "trigger", 0.0);
    g.frame(&p, true);
    set(&mut p, "select", 1.0);
    set(&mut p, "trigger", 1.0);
    g.frame(&p, true);
    assert!((g.grains[1].step[0] / 8.0 - 0.1).abs() < 1e-10);
}

#[test]
fn cloned_idle_pools_are_reserved_and_do_not_grow_while_spawning() {
    let mut config = flat_config();
    config.max_grains = 8;
    let original = Granular::new(config).unwrap();
    let mut g = original.clone();
    assert!(g.grains.is_empty());
    assert!(g.grains.capacity() >= 8);
    let pointer = g.grains.as_ptr();
    let p = full_controls();
    for _ in 0..16 {
        g.spawn(&p);
        assert_eq!(g.grains.as_ptr(), pointer);
    }
    assert_eq!(g.grains.len(), 8);
    g.reset();
    assert!(g.grains.is_empty());
    assert_eq!(g.grains.as_ptr(), pointer);
}

#[test]
fn hard_sync_resets_phase_even_with_jittered_intervals() {
    let mut g = Granular::new(flat_config()).unwrap();
    let mut p = full_controls();
    set(&mut p, "jitter", 1.0);
    set(&mut p, "sync", 1.0);
    assert_eq!(g.frame(&p, true)[3], 0.0);
    for _ in 0..100 {
        let out = g.frame(&p, true);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.0);
    }
}
