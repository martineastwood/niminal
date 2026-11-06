//! `echo_loop` from `std/blocks`, at the sample level (120 bpm: a quarter of a beat is 125ms, 6000 samples).
use super::rhythm_gate::audio_body;

fn peak_near(out: &[f32], at: usize) -> f32 {
    out[at - 200..at + 200].iter().fold(0.0, |m, v| m.max(v.abs()))
}

#[test]
fn echoes_come_at_the_time_and_each_is_feedback_quieter() {
    let out = audio_body(
        "click = impulse(1hz) * 0.5\nout = click.echo_loop(time: 1/4beat, feedback: 0.5, filter: 20khz)",
    );
    // the echoes only: nothing where the click is
    assert!(peak_near(&out, 200) < 0.01);
    // a click is a single sample, so even a filter at 20khz takes a little off its peak each time round
    for (at, level) in [(6000, 0.5), (12_000, 0.25), (18_000, 0.125)] {
        let heard = peak_near(&out, at);
        assert!((heard - level).abs() < level * 0.2, "at {at}: {heard}");
    }
    assert!(peak_near(&out, 9000) < 0.01, "silence between the echoes");
}

#[test]
fn a_filter_in_the_loop_makes_each_echo_darker() {
    let bright =
        audio_body("click = impulse(1hz) * 0.5\nout = click.echo_loop(time: 1/4beat, feedback: 0.9, filter: 20khz)");
    let dark =
        audio_body("click = impulse(1hz) * 0.5\nout = click.echo_loop(time: 1/4beat, feedback: 0.9, filter: 1khz)");
    // the peak of a click falls as its top is filtered away, more with each pass round the loop
    let ratio = |out: &[f32]| peak_near(out, 18_000) / peak_near(out, 6000);
    assert!(ratio(&dark) < ratio(&bright) * 0.7, "{} {}", ratio(&dark), ratio(&bright));
}

#[test]
fn ping_pong_puts_the_first_echo_on_the_right_and_bounces() {
    let echo = "click = impulse(1hz) * 0.5\nw = click.echo_loop(time: 1/4beat, feedback: 0.5, filter: 20khz, ping_pong: on)";
    let left = audio_body(&format!("{echo}\nout = w[0]"));
    let right = audio_body(&format!("{echo}\nout = w[1]"));
    // right: 1, 3, 5; left: 2, 4; each half the one before
    for (out, at, level) in [
        (&right, 6000, 0.5),
        (&left, 12_000, 0.25),
        (&right, 18_000, 0.125),
        (&left, 24_000, 0.0625),
    ] {
        let heard = peak_near(out, at);
        assert!((heard - level).abs() < level * 0.1, "at {at}: {heard}");
    }
    assert!(peak_near(&left, 6000) < 0.01 && peak_near(&right, 12_000) < 0.01);
}

#[test]
fn duck_pulls_the_echoes_down_under_a_loud_sound() {
    let wet = |duck: &str| {
        let out = audio_body(&format!(
            "tone = osc(sine, 220hz) * 0.3\nout = tone.echo_loop(time: 1/4beat, feedback: 0.5, duck: {duck})"
        ));
        crate::common::rms(&out[24_000..48_000])
    };
    let (open, ducked) = (wet("0"), wet("1"));
    assert!(open > 0.05, "{open}");
    assert!(ducked < open * 0.3, "{ducked} of {open}");
}

#[test]
fn the_default_time_is_a_dotted_eighth_at_the_tempo() {
    // 3/16 of a beat at 120 bpm is 93.75ms, 4500 samples
    let out = audio_body("click = impulse(1hz) * 0.5\nout = click.echo_loop(filter: 20khz)");
    assert!(peak_near(&out, 4500) > 0.4, "{}", peak_near(&out, 4500));
    assert!(peak_near(&out, 6000) < 0.01);
}
