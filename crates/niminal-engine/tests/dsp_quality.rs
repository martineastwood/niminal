//! Audible DSP contracts: pitch, tuning, neutral tone and fractional-delay loss.
use niminal_engine::ops::{Chorus, Comb, Distortion, Ladder, PitchShift};
use niminal_engine::{Opcode, ProcessCtx};

fn render(op: &mut dyn Opcode, sr: f32, inputs: &[Vec<f32>], block: usize) -> Vec<f32> {
    op.prepare(sr);
    let mut out = vec![0.0; inputs[0].len()];
    for start in (0..out.len()).step_by(block) {
        let end = (start + block).min(out.len());
        let slices: Vec<_> = inputs.iter().map(|v| &v[start..end]).collect();
        op.process(&ProcessCtx::new(sr, true), &slices, &mut out[start..end]);
    }
    out
}

fn sine(sr: f32, hz: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            amp * (std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr)).sin() as f32
        })
        .collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

fn magnitude(x: &[f32], sr: f32, hz: f32) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in x.iter().enumerate() {
        let phase = std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr);
        re += f64::from(x) * phase.cos();
        im += f64::from(x) * phase.sin();
    }
    2.0 * re.hypot(im) / x.len() as f64
}

#[test]
fn pitch_shifts_in_the_requested_direction_and_zero_preserves_the_signal() {
    for sr in [44100.0, 48000.0, 96000.0] {
        let n = sr as usize;
        // Forty input cycles per window align the overlapping grains, isolating
        // read-head speed from the expected granular crossfade sidebands.
        for shift in [-24.0, -12.0, 0.0, 12.0, 24.0] {
            let inputs = [sine(sr, 1000.0, n, 0.5), vec![shift; n], vec![0.04; n]];
            let out = render(&mut PitchShift::default(), sr, &inputs, 64);
            let hz = 1000.0 * 2.0_f32.powf(shift / 12.0);
            assert!(
                magnitude(&out[n / 2..], sr, hz) > 0.4,
                "{sr}Hz, shift {shift}: missing {hz}Hz"
            );
            if shift == 0.0 {
                assert!(
                    out[2..]
                        .iter()
                        .zip(&inputs[0])
                        .all(|(a, b)| (a - b).abs() < 1e-6)
                );
            }
            let split = render(&mut PitchShift::default(), sr, &inputs, 1);
            assert_eq!(out, split);
        }
    }
}

#[test]
fn ladder_cutoff_is_tuned_across_sample_rates_and_resonance() {
    for sr in [44100.0, 48000.0, 96000.0] {
        for fc in [200.0, 2000.0, 10000.0] {
            let n = sr as usize;
            let inputs = [
                sine(sr, fc, n, 0.0001),
                vec![fc; n],
                vec![0.0; n],
                vec![1.0; n],
            ];
            let out = render(&mut Ladder::default(), sr, &inputs, 64);
            // Four one-poles are each -3dB at cutoff: total gain 1/4.
            let gain = rms(&out[n / 2..]) / rms(&inputs[0][n / 2..]);
            assert!((gain - 0.25).abs() < 0.002, "{sr}Hz cutoff {fc}: {gain}");
            let mut resonant = inputs.clone();
            resonant[2].fill(0.95);
            let out = render(&mut Ladder::default(), sr, &resonant, 64);
            assert!(rms(&out[n / 2..]) > rms(&inputs[0][n / 2..]) * 2.0);
        }
    }
}

#[test]
fn ladder_survives_driven_audio_rate_cutoff_and_resonance_changes() {
    let n = 48000;
    let inputs = [
        sine(48000.0, 997.0, n, 10.0),
        (0..n)
            .map(|i| if i % 2 == 0 { 20.0 } else { 1e9 })
            .collect(),
        (0..n)
            .map(|i| if i % 3 == 0 { -10.0 } else { 10.0 })
            .collect(),
        vec![100.0; n],
    ];
    let out = render(&mut Ladder::default(), 48000.0, &inputs, 64);
    assert!(out.iter().all(|x| x.is_finite() && x.abs() < 10.0));
}

#[test]
fn distortion_midpoint_is_neutral_and_tone_tilts_both_ways() {
    let sr = 48000.0;
    let n = sr as usize;
    for hz in [100.0, 10000.0] {
        let mut levels = Vec::new();
        for tone in [0.0, 0.5, 1.0] {
            let inputs = [
                sine(sr, hz, n, 0.0001),
                vec![1.0; n],
                vec![tone; n],
                vec![1.0; n],
            ];
            let out = render(&mut Distortion::default(), sr, &inputs, 64);
            levels.push(rms(&out[n / 2..]) / rms(&inputs[0][n / 2..]));
        }
        assert!(
            (levels[1] - 1.0).abs() < 0.01,
            "neutral at {hz}: {levels:?}"
        );
        if hz < 1000.0 {
            assert!(levels[0] > levels[1] && levels[1] > levels[2]);
        } else {
            assert!(levels[0] < levels[1] && levels[1] < levels[2]);
        }
    }
}

