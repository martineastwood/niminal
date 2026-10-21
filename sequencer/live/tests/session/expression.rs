use crate::common::*;

#[test]
fn pitch_bend_moves_a_held_note_and_the_next_one_starts_bent() {
    use niminal_live::Expression::Bend;
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    s.key_input(1, 69, 100);
    let steady = run(&mut s, 9600);
    assert!((hz(&steady[2000..]) - 440.0).abs() < 12.0);
    assert_eq!(s.expression_input(1, Bend, 2.0, None), 1);
    let up = run(&mut s, 9600);
    assert!(
        (hz(&up[2000..]) - 493.9).abs() < 14.0,
        "two semitones up: {}",
        hz(&up[2000..])
    );
    s.key_input(1, 69, 0);
    run(&mut s, 4800);
    s.key_input(1, 60, 100);
    let next = run(&mut s, 9600);
    assert!(
        (hz(&next[2000..]) - 293.7).abs() < 12.0,
        "c4 bent up to d4: {}",
        hz(&next[2000..])
    );
    assert_eq!(
        s.expression_input(2, Bend, 1.0, None),
        0,
        "another channel has no keys"
    );
}

#[test]
fn pressure_changes_the_level_of_the_note_it_is_on() {
    use niminal_live::Expression::Pressure;
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    s.key_input(1, 69, 100);
    s.key_input(1, 72, 100);
    let soft = peak(&run(&mut s, 9600)[2000..]);
    assert_eq!(
        s.expression_input(1, Pressure, 1.0, Some(69)),
        1,
        "one key's pressure reaches that key only"
    );
    let a = run(&mut s, 9600);
    assert!(
        peak(&a[2000..]) > soft * 1.25,
        "{} vs {soft}",
        peak(&a[2000..])
    );
    assert_eq!(s.expression_input(1, Pressure, 0.5, None), 2);
}

#[test]
fn the_sustain_pedal_holds_notes_whose_keys_have_come_up() {
    use niminal_live::Expression::Sustain;
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    s.expression_input(1, Sustain, 1.0, None);
    s.key_input(1, 69, 100);
    s.key_input(1, 69, 0);
    assert!(
        peak(&run(&mut s, 2 * BAR)) > 0.05,
        "still sounding with the key up"
    );
    assert_eq!(
        s.expression_input(1, Sustain, 0.0, None),
        1,
        "the pedal comes up and lets it go"
    );
    run(&mut s, 2400);
    assert!(peak(&run(&mut s, BAR)) < 1e-3);
    // pressing the same key again with the pedal down damps the old note
    s.expression_input(1, Sustain, 1.0, None);
    s.key_input(1, 69, 100);
    s.key_input(1, 69, 0);
    s.key_input(1, 69, 100);
    s.key_input(1, 69, 0);
    s.expression_input(1, Sustain, 0.0, None);
    run(&mut s, 2400);
    assert!(peak(&run(&mut s, BAR)) < 1e-3);
}

#[test]
fn expression_replays_to_the_same_audio() {
    use niminal_live::Expression::{Bend, Pressure, Sustain};
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    let mut original = run(&mut s, 1000);
    s.key_input(1, 69, 100);
    original.extend(run(&mut s, 2000));
    s.expression_input(1, Bend, 1.5, None);
    original.extend(run(&mut s, 1500));
    s.expression_input(1, Pressure, 0.8, None);
    s.expression_input(1, Sustain, 1.0, None);
    s.key_input(1, 69, 0);
    original.extend(run(&mut s, 5000));
    s.expression_input(1, Sustain, 0.0, None);
    original.extend(run(&mut s, 9000));
    let replayed =
        niminal_server::Session::replay(SR, Layout::Mono, &s.log(), original.len() as u64, false);
    assert_eq!(original, replayed[0]);
}
