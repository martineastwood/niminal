use niminal_daemon::{At, Endpoint, Op, RoutingEdit as Edit, Session};
use niminal_lang::Layout;
use niminal_score::{Time, Value};
use std::collections::BTreeMap;
const RATE: f32 = 48_000.0;
fn node(id: &str) -> Endpoint {
    Endpoint::Node { id: id.into() }
}
fn bus(id: &str) -> Endpoint {
    Endpoint::Bus {
        id: id.into(),
        owner: None,
    }
}
fn route(at: u64, edits: Vec<Edit>) -> Op {
    Op::Routing {
        at: At::Sample(at),
        edits,
        tag: None,
    }
}
fn connect(id: &str, from: Endpoint, to: Endpoint, gain: f64) -> Edit {
    Edit::Connect {
        id: id.into(),
        from,
        to,
        gain,
        glide: 0.0,
        delayed: false,
    }
}
fn instance(id: &str, processor: Option<&str>, level: Option<f64>) -> Edit {
    Edit::Node {
        id: id.into(),
        processor: processor.map(str::to_string),
        instrument: None,
        args: level
            .map(|n| [("level".into(), Value::Num(n))].into())
            .unwrap_or_default(),
    }
}
fn note(to: &str, at: u64, duration: f64) -> Op {
    Op::Note {
        target: "flat".into(),
        to: Some(to.into()),
        at: At::Sample(at),
        args: BTreeMap::new(),
        dur: Some(Time::Seconds(duration)),
        id: None,
        tag: None,
    }
}
fn ready() -> Session {
    let mut session = Session::new(RATE, Layout::Mono).without_limiter();
    session
        .send_events(
            1,
            vec![Op::Define {
                source: "instr flat() with(tail: 0ms) { 1 }".into(),
                at: At::Now,
                dir: None,
                tag: None,
            }],
        )
        .unwrap();
    session
}
fn setup(session: &mut Session) {
    session
        .send_events(
            1,
            vec![route(
                0,
                vec![
                    Edit::Processor {
                        name: "volume".into(),
                        source: "(level: num = 1) { out = it * level }".into(),
                    },
                    instance("a", Some("volume"), Some(0.25)),
                    instance("b", Some("volume"), Some(0.75)),
                    connect("a_out", node("a"), Endpoint::Master, 1.0),
                    connect("b_out", node("b"), Endpoint::Master, 1.0),
                ],
            )],
        )
        .unwrap();
}

#[test]
fn one_processor_definition_has_independent_instances_and_sample_timed_parameters() {
    let mut s = ready();
    setup(&mut s);
    s.send_events(
        1,
        vec![
            note("a", 0, 0.1),
            note("b", 0, 0.1),
            route(
                37,
                vec![Edit::Set {
                    id: "a".into(),
                    args: [("level".into(), Value::Num(0.0))].into(),
                    glide: 0.0,
                }],
            ),
        ],
    )
    .unwrap();
    let out = s.process(100).remove(0);
    assert!(
        out[..37].iter().all(|v| (*v - 1.0).abs() < 1e-6),
        "{:?}",
        &out[..40]
    );
    assert!(out[37..].iter().all(|v| (*v - 0.75).abs() < 1e-6));
}

#[test]
fn connection_edits_are_sample_exact_and_preserve_sounding_voices() {
    let mut s = ready();
    setup(&mut s);
    s.send_events(
        1,
        vec![
            note("a", 0, 0.1),
            note("b", 0, 0.1),
            route(43, vec![connect("a_out", node("a"), Endpoint::Master, 0.0)]),
        ],
    )
    .unwrap();
    let out = s.process(100).remove(0);
    assert!(out[..43].iter().all(|v| (*v - 1.0).abs() < 1e-6));
    assert!(out[43..].iter().all(|v| (*v - 0.75).abs() < 1e-6));
    assert_eq!(s.state().voices, 2);
}

#[test]
fn cycles_invalid_parameters_and_dangling_references_reject_the_whole_batch() {
    let mut s = ready();
    setup(&mut s);
    let before = s.routing_status();
    for edits in [
        vec![connect("loop", node("a"), node("a"), 1.0)],
        vec![Edit::Set {
            id: "a".into(),
            args: [("missing".into(), Value::Num(1.0))].into(),
            glide: 0.0,
        }],
        vec![connect("bad", node("a"), bus("missing"), 1.0)],
        vec![instance("bad", Some("missing"), None)],
    ] {
        assert!(
            s.send_events(1, vec![route(100, edits), note("a", 100, 0.1)])
                .is_err()
        );
        assert_eq!(s.routing_status(), before);
        assert_eq!(s.waiting_changes(), 0);
    }
    let mut delayed = connect("feedback", node("a"), node("a"), 0.5);
    if let Edit::Connect { delayed, .. } = &mut delayed {
        *delayed = true;
    }
    s.send_events(1, vec![route(0, vec![delayed])]).unwrap();
}

