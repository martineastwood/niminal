//! Response and live-control contracts, independent of the filter implementation.
use niminal_engine::ops::{Biquad, BiquadMode, Eq, EqMode, FilterMode, Formant, Svf};
use niminal_engine::{Opcode, ProcessCtx};

fn render(op: &mut dyn Opcode, sr: f32, inputs: &[Vec<f32>], block: usize) -> Vec<f32> {
    op.prepare(sr);
    let mut out = vec![0.0; inputs[0].len()];
    for start in (0..out.len()).step_by(block) {
        let end = (start + block).min(out.len());
        let refs: Vec<_> = inputs.iter().map(|x| &x[start..end]).collect();
        op.process(&ProcessCtx::new(sr, true), &refs, &mut out[start..end]);
    }
    out
}

fn sine(sr: f32, hz: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            (std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr)).sin() as f32 * 0.1
        })
        .collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn bandpass_peak_stays_unity_across_resonance_and_sample_rates() {
    for sr in [44100.0, 48000.0, 96000.0] {
        for cutoff in [100.0, 1000.0, 10000.0] {
            for res in [0.0, 0.2, 0.95, 1.0] {
                let n = sr as usize * 2;
                let x = sine(sr, cutoff, n);
                let out = render(
                    &mut Svf::new(FilterMode::Band),
                    sr,
                    &[x.clone(), vec![cutoff; n], vec![res; n]],
                    64,
                );
                let gain = rms(&out[n / 2..]) / rms(&x[n / 2..]);
                assert!((gain - 1.0).abs() < 0.002, "{sr} {cutoff} {res}: {gain}");
            }
        }
    }
}

#[test]
fn vowel_endpoint_is_continuous_and_u_has_its_first_resonance() {
    for sr in [44100.0, 48000.0, 96000.0] {
        let n = sr as usize;
        let x = sine(sr, 325.0, n);
        let a = render(&mut Formant::default(), sr, &[x.clone(), vec![1.0; n]], 64);
        let b = render(
            &mut Formant::default(),
            sr,
            &[x.clone(), vec![1.0 - 1e-6; n]],
            64,
        );
        assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.0001));
        let gain = rms(&a[n / 2..]) / rms(&x[n / 2..]);
        assert!((gain - 1.0).abs() < 0.05, "U centre gain {gain} at {sr}");
    }
}

#[test]
fn formant_bandwidth_is_in_hz_across_sample_rates() {
    // A's first resonance: 800 Hz, 80 Hz wide. Other bands contribute little
    // here; both shoulders should be near -3 dB rather than almost silent.
    for sr in [44100.0, 48000.0, 96000.0] {
        for hz in [760.0, 840.0] {
            let n = sr as usize;
            let x = sine(sr, hz, n);
            let out = render(&mut Formant::default(), sr, &[x.clone(), vec![0.0; n]], 64);
            let gain = rms(&out[n / 2..]) / rms(&x[n / 2..]);
            assert!((0.6..0.82).contains(&gain), "{sr} {hz}: {gain}");
        }
    }
}

type FilterCase = (Box<dyn Opcode>, Vec<Vec<f32>>);

fn cases(n: usize, moving: Vec<f32>) -> Vec<FilterCase> {
    let x = sine(48000.0, 997.0, n);
    let mut cases: Vec<FilterCase> = Vec::new();
    for mode in [
        BiquadMode::Low,
        BiquadMode::High,
        BiquadMode::Band,
        BiquadMode::Notch,
        BiquadMode::All,
    ] {
        cases.push((
            Box::new(Biquad::new(mode)),
            vec![x.clone(), moving.clone(), vec![25.0; n]],
        ));
    }
    for mode in [EqMode::LowShelf, EqMode::HighShelf, EqMode::Peak] {
        cases.push((
            Box::new(Eq::new(mode)),
            vec![x.clone(), moving.clone(), vec![2.0; n], vec![4.0; n]],
        ));
    }
    for mode in [
        FilterMode::Low,
        FilterMode::High,
        FilterMode::Band,
        FilterMode::Notch,
    ] {
        cases.push((
            Box::new(Svf::new(mode)),
            vec![x.clone(), moving.clone(), vec![0.95; n]],
        ));
    }
    cases.push((
        Box::new(Formant::default()),
        vec![
            x,
            moving
                .iter()
                .map(|f| (f / 18000.0).clamp(0.0, 1.0))
                .collect(),
        ],
    ));
    cases
}

