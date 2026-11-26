//! The web entry point renders what `niminal render` renders.
#![allow(clippy::unwrap_used)]

use niminal_web::{Failure, render_piece};

const PIECE: &str = "tempo 120bpm\ninstr tone(freq: hz) { out = osc(sine, freq) * env[0 2ms 1 | 100ms 0] }\nclip c {\n  instrument: tone\n  length: 1bar\n  notes: [a3 ~ c4 ~]\n}\nc.start\n";

#[test]
fn a_piece_renders_to_audio() {
    let rendered = render_piece(PIECE, 2.0).ok().unwrap();
    assert_eq!(rendered.sample_rate, 48_000);
    assert!(!rendered.channels.is_empty());
    // the last note starts in the second bar; trailing silence is trimmed
    assert!(rendered.channels[0].len() > 2 * 48_000);
    assert!(rendered.channels[0].iter().any(|s| s.abs() > 0.01));
}

#[test]
fn a_mistake_is_reported_where_it_is() {
    let Err(Failure::Source(errors)) = render_piece("instr a() { out = nonsense(1) }\n", 1.0) else {
        panic!("expected a located error");
    };
    assert!(!errors.is_empty());
    assert!(errors[0].span.start < "instr a() { out = nonsense(1) }\n".len());
}

#[test]
fn a_piece_with_no_notes_says_so() {
    let Err(Failure::Other(message)) = render_piece("instr a() { out = osc(sine, 220hz) }\n", 1.0) else {
        panic!("expected a message");
    };
    assert!(message.contains("no notes"));
}
