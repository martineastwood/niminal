use crate::common::*;

#[test]
fn new_forgets_the_old_piece_so_another_can_start() {
    let mut s = ready();
    s.eval("lead.play([c4 e4])", Some("now")).unwrap();
    run(&mut s, 4_800);
    s.eval("new", Some("now")).unwrap();
    let silent = run(&mut s, BAR);
    assert!(peak(&silent) < 1e-6, "everything stopped");
    assert!(
        s.parts()
            .iter()
            .all(|t| t.name != "lead" && t.name != "bass"),
        "the old nodes are gone"
    );
    s.eval("instr pluck(freq: hz) { osc(saw, freq) }\npart drone { instrument = pluck }\ndrone.play([a2])", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)) > 1e-3, "the new piece plays");
    assert_eq!(
        s.parts()
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["drone"]
    );
}

#[test]
fn new_and_the_next_piece_can_arrive_together() {
    let mut s = ready();
    s.eval("new\ninstr pluck(freq: hz) { osc(saw, freq) }\npart drone { instrument = pluck }\ndrone.play([a2])", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)) > 1e-3);
    assert_eq!(
        s.parts()
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["drone"]
    );
}

#[test]
fn an_envelope_lane_moves_a_node_parameter_as_the_clip_plays() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\nbus pad_audio\nprocessor pad_fx(cut: hz = 300hz) {out = pad_audio.lpf(cutoff: cut)}\npart pad { instrument = saw1\noutput = pad_audio}", Some("now")).unwrap();
    s.eval(
        "clip c { notes: [c5]\npad_fx.cut: env[300hz 1bar 1300hz] }\npad.play(c)",
        Some("now"),
    )
    .unwrap();
    let cut = |s: &Offline| {
        s.control_values()
            .into_iter()
            .find(|v| v.0 == "pad_fx::cut")
            .unwrap()
            .1
    };
    run(&mut s, 1_000);
    assert!((cut(&s) - 300.0).abs() < 20.0, "{}", cut(&s));
    run(&mut s, BAR / 2 - 1_000);
    assert!((cut(&s) - 800.0).abs() < 30.0, "half way up: {}", cut(&s));
    run(&mut s, BAR);
    assert!(
        (cut(&s) - 1300.0).abs() < 1.0,
        "it holds the last level: {}",
        cut(&s)
    );
}

#[test]
fn a_signal_lane_moves_a_node_parameter_continuously_even_without_notes() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\nbus pad_audio\nprocessor pad_fx(cut: hz = 300hz) {out = pad_audio.lpf(cutoff: cut)}\npart pad { instrument = saw1\noutput = pad_audio}", Some("now")).unwrap();
    s.eval(
        "clip c { pad_fx.cut: saw.range(300hz, 1300hz) }\npad.play(c)",
        Some("now"),
    )
    .unwrap();
    let cut = |s: &Offline| {
        s.control_values()
            .into_iter()
            .find(|v| v.0 == "pad_fx::cut")
            .unwrap()
            .1
    };
    run(&mut s, 1_000);
    assert!((cut(&s) - 300.0).abs() < 60.0, "{}", cut(&s));
    run(&mut s, BAR / 2 - 1_000);
    assert!((cut(&s) - 800.0).abs() < 60.0, "half way up: {}", cut(&s));
    run(&mut s, BAR / 4);
    assert!(
        (cut(&s) - 1050.0).abs() < 60.0,
        "three quarters: {}",
        cut(&s)
    );
    run(&mut s, BAR / 4 + 1_000);
    assert!(
        cut(&s) < 400.0,
        "the saw starts over with the next bar: {}",
        cut(&s)
    );
}

