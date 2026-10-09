mod common;
use common::*;
use niminal_engine::ops::{Delay, Reverb};
use niminal_engine::{Graph, GraphBuilder, Opcode, Port, ProcessCtx, Src};
use std::sync::Arc;

/// Emits 1.0 on the first sample, then silence.
#[derive(Clone)]
struct Impulse(bool);

impl Opcode for Impulse {
    fn name(&self) -> &str {
        "impulse"
    }
    fn ports(&self) -> &[Port] {
        &[]
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Impulse(false))
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], out: &mut [f32]) {
        for o in out {
            *o = if self.0 { 0.0 } else { 1.0 };
            self.0 = true;
        }
    }
}

fn delay_graph(max: f32, time: f32, feedback: f32) -> Arc<Graph> {
    let mut g = GraphBuilder::new();
    let i = g.add(Impulse(false), &[]).unwrap();
    let d = g
        .add(Delay::new(max), &[("x", i), ("time", Src::Const(time)), ("feedback", Src::Const(feedback))])
        .unwrap();
    Arc::new(g.build(d))
}

fn first_peak(x: &[f32]) -> usize {
    x.iter().position(|s| s.abs() > 0.5).unwrap()
}

#[test]
fn an_impulse_comes_back_after_exactly_the_delay_time() {
    // 10ms at 48kHz is 480 samples
    let out = render(&delay_graph(1.0, 0.010, 0.0), 2000, None, 32);
    assert_eq!(first_peak(&out), 480);
    assert_eq!(out[480], 1.0);
    assert!(out[..480].iter().all(|s| *s == 0.0));
    assert!(out[481..].iter().all(|s| *s == 0.0), "no feedback, so one echo");
}

#[test]
fn feedback_repeats_decaying_echoes() {
    let out = render(&delay_graph(1.0, 0.010, 0.5), 3000, None, 32);
    for (n, expected) in [(480, 1.0), (960, 0.5), (1440, 0.25), (1920, 0.125)] {
        assert!((out[n] - expected).abs() < 1e-4, "echo at {n}: {}", out[n]);
    }
}

#[test]
fn fractional_times_interpolate_between_samples() {
    // 10.5 samples: the impulse is shared between two neighbouring samples
    let out = render(&delay_graph(1.0, 10.5 / 48_000.0, 0.0), 100, None, 32);
    assert!((out[10] - 0.5).abs() < 1e-4 && (out[11] - 0.5).abs() < 1e-4, "{} {}", out[10], out[11]);
}

#[test]
fn time_is_clamped_to_the_buffer_and_feedback_stays_stable() {
    let out = render(&delay_graph(0.01, 5.0, 0.0), 2000, None, 32);
    assert_eq!(first_peak(&out), 480, "asked for 5s but the buffer holds 10ms");
    let loud = render(&delay_graph(0.01, 0.002, 50.0), 48_000, None, 32);
    assert!(loud.iter().all(|s| s.is_finite() && s.abs() <= 1.0), "feedback is capped below 1");
    let zero = render(&delay_graph(1.0, 0.0, 0.0), 100, None, 32);
    assert!(zero.iter().all(|s| s.is_finite()));
}

#[test]
fn delay_output_does_not_depend_on_block_size() {
    let g = delay_graph(1.0, 0.0123, 0.7);
    assert_eq!(render(&g, 5000, None, 32), render(&g, 5000, None, 11));
}

fn reverb_graph(room: f32, damp: f32) -> Arc<Graph> {
    let mut g = GraphBuilder::new();
    let i = g.add(Impulse(false), &[]).unwrap();
    let r = g
        .add(Reverb::new(), &[("x", i), ("room", Src::Const(room)), ("damp", Src::Const(damp))])
        .unwrap();
    Arc::new(g.build(r))
}

/// Seconds until the response falls below -60db of its peak.
fn decay_time(x: &[f32]) -> f32 {
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let last = x.iter().rposition(|s| s.abs() > peak * 0.001).unwrap();
    last as f32 / SR
}

