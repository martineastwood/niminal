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
