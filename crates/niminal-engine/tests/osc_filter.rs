mod common;
use common::*;
use niminal_engine::ops::{FilterMode, Gain, Osc, Svf, Wave};
use niminal_engine::{GraphBuilder, GraphError, Src};
use std::sync::Arc;

fn osc_graph(wave: Wave, freq: f32) -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    let o = g.add(Osc::new(wave), &[("freq", Src::Const(freq))]).unwrap();
    Arc::new(g.build(o))
}

fn rising_zero_crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

#[test]
fn waves_have_the_right_frequency() {
    for wave in [Wave::Sine, Wave::Saw, Wave::Square, Wave::Tri] {
        let out = render(&osc_graph(wave, 440.0), SR as usize, None, 32);
        let n = rising_zero_crossings(&out);
        assert!((439..=441).contains(&n), "{wave:?}: {n} cycles in one second");
    }
}

#[test]
fn waves_stay_in_range_and_are_centred() {
    for wave in [Wave::Sine, Wave::Saw, Wave::Square, Wave::Tri] {
        let out = render(&osc_graph(wave, 1000.0), SR as usize, None, 32);
        let mean = out.iter().sum::<f32>() / out.len() as f32;
        assert!(mean.abs() < 0.01, "{wave:?} mean {mean}");
        assert!(out.iter().all(|v| v.abs() <= 1.1), "{wave:?} out of range");
    }
}

#[test]
fn sine_has_unit_amplitude_and_starts_at_zero() {
    let out = render(&osc_graph(Wave::Sine, 100.0), 4800, None, 32);
    assert_eq!(out[0], 0.0);
    let peak = out.iter().cloned().fold(0.0, f32::max);
    assert!((peak - 1.0).abs() < 1e-3);
}

#[test]
fn pulse_width_sets_duty_cycle() {
    let mut g = GraphBuilder::new();
    let o = g
        .add(Osc::new(Wave::Pulse), &[("freq", Src::Const(100.0)), ("width", Src::Const(0.25))])
        .unwrap();
    let out = render(&Arc::new(g.build(o)), SR as usize, None, 32);
    let high = out.iter().filter(|v| **v > 0.0).count() as f32 / out.len() as f32;
    assert!((high - 0.25).abs() < 0.01, "duty {high}");
}

/// Magnitude of `x` at `freq` (single-bin DFT).
fn bin_magnitude(x: &[f32], freq: f32) -> f32 {
    let w = std::f64::consts::TAU * f64::from(freq) / f64::from(SR);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, v) in x.iter().enumerate() {
        re += f64::from(*v) * (w * n as f64).cos();
        im -= f64::from(*v) * (w * n as f64).sin();
    }
    (re.hypot(im) * 2.0 / x.len() as f64) as f32
}

#[test]
fn saw_aliasing_is_suppressed() {
    // A 9.1kHz saw's 5th harmonic (45.5kHz) folds to 2.5kHz, where nothing
    // should be. Compare against a naive saw rendered the same way.
    let out = render(&osc_graph(Wave::Saw, 9100.0), SR as usize, None, 32);
    let naive: Vec<f32> = (0..SR as usize)
        .map(|n| (2.0 * (f64::from(n as u32) * 9100.0 / f64::from(SR)).fract() - 1.0) as f32)
        .collect();

    let alias = bin_magnitude(&out, 2500.0);
    let alias_naive = bin_magnitude(&naive, 2500.0);
    assert!(alias_naive > 0.1, "sanity: naive saw aliases ({alias_naive})");
    assert!(alias < alias_naive / 4.0, "polyblep {alias} vs naive {alias_naive}");
    assert!(bin_magnitude(&out, 9100.0) > 0.5, "fundamental intact");
}

fn filter_gain(mode: FilterMode, cutoff: f32, res: f32, freq: f32) -> f32 {
    let mut g = GraphBuilder::new();
    let o = g.add(Osc::new(Wave::Sine), &[("freq", Src::Const(freq))]).unwrap();
    let f = g
        .add(Svf::new(mode), &[("x", o), ("cutoff", Src::Const(cutoff)), ("res", Src::Const(res))])
        .unwrap();
    let out = render(&Arc::new(g.build(f)), 24_000, None, 32);
    rms(&out[12_000..]) / (1.0 / 2f32.sqrt())
}