#[test]
fn set_can_give_a_part_a_node_parameter_lane_and_a_signal() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\nbus pad_audio\nprocessor pad_fx(cut: hz = 300hz) {out = pad_audio.lpf(cutoff: cut)}\npart pad { instrument = saw1\noutput = pad_audio}", Some("now")).unwrap();
    s.eval("pad.play([c5])", Some("now")).unwrap();
    let cut = |s: &Offline| {
        s.control_values()
            .into_iter()
            .find(|v| v.0 == "pad_fx::cut")
            .unwrap()
            .1
    };
    run(&mut s, 1_000);
    s.eval("pad.set(pad_fx.cut: 900hz)", Some("now")).unwrap();
    run(&mut s, BAR + 1_000);
    assert!(
        (cut(&s) - 900.0).abs() < 1.0,
        "a note sets it as it starts: {}",
        cut(&s)
    );
    s.eval("pad.set(pad_fx.cut: saw.range(300hz, 1300hz))", Some("now"))
        .unwrap();
    run(&mut s, BAR / 2);
    assert!(
        (cut(&s) - 800.0).abs() < 100.0,
        "and a signal moves it: {}",
        cut(&s)
    );
}

mod mpe {
    use super::*;
    use niminal_live::ControlSignal;

    const ORGAN: &str = "tempo 120bpm\ninstr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0, slide: 0..1 = 0) { osc(sine, freq + bend) * (0.1 + pressure * 0.4 + slide * 0.4) * env[1 | 10ms 0] }\nclip pad { instrument: organ\n keys: midi.keys(mpe: true) }\npad.start";

    fn mpe_session() -> Offline {
        let mut s = session();
        s.eval(ORGAN, Some("now")).unwrap();
        s
    }

    fn hz(x: &[f32]) -> f64 {
        crossings(x) as f64 / (x.len() as f64 / f64::from(SR))
    }

    fn bend(channel: u8, value: u16) -> ControlSignal<'static> {
        ControlSignal::MidiBend { channel, value }
    }

    #[test]
    fn every_member_channel_plays_and_the_master_channel_does_not() {
        let mut s = mpe_session();
        run(&mut s, 600);
        assert_eq!(
            s.key_input(1, 69, 100),
            0,
            "the master channel carries no notes"
        );
        assert_eq!(s.key_input(2, 69, 100), 1);
        assert_eq!(s.key_input(9, 72, 100), 1);
        assert!(peak(&run(&mut s, 2048)) > 0.05);
        // a plain keyboard clip still answers to its one channel
        let mut plain = session();
        plain
            .eval(&ORGAN.replace("mpe: true", "channel: 1"), Some("now"))
            .unwrap();
        assert_eq!(plain.key_input(2, 69, 100), 0);
        assert_eq!(plain.key_input(1, 69, 100), 1);
    }

    #[test]
    fn a_note_is_bent_by_its_own_channel_over_forty_eight_semitones() {
        let mut s = mpe_session();
        run(&mut s, 600);
        s.key_input(2, 69, 100);
        s.key_input(3, 69, 100);
        run(&mut s, 2048);
        // a quarter of the way up the range is 12 semitones: an octave
        s.control_input(&bend(2, 8192 + 2048));
        let both = run(&mut s, 24_000);
        // the other note stays at 440, so the pair sound at 440 and 880 together
        let alone = {
            let mut one = mpe_session();
            run(&mut one, 600);
            one.key_input(2, 69, 100);
            run(&mut one, 2048);
            one.control_input(&bend(2, 8192 + 2048));
            run(&mut one, 24_000)
        };
        assert!(
            (hz(&alone[4000..]) - 880.0).abs() < 20.0,
            "{}",
            hz(&alone[4000..])
        );
        assert!(
            rms(&both) > rms(&alone) * 1.1,
            "two notes are louder than one"
        );
    }

    #[test]
    fn the_master_channel_moves_every_note_by_two_semitones_at_most() {
        let mut s = mpe_session();
        run(&mut s, 600);
        s.key_input(2, 69, 100);
        run(&mut s, 2048);
        s.control_input(&bend(1, 16383));
        let out = run(&mut s, 24_000);
        let expected = 440.0 * 2f64.powf(2.0 / 12.0);
        assert!(
            (hz(&out[4000..]) - expected).abs() < 15.0,
            "{} vs {expected}",
            hz(&out[4000..])
        );
    }

    #[test]
    fn pressure_and_slide_reach_the_note_on_their_channel() {
        let level = |expression: ControlSignal| {
            let mut s = mpe_session();
            run(&mut s, 600);
            s.key_input(2, 69, 100);
            s.key_input(3, 69, 100);
            run(&mut s, 2048);
            s.control_input(&expression);
            peak(&run(&mut s, 4096))
        };
        let quiet = level(ControlSignal::MidiCc {
            cc: 1,
            channel: 2,
            value: 0,
        });
        assert!(
            level(ControlSignal::MidiPressure {
                channel: 2,
                note: None,
                value: 127
            }) > quiet * 1.4
        );
        assert!(
            level(ControlSignal::MidiCc {
                cc: 74,
                channel: 3,
                value: 127
            }) > quiet * 1.4
        );
    }

    #[test]
    fn the_pedal_on_the_master_channel_holds_every_note() {
        let mut s = mpe_session();
        run(&mut s, 600);
        s.key_input(2, 69, 100);
        s.key_input(3, 72, 100);
        run(&mut s, 2048);
        s.control_input(&ControlSignal::MidiCc {
            cc: 64,
            channel: 1,
            value: 127,
        });
        s.key_input(2, 69, 0);
        s.key_input(3, 72, 0);
        assert!(peak(&run(&mut s, 12_000)) > 0.05, "held by the pedal");
        s.control_input(&ControlSignal::MidiCc {
            cc: 64,
            channel: 1,
            value: 0,
        });
        run(&mut s, 4_000);
        assert!(peak(&run(&mut s, 8_000)) < 1e-3, "and let go with it");
    }

    #[test]
    fn mpe_keys_cannot_name_a_channel() {
        let mut s = session();
        let err = s
            .eval(
                &ORGAN.replace("mpe: true", "mpe: true, channel: 1"),
                Some("now"),
            )
            .unwrap_err();
        assert!(
            err[0].message.contains("takes no `channel:`"),
            "{}",
            err[0].message
        );
    }
}

