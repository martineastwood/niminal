use niminal_engine::ops::{Bowed, Reed};
use niminal_engine::{Opcode, ProcessCtx};

fn render(op: &mut dyn Opcode, sr: f32, hz: f32, seconds: f32) -> Vec<f32> {
    op.prepare(sr);
    let mut inputs: Vec<_> = op.ports().iter().map(|p| p.default.unwrap_or(hz)).collect();
    inputs[0] = hz;
    let ctx = ProcessCtx::new(sr, true);
    (0..(sr * seconds) as usize)
        .map(|_| op.process_one(&ctx, &inputs))
        .collect()
}
fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}
fn pitch(x: &[f32], sr: f32, hz: f32) -> (f64, f64) {
    let ideal = f64::from(sr / hz);
    let first = (ideal * 0.94).floor().max(2.0) as usize;
    let last = (ideal * 1.06).ceil() as usize;
    let corr = |lag: usize| {
        let n = x.len() - last - 2;
        let (mut ab, mut aa, mut bb) = (0., 0., 0.);
        for i in 0..n {
            let a = f64::from(x[i]);
            let b = f64::from(x[i + lag]);
            ab += a * b;
            aa += a * a;
            bb += b * b;
        }
        ab / (aa * bb).sqrt().max(1e-30)
    };
    let (lag, quality) = (first..=last)
        .map(|l| (l, corr(l)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let (a, b, c) = (corr(lag - 1), quality, corr(lag + 1));
    let offset = if (a - 2. * b + c).abs() > 1e-12 {
        0.5 * (a - c) / (a - 2. * b + c)
    } else {
        0.
    };
    (f64::from(sr) / (lag as f64 + offset), quality)
}

#[test]
fn sustained_models_sound_and_follow_the_requested_pitch() {
    for mut op in [
        Box::new(Bowed::default()) as Box<dyn Opcode>,
        Box::new(Reed::default()),
    ] {
        for sr in [44100., 48000., 96000.] {
            for hz in [110., 220., 440., 880., 1760., 2640.] {
                let out = render(&mut *op, sr, hz, 1.5);
                let tail = &out[(sr * 0.75) as usize..];
                let level = rms(tail);
                let (measured, quality) = pitch(tail, sr, hz);
                let cents = 1200. * (measured / f64::from(hz)).log2();
                eprintln!(
                    "{} {sr} {hz}: rms={level:.6} pitch={measured:.2} cents={cents:.1} correlation={quality:.4}",
                    op.name()
                );
                // Correlation at a requested period also accepts its harmonics.
                // Require energy at the fundamental to catch register jumps.
                let fundamental = fundamental_level(tail, f64::from(sr), f64::from(hz));
                assert!(
                    fundamental > level * 0.2,
                    "{} {sr}/{hz}: missing fundamental ({fundamental}, RMS {level})",
                    op.name()
                );
                assert!(level > 0.001, "{} silent at {sr}/{hz}", op.name());
                assert!(
                    quality > 0.9 && cents.abs() < 35.,
                    "{} {sr}/{hz}: {measured}, {cents} cents, correlation {quality}",
                    op.name()
                );
                assert!(out.iter().all(|v| v.is_finite() && v.abs() < 2.));
            }
        }
    }
}

fn models() -> [Box<dyn Opcode>; 2] {
    [Box::new(Bowed::default()), Box::new(Reed::default())]
}

#[test]
fn no_pressure_is_silent_and_excitation_releases() {
    let ctx = ProcessCtx::new(48000., true);
    for mut op in models() {
        op.prepare(48000.);
        let mut inputs: Vec<_> = op
            .ports()
            .iter()
            .map(|p| p.default.unwrap_or(220.))
            .collect();
        inputs[1] = 0.;
        for _ in 0..24000 {
            assert_eq!(op.process_one(&ctx, &inputs), 0., "{}", op.name());
        }
        inputs[1] = if op.name() == "bowed" { 0.7 } else { 0.65 };
        let active: Vec<_> = (0..48000).map(|_| op.process_one(&ctx, &inputs)).collect();
        let level = rms(&active[24000..]);
        assert!(level > 0.001, "{} does not start speaking", op.name());
        inputs[1] = 0.;
        let tail: Vec<_> = (0..48000 * 6)
            .map(|_| op.process_one(&ctx, &inputs))
            .collect();
        assert!(
            rms(&tail[tail.len() - 24000..]) < level * 0.01,
            "{} does not release",
            op.name()
        );
    }
}

#[test]
fn every_port_stays_finite_under_extremes_and_audio_rate_changes() {
    let values = [
        -f32::MAX,
        -100.,
        0.,
        0.5,
        1.,
        100.,
        f32::MAX,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    for sr in [8000., 44100., 48000., 96000., 192000.] {
        let ctx = ProcessCtx::new(sr, true);
        for mut op in models() {
            op.prepare(sr);
            let defaults: Vec<_> = op
                .ports()
                .iter()
                .map(|p| p.default.unwrap_or(220.))
                .collect();
            for port in 0..defaults.len() {
                for value in values {
                    let mut inputs = defaults.clone();
                    inputs[port] = value;
                    for _ in 0..1000 {
                        let y = op.process_one(&ctx, &inputs);
                        assert!(
                            y.is_finite() && y.abs() < 2.,
                            "{} {sr} port {port}={value}: {y}",
                            op.name()
                        );
                    }
                }
            }
            for i in 0..sr as usize {
                let inputs = [
                    if i % 2 == 0 { 55. } else { 100000. },
                    values[i % values.len()],
                    values[(i + 3) % values.len()],
                    values[(i + 5) % values.len()],
                    values[(i + 7) % values.len()],
                ];
                let y = op.process_one(&ctx, &inputs);
                assert!(
                    y.is_finite() && y.abs() < 2.,
                    "{} modulated at {sr}: {y}",
                    op.name()
                );
            }
        }
    }
}

#[test]
fn blocks_scalar_reset_and_live_redefinition_preserve_the_same_sound() {
    let sr = 48000.;
    let ctx = ProcessCtx::new(sr, true);
    let n = 12000;
    for mut op in models() {
        let inputs: Vec<Vec<f32>> = (0..5)
            .map(|p| {
                (0..n)
                    .map(|i| match p {
                        0 => 220. + (i as f32 * 0.001).sin() * 12.,
                        1 => 0.65 + 0.1 * (i as f32 * 0.0007).sin(),
                        _ => 0.4 + 0.1 * (i as f32 * 0.0011).sin(),
                    })
                    .collect()
            })
            .collect();
        op.prepare(sr);
        let mut expected = vec![0.; n];
        for start in (0..n).step_by(113) {
            let end = (start + 113).min(n);
            let refs: Vec<_> = inputs.iter().map(|x| &x[start..end]).collect();
            op.process(&ctx, &refs, &mut expected[start..end]);
        }
        let mut scalar = op.box_clone();
        scalar.prepare(sr);
        for i in 0..n / 2 {
            let x: [f32; 5] = std::array::from_fn(|p| inputs[p][i]);
            assert_eq!(
                scalar.process_one(&ctx, &x),
                expected[i],
                "{} at {i}",
                op.name()
            );
        }
        let mut replacement = scalar.box_clone();
        replacement.prepare(sr);
        replacement.carry_state(&mut *scalar);
        for i in n / 2..n {
            let x: [f32; 5] = std::array::from_fn(|p| inputs[p][i]);
            assert_eq!(
                replacement.process_one(&ctx, &x),
                expected[i],
                "{} state carry at {i}",
                op.name()
            );
        }
        replacement.reset();
        for i in 0..n {
            let x: [f32; 5] = std::array::from_fn(|p| inputs[p][i]);
            assert_eq!(
                replacement.process_one(&ctx, &x),
                expected[i],
                "{} reset at {i}",
                op.name()
            );
        }
    }
}

fn harmonic(x: &[f32], sr: f64, hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in x.iter().enumerate() {
        let phase = std::f64::consts::TAU * hz * i as f64 / sr;
        re += f64::from(x) * phase.cos();
        im += f64::from(x) * phase.sin();
    }
    2.0 * re.hypot(im) / x.len() as f64
}

#[test]
fn string_keeps_even_harmonics_and_cylindrical_reed_favors_odd_ones() {
    let bow = render(&mut Bowed::default(), 48000., 220., 2.);
    let reed = render(&mut Reed::default(), 48000., 220., 2.);
    let bow = &bow[48000..];
    let reed = &reed[48000..];
    let bow_fund = harmonic(bow, 48000., 220.);
    assert!(harmonic(bow, 48000., 440.) > bow_fund * 0.1);
    let reed_fund = harmonic(reed, 48000., 220.);
    assert!(harmonic(reed, 48000., 440.) < reed_fund * 0.05);
    assert!(harmonic(reed, 48000., 660.) > reed_fund * 0.1);
}

// A windowed spectral peak avoids a single-bin null when the period estimate
// falls between cycles. The narrow band excludes octave/register overblows.
fn fundamental_level(x: &[f32], sr: f64, hz: f64) -> f64 {
    let mut planner = realfft::RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(x.len().next_power_of_two());
    let mut input = fft.make_input_vec();
    let mut sum = 0.0;
    for (i, &sample) in x.iter().enumerate() {
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / x.len() as f64).cos();
        input[i] = f64::from(sample) * window;
        sum += window;
    }
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut input, &mut spectrum).unwrap();
    spectrum
        .iter()
        .enumerate()
        .filter_map(|(i, value)| {
            let frequency = i as f64 * sr / input.len() as f64;
            ((hz * 0.97..=hz * 1.03).contains(&frequency)).then_some(value.norm() * 2.0 / sum)
        })
        .fold(0.0, f64::max)
}
