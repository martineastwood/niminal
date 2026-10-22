use niminal_lang::Layout;
use niminal_score::Time;
use niminal_server::{At, Op, Session};

fn render(source: &str, targets: &[&str]) -> Vec<f32> {
    let mut session = Session::new(48_000.0, Layout::Mono).without_limiter();
    let mut events = vec![Op::Define {
        source: source.into(),
        at: At::Now,
        dir: None,
        tag: None,
    }];
    events.extend(targets.iter().map(|target| Op::Note {
        target: (*target).into(),
        to: None,
        at: At::Now,
        args: Default::default(),
        dur: Some(Time::Seconds(0.01)),
        id: None,
        tag: None,
    }));
    // Exercise private definitions and the generated fx input buses too.
    session.send_events(7, events).unwrap();
    session.process(3_840).remove(0)
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().copied().map(f32::abs).fold(0.0, f32::max)
}

#[test]
fn collections_share_one_persistent_fx_and_its_tail_outlives_the_notes() {
    let audio = render(
        "instr flat() { 0.25 }
         fx echo { out = in.delay(time: 20ms, feedback: 0) }
         node a { instrument = flat
                  output = echo }
         node b { instrument = flat
                  output = echo }",
        &["a", "b"],
    );
    assert_eq!(peak(&audio[..960]), 0.0, "routing must not leak a dry path");
    assert!((peak(&audio[960..1_440]) - 0.5).abs() < 1e-5);
    assert_eq!(peak(&audio[2_880..]), 0.0);
}

#[test]
fn separate_fx_instances_have_independent_state() {
    let audio = render(
        "instr flat() { 0.25 }
         fx first { out = in.delay(time: 20ms, feedback: 0) }
         fx second { out = in.delay(time: 40ms, feedback: 0) }
         node a { instrument = flat
                  output = first }
         node b { instrument = flat
                  output = second }",
        &["a", "b"],
    );
    assert_eq!(peak(&audio[..960]), 0.0);
    assert!((peak(&audio[960..1_440]) - 0.25).abs() < 1e-5);
    assert!((peak(&audio[1_920..2_400]) - 0.25).abs() < 1e-5);
}