mod quantize_settings {
    use super::*;

    #[test]
    fn a_projects_config_sets_when_each_kind_of_change_lands() {
        let mut s = ready();
        s.eval(
            "config { quantize: { play: beat\n tempo: 2bars\n default: 4beats } }",
            Some("now"),
        )
        .unwrap();
        run(&mut s, 10_000);
        // starting a clip now waits for the next beat (24000 samples), not the next bar
        let play = s.eval("lead.play([c4])", None).unwrap();
        assert_eq!(play.lands_at, 24_000);
        // other commands take `default`: four beats is a bar
        let mute = s.eval("lead.mute", None).unwrap();
        assert_eq!(mute.lands_at, BAR as u64);
        let tempo = s.eval("tempo 100bpm", None).unwrap();
        assert_eq!(
            tempo.lands_at,
            2 * BAR as u64,
            "a tempo change waits for two bars"
        );
    }

    #[test]
    fn a_definition_can_say_when_it_lands() {
        let mut s = ready();
        run(&mut s, 10_000);
        // a node redefinition lands on the bar unless it says otherwise
        let plain = s.eval("part fx { instrument = pluck }", None).unwrap();
        assert_eq!(plain.lands_at, BAR as u64);
        let now = s
            .eval("part fx2 { instrument = pluck\nquantize = now}", None)
            .unwrap();
        assert_eq!(now.lands_at, 10_000, "it says now");
        let beat = s
            .eval("clip riff { notes: [c4], quantize: 1beat }", None)
            .unwrap();
        assert_eq!(beat.lands_at, 24_000);
        let instr = s
            .eval(
                "instr organ(freq: hz) { quantize = 2beats\n osc(sine, freq) }",
                None,
            )
            .unwrap();
        assert_eq!(instr.lands_at, 48_000);
        // the evaluation's own setting still wins over the definition's
        let over = s
            .eval(
                "part fx4 { instrument = pluck\nquantize = now}",
                Some("next 4bars"),
            )
            .unwrap();
        assert_eq!(over.lands_at, 4 * BAR as u64);
    }

    #[test]
    fn quantum_mistakes_are_reported() {
        let mut s = ready();
        let message =
            |s: &mut Offline, src: &str| s.eval(src, Some("now")).unwrap_err()[0].message.clone();
        assert_eq!(
            message(&mut s, "config { quantize: { sometimes: bar } }"),
            "unknown kind of change `sometimes`"
        );
        assert_eq!(
            message(&mut s, "config { quantize: { play: soon } }"),
            "expected when a change lands"
        );
        assert_eq!(
            message(
                &mut s,
                "part t2 { instrument = pluck\nquantize = 2hz}"
            ),
            "expected when a change lands"
        );
    }
}