#[test]
fn graph_objects_are_private_and_shared_buses_are_explicit() {
    let mut s = ready();
    setup(&mut s);
    assert!(s.send_events(2, vec![note("a", 0, 0.1)]).is_err());
    s.send_events(
        1,
        vec![route(
            0,
            vec![Edit::Bus {
                id: "shared".into(),
                channels: 1,
                shared: false,
                source: None,
            }],
        )],
    )
    .unwrap();
    let shared = Endpoint::Bus {
        id: "shared".into(),
        owner: Some(1),
    };
    let edit = connect("send", node("mine"), shared.clone(), 1.0);
    assert!(
        s.send_events(
            2,
            vec![route(0, vec![instance("mine", None, None), edit.clone()])]
        )
        .is_err()
    );
    s.send_events(
        1,
        vec![route(
            0,
            vec![Edit::Bus {
                id: "shared".into(),
                channels: 1,
                shared: true,
                source: None,
            }],
        )],
    )
    .unwrap();
    s.send_events(
        2,
        vec![
            route(0, vec![instance("mine", None, None), edit]),
            note("mine", 0, 0.1),
        ],
    )
    .unwrap();
    s.send_events(
        1,
        vec![route(
            0,
            vec![connect("return", shared, Endpoint::Master, 0.5)],
        )],
    )
    .unwrap();
    assert!((s.process(32)[0][10] - 0.5).abs() < 1e-6);
    s.drop_client(1);
    assert!(
        s.routing_status()
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["client"] != 1 || p["nodes"].as_array().unwrap().is_empty())
    );
    assert!(s.process(32)[0].iter().all(|v| *v == 0.0));
}

#[test]
fn a_draining_effect_rings_out_and_an_immediate_free_cuts_it_off() {
    let render = |drain: bool| {
        let mut s = ready();
        s.send_events(
            1,
            vec![
                route(
                    0,
                    vec![
                        Edit::Processor {
                            name: "echo".into(),
                            source: "() { out = it.delay(time: 1ms, feedback: 0.5) }".into(),
                        },
                        instance("echo", Some("echo"), None),
                        connect("out", node("echo"), Endpoint::Master, 1.0),
                    ],
                ),
                note("echo", 0, 1.0 / 48_000.0),
                route(
                    16,
                    vec![if drain {
                        Edit::Drain { id: "echo".into() }
                    } else {
                        Edit::FreeNode { id: "echo".into() }
                    }],
                ),
            ],
        )
        .unwrap();
        let out = s.process(160).remove(0);
        if drain {
            assert!(s.send_events(1, vec![note("echo", 161, 0.1)]).is_err());
        }
        out
    };
    let tail = render(true);
    let cut = render(false);
    assert!(tail[48].abs() > 0.9 && tail[96].abs() > 0.4, "{tail:?}");
    assert!(cut.iter().all(|v| *v == 0.0));
}

#[test]
fn routing_changes_cancel_and_replay_with_the_same_audio() {
    let mut s = ready();
    setup(&mut s);
    let mut change = route(64, vec![connect("a_out", node("a"), Endpoint::Master, 0.0)]);
    if let Op::Routing { tag, .. } = &mut change {
        *tag = Some("cancel".into());
    }
    s.send_events(1, vec![note("a", 0, 0.1), change]).unwrap();
    assert_eq!(
        s.cancel_events(1, Some("cancel"), At::Now, false).cancelled,
        1
    );
    let mut original = s.process(100).remove(0);
    s.send_events(
        1,
        vec![route(
            150,
            vec![connect("a_out", node("a"), Endpoint::Master, 0.5)],
        )],
    )
    .unwrap();
    original.extend(s.process(100).remove(0));
    let log = niminal_daemon::from_lines(&niminal_daemon::to_lines(s.log())).unwrap();
    let replay = Session::replay(RATE, Layout::Mono, &log, 200, false);
    assert_eq!(original, replay[0]);
}

#[test]
fn prepared_and_offline_routing_match_through_graph_swaps() {
    let session = || {
        let mut s = ready();
        setup(&mut s);
        s.send_events(
            1,
            vec![
                note("a", 0, 0.1),
                note("b", 0, 0.1),
                route(71, vec![connect("a_out", node("a"), Endpoint::Master, 0.2)]),
            ],
        )
        .unwrap();
        s
    };
    let expected = session().process(256);
    let mut s = session();
    let mut renderer = s.start_realtime(niminal_daemon::RealtimeConfig::default());
    s.prepare_realtime();
    let mut actual = vec![vec![0.0; 256]];
    let mut offset = 0;
    renderer.render(256, &mut |block, frames| {
        actual[0][offset..offset + frames].copy_from_slice(&block[0][..frames]);
        offset += frames;
    });
    assert_eq!(actual, expected);
}