#[test]
fn lowpass_passes_low_and_attenuates_high() {
    assert!(filter_gain(FilterMode::Low, 2000.0, 0.0, 200.0) > 0.95);
    let high = filter_gain(FilterMode::Low, 2000.0, 0.0, 16_000.0);
    assert!(high < 0.03, "16k through a 2k lowpass: {high}"); // ~ -12dB/oct, > 30dB down
}

#[test]
fn highpass_and_bandpass_and_notch_behave() {
    assert!(filter_gain(FilterMode::High, 2000.0, 0.0, 100.0) < 0.01);
    assert!(filter_gain(FilterMode::High, 2000.0, 0.0, 12_000.0) > 0.95);
    assert!(filter_gain(FilterMode::Band, 2000.0, 0.5, 2000.0) > 0.9);
    assert!(filter_gain(FilterMode::Notch, 2000.0, 0.5, 2000.0) < 0.05);
}

#[test]
fn resonance_boosts_at_cutoff_and_stays_stable() {
    let flat = filter_gain(FilterMode::Low, 2000.0, 0.0, 2000.0);
    let res = filter_gain(FilterMode::Low, 2000.0, 1.0, 2000.0);
    assert!(res > 5.0 * flat);
    assert!(res.is_finite());
}

#[test]
fn filter_survives_extreme_cutoffs() {
    for cutoff in [0.0, -100.0, 1.0, 30_000.0, 1e9] {
        let g = filter_gain(FilterMode::Low, cutoff, 1.0, 440.0);
        assert!(g.is_finite(), "cutoff {cutoff}");
    }
}

#[test]
fn builder_reports_port_errors() {
    let mut g = GraphBuilder::new();
    let e = g.add(Osc::new(Wave::Sine), &[]).unwrap_err();
    assert_eq!(e, GraphError::MissingPort { opcode: "osc".into(), port: "freq".into() });

    let e = g.add(Osc::new(Wave::Sine), &[("frequency", Src::Const(1.0))]).unwrap_err();
    assert!(matches!(e, GraphError::UnknownPort { .. }));
    assert!(e.to_string().contains("no argument named `frequency`"));

    let e = g
        .add(Osc::new(Wave::Sine), &[("freq", Src::Const(1.0)), ("freq", Src::Const(2.0))])
        .unwrap_err();
    assert!(matches!(e, GraphError::DuplicatePort { .. }));

    g.param("freq", 440.0).unwrap();
    assert_eq!(g.param("freq", 1.0).unwrap_err(), GraphError::DuplicateParam("freq".into()));
}

#[test]
fn params_wire_through_and_update() {
    let mut g = GraphBuilder::new();
    let freq = g.param("freq", 100.0).unwrap();
    let amp = g.param("amp", 0.5).unwrap();
    let o = g.add(Osc::new(Wave::Sine), &[("freq", freq)]).unwrap();
    let out = g.add(Gain, &[("x", o), ("gain", amp)]).unwrap();
    let graph = Arc::new(g.build(out));

    let mut v = niminal_engine::Voice::new(graph.clone(), SR);
    let mut buf = vec![0.0; 32];
    let mut all = vec![];
    for _ in 0..150 {
        v.process(&mut buf);
        all.extend_from_slice(&buf);
    }
    let peak = all.iter().cloned().fold(0.0, f32::max);
    assert!((peak - 0.5).abs() < 0.01);

    assert!(v.set_param_by_name("amp", 1.0));
    assert!(!v.set_param_by_name("nope", 1.0));
    v.process(&mut buf);
}

#[test]
fn voices_are_independent_and_deterministic() {
    let g = osc_graph(Wave::Saw, 330.0);
    assert_eq!(render(&g, 5000, None, 32), render(&g, 5000, None, 13));
}
