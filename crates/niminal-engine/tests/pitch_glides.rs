use niminal_engine::{GlideCurve, GraphBuilder, Voice, block_size};
use std::sync::Arc;

fn voice(from: f32) -> Voice {
    let mut graph = GraphBuilder::new();
    let pitch = graph.param("pitch", from).unwrap();
    Voice::new(Arc::new(graph.build(pitch)), 48000.0)
}
fn render(voice: &mut Voice, frames: usize, chunk: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    for block in out.chunks_mut(chunk.min(block_size())) {
        voice.process(block);
    }
    out
}
#[test]
fn pitch_glides_have_equal_intervals_exact_endpoints_and_no_block_drift() {
    for (from, to) in [(440.0_f32, 880.0_f32), (880.0, 440.0), (440.0, 440.1)] {
        let run = |chunk| {
            let mut v = voice(from);
            v.set_param_glide(0, to, 48000, GlideCurve::Pitch);
            render(&mut v, 48001, chunk)
        };
        let audio = run(64);
        assert_eq!(audio, run(7));
        assert!((audio[23999] - (from * to).sqrt()).abs() < 0.0001);
        assert_eq!(audio[47999], to);
        assert_eq!(audio[48000], to);
        for (n, value) in audio[..48000].iter().enumerate() {
            let expected =
                f64::from(from) * (f64::from(to) / f64::from(from)).powf((n + 1) as f64 / 48000.0);
            assert!((f64::from(*value) - expected).abs() < 0.0001);
        }
    }
}
#[test]
fn interrupted_pitch_glides_start_at_the_current_pitch_and_zero_duration_jumps() {
    let mut v = voice(440.0);
    v.set_param_glide(0, 880.0, 100, GlideCurve::Pitch);
    let midpoint = render(&mut v, 50, 13)[49];
    v.set_param_glide(0, 220.0, 100, GlideCurve::Pitch);
    let next = render(&mut v, 100, 17);
    assert!((next[49] - (midpoint * 220.0).sqrt()).abs() < 0.0001);
    assert_eq!(next[99], 220.0);
    v.set_param_glide(0, 330.0, 0, GlideCurve::Pitch);
    assert_eq!(render(&mut v, 1, 1), [330.0]);
}
#[test]
fn other_parameters_stay_linear_and_nonpositive_pitch_endpoints_are_finite() {
    for (from, to, curve) in [
        (440.0, 880.0, GlideCurve::Linear),
        (0.0, 440.0, GlideCurve::Pitch),
        (440.0, 0.0, GlideCurve::Pitch),
        (-1.0, 1.0, GlideCurve::Pitch),
    ] {
        let mut v = voice(from);
        v.set_param_glide(0, to, 100, curve);
        let audio = render(&mut v, 101, 32);
        assert_eq!(audio[49], (from + to) / 2.0);
        assert_eq!(audio[99], to);
        assert!(audio.iter().all(|v| v.is_finite()));
    }
}
