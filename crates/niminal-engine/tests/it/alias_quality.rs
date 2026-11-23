//! Aliasing and passband of the oscillators and the oversampler, measured on the spectrum.
#![allow(clippy::unwrap_used)]
mod common;
use common::SR;
use niminal_engine::ops::{Delay, Distortion, Gain, Ladder, Osc, Oversample, Wave};
use niminal_engine::{GraphBuilder, Opcode, ProcessCtx, Src, Voice};
use realfft::RealFftPlanner;
use std::sync::Arc;

const N: usize = 1 << 16;

fn run_graph(g: GraphBuilder, out: Src, n: usize) -> Vec<f32> {
    let mut v = Voice::new(Arc::new(g.build(out)), SR);
    let mut samples = vec![0.0f32; n];
    for c in samples.chunks_mut(64) {
        v.process(c);
    }
    samples
}

fn spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut input: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
            f64::from(*v) * w
        })
        .collect();
    let mut out = fft.make_output_vec();
    fft.process(&mut input, &mut out).unwrap();
    out.iter().map(|c| c.norm_sqr()).collect()
}

/// Energy away from the harmonics of `f0` (20hz to 22khz) against the energy on them, in dB.
fn alias_db(x: &[f32], f0: f64) -> f64 {
    let sp = spectrum(x);
    let bin = f64::from(SR) / x.len() as f64;
    let (mut signal, mut other) = (0.0, 0.0);
    for (i, p) in sp.iter().enumerate() {
        let hz = i as f64 * bin;
        if !(20.0..=22000.0).contains(&hz) {
            continue;
        }
        let k = (hz / f0).round();
        if k >= 1.0 && (hz - k * f0).abs() < 6.0 * bin {
            signal += p;
        } else {
            other += p;
        }
    }
    10.0 * (other / signal).log10()
}

fn osc_alias(wave: Wave, f0: f32) -> f64 {
    let mut g = GraphBuilder::new();
    let o = g
        .add(Osc::new(wave), &[("freq", Src::Const(f0)), ("width", Src::Const(0.3))])
        .unwrap();
    alias_db(&run_graph(g, o, N)[4096..], f64::from(f0))
}

#[test]
fn saw_and_pulse_alias_far_below_the_wave() {
    for (f0, limit) in [(440.0, -75.0), (1234.5, -75.0), (2500.0, -75.0), (5000.0, -75.0), (9000.0, -75.0)] {
        for wave in [Wave::Saw, Wave::Square, Wave::Pulse] {
            let db = osc_alias(wave, f0);
            println!("{wave:?} {f0}: {db:.1}");
            assert!(db < limit, "{wave:?} at {f0}hz: {db:.1}dB");
        }
    }
}

#[test]
fn saw_keeps_its_top_end() {
    // The table must not be so band-limited that the saw goes dull: at 440hz the harmonics near 16khz
    // are still there, within 3dB of the ideal 1/h.
    let mut g = GraphBuilder::new();
    let o = g.add(Osc::new(Wave::Saw), &[("freq", Src::Const(440.0))]).unwrap();
    let out = run_graph(g, o, N);
    let sp = spectrum(&out[4096..]);
    let bin = f64::from(SR) / (N - 4096) as f64;
    let level = |hz: f64| {
        let i = (hz / bin).round() as usize;
        (i - 3..=i + 3).map(|j| sp[j]).fold(0.0, f64::max).sqrt()
    };
    for h in [5.0, 20.0, 35.0] {
        let ratio = level(440.0 * h) / level(440.0) * h;
        println!("harmonic {h}: {ratio:.3} of ideal");
        assert!((0.7..1.4).contains(&ratio), "harmonic {h}: {ratio}");
    }
}

fn nonlinear_alias(make: &dyn Fn(usize, Src, &mut GraphBuilder) -> Src, factor: usize) -> f64 {
    let f = 3100.0f32;
    let mut g = GraphBuilder::new();
    let o = g.add(Osc::new(Wave::Sine), &[("freq", Src::Const(f))]).unwrap();
    let x = g.add(Gain, &[("x", o), ("gain", Src::Const(0.8))]).unwrap();
    let y = make(factor, x, &mut g);
    alias_db(&run_graph(g, y, N)[4096..], f64::from(f))
}

#[test]
fn oversampling_removes_the_aliasing_of_distortion() {
    let distortion = |factor: usize, x: Src, g: &mut GraphBuilder| {
        let args = [
            ("x", x),
            ("drive", Src::Const(8.0)),
            ("tone", Src::Const(0.5)),
            ("mix", Src::Const(1.0)),
        ];
        if factor == 1 {
            g.add(Distortion::default(), &args).unwrap()
        } else {
            g.add(Oversample::new(Box::new(Distortion::default()), factor), &args).unwrap()
        }
    };
    let native = nonlinear_alias(&distortion, 1);
    let x4 = nonlinear_alias(&distortion, 4);
    let x8 = nonlinear_alias(&distortion, 8);
    println!("distortion: native {native:.1}, x4 {x4:.1}, x8 {x8:.1}");
    assert!(x4 < native - 30.0 && x8 < x4, "{native} {x4} {x8}");
}

