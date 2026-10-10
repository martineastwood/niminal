use niminal_daemon::{ControlSignal, Session, QuantizeDefaults, from_lines, to_lines};
use niminal_lang::Layout;

const SOURCE: &str = "ctl level = 0.0.smooth(0ms)\ninstr flat() { level }\nflat() for 1beat";
fn session(source: &str) -> Session {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(source, Some("now")).unwrap();
    s
}

#[test]
fn controls_change_held_notes_on_the_exact_sample_without_recompiling() {
    let mut s = session(SOURCE);
    let graph = s.program().instruments[0].graph.clone();
    s.set_control("level", 1.0, None, Some(37), None).unwrap();
    let out = s.process(100).remove(0);
    assert_eq!(out[..37], [0.0; 37]);
    assert_eq!(out[37..], [1.0; 63]);
    assert!(std::sync::Arc::ptr_eq(&graph, &s.program().instruments[0].graph));
}

#[test]
fn smoothing_is_linear_and_reaches_the_target_at_the_requested_duration() {
    let mut s = session(SOURCE);
    s.set_control("level", 1.0, None, Some(48), Some(1.0)).unwrap();
    let out = s.process(100).remove(0);
    assert_eq!(out[..48], [0.0; 48]);
    for (i, &v) in out[48..96].iter().enumerate() {
        assert!((v - (i+1) as f32 / 48.0).abs() < 1e-6);
    }
    assert_eq!(out[95..], [1.0; 5]);
}

#[test]
fn one_control_drives_instruments_and_effects_and_initializes_new_notes() {
    let mut s = session("ctl level = 0.0.smooth(0ms)\ninstr flat() { level }\ntrack t { instrument = flat\n out = it * level }\nat 100ms t() for 20ms");
    s.set_control("level", 0.5, None, Some(48), None).unwrap();
    let out = s.process(5200).remove(0);
    assert_eq!(out[..4800], vec![0.0; 4800]);
    assert_eq!(out[4800..], vec![0.25; 400]);
}

#[test]
fn unrelated_edits_keep_control_values_and_redefinitions_glide() {
    let mut s = session(SOURCE);
    s.set_control("level", 0.75, None, None, None).unwrap();
    assert_eq!(s.process(64)[0], vec![0.75; 64]);
    s.eval("instr other() { 0.0 }", Some("now")).unwrap();
    assert_eq!(s.process(64)[0], vec![0.75; 64]);
    s.eval("ctl level = 1.0.smooth(1ms)", Some("now")).unwrap();
    let out = s.process(64).remove(0);
    assert!(out[0] > 0.75 && out[0] < 1.0);
    assert_eq!(out[47..], [1.0; 17]);
}

#[test]
fn units_smoothing_timestamps_and_unknown_controls_are_validated() {
    let mut s = session("ctl cutoff = 1khz\nctl gain = -6db");
    assert!(s.set_control("missing", 1.0, None, None, None).is_err());
    assert!(s.set_control("cutoff", 1.0, Some("db"), None, None).is_err());
    assert!(s.set_control("cutoff", f64::NAN, None, None, None).is_err());
    assert!(s.set_control("cutoff", 1.0, None, None, Some(-1.0)).is_err());
    assert!(s.set_control("cutoff", 1.0, None, None, Some(f64::INFINITY)).is_err());
    s.set_control("cutoff", 2.0, Some("khz"), None, Some(0.0)).unwrap();
    s.set_control("gain", -12.0, None, None, Some(0.0)).unwrap();
    s.process(1);
    let values = s.control_values();
    assert_eq!(values[0].1, 2000.0);
    assert!((values[1].1 - 10f32.powf(-12.0 / 20.0)).abs() < 1e-6);
    assert!(s.set_control("cutoff", 1.0, None, Some(0), None).is_err());
}

#[test]
fn out_of_order_updates_and_same_sample_updates_are_deterministic() {
    let mut s = session(SOURCE);
    s.set_control("level", 0.2, None, Some(80), None).unwrap();
    s.set_control("level", 0.4, None, Some(32), None).unwrap();
    s.set_control("level", 0.6, None, Some(32), None).unwrap();
    let out = s.process(100).remove(0);
    assert_eq!(out[..32], [0.0; 32]);
    assert_eq!(out[32..80], [0.6; 48]);
    assert_eq!(out[80..], [0.2; 20]);
}

#[test]
fn controls_replay_exactly_across_block_sizes_and_log_serialization() {
    let mut s = session(SOURCE);
    s.set_control("level", 1.0, None, Some(37), Some(2.0)).unwrap();
    let mut original = s.process(100).remove(0);
    s.set_control("level", 0.3, None, None, Some(1.0)).unwrap();
    s.eval("at 3ms flat() for 1beat", Some("now")).unwrap();
    original.extend(s.process(800).remove(0));
    let log = from_lines(&to_lines(s.log())).unwrap();
    let replay = Session::replay(48_000.0, Layout::Mono, QuantizeDefaults::default(), &log, 900, false);
    assert_eq!(original, replay[0]);
}

