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

mod channels {
    use super::*;
    use niminal_engine::ops::{PanChannel, pan_gains};

    fn stereo() -> Arc<[Option<f32>]> {
        Arc::from(vec![Some(-30.0), Some(30.0)])
    }

    fn surround() -> Arc<[Option<f32>]> {
        // L R C LFE Ls Rs
        Arc::from(vec![Some(-30.0), Some(30.0), Some(0.0), None, Some(-110.0), Some(110.0)])
    }

    fn power(gains: &[f32]) -> f32 {
        gains.iter().map(|g| g * g).sum()
    }

    #[test]
    fn stereo_panning_is_constant_power_between_the_speakers() {
        let centre = pan_gains(&stereo(), 0.0, 0.0);
        assert!((centre[0] - centre[1]).abs() < 1e-6);
        assert!((power(&centre) - 1.0).abs() < 1e-5);

        let left = pan_gains(&stereo(), -30.0, 0.0);
        assert!((left[0] - 1.0).abs() < 1e-6 && left[1].abs() < 1e-6);
        let right = pan_gains(&stereo(), 30.0, 0.0);
        assert!((right[1] - 1.0).abs() < 1e-6 && right[0].abs() < 1e-6);

        for az in (-30..=30).step_by(5) {
            assert!((power(&pan_gains(&stereo(), az as f32, 0.0)) - 1.0).abs() < 1e-5, "{az}");
        }
    }

    #[test]
    fn beyond_a_stereo_pair_the_nearer_speaker_takes_the_sound() {
        let g = pan_gains(&stereo(), 120.0, 0.0);
        assert!((g[1] - 1.0).abs() < 1e-6 && g[0] == 0.0);
        let g = pan_gains(&stereo(), -150.0, 0.0);
        assert!((g[0] - 1.0).abs() < 1e-6 && g[1] == 0.0);
    }

    #[test]
    fn surround_panning_goes_round_the_room_and_skips_the_lfe() {
        let ahead = pan_gains(&surround(), 0.0, 0.0);
        assert!((ahead[2] - 1.0).abs() < 1e-6, "straight ahead is the centre speaker");

        let behind = pan_gains(&surround(), 180.0, 0.0);
        assert!((behind[4] - behind[5]).abs() < 1e-5 && behind[4] > 0.5, "{:?}", &behind[..6]);

        for az in (0..360).step_by(10) {
            let g = pan_gains(&surround(), az as f32, 0.0);
            assert!((power(&g) - 1.0).abs() < 1e-4, "azimuth {az}: power {}", power(&g));
            assert_eq!(g[3], 0.0, "nothing is panned to the LFE");
        }
    }

    #[test]
    fn spread_blends_towards_all_speakers_keeping_the_power() {
        let narrow = pan_gains(&surround(), 0.0, 0.0);
        let wide = pan_gains(&surround(), 0.0, 1.0);
        assert!(wide[0] > narrow[0] && wide[4] > narrow[4]);
        assert!((power(&wide) - 1.0).abs() < 1e-4);
        assert!((wide[0] - wide[2]).abs() < 1e-5, "fully spread is equal across speakers");
    }

    #[test]
    fn wrapping_azimuths_are_equivalent() {
        let a = pan_gains(&surround(), -90.0, 0.2);
        let b = pan_gains(&surround(), 270.0, 0.2);
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5));
    }

    #[test]
    fn a_pan_channel_scales_its_input_by_its_gain() {
        let mut g = GraphBuilder::new();
        let tone = g.add(niminal_engine::ops::Osc::new(niminal_engine::ops::Wave::Sine), &[("freq", Src::Const(100.0))]).unwrap();
        let right = g
            .add(PanChannel::new(stereo(), 1), &[("x", tone), ("azimuth", Src::Const(30.0))])
            .unwrap();
        let out = render(&Arc::new(g.build(right)), 480, None, 32);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 1.0).abs() < 1e-3, "hard right passes the full signal: {peak}");
    }

    #[test]
    fn reverb_channels_are_decorrelated() {
        let channel = |c: usize| {
            let mut g = GraphBuilder::new();
            let i = g.add(Impulse(false), &[]).unwrap();
            let r = g.add(Reverb::for_channel(c), &[("x", i), ("room", Src::Const(0.8))]).unwrap();
            render(&Arc::new(g.build(r)), 20_000, None, 32)
        };
        let (l, r) = (channel(0), channel(1));
        assert_ne!(l, r);
        let diff: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 0.01);
        // but they are similar in character: comparable energy
        assert!((rms(&l) / rms(&r) - 1.0).abs() < 0.3);
    }
}
