use crate::common::*;

fn crossings_per_second(src: &str) -> f32 {
    let p = program(src);
    let out = mono(
        render_with(
            &p,
            &[],
            &RenderOptions {
                limiter: false,
                until: None,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let n = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    n as f32 / (out.len() as f32 / SR as f32)
}

#[test]
fn semitones_move_a_frequency_by_powers_of_two() {
    let hz = crossings_per_second(
        "instr a() { osc(sine, 220hz + 12st) * env[1 | 1ms 0] }\na() for 2beats",
    );
    assert!((hz - 440.0).abs() < 8.0, "an octave up from 220: {hz}");
    let fifth = crossings_per_second(
        "instr a() { osc(sine, 200hz + 7st) * env[1 | 1ms 0] }\na() for 2beats",
    );
    assert!((fifth - 299.7).abs() < 8.0, "a fifth up from 200: {fifth}");
    let down = crossings_per_second(
        "instr a() { osc(sine, 440hz - 12st) * env[1 | 1ms 0] }\na() for 2beats",
    );
    assert!((down - 220.0).abs() < 8.0, "{down}");
}

#[test]
fn a_semitone_parameter_changes_the_pitch_per_note() {
    let src = |up: &str| {
        format!(
            "instr a(up: st = 0st) {{ osc(sine, a3 + up) * env[1 | 1ms 0] }}\na(up: {up}) for 2beats"
        )
    };
    let base = crossings_per_second(&src("0st"));
    let octave = crossings_per_second(&src("12st"));
    assert!((octave / base - 2.0).abs() < 0.05, "{octave} vs {base}");
}