#[test]
fn queued_updates_are_bounded() {
    let mut s = session(SOURCE);
    for i in 0..1024 { s.set_control("level", 1.0, None, Some(1000 + i), None).unwrap(); }
    assert!(s.set_control("level", 1.0, None, Some(2000), None).is_err());
    s.process(2100);
    assert!(s.set_control("level", 0.0, None, None, None).is_ok());
}

#[test]
fn controls_have_compiler_diagnostics_and_cannot_shadow_other_names() {
    for source in [
        "ctl x = 0.0\nctl x = 1.0",
        "ctl it = 0.0",
        "ctl out = 1.0",
        "ctl x = osc(sine, 440hz)",
        "ctl x = 1.0.smooth(-1ms)",
        "ctl x = 1.0.smooth(1.0)",
        "ctl x = 1.0.smooth(61sec)",
        "ctl x = 1.0\ninstr x() { 0.0 }",
        "ctl cutoff = 0.5\ninstr x() { osc(sine, cutoff) }",
    ] {
        assert!(niminal_lang::compile(source).is_err(), "{source}");
    }
    let program = niminal_lang::compile("ctl freq = 440hz\ninstr tone() { osc(sine, freq) }").unwrap();
    assert_eq!(program.controls[0].unit, niminal_lang::Unit::Hz);
}

#[test]
fn code_redeclaration_restores_an_external_override() {
    let mut s = session(SOURCE);
    s.set_control("level", 0.5, None, None, None).unwrap();
    s.process(64);
    s.eval("ctl level = 0.0.smooth(0ms)", Some("now")).unwrap();
    assert_eq!(s.process(64)[0], [0.0; 64]);
}

#[test]
fn negative_decibel_defaults_can_declare_smoothing() {
    let program = niminal_lang::compile("ctl volume = -6db.smooth(10ms)").unwrap();
    let def = &program.controls[0];
    assert_eq!(def.unit, niminal_lang::Unit::Db);
    assert!((def.initial - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
    assert_eq!(def.smooth_seconds, 0.01);
}

#[test]
fn smoothing_between_finite_extremes_remains_finite() {
    let mut s = session(SOURCE);
    s.set_control("level", -3e38, None, Some(0), Some(0.0)).unwrap();
    s.set_control("level", 3e38, None, Some(1), Some(1.0)).unwrap();
    let out = s.process(64).remove(0);
    assert!(out.iter().all(|v| v.is_finite()));
    assert!(out[1] < 0.0 && out[48] > 0.0);
    assert_eq!(s.take_silenced(), 0);
}

const BOUND: &str = "ctl level = midi.cc(7).range(0..1).smooth(0ms)\nctl pan = midi.cc(10, 2).range(0..1).smooth(0ms)\nctl wet = osc_in(\"/fx/wet\").range(0..2).smooth(0ms)\ninstr flat() { level + pan * 10 + wet * 100 }\nflat() for 1beat";

#[test]
fn midi_and_osc_messages_move_the_controls_bound_to_them() {
    let mut s = session(BOUND);
    assert_eq!(s.control_input(&ControlSignal::MidiCc { cc: 7, channel: 1, value: 127 }), 1);
    // bound to channel 2 only, and to a different CC
    assert_eq!(s.control_input(&ControlSignal::MidiCc { cc: 10, channel: 1, value: 127 }), 0);
    assert_eq!(s.control_input(&ControlSignal::MidiCc { cc: 11, channel: 2, value: 127 }), 0);
    assert_eq!(s.control_input(&ControlSignal::MidiCc { cc: 10, channel: 2, value: 127 }), 1);
    assert_eq!(s.control_input(&ControlSignal::Osc { address: "/fx/wet", value: 0.5 }), 1);
    assert_eq!(s.control_input(&ControlSignal::Osc { address: "/fx/other", value: 0.5 }), 0);
    // outside 0..1 clamps to the range
    assert_eq!(s.control_input(&ControlSignal::Osc { address: "/fx/wet", value: 7.0 }), 1);
    assert_eq!(s.process(8)[0], vec![1.0 + 10.0 + 200.0; 8]);
}

#[test]
fn bound_controls_replay_from_the_log() {
    let mut s = session(BOUND);
    let mut original = Vec::new();
    for (i, value) in [0u8, 32, 64, 127].into_iter().enumerate() {
        s.control_input(&ControlSignal::MidiCc { cc: 7, channel: 1, value });
        s.control_input(&ControlSignal::Osc { address: "/fx/wet", value: i as f64 / 3.0 });
        original.extend(s.process(100).remove(0));
    }
    let log = from_lines(&to_lines(s.log())).unwrap();
    let replay = Session::replay(48_000.0, Layout::Mono, QuantizeDefaults::default(), &log, 400, false);
    assert_eq!(original, replay[0]);
}
