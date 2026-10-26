use crate::common::*;
use niminal_live_cli::render::{End, render_session, split_ramps};

const FILE: &str = "tempo 120bpm\ninstr p(freq: hz) { osc(sine, freq) * env[1 | 50ms 0] * 0.3 }\nclip t { instrument: p, notes: [c4 e4 g4 c5] }\ntempo 240bpm @ next bar over 2bars\nt.start";

#[test]
fn a_ramp_is_split_from_the_rest_of_the_file() {
    let (rest, ramps) = split_ramps(FILE).unwrap();
    assert_eq!(ramps, ["tempo 240bpm @ next bar over 2bars"]);
    assert!(
        rest.contains("tempo 120bpm")
            && !rest.contains("240bpm")
            && rest.lines().count() == FILE.lines().count()
    );
    assert_eq!(split_ramps("tempo 120bpm").unwrap().1.len(), 0);
}

#[test]
fn a_ramped_render_speeds_up_between_the_two_tempos() {
    let rendered = |src: &str| {
        let (rest, _) = split_ramps(src).unwrap();
        let layout = compile(&rest).unwrap().master;
        render_session(
            src,
            std::path::Path::new("."),
            layout,
            End::Bars(4.0),
            true,
            false,
            Vec::new(),
        )
        .unwrap()
        .len() as f64
            / SR
    };
    let ramped = rendered(FILE);
    // four bars at 120bpm last 8s and at 240bpm 4s; the first bar is slow and the rest speed up
    assert!(ramped > 4.5 && ramped < 7.0, "{ramped}");
    let steady = rendered(&FILE.replace("tempo 240bpm @ next bar over 2bars\n", ""));
    assert!(
        steady > 7.9 && ramped < steady - 1.0,
        "{steady} vs {ramped}"
    );
}

#[test]
fn a_ramped_render_keeps_stems_through_the_tempo_changes() {
    let (rest, _) = split_ramps(FILE).unwrap();
    let layout = compile(&rest).unwrap().master;
    let out = render_session(
        FILE,
        std::path::Path::new("."),
        layout,
        End::Bars(4.0),
        false,
        true,
        Vec::new(),
    )
    .unwrap();
    let node = out
        .stems
        .iter()
        .find(|s| s.name == "t")
        .expect("the node's stem");
    assert_eq!(
        node.channels[0].len(),
        out.channels[0].len(),
        "a stem is as long as the mix"
    );
    assert!(rms(&node.channels[0]) > 0.01);
    // the whole piece is on the stem, not just the stretch after the last tempo step
    let third = node.channels[0].len() / 3;
    assert!(rms(&node.channels[0][..third]) > 0.01 && rms(&node.channels[0][2 * third..]) > 0.01);
    // the node is the only sound, so the stem is the mix
    let diff = node.channels[0]
        .iter()
        .zip(&out.channels[0])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(diff < 1e-4, "{diff}");
}
