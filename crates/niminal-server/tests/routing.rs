use niminal_lang::Layout;
use niminal_score::{Time, Value};
use niminal_server::{At, Endpoint, Op, RoutingEdit as Edit, Session};
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
fn instance(id: &str, fx: Option<&str>, level: Option<f64>) -> Edit {
    Edit::Node {
        id: id.into(),
        fx: fx.map(str::to_string),
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
                source: "instr flat() { tail = 0ms\n 1 }".into(),
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
                    Edit::Fx {
                        output_channels: None,
                        name: "volume".into(),
                        source: "(level: num = 1) { out = in * level }".into(),
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
fn one_fx_definition_has_independent_instances_and_sample_timed_parameters() {
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
    s.send_events(
        2,
        vec![Op::Define {
            source: "instr flat() { 1 }".into(),
            at: At::Now,
            dir: None,
            tag: None,
        }],
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
                        Edit::Fx {
                            output_channels: None,
                            name: "echo".into(),
                            source: "() { out = in.delay(time: 1ms, feedback: 0.5) }".into(),
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
    let log = niminal_server::from_lines(&niminal_server::to_lines(s.log())).unwrap();
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
    let mut renderer = s.start_realtime(niminal_server::RealtimeConfig::default());
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
                    Edit::Fx {
                        output_channels: None,
                        name: "volume".into(),
                        source: "(level: num = 1) { out = in * level * 0 }".into(),
                    },
                    instance("new", Some("volume"), None),
                    connect("new_out", node("new"), Endpoint::Master, 1.0),
                    Edit::FreeFx {
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
fn node_ids_cannot_alias_fx_instances_or_the_implicit_node() {
    let mut s = ready();
    setup(&mut s);
    assert!(
        s.send_events(
            1,
            vec![route(
                0,
                vec![Edit::Import {
                    name: "bad".into(),
                    node: String::new()
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
                    instance("a__fx", None, None),
                    connect("extra", node("a__fx"), Endpoint::Master, 0.5),
                ],
            ),
            note("a", 0, 0.1),
            note("a__fx", 0, 0.1),
        ],
    )
    .unwrap();
    assert!(s.process(32)[0].iter().all(|v| (*v - 0.75).abs() < 1e-6));
    assert!(
        s.send_events(
            1,
            vec![route(
                32,
                vec![Edit::Fx {
                    output_channels: None,
                    name: "bad".into(),
                    source: "() { out = in }\nfx injected { out = 1 }".into()
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
                    Edit::Fx {
                        output_channels: None,
                        name: "echo".into(),
                        source: "() { out = in.delay(time: 1ms, feedback: 0.5) }".into(),
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

fn json_op(value: serde_json::Value) -> Op {
    serde_json::from_value(value).unwrap()
}

#[test]
fn control_edits_keep_the_program_and_smooth_gains_sample_exactly() {
    let mut s = ready();
    setup(&mut s);
    let program = s.program() as *const _;
    s.send_events(
        1,
        vec![
            note("a", 0, 1.0),
            json_op(serde_json::json!({
                "op":"routing","at":16,"edits":[
                    {"op":"set","id":"a","args":{"level":1}},
                    {"op":"gain","id":"a_out","value":0,"glide":0.00016666666666666666}
                ]
            })),
        ],
    )
    .unwrap();
    assert_eq!(s.program() as *const _, program);
    let out = s.process(40).remove(0);
    assert!(out[..16].iter().all(|v| *v == 0.25));
    for (i, value) in out[16..24].iter().enumerate() {
        assert!((*value - (1.0 - (i + 1) as f32 / 8.0)).abs() < 1e-6);
    }
    assert!(out[24..].iter().all(|v| *v == 0.0));
}

#[test]
fn a_transaction_validates_only_the_final_definition_and_routing_graph() {
    use serde_json::json;
    let mut s = ready();
    s.send_events(
        1,
        vec![json_op(
            json!({"op":"transaction","at":0,"source":"bus shared: mono","edits":[
                {"op":"bus","id":"imported","channels":1,"source":"shared"}
            ]}),
        )],
    )
    .unwrap();
    assert!(
        s.send_events(
            1,
            vec![json_op(
                json!({"op":"define","at":10,"source":"bus shared: stereo"})
            )]
        )
        .is_err()
    );
    let result = s
        .send_events(
            1,
            vec![json_op(
                json!({"op":"transaction","at":10,"source":"bus shared: stereo","edits":[
                    {"op":"free_bus","id":"imported"},
                    {"op":"bus","id":"imported","channels":2,"source":"shared"}
                ]}),
            )],
        )
        .unwrap();
    assert_eq!(result.transactions.len(), 1);
    let id = result.transactions[0].id;
    s.process(20);
    let status = s.transaction_status(1);
    let transaction = status
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap();
    assert_eq!(transaction["status"], "applied");
    assert_eq!(transaction["applied_at"], 10);
    assert_eq!(s.routing_status()[0]["buses"][0]["channels"], 2);
}

#[test]
fn replacement_crossfades_survive_unrelated_graph_changes_and_replay() {
    use serde_json::json;
    let mut s = ready();
    s.send_events(
        1,
        vec![
            json_op(json!({"op":"routing","at":0,"edits":[
                {"op":"fx","name":"one","source":"() { out = in }"},
                {"op":"fx","name":"two","source":"() { out = in * 2 }"},
                {"op":"node","id":"a","fx":"one"},
                {"op":"connect","id":"out","from":{"kind":"node","id":"a"},"to":{"kind":"master"}}
            ]})),
            note("a", 0, 1.0),
            json_op(json!({"op":"routing","at":17,"edits":[
                {"op":"replace","id":"a","fx":"two","fade":8.0/48000.0}
            ]})),
            route(
                21,
                vec![Edit::Bus {
                    id: "unused".into(),
                    channels: 1,
                    source: None,
                    shared: false,
                }],
            ),
        ],
    )
    .unwrap();
    let out = s.process(64).remove(0);
    assert!(out[..17].iter().all(|v| *v == 1.0));
    for (i, value) in out[17..25].iter().enumerate() {
        assert!(
            (*value - (1.0 + i as f32 / 8.0)).abs() < 1e-6,
            "fade sample {i}: {value}"
        );
    }
    assert!(out[25..].iter().all(|v| *v == 2.0));
    assert_eq!(
        Session::replay(RATE, Layout::Mono, s.log(), 64, false).remove(0),
        out
    );
}

#[test]
fn replacement_preserves_matching_live_parameters_and_allows_explicit_overrides() {
    use serde_json::json;
    let mut s = ready();
    setup(&mut s);
    s.send_events(1,vec![note("a",0,1.0),json_op(json!({"op":"routing","at":4,"edits":[{"op":"set","id":"a","args":{"level":0.6}}]})),json_op(json!({"op":"routing","at":12,"edits":[
        {"op":"fx","name":"new","source":"(level: num = 0.1) { out = in * level * 2 }"},
        {"op":"replace","id":"a","fx":"new"}
    ]})),json_op(json!({"op":"routing","at":24,"edits":[{"op":"replace","id":"a","fx":"volume","args":{"level":0.8}}]}))]).unwrap();
    let out = s.process(40).remove(0);
    assert!((out[6] - 0.6).abs() < 1e-6);
    assert!((out[16] - 1.2).abs() < 1e-6);
    assert!((out[30] - 0.8).abs() < 1e-6);
}

#[test]
fn routing_meters_expose_public_ids_and_reset_without_changing_audio() {
    let mut s = ready();
    setup(&mut s);
    s.send_events(
        1,
        vec![
            route(
                0,
                vec![
                    Edit::Bus {
                        id: "sum".into(),
                        channels: 1,
                        source: None,
                        shared: false,
                    },
                    connect("send", node("a"), bus("sum"), 0.5),
                ],
            ),
            note("a", 0, 1.0),
        ],
    )
    .unwrap();
    s.process(32);
    let meters = s.routing_meters(true);
    let a = meters[0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "a")
        .unwrap();
    assert_eq!(a["peaks"], serde_json::json!([0.25]));
    assert_eq!(meters[0]["buses"][0]["peaks"], serde_json::json!([0.125]));
    assert_eq!(
        s.routing_meters(false)[0]["nodes"][0]["peaks"],
        serde_json::json!([0.0])
    );
    assert_eq!(
        s.routing_status()[0]["nodes"][0]["params"][0]["name"],
        "level"
    );
    assert!(s.process(32)[0].iter().all(|v| *v == 0.25));
}

#[test]
fn replacement_resets_parameters_whose_units_changed() {
    use serde_json::json;
    let mut s = ready();
    setup(&mut s);
    s.send_events(1,vec![note("a",0,1.0),json_op(json!({"op":"routing","at":4,"edits":[{"op":"set","id":"a","args":{"level":0.6}}]})),json_op(json!({"op":"routing","at":12,"edits":[
        {"op":"fx","name":"frequency","source":"(level: hz = 100hz) { out = in * (level / 100hz) }"},
        {"op":"replace","id":"a","fx":"frequency"}
    ]}))]).unwrap();
    let out = s.process(20).remove(0);
    assert!((out[16] - 1.0).abs() < 1e-6);
    let parameter = &s.routing_status()[0]["nodes"][0]["params"][0];
    assert_eq!(parameter["unit"], "hz");
    assert_eq!(parameter["value"], 100.0);
}

#[test]
fn overlapping_crossfades_are_rejected_without_disturbing_the_accepted_fade() {
    use serde_json::json;
    let mut s = ready();
    setup(&mut s);
    let replace = |at| {
        json_op(
            json!({"op":"routing","at":at,"edits":[{"op":"replace","id":"a","fx":"volume","fade":0.02}]}),
        )
    };
    let receipt = s.send_events(1, vec![replace(10)]).unwrap().transactions[0].id;
    assert!(s.send_events(1, vec![replace(20)]).is_err());
    let status = s.transaction_status(1);
    assert_eq!(
        status
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == receipt)
            .unwrap()["status"],
        "queued"
    );
    assert!(s.send_events(1, vec![replace(1000)]).is_ok());
}

#[test]
fn mono_native_modulation_bus_drives_a_stereo_fx_parameter() {
    use serde_json::json;
    let mut session = Session::new(RATE, Layout::Stereo).without_limiter();
    session.send_events(1, vec![
        json_op(json!({"op":"define","at":0,"source":"instr held() { tail = 0ms\n [1, 1].to_layout(stereo) }"})),
        json_op(json!({"op":"routing","at":0,"edits":[
            {"op":"fx","name":"modulator","output_channels":1,"source":"() { out = 0.5 }"},
            {"op":"fx","name":"gain","source":"(amount: num = 0) { out = in * amount }"},
            {"op":"bus","id":"mod","channels":1},
            {"op":"node","id":"lfo","fx":"modulator"},
            {"op":"node","id":"audio","fx":"gain"},
            {"op":"connect","id":"send","from":{"kind":"node","id":"lfo"},"to":{"kind":"bus","id":"mod"}},
            {"op":"connect","id":"out","from":{"kind":"node","id":"audio"},"to":{"kind":"master"}},
            {"op":"map","id":"audio","param":"amount","source":{"kind":"bus","id":"mod"}}
        ]})),
        json_op(json!({"op":"note","at":0,"target":"held","to":"audio","dur":"1sec"}))
    ]).unwrap();
    let audio = session.process(512);
    for channel in audio {
        assert!(
            channel[256..]
                .iter()
                .all(|sample| (*sample - 0.5).abs() < 0.00001)
        );
    }
    assert!(session.send_events(1, vec![json_op(json!({"op":"routing","at":512,"edits":[{"op":"fx","name":"bad","output_channels":0,"source":"() { out = 0 }"}]}))]).is_err());
}

#[test]
fn a_native_fx_reset_restarts_dsp_without_releasing_voices_or_changing_parameters() {
    use serde_json::json;
    let mut session = Session::new(RATE, Layout::Stereo).without_limiter();
    session.send_events(1, vec![
        json_op(json!({"op":"define","at":0,"source":"instr held() { tail = 0ms\n [0.2, 0.2].to_layout(stereo) }"})),
        json_op(json!({"op":"routing","at":0,"edits":[
            {"op":"fx","name":"moving","source":"(amount: num = 0.1) { out = in + osc(sine, 100hz) * amount }"},
            {"op":"node","id":"audio","fx":"moving","args":{"amount":0.2}},
            {"op":"connect","id":"out","from":{"kind":"node","id":"audio"},"to":{"kind":"master"}}
        ]})),
        json_op(json!({"op":"note","at":0,"target":"held","to":"audio","dur":"1sec"})),
        json_op(json!({"op":"routing","at":512,"edits":[{"op":"reset","id":"audio"}]}))
    ]).unwrap();
    let audio = session.process(768);
    for channel in audio {
        assert_eq!(&channel[..256], &channel[512..768]);
    }
}

#[test]
fn an_fx_parameter_in_beats_follows_the_tempo_and_one_in_seconds_does_not() {
    let mut s = ready();
    // `level` is the fx's output, so the level that comes out says its length in seconds
    s.send_events(
        1,
        vec![route(
            0,
            vec![
                Edit::Fx { output_channels: None, name: "timed".into(), source: "(level: num = 1, length: sec = 0.5sec) { out = in * length / 1sec }".into() },
                Edit::Node {
                    id: "a".into(),
                    fx: Some("timed".into()),
                    instrument: None,
                    args: [("length".into(), Value::Beats(1.0))].into(),
                },
                Edit::Node {
                    id: "b".into(),
                    fx: Some("timed".into()),
                    instrument: None,
                    args: [("length".into(), Value::Seconds(0.1))].into(),
                },
                connect("a_out", node("a"), Endpoint::Master, 1.0),
                connect("b_out", node("b"), Endpoint::Master, 1.0),
            ],
        )],
    )
    .unwrap();
    let tempo = |at: u64, bpm: f64| Op::Tempo { at: At::Sample(at), bpm, meter: None, beat: None, tag: None };
    s.send_events(1, vec![note("a", 0, 5.0), note("b", 0, 5.0), tempo(0, 120.0), tempo(9_600, 60.0)]).unwrap();
    let out = s.process(24_000).remove(0);
    // a beat at 120 bpm is half a second, and the other is a tenth: 0.6 together; at 60 bpm, 1.1
    assert!((out[4_000] - 0.6).abs() < 1e-3, "{}", out[4_000]);
    assert!((out[23_999] - 1.1).abs() < 1e-3, "{}", out[23_999]);
}
