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
