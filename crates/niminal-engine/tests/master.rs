use niminal_engine::Master;

const SR: f32 = 48_000.0;
const CEILING: f32 = 0.966; // -0.3db

fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR).sin()).collect()
}

fn run(input: &[f32]) -> (Vec<f32>, usize) {
    let mut m = Master::new(SR, CEILING);
    let latency = m.latency();
    let mut buf = input.to_vec();
    buf.extend(std::iter::repeat_n(0.0, latency));
    m.process(&mut buf);
    (buf[latency..].to_vec(), latency)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |p, s| p.max(s.abs()))
}

#[test]
fn never_exceeds_the_ceiling() {
    for amp in [1.0, 2.0, 10.0, 100.0] {
        let (out, _) = run(&sine(220.0, amp, 48_000));
        assert!(peak(&out) <= CEILING + 1e-6, "amp {amp}: peak {}", peak(&out));
        assert!(peak(&out) > CEILING * 0.9, "amp {amp} should be limited to the ceiling, not crushed");
    }
}

#[test]
fn a_single_huge_spike_is_caught_by_the_lookahead() {
    let mut x = vec![0.0; 4800];
    x[2400] = 50.0;
    x[2401] = -50.0;
    let (out, _) = run(&x);
    assert!(peak(&out) <= CEILING + 1e-6, "{}", peak(&out));
}

#[test]
fn quiet_signals_pass_unchanged() {
    let x = sine(1000.0, 0.5, 24_000);
    let (out, _) = run(&x);
    // after the DC blocker has settled, output matches input
    for i in 4800..24_000 {
        assert!((out[i] - x[i]).abs() < 0.01, "sample {i}: {} vs {}", out[i], x[i]);
    }
}

#[test]
fn gain_recovers_after_a_loud_passage() {
    let mut x = sine(220.0, 4.0, 4800);
    x.extend(sine(220.0, 0.5, 48_000));
    let (out, _) = run(&x);
    let tail = &out[out.len() - 4800..];
    assert!(peak(tail) > 0.45, "back to full level after the release: {}", peak(tail));
}

#[test]
fn removes_dc_offset() {
    let x: Vec<f32> = sine(440.0, 0.3, 96_000).iter().map(|s| s + 0.2).collect();
    let (out, _) = run(&x);
    let mean = out[48_000..].iter().sum::<f32>() / 48_000.0;
    assert!(mean.abs() < 0.01, "{mean}");
}

#[test]
fn non_finite_input_becomes_silence() {
    let mut m = Master::new(SR, CEILING);
    let mut buf = vec![0.5, f32::NAN, f32::INFINITY, -f32::INFINITY, 0.5];
    m.process(&mut buf);
    assert!(buf.iter().all(|s| s.is_finite()));
    let mut more = vec![0.1; 256];
    m.process(&mut more);
    assert!(more.iter().all(|s| s.is_finite()));
}

#[test]
fn latency_is_about_a_millisecond_and_silence_stays_silent() {
    let m = Master::new(SR, CEILING);
    assert_eq!(m.latency(), 47);
    let (out, _) = run(&vec![0.0; 1000]);
    assert!(out.iter().all(|s| *s == 0.0));
}

#[test]
fn block_size_does_not_change_the_result() {
    let x = sine(330.0, 3.0, 10_000);
    let (a, _) = run(&x);
    let mut m = Master::new(SR, CEILING);
    let mut b = x.clone();
    b.extend(std::iter::repeat_n(0.0, m.latency()));
    for chunk in b.chunks_mut(7) {
        m.process(chunk);
    }
    assert_eq!(a, b[m.latency()..]);
}

#[test]
fn linked_channels_share_one_gain_so_the_image_does_not_shift() {
    let mut master = Master::with_channels(SR, CEILING, 2);
    let latency = master.latency();
    // left is far too loud, right is quiet: both must be turned down together
    let mut left = sine(220.0, 4.0, 24_000);
    let mut right = sine(330.0, 0.25, 24_000);
    left.extend(vec![0.0; latency]);
    right.extend(vec![0.0; latency]);
    let (orig_right, orig_left) = (right.clone(), left.clone());
    master.process_linked(&mut [&mut left, &mut right]);

    let (l, r) = (&left[latency..], &right[latency..]);
    assert!(peak(l) <= CEILING + 1e-6, "{}", peak(l));
    // an unlinked limiter would leave the quiet side alone; linked, it ducks too
    let window = 12_000..20_000;
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    let ratio = rms(&r[window.clone()]) / rms(&orig_right[window]);
    assert!(ratio < 0.5, "the quiet channel is ducked along with the loud one: {ratio}");
    let _ = orig_left;
}

#[test]
fn a_multichannel_master_passes_quiet_audio_unchanged() {
    let mut master = Master::with_channels(SR, CEILING, 3);
    let latency = master.latency();
    let src: Vec<Vec<f32>> = vec![sine(300.0, 0.3, 12_000), sine(500.0, 0.2, 12_000), sine(700.0, 0.1, 12_000)];
    let mut bufs = src.clone();
    for b in &mut bufs {
        b.extend(vec![0.0; latency]);
    }
    let mut views: Vec<&mut [f32]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
    master.process_linked(&mut views);
    for (out, orig) in bufs.iter().zip(&src) {
        for i in 4800..12_000 {
            assert!((out[i + latency] - orig[i]).abs() < 0.01);
        }
    }
}
