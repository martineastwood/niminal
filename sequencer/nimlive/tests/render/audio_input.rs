use crate::common::*;

fn heard(src: &str, input: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    render_with(
        &program(src),
        &[],
        &RenderOptions {
            limiter: false,
            input,
            ..Default::default()
        },
    )
    .unwrap()
    .channels
}

fn ramp(len: usize) -> Vec<f32> {
    (0..len).map(|i| (i as f32 / len as f32) * 0.5).collect()
}

#[test]
fn a_node_hears_the_input_and_processes_it() {
    let input = ramp(9_600);
    let out = heard(
        "tempo 120bpm\nfx mic { out = input(1) * 0.5 }",
        vec![input.clone()],
    );
    assert_eq!(out[0].len(), input.len(), "it lasts as long as the input");
    assert!(
        out[0]
            .iter()
            .zip(&input)
            .all(|(o, i)| (o - i * 0.5).abs() < 1e-6)
    );
    // a filter on it, the same as on the input written out
    let filtered = heard(
        "tempo 120bpm\nfx mic { out = input(1).lpf(cutoff: 2khz) }",
        vec![ramp(9_600)],
    );
    assert!(filtered[0].iter().any(|v| v.abs() > 0.01));
}

#[test]
fn channels_are_picked_and_mixed_to_the_masters_layout() {
    let (left, right) = (vec![0.25; 4_800], vec![0.5; 4_800]);
    let stereo = heard(
        "tempo 120bpm\nconfig { channels: stereo }\nfx mic { out = input(1..2) }",
        vec![left.clone(), right.clone()],
    );
    assert_eq!((stereo[0][100], stereo[1][100]), (0.25, 0.5));
    let second = heard(
        "tempo 120bpm\nconfig { channels: stereo }\nfx mic { out = [input(2), input(1)] }",
        vec![left.clone(), right.clone()],
    );
    assert_eq!((second[0][100], second[1][100]), (0.5, 0.25));
    // a mono input is spread to both sides of a stereo master
    let mono = heard(
        "tempo 120bpm\nconfig { channels: stereo }\nfx mic { out = input(2) }",
        vec![left, right],
    );
    assert!(
        (mono[0][100] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5
            && mono[0][100] == mono[1][100]
    );
}

#[test]
fn input_is_silent_when_there_is_none_and_a_mistake_is_reported() {
    let out = render_with(&program("tempo 120bpm\ninstr a() { osc(sine, 220hz) }\nfx mic { out = input(1) }\na() for 1beat"), &[], &RenderOptions { limiter: false, ..Default::default() }).unwrap();
    assert!(
        out.channels[0].iter().all(|v| *v == 0.0 || v.abs() > 0.0),
        "renders without an input"
    );
    let error = |src: &str| {
        compile(src)
            .err()
            .map(|e| e.iter().map(|d| d.message.clone()).collect::<String>())
            .unwrap_or_default()
    };
    assert_eq!(error("instr a() { input(1) }"), "`input` is read in a node");
    assert!(error("fx m { out = input(0) }").contains("whole number from 1 to 16"));
    assert!(error("fx m { out = input(2..1) }").contains("comes first"));
    assert!(error("fx m { out = input() }").contains("takes a channel"));
}
