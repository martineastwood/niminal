mod support;
use niminal_lang::Layout;
use niminal_server::{RealtimeConfig, Session};
use serde_json::json;
use support::Eval;

fn session(kind: &str, parameter: &str) -> Session {
    let mut s = Session::new(48000.0, Layout::Mono).without_limiter();
    s.eval(&format!("{kind} probe(tone: hz = 440hz, cutoff: hz = 440hz) {{\npitch = tone\n{parameter}.num / 2000\n}}"), Some("now")).unwrap();
    s.send_events(
        0,
        serde_json::from_value(json!([
            {"op":"note", "id":1, "at":0, "target":"probe"},
            {"op":"set", "id":1, "at":64, "args":{parameter:"880hz"}, "glide":"10ms"}
        ]))
        .unwrap(),
    )
    .unwrap();
    s
}
#[test]
fn declared_pitch_uses_musical_glides_in_offline_replay_and_realtime() {
    for kind in ["instr", "mono"] {
        for parameter in ["tone", "cutoff"] {
            let mut offline = session(kind, parameter);
            let expected = offline.process(600).remove(0);
            let middle = expected[64 + 239] * 2000.0;
            let target = if parameter == "tone" {
                (440.0_f32 * 880.0).sqrt()
            } else {
                660.0
            };
            assert!(
                (middle - target).abs() < 0.001,
                "{kind} {parameter}: {middle}"
            );
            assert!((expected[64 + 479] * 2000.0 - 880.0).abs() < 0.001);
            assert_eq!(
                expected,
                Session::replay(48000.0, Layout::Mono, offline.log(), 600, false)[0]
            );
            let mut live = session(kind, parameter);
            let mut renderer = live.start_realtime(RealtimeConfig::default());
            let mut actual = Vec::new();
            renderer.render(600, &mut |block, n| {
                actual.extend_from_slice(&block[0][..n])
            });
            assert_eq!(actual, expected);
        }
    }
}
#[test]
fn freq_alias_uses_the_custom_pitch_curve() {
    let mut s = Session::new(48000.0, Layout::Mono).without_limiter();
    s.eval(
        "instr probe(tone: hz = 440hz) { pitch = tone\ntone.num / 2000 }",
        Some("now"),
    )
    .unwrap();
    s.send_events(
        0,
        serde_json::from_value(json!([
            {"op":"note","id":1,"at":0,"target":"probe"},
            {"op":"set","id":1,"at":64,"args":{"freq":"880hz"},"glide":"10ms"}
        ]))
        .unwrap(),
    )
    .unwrap();
    let audio = s.process(600).remove(0);
    assert!((audio[303] * 2000.0 - (440.0_f32 * 880.0).sqrt()).abs() < 0.001);
}