#[test]
fn live_sweeps_stay_bounded_and_do_not_depend_on_block_size() {
    let n = 24000;
    for abrupt in [false, true] {
        let moving = (0..n)
            .map(|i| {
                let t = (i % 4800) as f32 / 4799.0;
                if abrupt {
                    if t < 0.5 { 40.0 } else { 18000.0 }
                } else {
                    40.0 * (450.0_f32).powf(t)
                }
            })
            .collect();
        for (mut op, inputs) in cases(n, moving) {
            let out = render(&mut *op, 48000.0, &inputs, 64);
            let peak = out.iter().map(|x| x.abs()).fold(0.0_f32, f32::max);
            assert!(
                out.iter().all(|x| x.is_finite()) && peak < 10.0,
                "{} abrupt={abrupt}: peak {peak}",
                op.name()
            );
            assert_eq!(
                out,
                render(&mut *op.box_clone(), 48000.0, &inputs, 1),
                "{}",
                op.name()
            );
        }
    }
}

#[test]
fn live_redefinition_preserves_filter_tail_at_a_control_change() {
    let n = 4096;
    let moving = (0..n)
        .map(|i| if i < n / 2 { 800.0 } else { 5000.0 })
        .collect();
    for (mut op, inputs) in cases(n, moving) {
        let expected = render(&mut *op.box_clone(), 48000.0, &inputs, 64);
        op.prepare(48000.0);
        let ctx = ProcessCtx::new(48000.0, true);
        let refs: Vec<_> = inputs.iter().map(|x| &x[..n / 2]).collect();
        op.process(&ctx, &refs, &mut vec![0.0; n / 2]);
        let mut replacement = op.box_clone();
        replacement.prepare(48000.0);
        replacement.carry_state(&mut *op);
        let refs: Vec<_> = inputs.iter().map(|x| &x[n / 2..]).collect();
        let mut out = vec![0.0; n / 2];
        replacement.process(&ctx, &refs, &mut out);
        assert_eq!(out, expected[n / 2..], "{}", op.name());
    }
}

#[test]
fn named_biquads_and_eq_survive_audio_rate_cutoff_switching() {
    let n = 48000;
    let moving = (0..n)
        .map(|i| if i % 2 == 0 { 40.0 } else { 18000.0 })
        .collect();
    for (mut op, inputs) in cases(n, moving).into_iter().take(8) {
        let out = render(&mut *op, 48000.0, &inputs, 64);
        assert!(
            out.iter().all(|x| x.is_finite() && x.abs() < 10.0),
            "{}",
            op.name()
        );
    }
}

#[test]
fn named_biquads_keep_cookbook_response() {
    for sr in [44100.0, 48000.0, 96000.0] {
        for cutoff in [100.0, 1000.0, 10000.0] {
            let n = sr as usize;
            let x = sine(sr, cutoff, n);
            for (mode, expected) in [
                (BiquadMode::Low, std::f64::consts::FRAC_1_SQRT_2),
                (BiquadMode::High, std::f64::consts::FRAC_1_SQRT_2),
                (BiquadMode::Band, 1.0),
                (BiquadMode::Notch, 0.0),
                (BiquadMode::All, 1.0),
            ] {
                let out = render(
                    &mut Biquad::new(mode),
                    sr,
                    &[
                        x.clone(),
                        vec![cutoff; n],
                        vec![std::f32::consts::FRAC_1_SQRT_2; n],
                    ],
                    64,
                );
                let gain = rms(&out[n / 2..]) / rms(&x[n / 2..]);
                assert!(
                    (gain - expected).abs() < 0.002,
                    "{mode:?} {sr} {cutoff}: {gain}"
                );
            }
        }
    }
}

#[test]
fn eq_preserves_neutral_tone_and_requested_gains() {
    for sr in [44100.0, 48000.0, 96000.0] {
        let n = sr as usize;
        for mode in [EqMode::LowShelf, EqMode::HighShelf, EqMode::Peak] {
            for gain in [0.25_f32, 1.0, 4.0] {
                let x = sine(sr, 1000.0, n);
                let out = render(
                    &mut Eq::new(mode),
                    sr,
                    &[x.clone(), vec![1000.0; n], vec![gain; n], vec![0.707; n]],
                    64,
                );
                let measured = rms(&out[n / 2..]) / rms(&x[n / 2..]);
                let expected = if mode == EqMode::Peak {
                    gain
                } else {
                    gain.sqrt()
                };
                assert!(
                    (measured - f64::from(expected)).abs() < 0.002,
                    "{mode:?} {sr} gain {gain}: {measured}"
                );
                if gain == 1.0 {
                    assert_eq!(out, x);
                }
            }
        }
    }
}
