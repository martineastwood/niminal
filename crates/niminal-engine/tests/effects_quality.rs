//! Live automation must preserve continuity, tails and deterministic timing.
#![allow(clippy::unwrap_used)]
use niminal_engine::ops::{Allpass, Compressor, Delay, Fdn, Mverb};
use niminal_engine::{Opcode, ProcessCtx};

fn render(op: &mut dyn Opcode, inputs: &[Vec<f32>], block: usize) -> Vec<f32> {
    op.prepare(48000.0);
    let mut out = vec![0.0; inputs[0].len()];
    for start in (0..out.len()).step_by(block) {
        let end = (start + block).min(out.len());
        let refs: Vec<_> = inputs.iter().map(|x| &x[start..end]).collect();
        op.process(&ProcessCtx::new(48000.0, true), &refs, &mut out[start..end]);
    }
    out
}

fn sine(hz: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.1 * (std::f64::consts::TAU * f64::from(hz) * i as f64 / 48000.0).sin() as f32)
        .collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn integer_allpass_is_unity_gain_including_high_feedback() {
    let n = 48000;
    for g in [-0.995, -0.5, 0.5, 0.995] {
        for hz in [0.0, 50.0, 500.0, 5000.0, 20000.0] {
            let x = if hz == 0.0 { vec![0.1; n] } else { sine(hz, n) };
            let out = render(
                &mut Allpass::new(0.1),
                &[x.clone(), vec![10.0 / 48000.0; n], vec![g; n]],
                64,
            );
            let gain = rms(&out[n / 2..]) / rms(&x[n / 2..]);
            assert!((gain - 1.0).abs() < 0.002, "feedback {g}, {hz}Hz: {gain}");
        }
    }
}

#[test]
fn delay_time_jump_bends_pitch_without_jumping_the_read_head() {
    let n = 24000;
    for increasing in [true, false] {
        let time = (0..n)
            .map(|i| if (i < 9600) == increasing { 0.02 } else { 0.12 })
            .collect();
        let inputs = [sine(1000.0, n), time, vec![0.0; n], vec![0.0; n]];
        let out = render(&mut Delay::new(0.2), &inputs, 64);
        let window = &out[11000..13000];
        let crossings = window
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        let hz = crossings as f64 * 48000.0 / window.len() as f64;
        let expected = if increasing { 500.0 } else { 1500.0 };
        assert!(
            (hz - expected).abs() < 30.0,
            "increasing={increasing}: {hz}Hz"
        );
        assert!(out[9000..].windows(2).all(|w| (w[1] - w[0]).abs() < 0.025));
        assert_eq!(out, render(&mut Delay::new(0.2), &inputs, 1));
    }
}

type EffectCase = (Box<dyn Opcode>, Vec<Vec<f32>>);
fn moving_effects(n: usize) -> Vec<EffectCase> {
    let x = sine(997.0, n);
    let ramp: Vec<_> = (0..n).map(|i| (i % 4800) as f32 / 4799.0).collect();
    let map = |low: f32, high: f32| {
        ramp.iter()
            .map(|t| low + (high - low) * t)
            .collect::<Vec<_>>()
    };
    vec![
        (
            Box::new(Compressor::default()),
            vec![
                x.clone(),
                vec![0.03; n],
                vec![4.0; n],
                map(0.001, 0.02),
                map(0.01, 0.1),
                vec![1.0; n],
                vec![f32::NAN; n],
                map(1.0, 4.0),
            ],
        ),
        (
            Box::new(Fdn::default()),
            vec![
                x.clone(),
                map(0.2, 4.0),
                map(500.0, 16000.0),
                map(0.25, 2.0),
            ],
        ),
        (
            Box::new(Mverb::default()),
            vec![
                x.clone(),
                x.clone(),
                map(0.0, 1.0),
                map(0.1, 0.9),
                vec![0.8; n],
                map(0.1, 0.9),
                map(0.0, 0.1),
                map(0.0, 1.0),
                vec![1.0; n],
                vec![1.0; n],
                vec![0.75; n],
            ],
        ),
        (
            Box::new(Delay::new(0.2)),
            vec![
                x.clone(),
                map(0.001, 0.19),
                vec![0.99; n],
                map(0.0, 10000.0),
            ],
        ),
        (
            Box::new(Allpass::new(0.2)),
            vec![x, map(0.001, 0.19), vec![0.9; n]],
        ),
    ]
}

#[test]
fn moving_controls_are_bounded_and_independent_of_caller_blocks() {
    for (mut op, inputs) in moving_effects(24000) {
        let out = render(&mut *op, &inputs, 64);
        assert!(
            out.iter().all(|x| x.is_finite() && x.abs() < 10.0),
            "{}",
            op.name()
        );
        assert_eq!(
            out,
            render(&mut *op.box_clone(), &inputs, 1),
            "{}",
            op.name()
        );
        assert_eq!(
            out,
            render(&mut *op.box_clone(), &inputs, 113),
            "{}",
            op.name()
        );
    }
}

#[test]
fn redefinition_preserves_moving_delay_heads_and_control_memory() {
    let n = 12000;
    for (mut op, inputs) in moving_effects(n) {
        let expected = render(&mut *op.box_clone(), &inputs, 64);
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