#[test]
fn an_osc_out_tap_holds_the_latest_value_of_its_signal_while_audio_runs() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\nbus lead_audio\nprocessor lead_fx {out = lead_audio.osc_out(\"/level\", port: 9000)}\npart lead { instrument = tone\noutput = lead_audio}\nctl k = 0.25\nprocessor ctl_node {\n out = k.osc_out(\"/knob\", port: 9000)\n}", Some("now")).unwrap();
    let taps = s.engine().session().program().taps.clone();
    assert_eq!(taps.len(), 2);
    let (lead, knob) = (taps[0].clone(), taps[1].clone());
    run(&mut s, 2_000);
    assert_eq!(knob.value.read(), 0.25, "a control's value, as it is");
    assert_eq!(lead.value.read(), 0.0, "silence until a note");
    s.eval("lead.play([a4])", Some("now")).unwrap();
    let mut seen = 0.0f32;
    for _ in 0..40 {
        run(&mut s, 128);
        seen = seen.max(lead.value.read().abs());
    }
    assert!(seen > 0.3, "the node's signal reaches the tap: {seen}");
    s.eval("ctl k = 0.75", Some("now")).unwrap();
    run(&mut s, 4_000);
    let knob = s.engine().session().program().taps[1].value.read();
    assert!((knob - 0.75).abs() < 1e-3, "{knob}");
}

#[test]
fn a_session_can_be_given_audio_for_nodes_that_read_input() {
    let mut s = session();
    s.eval(
        "tempo 120bpm\nprocessor mic { out = input(1) * 0.5 }",
        Some("now"),
    )
    .unwrap();
    s.set_audio_input(vec![(0..10_000).map(|i| i as f32 / 20_000.0).collect()]);
    let out = run(&mut s, 12_000);
    assert!(
        (0..10_000).all(|i| (out[i] - i as f32 / 40_000.0).abs() < 1e-6),
        "it hears the buffer from sample 0"
    );
    assert!(
        out[10_000..].iter().all(|v| *v == 0.0),
        "and nothing after it ends"
    );
}

#[test]
fn a_midi_out_instrument_sends_notes_with_their_times_and_stays_silent() {
    let mut s = session();
    s.eval("tempo 120bpm\nmidi_out synth(port: \"IAC\", channel: 3)\npart lead { instrument = synth }", Some("now")).unwrap();
    s.eval("lead.play([c4 e4 ~ g4])", Some("now")).unwrap();
    let out = run(&mut s, BAR);
    assert!(peak(&out) == 0.0, "the sound is outside");
    let events = s.take_midi();
    let shown: Vec<_> = events
        .iter()
        .map(|e| (e.at, e.on, e.note, e.channel, &*e.port))
        .collect();
    // a bar is 96000 samples: four steps of 24000, the third a rest
    assert_eq!(
        shown,
        [
            (0, true, 60, 3, "IAC"),
            (24_000, false, 60, 3, "IAC"),
            (24_000, true, 64, 3, "IAC"),
            (48_000, false, 64, 3, "IAC"),
            (72_000, true, 67, 3, "IAC")
        ],
        "{shown:?}"
    );
    assert!(s.take_midi().is_empty(), "taken once");
    // the last note ends with its step, as the clip starts over
    run(&mut s, 24_000);
    let rest = s.take_midi();
    assert_eq!(
        rest.iter()
            .map(|e| (e.at, e.on, e.note))
            .collect::<Vec<_>>(),
        [(BAR as u64, false, 67), (BAR as u64, true, 60)]
    );
}

#[test]
fn panic_and_hush_end_midi_notes_that_are_sounding() {
    let mut s = session();
    s.eval("tempo 120bpm\nmidi_out synth(port: \"IAC\")\npart lead { instrument = synth }\nlead.play([c4])", Some("now")).unwrap();
    run(&mut s, 1_000);
    assert_eq!(s.take_midi().len(), 1);
    s.eval("panic", Some("now")).unwrap();
    run(&mut s, 100);
    let events = s.take_midi();
    assert_eq!(
        events.iter().map(|e| (e.on, e.note)).collect::<Vec<_>>(),
        [(false, 60)],
        "{events:?}"
    );
}
