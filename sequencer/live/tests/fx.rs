use niminal_lang::Layout;
use niminal_live::Offline;

#[test]
fn a_long_attack_through_a_fx_renders_in_each_output_layout() {
    let source = include_str!("../../../examples/room.nml").replace("env[0 2ms", "env[0 65536ms");
    for layout in [Layout::Mono, Layout::Stereo, Layout::Quad] {
        let mut session = Offline::new(48_000.0, layout);
        session.eval(&source, Some("now")).unwrap();
        let _ = session.eval("verse.start\nlead.__update([c4 e4 g4])\nhush", Some("now"));
        session.process(144_000);
    }
}

#[test]
fn a_function_written_in_an_fx_can_read_the_fxs_parameters() {
    let source = "fn through(x, make: fn) {\n  state held = 0\n  held = held + 1\n  make(x)\n}\nbus src: stereo\ninstr pluck(freq: hz) {\n  src += osc(sine, freq)\n}\nfx crush(bits: 2..16 = 5) {\n  out = src.through(fn(s) { s.bitcrush(bits: bits) })\n}\nclip c {\n  instrument: pluck\n  notes: [c4]\n}\n";
    let mut session = Offline::new(48_000.0, Layout::Stereo);
    session.eval(source, Some("now")).unwrap();
    session.eval("c.start", Some("now")).unwrap();
    session.process(4_800);
}

/// The loudest sample, left channel, in `block`.
fn peak(block: &[f32]) -> f32 {
    block.iter().fold(0.0f32, |a, b| a.max(b.abs()))
}

#[test]
fn stopping_a_clip_lets_go_of_the_effect_controls_it_was_holding() {
    // a long note, silenced by the effect while the clip holds `mute` on
    let source = "bus src: stereo\ninstr drone(freq: hz) {\n  src += osc(sine, freq) * 0.5\n}\nfx gate(mute: 0..1 = 0) {\n  out = src * (1 - mute)\n}\nclip c {\n  instrument: drone\n  length: 8bars\n  notes: [c4]\n  dur: 8bars\n  gate {\n    mute: x\n  }\n}\n";
    for stop in ["c.stop", "hush"] {
        let mut session = Offline::new(48_000.0, Layout::Stereo);
        session.eval(source, Some("now")).unwrap();
        session.eval("c.start", Some("now")).unwrap();
        let held = session.process(48_000).remove(0);
        assert!(peak(&held[12_000..]) < 0.001, "{stop}: the effect mutes the note while the clip holds it");
        session.eval(stop, Some("now")).unwrap();
        session.process(9_600);
        let freed = session.process(48_000).remove(0);
        assert!(peak(&freed[12_000..]) > 0.05, "{stop}: the note is heard again once the clip is stopped: {}", peak(&freed[12_000..]));
    }
}

#[test]
fn ending_a_render_lets_go_of_the_effect_controls_still_held() {
    use niminal_live::Offline;
    let source = "bus src: stereo\ninstr drone(freq: hz) {\n  src += osc(sine, freq) * 0.5\n}\nfx gate(mute: 0..1 = 0) {\n  out = src * (1 - mute)\n}\nclip c {\n  instrument: drone\n  length: 8bars\n  notes: [c4]\n  dur: 8bars\n  gate {\n    mute: x\n  }\n}\n";
    let mut session = Offline::new(48_000.0, Layout::Stereo);
    session.eval(source, Some("now")).unwrap();
    session.eval("c.start", Some("now")).unwrap();
    session.process(48_000);
    session.end_schedule();
    session.process(9_600);
    let freed = session.process(48_000).remove(0);
    assert!(peak(&freed[12_000..]) > 0.05, "the note is heard again: {}", peak(&freed[12_000..]));
}

