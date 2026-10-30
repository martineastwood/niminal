use niminal_lang::Layout;
use niminal_live::{ControlSignal, Offline};
use niminal_server::{from_lines, to_lines};

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
    let replay = niminal_server::Session::replay(48_000.0, Layout::Mono, &log, 400, false);
    assert_eq!(original, replay[0]);
}

#[test]
fn a_control_bound_to_a_key_is_open_while_the_key_is_down() {
    let mut s = session(
        "ctl held = midi.note(36).range(0..1).smooth(0ms)\ninstr flat() { held }\nflat() for 1beat",
    );
    let key = |note, velocity| ControlSignal::MidiNote {
        note,
        channel: 1,
        velocity,
    };
    assert_eq!(s.process(4)[0], vec![0.0; 4]);
    // another key leaves it alone
    s.control_input(&key(37, 127));
    assert_eq!(s.process(4)[0], vec![0.0; 4]);
    s.control_input(&key(36, 127));
    assert_eq!(s.process(4)[0], vec![1.0; 4]);
    s.control_input(&key(36, 0));
    assert_eq!(s.process(4)[0], vec![0.0; 4]);
}

const MACRO: &str = "ctl cutoff = 1khz\nctl drive = 0.3\nmacro bright {\n  input: midi.cc(21)\n  smooth: 0ms\n  cutoff: 200hz..4khz\n  drive: 1..0\n}\ninstr flat() { drive }\nflat() for 1beat";

#[test]
fn a_macro_moves_its_controls_when_it_is_moved() {
    let mut s = session(MACRO);
    let value = |s: &Offline, name: &str| {
        s.control_values()
            .into_iter()
            .find(|c| c.0 == name)
            .unwrap()
            .2
    };
    // a controller sends the macro to the top: the frequency is at the top of its range and the other control
    // at the end of its range, which is lower
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc: 21,
            channel: 1,
            value: 127
        }),
        1
    );
    s.process(64);
    assert!((value(&s, "bright") - 1.0).abs() < 1e-6);
    assert!(
        (value(&s, "cutoff") - 4000.0).abs() < 0.5,
        "{}",
        value(&s, "cutoff")
    );
    assert!(value(&s, "drive").abs() < 1e-6);
    // half way is half way by ear for the frequency, and half way for the plain number
    s.control_input(&ControlSignal::MidiCc {
        cc: 21,
        channel: 1,
        value: 64,
    });
    s.process(64);
    let t = 64.0 / 127.0;
    assert!(
        (value(&s, "cutoff") - 200.0 * 20.0_f32.powf(t)).abs() < 1.0,
        "{}",
        value(&s, "cutoff")
    );
    assert!(
        (value(&s, "drive") - (1.0 - t)).abs() < 1e-4,
        "{}",
        value(&s, "drive")
    );
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

const PADS: &str = "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip beat { instrument: pluck\n notes: [c4] }\nclip fill { instrument: pluck\n notes: [e4] }\non midi.note(36) beat.start\non midi.note(37, 10) {\n  fill.start\n  beat.stop\n}\non midi.cc(64) beat.reverse\non osc_in(\"/stop\") hush\n";

fn running(s: &Offline) -> Vec<String> {
    s.clips()
        .into_iter()
        .filter(|c| c.running)
        .map(|c| c.name)
        .collect()
}

fn note(note: u8, channel: u8, velocity: u8) -> ControlSignal<'static> {
    ControlSignal::MidiNote {
        note,
        channel,
        velocity,
    }
}

#[test]
fn a_key_can_run_commands() {
    let mut s = session(PADS);
    s.process(100);
    assert!(running(&s).is_empty());
    assert_eq!(s.control_input(&note(36, 1, 100)), 1);
    s.process(96_000 * 2);
    s.advance();
    assert_eq!(running(&s), ["beat"], "the pad started the clip");
    // the wrong channel, and a key coming up, do nothing
    assert_eq!(s.control_input(&note(37, 1, 100)), 0);
    assert_eq!(s.control_input(&note(37, 10, 0)), 0);
    assert_eq!(s.control_input(&note(37, 10, 90)), 1);
    s.process(96_000 * 2);
    s.advance();
    assert_eq!(running(&s), ["fill"], "both commands of the pad ran");
}

#[test]
fn a_controller_sets_off_once_each_time_it_goes_down() {
    let mut s = session(PADS);
    s.eval("beat.start", Some("now")).unwrap();
    s.process(96_000);
    let edits = |s: &Offline| {
        s.clips()
            .into_iter()
            .find(|c| c.name == "beat")
            .unwrap()
            .edits
            .len()
    };
    let cc = |value| ControlSignal::MidiCc {
        cc: 64,
        channel: 1,
        value,
    };
    assert_eq!(s.control_input(&cc(127)), 1);
    assert_eq!(s.control_input(&cc(110)), 0, "still down");
    s.process(96_000 * 2);
    s.advance();
    assert_eq!(edits(&s), 1);
    assert_eq!(s.control_input(&cc(0)), 0);
    assert_eq!(s.control_input(&cc(127)), 1, "down again");
    s.process(96_000 * 2);
    s.advance();
    assert_eq!(edits(&s), 2);
}

#[test]
fn an_osc_message_can_run_commands() {
    let mut s = session(PADS);
    s.eval("beat.start", Some("now")).unwrap();
    s.process(96_000);
    assert_eq!(
        s.control_input(&ControlSignal::Osc {
            address: "/other",
            value: 1.0
        }),
        0
    );
    assert_eq!(
        s.control_input(&ControlSignal::Osc {
            address: "/stop",
            value: 1.0
        }),
        1
    );
    s.process(96_000 * 2);
    s.advance();
    assert!(running(&s).is_empty(), "hush stopped everything");
}

#[test]
fn pads_launch_scenes_and_each_switches_the_other_off() {
    let mut s = session(
        "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip drums { instrument: pluck\n notes: [c3] }\nclip bass { instrument: pluck\n notes: [c2] }\nclip pad { instrument: pluck\n notes: [e4] }\nscene verse { drums, bass\n launch: midi.note(36) }\nscene chorus { drums, pad\n launch: midi.note(37) }\n",
    );
    s.process(100);
    s.control_input(&note(36, 1, 100));
    s.process(96_000 * 2);
    s.advance();
    assert_eq!(running(&s), ["drums", "bass"], "the verse is playing");
    s.control_input(&note(37, 1, 100));
    s.process(96_000 * 2);
    s.advance();
    assert_eq!(
        running(&s),
        ["drums", "pad"],
        "the chorus took over: bass stopped, drums carried on"
    );
}
