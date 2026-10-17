use niminal_daemon::{from_lines, to_lines};
use niminal_lang::Layout;
use niminal_live::{ControlSignal, Offline};

fn session(source: &str) -> Offline {
    let mut s = Offline::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(source, Some("now")).unwrap();
    s
}

const BOUND: &str = "ctl level = midi.cc(7).range(0..1).smooth(0ms)\nctl pan = midi.cc(10, 2).range(0..1).smooth(0ms)\nctl wet = osc_in(\"/fx/wet\").range(0..2).smooth(0ms)\ninstr flat() { level + pan * 10 + wet * 100 }\nflat() for 1beat";

#[test]
fn midi_and_osc_messages_move_the_controls_bound_to_them() {
    let mut s = session(BOUND);
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 7,
            channel: 1,
            value: 127
        }),
        1
    );
    // bound to channel 2 only, and to a different CC
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 10,
            channel: 1,
            value: 127
        }),
        0
    );
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 11,
            channel: 2,
            value: 127
        }),
        0
    );
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 10,
            channel: 2,
            value: 127
        }),
        1
    );
    assert_eq!(
        s.control_input(&ControlSignal::Osc {
            address: "/fx/wet",
            value: 0.5
        }),
        1
    );
    assert_eq!(
        s.control_input(&ControlSignal::Osc {
            address: "/fx/other",
            value: 0.5
        }),
        0
    );
    // outside 0..1 clamps to the range
    assert_eq!(
        s.control_input(&ControlSignal::Osc {
            address: "/fx/wet",
            value: 7.0
        }),
        1
    );
    assert_eq!(s.process(8)[0], vec![1.0 + 10.0 + 200.0; 8]);
}

#[test]
fn bound_controls_replay_from_the_log() {
    let mut s = session(BOUND);
    let mut original = Vec::new();
    for (i, value) in [0u8, 32, 64, 127].into_iter().enumerate() {
        s.control_input(&ControlSignal::MidiCc {
            cc: 7,
            channel: 1,
            value,
        });
        s.control_input(&ControlSignal::Osc {
            address: "/fx/wet",
            value: i as f64 / 3.0,
        });
        original.extend(s.process(100).remove(0));
    }
    let log = from_lines(&to_lines(&s.log())).unwrap();
    let replay = niminal_daemon::Session::replay(48_000.0, Layout::Mono,
        &log,
        400,
        false,
    );
    assert_eq!(original, replay[0]);
}

const LEARNING: &str = "ctl level = midi.learn.range(0..1).smooth(0ms)\nctl pan = midi.learn.range(0..1).smooth(0ms)\ninstr flat() { level + pan * 10 }\nflat() for 1beat";

#[test]
fn midi_learn_takes_the_next_control_change_and_reports_it() {
    let mut s = session(LEARNING);
    // nothing is moved until a control change is heard
    assert_eq!(s.control_values()[0].1, 0.0);
    // the first control waiting takes the first CC
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 21,
            channel: 3,
            value: 127
        }),
        1
    );
    assert_eq!(
        s.take_learned(),
        [("level".to_string(), "midi.cc(21)".to_string())]
    );
    // and the next control the next CC; a CC that is already learned moves only its control
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 22,
            channel: 1,
            value: 127
        }),
        1
    );
    assert_eq!(
        s.take_learned(),
        [("pan".to_string(), "midi.cc(22)".to_string())]
    );
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 21,
            channel: 1,
            value: 0
        }),
        1
    );
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 30,
            channel: 1,
            value: 127
        }),
        0,
        "nothing left to learn"
    );
    assert!(s.take_learned().is_empty());
    // the control still follows what it learned
    assert_eq!(s.process(8)[0], vec![0.0 + 10.0; 8]);
}
