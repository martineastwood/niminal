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