#[test]
fn reverb_rings_and_a_bigger_room_rings_longer() {
    let small = render(&reverb_graph(0.2, 0.5), 5 * 48_000, None, 32);
    let big = render(&reverb_graph(0.9, 0.5), 5 * 48_000, None, 32);
    assert!(small.iter().any(|s| s.abs() > 0.001), "something comes out");
    assert!(decay_time(&big) > decay_time(&small) * 1.5, "{} vs {}", decay_time(&big), decay_time(&small));
    assert!(small.iter().chain(&big).all(|s| s.is_finite()));
}

#[test]
fn the_biggest_room_still_decays() {
    let out = render(&reverb_graph(1.0, 0.0), 20 * 48_000, None, 32);
    let tail = &out[out.len() - 48_000..];
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(rms(tail) < rms(&out[..48_000]) / 10.0, "decaying: {} vs {}", rms(tail), rms(&out[..48_000]));
}

#[test]
fn damping_takes_the_highs_out_of_the_tail() {
    let bright = render(&reverb_graph(0.8, 0.0), 2 * 48_000, None, 32);
    let dark = render(&reverb_graph(0.8, 1.0), 2 * 48_000, None, 32);
    // compare the energy of the first difference (a crude high-pass) over the tail
    let hf = |x: &[f32]| {
        let t = &x[24_000..];
        let d: Vec<f32> = t.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d) / rms(t)
    };
    assert!(hf(&dark) < hf(&bright) * 0.7, "{} vs {}", hf(&dark), hf(&bright));
}

#[test]
fn reverb_is_deterministic_and_block_size_independent() {
    let g = reverb_graph(0.7, 0.3);
    assert_eq!(render(&g, 20_000, None, 32), render(&g, 20_000, None, 13));
}

mod tails {
    use super::*;
    use niminal_engine::Voice;

    /// Samples a released voice runs before it reports finished, and the
    /// loudest output in its final block.
    fn samples_until_finished(graph: Arc<Graph>) -> (usize, f32) {
        let mut v = Voice::new(graph, SR);
        let mut buf = [0.0f32; 32];
        v.process(&mut buf); // the note has started and the signal is in the effect
        v.release();
        let mut n = 32;
        let mut last_peak = 0.0;
        while !v.is_finished() {
            v.process(&mut buf);
            last_peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            n += 32;
            assert!(n < 60 * 48_000, "never finishes");
        }
        (n, last_peak)
    }

    #[test]
    fn a_delay_keeps_its_voice_alive_until_the_echoes_fade() {
        let (n, last) = samples_until_finished(delay_graph(1.0, 0.010, 0.5));
        // 0.5^n < -90db after about 15 passes of 480 samples
        assert!((13 * 480..20 * 480).contains(&n), "{n}");
        assert!(last < 1e-4, "no click when the voice goes: {last}");
    }

    #[test]
    fn without_feedback_one_echo_is_all_there_is() {
        let (n, _) = samples_until_finished(delay_graph(1.0, 0.010, 0.0));
        assert!(n < 3 * 480, "{n}");
    }

    #[test]
    fn a_reverb_keeps_its_voice_alive_until_the_tail_fades() {
        let (small, last) = samples_until_finished(reverb_graph(0.2, 0.5));
        let (big, _) = samples_until_finished(reverb_graph(0.9, 0.5));
        assert!(big > small * 2, "{big} vs {small}");
        assert!(big > 48_000, "a big room rings for more than a second: {big}");
        assert!(last < 1e-3, "{last}");
    }

    #[test]
    fn a_silent_delay_or_reverb_does_not_hold_a_voice() {
        let mut g = GraphBuilder::new();
        let d = g.add(Delay::new(2.0), &[("x", Src::Const(0.0)), ("time", Src::Const(1.0))]).unwrap();
        let (n, _) = samples_until_finished(Arc::new(g.build(d)));
        assert!(n <= 64, "{n}");
    }
}
