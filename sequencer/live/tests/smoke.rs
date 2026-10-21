use niminal_lang::Layout;
use niminal_live::Offline;

const SETUP: &str = "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip lead { instrument: pluck }\nclip bass { instrument: pluck }\n";

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn a_clip_plays_through_the_engine() {
    let mut s = Offline::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(SETUP, Some("now")).unwrap();
    s.eval("lead.__update([c4 e4 g4 b4])", Some("now")).unwrap();
    let out = s.process(96_000).remove(0);
    eprintln!(
        "notices {:?} pending {:?} clock {} peak {}",
        s.take_notices(),
        s.pending(),
        s.clock(),
        peak(&out)
    );
    assert!(peak(&out[..2_000]) > 0.1, "the first note");
    assert!(peak(&out[24_000..26_000]) > 0.1, "the second");
}

#[test]
fn breakbeat_example_produces_audio_when_evaluated_live() {
    let mut s = Offline::new(48_000.0, Layout::Stereo).without_limiter();
    s.set_sample_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"));
    s.eval(include_str!("../../../examples/breakbeat.nml"), Some("now"))
        .unwrap();
    let out = s.process(96_000);
    let notices = s.take_notices();
    assert!(peak(&out[0]) > 0.1, "breakbeat is silent: {notices:?}");
}
