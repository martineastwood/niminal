use crate::common::*;

#[test]
fn a_keyboard_clip_plays_the_notes_held_and_lets_go_when_the_key_comes_up() {
    let mut s = session();
    s.eval(KEYBOARD, Some("now")).unwrap();
    assert_eq!(
        peak(&run(&mut s, 2000)),
        0.0,
        "nothing sounds until a key is pressed"
    );
    assert_eq!(s.key_input(2, 69, 100), 0, "another channel");
    assert_eq!(s.key_input(1, 69, 127), 1);
    let held = run(&mut s, 3 * BAR);
    assert!(
        (hz(&held[2000..BAR]) - 440.0).abs() < 12.0,
        "a4: {}",
        hz(&held[2000..BAR])
    );
    assert!(peak(&held[2 * BAR..]) > 0.5, "still held after three bars");
    assert_eq!(s.key_input(1, 69, 0), 1);
    run(&mut s, 2000);
    assert!(peak(&run(&mut s, BAR)) < 1e-3, "released");
}

#[test]
fn harder_keys_are_louder_and_two_keys_sound_together() {
    let level = |velocity: u8| {
        let mut s = session();
        s.eval(KEYBOARD, Some("now")).unwrap();
        s.key_input(1, 60, velocity);
        peak(&run(&mut s, 4800))
    };
    assert!(
        level(127) > 2.0 * level(40),
        "{} vs {}",
        level(127),
        level(40)
    );
    let mut s = session();
    s.eval(KEYBOARD, Some("now")).unwrap();
    s.key_input(1, 60, 100);
    s.key_input(1, 67, 100);
    let both = run(&mut s, 9600);
    assert!(peak(&both[2000..]) > 0.35, "two notes add up");
    s.key_input(1, 60, 0);
    run(&mut s, 2400);
    let one = run(&mut s, 4800);
    assert!(
        peak(&one) > 0.1 && peak(&one) < peak(&both),
        "the other key still sounds"
    );
}

#[test]
fn a_keyboard_set_replays_from_the_log() {
    let mut s = session();
    s.eval(KEYBOARD, Some("now")).unwrap();
    let mut original = run(&mut s, 1000);
    s.key_input(1, 64, 100);
    original.extend(run(&mut s, 5000));
    s.key_input(1, 64, 0);
    original.extend(run(&mut s, 5000));
    let log = from_lines(&to_lines(s.log())).unwrap();
    let replayed = Session::replay(
        SR,
        Layout::Mono,
        QuantizeDefaults::default(),
        &log,
        original.len() as u64,
        false,
    );
    assert!(peak(&original) > 0.1);
    assert_eq!(original, replayed[0]);
}

#[test]
fn a_keyboard_clip_checks_its_lanes() {
    let mut s = session();
    let both = "tempo 120bpm\ninstr organ(freq: hz) { osc(sine, freq) }\nclip pad { instrument: organ\n keys: midi.keys()\n notes: [c4] }\npad.start";
    let problems = s.eval(both, Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("both `notes` and `keys`"),
        "{}",
        problems[0].message
    );
    let bad = "tempo 120bpm\ninstr organ(freq: hz) { osc(sine, freq) }\nclip pad { instrument: organ\n keys: midi.notes() }";
    assert!(
        s.eval(bad, Some("now")).unwrap_err()[0]
            .message
            .contains("keys: midi.keys")
    );
}

#[test]
fn a_mono_instrument_sounds_one_note_at_a_time_and_max_caps_a_poly_one() {
    let sounding = |defs: &str, notes: &str| {
        let mut s = session();
        s.eval(
            &format!("tempo 120bpm\n{defs}\ntrack t {{ instrument = x }}\n{notes}"),
            Some("now"),
        )
        .unwrap();
        run(&mut s, 2000);
        s.transport().voices
    };
    let held = "x(freq: 220hz) for 4beats\nx(freq: 330hz) for 4beats\nx(freq: 440hz) for 4beats";
    assert_eq!(
        sounding("instr x(freq: hz) { osc(sine, freq) }", held),
        3,
        "poly: all three"
    );
    assert_eq!(
        sounding("mono x(freq: hz) { osc(sine, freq) }", held),
        1,
        "mono: the newest only (the others are fading out)"
    );
    assert_eq!(
        sounding("instr x(freq: hz) with(max: 2) { osc(sine, freq) }", held),
        2
    );
    assert_eq!(
        sounding(
            "instr x(freq: hz) with(same_note: release) { osc(sine, freq) * env[1 | 5ms 0] }",
            "x(freq: 220hz) for 4beats\nx(freq: 220hz) for 4beats\nx(freq: 330hz) for 4beats"
        ),
        2,
        "the second 220hz released the first, which has ended by now"
    );
}

#[test]
fn voice_options_are_checked() {
    let mut s = session();
    let err =
        |code: &str, s: &mut Session| s.eval(code, Some("now")).unwrap_err()[0].message.clone();
    assert!(err("mono x(freq: hz) with(max: 4) { osc(sine, freq) }", &mut s).contains("one voice"));
    assert!(
        err(
            "instr x(freq: hz) with(legato: true) { osc(sine, freq) }",
            &mut s
        )
        .contains("is for `mono`")
    );
    assert!(
        err(
            "instr x(freq: hz) with(steal: newest) { osc(sine, freq) }",
            &mut s
        )
        .contains("`oldest` or `quietest`")
    );
    assert!(
        err(
            "instr x(freq: hz) with(maxx: 3) { osc(sine, freq) }",
            &mut s
        )
        .contains("no option `maxx`")
    );
    assert!(
        err("instr x(freq: hz) with(max: 0) { osc(sine, freq) }", &mut s).contains("whole number")
    );
}