#[test]
fn oversampler_is_flat_across_the_audible_band() {
    for factor in [2, 4, 8] {
        for hz in [1000.0f32, 10000.0, 16000.0, 18000.0] {
            let mut g = GraphBuilder::new();
            let o = g.add(Osc::new(Wave::Sine), &[("freq", Src::Const(hz))]).unwrap();
            let y = g
                .add(Oversample::new(Box::new(Gain), factor), &[("x", o), ("gain", Src::Const(1.0))])
                .unwrap();
            let out = run_graph(g, y, 16384);
            let rms = (out[4096..].iter().map(|a| f64::from(*a).powi(2)).sum::<f64>() / (16384 - 4096) as f64).sqrt();
            let gain = rms / 0.5f64.sqrt();
            assert!((gain - 1.0).abs() < 0.02, "x{factor} at {hz}hz: {gain}");
        }
    }
}

#[test]
fn delay_keeps_the_top_end_at_a_fractional_time() {
    for (hz, least) in [(8000.0, 0.97), (12000.0, 0.85), (16000.0, 0.6)] {
        let s: Vec<f32> = (0..N)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / f64::from(SR)).sin() as f32)
            .collect();
        let mut d = Delay::new(0.1);
        d.prepare(SR);
        let mut out = vec![0.0; N];
        let time = vec![10.5 / SR; 64];
        let zero = vec![0.0; 64];
        for start in (0..N).step_by(64) {
            d.process(
                &ProcessCtx::new(SR, true),
                &[&s[start..start + 64], &time, &zero, &zero],
                &mut out[start..start + 64],
            );
        }
        let rms = |v: &[f32]| (v.iter().map(|a| f64::from(*a).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        let gain = rms(&out[8192..]) / rms(&s[8192..]);
        println!("{hz}: {gain:.3}");
        assert!(gain > least, "{hz}hz: {gain}");
    }
}

#[test]
fn ladder_alias_is_unchanged_or_better() {
    let ladder = |factor: usize, x: Src, g: &mut GraphBuilder| {
        let args = [
            ("x", x),
            ("cutoff", Src::Const(12000.0)),
            ("res", Src::Const(0.3)),
            ("drive", Src::Const(4.0)),
        ];
        if factor == 1 {
            g.add(Ladder::default(), &args).unwrap()
        } else {
            g.add(Oversample::new(Box::new(Ladder::default()), factor), &args).unwrap()
        }
    };
    let native = nonlinear_alias(&ladder, 1);
    let x2 = nonlinear_alias(&ladder, 2);
    println!("ladder: native {native:.1}, x2 {x2:.1}");
    assert!(x2 < native - 20.0);
}

#[test]
fn pulse_and_saw_never_read_outside_their_table() {
    // `phase - width` can round to exactly one cycle; that, extreme widths and any frequency must stay finite.
    for wave in [Wave::Saw, Wave::Square, Wave::Pulse] {
        for width in [0.0f32, 0.01, 0.1, 0.3, 0.5, 0.7, 0.9, 0.99, 1.0, -1.0, 7.5] {
            for freq in [0.0f32, 1.0, 100.0, 1000.0, 7000.0, 24000.0, 90000.0, -440.0, -24000.0] {
                let mut g = GraphBuilder::new();
                let o = g
                    .add(
                        Osc::new(wave),
                        &[("freq", Src::Const(freq)), ("width", Src::Const(width)), ("phase_mod", Src::Const(1e-17))],
                    )
                    .unwrap();
                let out = run_graph(g, o, 4096);
                assert!(out.iter().all(|x| x.is_finite() && x.abs() < 2.5), "{wave:?} {freq}hz width {width}");
            }
        }
    }
}

#[test]
fn pulse_width_sweeps_stay_finite() {
    // Width moving every sample, so many phases land exactly on an edge.
    let n = 1 << 15;
    let widths: Vec<f32> = (0..n).map(|i| 0.5 + 0.49 * (i as f32 * 0.001).sin()).collect();
    let freqs = vec![1000.0f32; n];
    let mut osc = Osc::new(Wave::Pulse);
    osc.prepare(SR);
    let mut out = vec![0.0; n];
    for s in (0..n).step_by(64) {
        let zero = vec![0.0; 64];
        osc.process(
            &ProcessCtx::new(SR, true),
            &[&freqs[s..s + 64], &zero, &widths[s..s + 64]],
            &mut out[s..s + 64],
        );
    }
    assert!(out.iter().all(|x| x.is_finite() && x.abs() < 2.5));
}
