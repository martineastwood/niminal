use niminal_lang::Layout;
use niminal_score::{Time, Value};
use niminal_server::{At, Op, RealtimeConfig, Session};

const SOURCE: &str = "const first = [0.25, 0.5]\nconst second = [-0.25, -0.5]\nbuffer one = first\nbuffer two = second\nctl level = 0.5.smooth(0ms)\nbus modulator\nfx lfo { modulator += osc(sine, 1khz)\n out = 0 }\ninstr a(weights: [num] = [0.25, 0.5], source: buffer = one, value: num = 0) { weights.sum + source[0] + value }";
fn session(source: &str) -> Session {
    let mut s = Session::new(48000.0, Layout::Mono).without_limiter();
    s.send_events(
        1,
        vec![Op::Define {
            source: source.into(),
            at: At::Now,
            dir: None,
            tag: None,
        }],
    )
    .unwrap();
    s
}
fn note() -> Op {
    Op::Note {
        to: None,
        target: "a".into(),
        at: At::Now,
        args: Default::default(),
        dur: Some(Time::Seconds(1.0)),
        id: Some(10),
        tag: None,
    }
}
fn array(at: u64) -> Op {
    Op::Array {
        id: 10,
        at: At::Sample(at),
        param: "weights".into(),
        values: vec![Value::Num(0.0), Value::Num(0.0)],
        glide: None,
        tag: None,
    }
}
fn bind(at: u64) -> Op {
    Op::BindBuffer {
        id: 10,
        at: At::Sample(at),
        param: "source".into(),
        buffer: "two".into(),
        tag: None,
    }
}
fn map(at: u64, source: Option<&str>) -> Op {
    Op::Map {
        id: 10,
        at: At::Sample(at),
        param: "value".into(),
        source: source.map(str::to_string),
        channel: 1,
        scale: 1.0,
        offset: 0.0,
        tag: Some("map".into()),
    }
}
#[test]
fn arrays_and_buffer_bindings_change_at_their_sample_and_replay() {
    let mut s = session(SOURCE);
    s.send_events(1, vec![note(), array(17), bind(49)]).unwrap();
    let log = s.log().to_vec();
    let out = s.process(96);
    assert!(out[0][..17].iter().all(|x| *x == 1.0));
    assert!(out[0][17..49].iter().all(|x| *x == 0.25));
    assert!(out[0][49..].iter().all(|x| *x == -0.25));
    assert_eq!(out, Session::replay(48000.0, Layout::Mono, &log, 96, false));
}
#[test]
fn control_mapping_attaches_changes_source_and_disconnects() {
    let mut s = session("ctl level = 0.5.smooth(0ms)\ninstr a(value: num = 0) { value }");
    s.send_events(
        1,
        vec![
            note(),
            map(17, Some("level")),
            Op::Control {
                name: "level".into(),
                at: At::Sample(33),
                value: 0.25,
                unit: None,
                glide: None,
                tag: None,
            },
            map(49, None),
            Op::Set {
                id: 10,
                at: At::Sample(65),
                args: [("value".into(), Value::Num(0.75))].into(),
                glide: None,
                tag: None,
            },
        ],
    )
    .unwrap();
    let log = s.log().to_vec();
    let out = s.process(96);
    assert!(out[0][..17].iter().all(|x| *x == 0.0));
    assert!(out[0][17..33].iter().all(|x| *x == 0.5));
    assert!(out[0][33..65].iter().all(|x| *x == 0.25));
    assert!(out[0][65..].iter().all(|x| *x == 0.75));
    assert_eq!(out, Session::replay(48000.0, Layout::Mono, &log, 96, false));
}
#[test]
fn rich_updates_and_mapping_match_realtime_with_prepared_window_injection() {
    let mut offline = session(SOURCE);
    offline.send_events(1, vec![note()]).unwrap();
    let events = vec![array(17), bind(49), map(65, Some("level")), map(81, None)];
    offline.send_events(1, events.clone()).unwrap();
    let expected = offline.process(128)[0].clone();
    let mut live = session(SOURCE);
    live.send_events(1, vec![note()]).unwrap();
    let mut renderer = live.start_realtime(RealtimeConfig::default());
    live.prepare_realtime();
    live.send_events(1, events).unwrap();
    live.prepare_realtime();
    let mut actual = Vec::new();
    renderer.render(128, &mut |b, n| actual.extend_from_slice(&b[0][..n]));
    assert_eq!(actual, expected);
}
#[test]
fn malformed_array_update_rolls_back_the_entire_batch() {
    let mut s = session(SOURCE);
    s.send_events(1, vec![note()]).unwrap();
    let bad = Op::Array {
        id: 10,
        at: At::Now,
        param: "weights".into(),
        values: vec![Value::Num(0.0)],
        glide: None,
        tag: None,
    };
    assert!(s.send_events(1, vec![bind(0), bad]).is_err());
    assert!(s.process(16)[0].iter().all(|x| *x == 1.0));
}
#[test]
fn bus_mapping_is_one_block_delayed_and_independent_of_render_chunk_size() {
    let source = "bus modulation\nfx generator { modulation += osc(sine, 1khz)\n out = 0 }\ninstr a(value: num = 0) { value }";
    let run = |chunk: usize| {
        let mut s = session(source);
        s.send_events(1, vec![note(), map(0, Some("modulation"))])
            .unwrap();
        let mut out = Vec::new();
        for _ in 0..256 / chunk {
            out.extend(s.process(chunk)[0].clone());
        }
        out
    };
    let a = run(32);
    let b = run(1);
    assert_eq!(a, b);
    assert!(a[..32].iter().all(|x| *x == 0.0));
    assert!(a[32..].iter().any(|x| x.abs() > 0.5));
}