#[test]
fn redefining_or_freeing_a_template_does_not_change_existing_instances() {
    let mut s = ready();
    setup(&mut s);
    s.send_events(
        1,
        vec![
            note("a", 0, 0.1),
            route(
                10,
                vec![
                    Edit::Processor {
                        name: "volume".into(),
                        source: "(level: num = 1) { out = it * level * 0 }".into(),
                    },
                    instance("new", Some("volume"), None),
                    connect("new_out", node("new"), Endpoint::Master, 1.0),
                    Edit::FreeProcessor {
                        name: "volume".into(),
                    },
                ],
            ),
            note("new", 10, 0.1),
        ],
    )
    .unwrap();
    assert!(s.process(100)[0].iter().all(|v| (*v - 0.25).abs() < 1e-6));
}

#[test]
fn draining_releases_held_notes_and_frees_their_client_ids() {
    let mut s = ready();
    setup(&mut s);
    let held = |at| Op::Note {
        target: "flat".into(),
        to: Some("a".into()),
        at: At::Sample(at),
        args: BTreeMap::new(),
        dur: None,
        id: Some(9),
        tag: None,
    };
    s.send_events(
        1,
        vec![held(0), route(43, vec![Edit::Drain { id: "a".into() }])],
    )
    .unwrap();
    let out = s.process(512).remove(0);
    assert!(out[..43].iter().all(|v| (*v - 0.25).abs() < 1e-6));
    // Unenveloped voices use the engine's short release fade.
    assert!(out[300..].iter().all(|v| *v == 0.0));
    assert_eq!(s.state().voices, 0);
    s.send_events(
        1,
        vec![
            route(512, vec![instance("a", Some("volume"), Some(0.25))]),
            held(512),
        ],
    )
    .unwrap();
}

#[test]
fn ownership_cleanup_is_logged_and_replays_exactly() {
    let mut s = ready();
    setup(&mut s);
    s.send_events(1, vec![note("a", 0, 1.0)]).unwrap();
    let mut actual = s.process(50).remove(0);
    s.drop_client(1);
    actual.extend(s.process(50).remove(0));
    let replay = Session::replay(RATE, Layout::Mono, s.log(), 100, false);
    assert_eq!(actual, replay[0]);
    assert!(actual[50..].iter().all(|v| *v == 0.0));
}

#[test]
fn rejecting_a_batch_restores_previously_accepted_future_graph_changes() {
    let mut s = ready();
    setup(&mut s);
    s.send_events(
        1,
        vec![route(
            200,
            vec![Edit::Set {
                id: "a".into(),
                args: [("level".into(), Value::Num(0.5))].into(),
                glide: 0.0,
            }],
        )],
    )
    .unwrap();
    let before = s.waiting_changes();
    let bad = Op::Note {
        target: "missing".into(),
        to: None,
        at: At::Sample(100),
        args: BTreeMap::new(),
        dur: Some(Time::Seconds(1.0)),
        id: None,
        tag: None,
    };
    assert!(
        s.send_events(1, vec![route(100, vec![Edit::Clear]), bad])
            .is_err()
    );
    assert_eq!(s.waiting_changes(), before);
    s.send_events(1, vec![note("a", 0, 0.1)]).unwrap();
    let out = s.process(256).remove(0);
    assert!((out[199] - 0.25).abs() < 1e-6);
    assert!((out[200] - 0.5).abs() < 1e-6);
}

#[test]
fn node_ids_cannot_alias_processor_instances_or_the_implicit_track() {
    let mut s = ready();
    setup(&mut s);
    assert!(
        s.send_events(
            1,
            vec![route(
                0,
                vec![Edit::Import {
                    name: "bad".into(),
                    track: String::new()
                }]
            )]
        )
        .is_err()
    );
    s.send_events(
        1,
        vec![
            route(
                0,
                vec![
                    instance("a__processor", None, None),
                    connect("extra", node("a__processor"), Endpoint::Master, 0.5),
                ],
            ),
            note("a", 0, 0.1),
            note("a__processor", 0, 0.1),
        ],
    )
    .unwrap();
    assert!(s.process(32)[0].iter().all(|v| (*v - 0.75).abs() < 1e-6));
    assert!(
        s.send_events(
            1,
            vec![route(
                32,
                vec![Edit::Processor {
                    name: "bad".into(),
                    source: "() { out = it }\ntrack injected { out = 1 }".into()
                }]
            )]
        )
        .is_err()
    );
}

#[test]
fn an_unrelated_graph_edit_preserves_effect_memory_when_bus_indices_change() {
    let render = |edit: bool| {
        let mut s = ready();
        let mut ops = vec![
            route(
                0,
                vec![
                    Edit::Processor {
                        name: "echo".into(),
                        source: "() { out = it.delay(time: 1ms, feedback: 0.5) }".into(),
                    },
                    instance("echo", Some("echo"), None),
                    connect("out", node("echo"), Endpoint::Master, 1.0),
                ],
            ),
            note("echo", 0, 1.0),
        ];
        if edit {
            ops.push(route(
                17,
                vec![Edit::Bus {
                    id: "unrelated".into(),
                    channels: 1,
                    shared: false,
                    source: None,
                }],
            ));
        }
        s.send_events(1, ops).unwrap();
        s.process(256)
    };
    let expected = render(false);
    let actual = render(true);
    assert_eq!(expected, actual);
}
