use niminal_lang::Layout;
use niminal_score::Time;
use niminal_server::{At, Op, RealtimeConfig, Session};

fn session() -> Session {
    let mut s = Session::new(48000.0, Layout::Mono).without_limiter();
    s.send_events(
        1,
        vec![Op::Define {
            source: "buffer data = zeros(8)\ninstr read() { data[0] }".into(),
            at: At::Now,
            dir: None,
            tag: None,
        }],
    )
    .unwrap();
    s.send_events(
        1,
        vec![Op::Note {
            to: None,
            target: "read".into(),
            at: At::Now,
            args: Default::default(),
            dur: Some(Time::Seconds(1.0)),
            id: None,
            tag: None,
        }],
    )
    .unwrap();
    s
}
fn write(at: u64, value: f32) -> Op {
    Op::BufferWrite {
        name: "data".into(),
        at: At::Sample(at),
        offset: 0,
        values: vec![value],
        tag: Some("write".into()),
    }
}

#[test]
fn buffer_writes_are_sample_exact_and_replay() {
    let mut s = session();
    s.send_events(1, vec![write(17, 0.25), write(49, -0.25)])
        .unwrap();
    let log = s.log().to_vec();
    let out = s.process(96);
    assert!(out[0][..17].iter().all(|x| *x == 0.0));
    assert!(out[0][17..49].iter().all(|x| *x == 0.25));
    assert!(out[0][49..].iter().all(|x| *x == -0.25));
    assert_eq!(out, Session::replay(48000.0, Layout::Mono, &log, 96, false));
}

#[test]
fn buffer_batches_validate_before_writing_and_cancel() {
    let mut s = session();
    let mut invalid = write(20, 0.5);
    if let Op::BufferWrite { offset, .. } = &mut invalid {
        *offset = usize::MAX;
    }
    assert!(s.send_events(1, vec![write(10, 0.25), invalid]).is_err());
    assert!(s.process(32)[0].iter().all(|x| *x == 0.0));
    s.send_events(1, vec![write(64, 0.25)]).unwrap();
    assert_eq!(
        s.cancel_events(1, Some("write"), At::Now, false).cancelled,
        1
    );
    assert!(s.process(64)[0].iter().all(|x| *x == 0.0));
}

#[test]
fn realtime_buffer_writes_match_offline_including_injection() {
    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    s.prepare_realtime();
    s.send_events(1, vec![write(17, 0.25), write(49, -0.25)])
        .unwrap();
    s.prepare_realtime();
    let mut out = Vec::new();
    r.render(96, &mut |block, n| out.extend_from_slice(&block[0][..n]));
    let mut reference = session();
    reference
        .send_events(1, vec![write(17, 0.25), write(49, -0.25)])
        .unwrap();
    assert_eq!(out, reference.process(96)[0]);
}

#[test]
fn held_voices_keep_buffer_connections_when_unrelated_resources_are_allocated() {
    let mut s = session();
    s.process(32);
    let old = s.program().buffers[0].1.clone();
    s.send_events(
        1,
        vec![Op::Define {
            source: "buffer other = zeros(16)".into(),
            at: At::Now,
            dir: None,
            tag: None,
        }],
    )
    .unwrap();
    let current = &s
        .program()
        .buffers
        .iter()
        .find(|(name, _)| name == "__client_1::data")
        .unwrap()
        .1;
    assert!(std::sync::Arc::ptr_eq(&old, current));
    s.send_events(1, vec![write(32, 0.25)]).unwrap();
    assert!(s.process(32)[0].iter().all(|x| *x == 0.25));
}