#[test]
fn parallel_nodes_and_bus_sends_are_latency_compensated() {
    for source in [
        "instr tone() { osc(sine, 1khz) }\nnode dry { instrument = tone }\nfx wet_fx {out = -oversample(4) { in }}\nnode wet { instrument = tone\noutput = wet_fx}",
        "bus sum\ninstr tone() { osc(sine, 1khz) }\nbus dry_audio\nfx dry_fx {sum += dry_audio\nout = 0}\nnode dry { instrument = tone\noutput = dry_audio}\nfx wet_fx {sum += -oversample(4) { in }\nout = 0}\nnode wet { instrument = tone\noutput = wet_fx}\nfx result { out = sum }",
    ] {
        let mut s = session(source);
        let notes = ["dry", "wet"]
            .into_iter()
            .map(|target| Op::Note {
                to: None,
                target: target.into(),
                at: At::Now,
                args: Default::default(),
                dur: Some(Time::Seconds(0.2)),
                id: None,
                tag: None,
            })
            .collect();
        s.send_events(1, notes).unwrap();
        assert_eq!(s.program().processing_latency(), 16);
        let output = s.process(4096);
        let rms = (output[0][1000..].iter().map(|v| v * v).sum::<f32>() / 3096.0).sqrt();
        assert!(rms < 0.005, "parallel paths cancel: {rms}");
    }
}

#[test]
fn mappings_and_array_updates_respect_parameter_ranges() {
    let mut s =
        session("ctl level = 2\ninstr a(value: 0..1 = 0, weights: [0..1] = [0.1, 0.2]) { value }");
    s.send_events(1, vec![note(), map(0, Some("level"))])
        .unwrap();
    assert!(s.process(16)[0].iter().all(|v| *v == 1.0));
    let bad = Op::Array {
        id: 10,
        at: At::Now,
        param: "weights".into(),
        values: vec![Value::Num(0.1), Value::Num(2.0)],
        glide: None,
        tag: None,
    };
    assert!(s.send_events(1, vec![bad]).is_err());
}

#[test]
fn mapping_cancellation_prevents_the_connection_from_landing() {
    let mut s = session("ctl level = 0.5\ninstr a(value: num = 0) { value }");
    s.send_events(1, vec![note(), map(32, Some("level"))])
        .unwrap();
    assert_eq!(s.cancel_events(1, Some("map"), At::Now, false).cancelled, 1);
    assert!(s.process(64)[0].iter().all(|v| *v == 0.0));
}