#[test]
fn fractional_comb_delay_does_not_dampen_highs_without_damping() {
    let n = 48000;
    let x = sine(48000.0, 18000.0, n, 0.5);
    for samples in [20.0, 20.1, 20.5, 20.9] {
        let inputs = [
            x.clone(),
            vec![samples / 48000.0; n],
            vec![0.0; n],
            vec![1.0; n],
            vec![0.0; n],
        ];
        let out = render(&mut Comb::new(0.1), 48000.0, &inputs, 64);
        let wet: Vec<_> = out.iter().zip(&x).map(|(a, b)| a - b).collect();
        let gain = rms(&wet[n / 2..]) / rms(&x[n / 2..]);
        assert!((gain - 1.0).abs() < 0.001, "delay {samples}: {gain}");
    }
}

#[test]
fn chorus_cubic_delay_retains_more_high_end_than_linear() {
    let n = 48000;
    let inputs = [
        sine(48000.0, 12000.0, n, 0.5),
        vec![0.0; n],
        vec![0.0; n],
        vec![100.5 / 48000.0; n],
        vec![1.0; n],
    ];
    let out = render(&mut Chorus::default(), 48000.0, &inputs, 64);
    let gain = rms(&out[n / 2..]) / rms(&inputs[0][n / 2..]);
    assert!(gain > 0.85, "12kHz gain {gain}; linear gives 0.707");
}

#[test]
fn comb_feedback_stays_finite_when_time_changes_at_audio_rate() {
    let n = 96000;
    let inputs = [
        sine(48000.0, 997.0, n, 0.5),
        (0..n)
            .map(|i| [1.0, 20.001, 20.999, 4800.0][i % 4] / 48000.0)
            .collect(),
        vec![0.9999; n],
        vec![1.0; n],
        vec![0.0; n],
    ];
    let out = render(&mut Comb::new(0.1), 48000.0, &inputs, 64);
    assert!(out.iter().all(|x| x.is_finite() && x.abs() < 1000.0));
}

#[test]
fn nonlinear_opcodes_run_natively_and_allow_explicit_oversampling() {
    use niminal_engine::ops::Oversample;
    for op in [
        Box::new(Ladder::default()) as Box<dyn Opcode>,
        Box::new(Distortion::default()),
    ] {
        assert_eq!(
            op.latency(),
            0,
            "native {} has no conversion latency",
            op.name()
        );
        for factor in [2, 4, 8] {
            let mut wrapped = Oversample::new(op.box_clone(), factor);
            wrapped.prepare(48000.0);
            assert_eq!(wrapped.latency(), 25);
        }
    }
}

#[test]
fn live_redefinitions_preserve_effect_memory_and_reset_rearms_it() {
    let n = 2048;
    let x = sine(48000.0, 997.0, n, 0.3);
    let cases = [
        (
            Box::new(Ladder::default()) as Box<dyn Opcode>,
            vec![x.clone(), vec![2000.0; n], vec![0.8; n], vec![3.0; n]],
        ),
        (
            Box::new(Comb::new(0.1)),
            vec![
                x.clone(),
                vec![20.5 / 48000.0; n],
                vec![0.9; n],
                vec![0.5; n],
                vec![0.2; n],
            ],
        ),
        (
            Box::new(PitchShift::default()),
            vec![x.clone(), vec![7.0; n], vec![0.02; n]],
        ),
        (
            Box::new(Distortion::default()),
            vec![x.clone(), vec![4.0; n], vec![0.7; n], vec![0.8; n]],
        ),
        (
            Box::new(Chorus::default()),
            vec![x, vec![0.5; n], vec![0.5; n], vec![0.007; n], vec![0.8; n]],
        ),
    ];
    let ctx = ProcessCtx::new(48000.0, true);
    for (mut original, inputs) in cases {
        let expected = render(&mut *original.box_clone(), 48000.0, &inputs, 64);
        original.prepare(48000.0);
        let prefix: Vec<_> = inputs.iter().map(|v| &v[..1024]).collect();
        original.process(&ctx, &prefix, &mut vec![0.0; 1024]);
        let mut replacement = original.box_clone();
        replacement.prepare(48000.0);
        replacement.carry_state(&mut *original);
        let suffix: Vec<_> = inputs.iter().map(|v| &v[1024..]).collect();
        let mut actual = vec![0.0; 1024];
        replacement.process(&ctx, &suffix, &mut actual);
        assert_eq!(
            actual,
            expected[1024..],
            "state carry for {}",
            replacement.name()
        );
        replacement.reset();
        replacement.process(&ctx, &prefix, &mut actual);
        assert_eq!(actual, expected[..1024], "reset for {}", replacement.name());
    }
}