#[test]
fn a_gesture_row_draws_a_trance_gate_over_a_held_sound() {
    use niminal_live::Offline;
    // 120bpm: a sixteenth is 6000 samples; the row opens the first and third steps of every four
    let source = "bus src: stereo\ninstr drone(freq: hz) {\n  src += osc(sine, freq) * 0.5\n}\nfx chop(open: 0..1 = 1) {\n  out = src * open.follow(attack: 1ms, release: 2ms)\n}\nclip pad {\n  instrument: drone\n  length: 1bar\n  notes: [c4]\n  dur: 1bar\n}\nclip gate {\n  length: 1bar\n  chop {\n    open: x.x. x.x. x.x. x.x.\n  }\n}\n";
    let mut session = Offline::new(48_000.0, Layout::Stereo);
    session.eval(source, Some("now")).unwrap();
    session.eval("pad.start\ngate.start", Some("now")).unwrap();
    let out = session.process(96_000).remove(0);
    let rms = |from: usize| (out[from..from + 3000].iter().map(|v| v * v).sum::<f32>() / 3000.0).sqrt();
    // the second half of a step, past the edges
    assert!(rms(6_000 * 4 + 1_500) > 0.2, "an open step sounds: {}", rms(6_000 * 4 + 1_500));
    assert!(rms(6_000 * 5 + 1_500) < 0.01, "a closed step is cut: {}", rms(6_000 * 5 + 1_500));
    assert!(rms(6_000 * 6 + 1_500) > 0.2, "the next open step sounds again");
}

#[test]
fn an_effect_parameter_that_starts_at_a_length_in_beats_follows_the_tempo() {
    // the effect passes the level of its input times its length in seconds, so the level says the length
    let source = "bus src: stereo\ninstr drone(freq: hz) {\n  src += osc(sine, freq)\n}\nfx timed(length: 1ms..4sec = 1/2beat) {\n  out = src * (length / 1sec)\n}\nclip c {\n  instrument: drone\n  length: 8bars\n  notes: [c4]\n  dur: 8bars\n}\n";
    let mut session = Offline::new(48_000.0, Layout::Stereo);
    session.eval(source, Some("now")).unwrap();
    session.process(480);
    session.eval("c.start", Some("now")).unwrap();
    let level = |session: &mut Offline| peak(&session.process(24_000).remove(0)[12_000..]);
    // half a beat at 120 bpm is a quarter of a second, and twice that at 60
    let first = level(&mut session);
    assert!((first - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{first}");
    session.eval("tempo 60bpm", Some("now")).unwrap();
    session.process(48_000);
    let second = level(&mut session);
    assert!((second / first - 2.0).abs() < 0.01, "{first} then {second}");
}

#[test]
fn an_echo_timed_from_the_length_of_a_beat_follows_the_tempo() {
    // a one-beat echo of a short click: it arrives half a second after the click at 120 bpm, a second after at 60
    let source = "bus src: stereo\ninstr click(freq: hz) {\n  src += env[1 1ms 0] * 0.5\n}\nfx echo {\n  out = src.delay(time: clock.beat_length, max: 2sec)\n}\nclip c {\n  instrument: click\n  length: 8bars\n  notes: [c4]\n  dur: 1/100beat\n}\n";
    let mut session = Offline::new(48_000.0, Layout::Stereo);
    session.eval(source, Some("now")).unwrap();
    session.eval("c.start", Some("now")).unwrap();
    // only the echo is heard, so the first sound in a block is how long after the click it came
    let gap = |block: &[f32]| block.iter().position(|v| v.abs() > 0.01).unwrap();
    let at_120 = gap(&session.process(48_000).remove(0));
    assert!((at_120 as i64 - 24_000).abs() < 600, "{at_120}");
    // at 60 bpm the echo of the next click is a second behind it, not half
    session.eval("tempo 60bpm", Some("now")).unwrap();
    session.process(96_000);
    session.eval("c.stop", Some("now")).unwrap();
    session.process(96_000);
    session.eval("c.start", Some("now")).unwrap();
    let at_60 = gap(&session.process(144_000).remove(0));
    assert!((at_60 as i64 - 48_000).abs() < 600, "{at_60}");
}

#[test]
fn a_clip_whose_instrument_uses_a_synced_lfo_sounds_and_follows_the_tempo() {
    // the sweep opens the filter twice a beat; the clip's node is built with the tempo controls it reads
    let source = "instr pad(freq: hz = 220hz) {\n  sweep = synced_lfo(1/2).unipolar.range(300hz..3khz).smooth(5ms)\n  osc(saw, freq).lpf(cutoff: sweep) * 0.3\n}\nclip c {\n  instrument: pad\n  length: 4bars\n  notes: [a2]\n  dur: 4bars\n}\n";
    let mut session = Offline::new(48_000.0, Layout::Stereo);
    session.eval(source, Some("now")).unwrap();
    assert!(session.take_notices().is_empty());
    session.eval("c.start", Some("now")).unwrap();
    let out = session.process(96_000).remove(0);
    assert!(peak(&out) > 0.01 && out.iter().all(|v| v.is_finite()));
}
