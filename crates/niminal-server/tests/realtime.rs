use niminal_server::{RealtimeConfig, RealtimeRenderer, Session};
mod support;
use niminal_lang::Layout;
use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::cell::Cell;
use support::Eval;

struct Allocator;
thread_local! {
    static MEASURE: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        MEASURE.with(|on| {
            if on.get() {
                ALLOCS.with(|n| n.set(n.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        MEASURE.with(|on| {
            if on.get() {
                FREES.with(|n| n.set(n.get() + 1));
            }
        });
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, size: usize) -> *mut u8 {
        MEASURE.with(|on| {
            if on.get() {
                ALLOCS.with(|n| n.set(n.get() + 1));
                FREES.with(|n| n.set(n.get() + 1));
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

const SETUP: &str =
    "instr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\nnode lead { instrument = tone }";
fn session() -> Session {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(SETUP, Some("now")).unwrap();
    s
}
fn measured(r: &mut RealtimeRenderer, n: usize, out: &mut [f32]) {
    ALLOCS.with(|v| v.set(0));
    FREES.with(|v| v.set(0));
    MEASURE.with(|v| v.set(true));
    let mut pos = 0;
    r.render(n, &mut |b, n| {
        out[pos..pos + n].copy_from_slice(&b[0][..n]);
        pos += n;
    });
    MEASURE.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 0, "render allocated");
    assert_eq!(FREES.with(Cell::get), 0, "render destroyed heap storage");
}

fn streamed_notes(count: usize, seconds: f64) -> Vec<niminal_server::Op> {
    use niminal_score::{Time, Value};
    use niminal_server::{At, Op};
    (0..count)
        .map(|i| Op::Note {
            to: None,
            target: "lead".into(),
            at: At::Sample(((0.05 + seconds * i as f64 / count as f64) * 48000.0).round() as u64),
            dur: Some(Time::Seconds(0.02)),
            id: None,
            tag: None,
            args: [(
                "freq".to_string(),
                Value::Hz(200.0 + (i % 12) as f64 * 40.0),
            )]
            .into(),
        })
        .collect()
}

/// Stream `events` in batches, each sent `ahead` frames before its first note, while the renderer plays.
/// Returns the audio and what the renderer reported.
fn stream_while_playing(
    events: &[niminal_server::Op],
    ahead: u64,
    frames: usize,
) -> (Vec<f32>, niminal_server::RenderMetrics) {
    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = vec![0.0; frames];
    let mut sent = 0;
    for chunk in out.chunks_mut(256) {
        let clock = r.clock();
        while sent < events.len() {
            let start = match &events[sent] {
                niminal_server::Op::Note {
                    at: niminal_server::At::Sample(t),
                    ..
                } => *t,
                _ => unreachable!(),
            };
            if start > clock + ahead {
                break;
            }
            s.send_events(0, vec![events[sent].clone()]).unwrap();
            sent += 1;
        }
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    (out, r.metrics())
}

fn offline(events: &[niminal_server::Op], frames: usize) -> Vec<f32> {
    let mut s = session();
    s.send_events(0, events.to_vec()).unwrap();
    s.process(frames).remove(0)
}

#[test]
fn streamed_notes_beyond_the_prepared_region_cause_no_replanning() {
    let events = streamed_notes(1000, 10.0);
    let frames = 11 * 48_000;
    // sent ~200ms ahead, which is past the 8192 frame lookahead
    let (audio, metrics) = stream_while_playing(&events, 9_600, frames);
    assert_eq!(audio, offline(&events, frames));
    assert_eq!(metrics.mixer_builds, 0);
    assert_eq!(metrics.late_events, 0);
    assert_eq!(metrics.starved_frames, 0);
}

#[test]
fn streamed_notes_inside_the_prepared_region_cause_no_replanning() {
    let events = streamed_notes(1000, 10.0);
    let frames = 11 * 48_000;
    // sent ~50ms ahead, inside the region the planner has prepared
    let (audio, metrics) = stream_while_playing(&events, 2_400, frames);
    assert_eq!(audio, offline(&events, frames));
    assert_eq!(metrics.mixer_builds, 0);
    assert_eq!(metrics.late_events, 0);
}

fn voice_script() -> Vec<(u64, niminal_server::Op)> {
    use niminal_score::{Time, Value};
    use niminal_server::{At, Op};
    let held = |id, at: u64, hz, dur: Option<f64>| {
        (
            at,
            Op::Note {
                to: None,
                target: "organ".into(),
                at: At::Sample(at),
                args: [("freq".to_string(), Value::Hz(hz))].into(),
                dur: dur.map(Time::Seconds),
                id: Some(id),
                tag: None,
            },
        )
    };
    vec![
        held(1, 4_800, 110.0, None),
        held(2, 9_600, 165.0, Some(0.3)),
        (
            14_400,
            Op::Set {
                id: 1,
                at: At::Sample(14_400),
                args: [("cut".to_string(), Value::Hz(6000.0))].into(),
                glide: Some(Time::Seconds(0.2)),
                tag: None,
            },
        ),
        (
            20_000,
            Op::Set {
                id: 2,
                at: At::Sample(20_000),
                args: [("cut".to_string(), Value::Hz(300.0))].into(),
                glide: None,
                tag: None,
            },
        ),
        (
            28_800,
            Op::Off {
                id: 1,
                at: At::Sample(28_800),
                tag: None,
            },
        ),
        held(3, 30_000, 220.0, None),
        (
            33_000,
            Op::Off {
                id: 3,
                at: At::Sample(33_000),
                tag: None,
            },
        ),
    ]
}

fn organ_session() -> Session {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval_as(1,"instr organ(freq: hz, cut: hz = 100hz) { osc(saw, freq).lpf(cutoff: cut) * env[1 | 100ms 0] * 0.3 }").unwrap();
    s
}

#[test]
fn held_notes_sets_and_offs_sound_the_same_prepared_as_offline_wherever_they_are_sent() {
    let script = voice_script();
    let frames = 60_000;
    let mut offline = organ_session();
    offline
        .send_events(1, script.iter().map(|(_, op)| op.clone()).collect())
        .unwrap();
    let expected = offline.process(frames).remove(0);
    assert!(expected.iter().any(|s| s.abs() > 0.05));

    // 20000 frames ahead is past the prepared region, 1000 ahead is inside it
    for ahead in [20_000u64, 1_000] {
        let mut s = organ_session();
        let mut r = s.start_realtime(RealtimeConfig::default());
        let mut out = vec![0.0; frames];
        let mut sent = 0;
        for chunk in out.chunks_mut(256) {
            while sent < script.len() && script[sent].0 <= r.clock() + ahead {
                s.send_events(1, vec![script[sent].1.clone()]).unwrap();
                sent += 1;
            }
            s.prepare_realtime();
            measured(&mut r, chunk.len(), chunk);
        }
        assert_eq!(out, expected, "sent {ahead} frames ahead");
        assert_eq!(r.metrics().mixer_builds, 0, "sent {ahead} frames ahead");
        assert_eq!(r.metrics().late_events, 0, "sent {ahead} frames ahead");
    }
}

fn bars(hz: f64, first: u32, count: u32, tag: &str) -> Vec<niminal_server::Op> {
    use niminal_score::{Time, Value};
    use niminal_server::{At, Op};
    (first * 4..(first + count) * 4)
        .map(|beat| Op::Note {
            to: None,
            target: "organ".into(),
            at: At::Sample(((f64::from(beat)) * 24000.0).round() as u64),
            args: [("freq".to_string(), Value::Hz(hz))].into(),
            dur: Some(Time::Seconds(0.2)),
            id: None,
            tag: Some(tag.into()),
        })
        .collect()
}

#[test]
fn cancelling_waiting_events_replans_without_building_a_mixer_even_inside_the_prepared_region() {
    use niminal_server::At;
    const BAR: usize = 96_000;
    let frames = 4 * BAR;

    // what it should sound like: bar 1 from `a`, the rest from `b`
    let mut expected = organ_session();
    expected.send_events(1, bars(220.0, 0, 1, "a")).unwrap();
    expected.send_events(1, bars(440.0, 1, 3, "b")).unwrap();
    let expected = expected.process(frames).remove(0);

    // the cancel is sent far ahead of bar 2, and again only just before it, when it is inside what is prepared
    for cancel_at in [BAR as u64 / 2, BAR as u64 - 3_000] {
        let mut s = organ_session();
        let mut r = s.start_realtime(RealtimeConfig::default());
        s.send_events(1, bars(220.0, 0, 4, "a")).unwrap();
        let mut out = vec![0.0; frames];
        let mut cancelled = false;
        for chunk in out.chunks_mut(256) {
            if !cancelled && r.clock() >= cancel_at {
                let done = s.cancel_events(1, Some("a"), At::Sample(96_000), false);
                assert_eq!(done.cancelled, 12);
                s.send_events(1, bars(440.0, 1, 3, "b")).unwrap();
                cancelled = true;
            }
            s.prepare_realtime();
            measured(&mut r, chunk.len(), chunk);
        }
        assert_eq!(out, expected, "cancelled at {cancel_at}");
        assert_eq!(r.metrics().mixer_builds, 0, "cancelled at {cancel_at}");
        assert_eq!(r.metrics().late_events, 0, "cancelled at {cancel_at}");
    }
}

#[test]
fn timestamped_definitions_and_notes_sound_the_same_prepared_as_offline() {
    use niminal_score::{Time, Value};
    use niminal_server::{At, Op};
    const SINE: &str =
        "instr organ(freq: hz, cut: hz = 100hz) { osc(sine, freq) * env[1 | 100ms 0] * 0.3 }";
    const SAW: &str = "instr organ(freq: hz, cut: hz = 100hz) { osc(saw, freq).lpf(cutoff: 3000hz) * env[1 | 100ms 0] * 0.3 }";
    let note = |at: At, hz: f64| Op::Note {
        to: None,
        target: "organ".into(),
        at,
        args: [("freq".to_string(), Value::Hz(hz))].into(),
        dur: Some(Time::Seconds(0.1)),
        id: None,
        tag: None,
    };
    let script = [
        (0u64, note(At::Sample(24_000), 220.0)),
        (0, note(At::Sample(120_000), 330.0)),
        (
            0,
            Op::Define {
                source: SAW.into(),
                at: At::Sample(72_000),
                dir: None,
                tag: None,
            },
        ),
        (30_000, note(At::Sample(192_000), 440.0)),
        (60_000, note(At::Sample(96_000), 110.0)),
        (60_000, note(At::Sample(150_000), 550.0)),
    ];
    let session = || {
        let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
        s.eval_as(1, SINE).unwrap();
        s
    };
    let frames = 4 * 48_000;
    let mut offline = session();
    let mut expected = Vec::new();
    let mut sent = 0;
    while expected.len() < frames {
        // the same things at the same clock as the prepared run sends them
        while sent < script.len() && script[sent].0 <= expected.len() as u64 {
            offline
                .send_events(1, vec![script[sent].1.clone()])
                .unwrap();
            sent += 1;
        }
        expected.extend(offline.process(256).remove(0));
    }
    assert!(expected.iter().any(|s| s.abs() > 0.05));

    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = vec![0.0; frames];
    let mut sent = 0;
    for chunk in out.chunks_mut(256) {
        while sent < script.len() && script[sent].0 <= r.clock() {
            s.send_events(1, vec![script[sent].1.clone()]).unwrap();
            sent += 1;
        }
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert_eq!(out, expected);
    assert_eq!(r.metrics().late_events, 0);
}

#[test]
fn a_timed_panic_sounds_the_same_prepared_as_offline() {
    use niminal_server::{At, Op};
    let script = |at: u64| Op::Panic {
        at: At::Sample(at),
        tag: None,
    };
    let held = voice_script();
    let frames = 60_000;
    let mut offline = organ_session();
    offline
        .send_events(1, held.iter().map(|(_, op)| op.clone()).collect())
        .unwrap();
    offline.send_events(1, vec![script(18_000)]).unwrap();
    let expected = offline.process(frames).remove(0);
    for ahead in [20_000u64, 500] {
        let mut s = organ_session();
        let mut r = s.start_realtime(RealtimeConfig::default());
        let mut out = vec![0.0; frames];
        let (mut sent, mut panicked) = (0, false);
        for chunk in out.chunks_mut(256) {
            while sent < held.len() && held[sent].0 <= r.clock() + ahead {
                s.send_events(1, vec![held[sent].1.clone()]).unwrap();
                sent += 1;
            }
            if !panicked && r.clock() + ahead >= 18_000 {
                s.send_events(1, vec![script(18_000)]).unwrap();
                panicked = true;
            }
            s.prepare_realtime();
            measured(&mut r, chunk.len(), chunk);
        }
        assert_eq!(out, expected, "sent {ahead} frames ahead");
    }
}

#[test]
fn instrument_scopes_and_buffer_events_allocate_nothing_while_rendering() {
    use niminal_score::Time;
    use niminal_server::{At, Op};
    let mut s = Session::new(48000.0, Layout::Mono).without_limiter();
    s.send_events(0, vec![Op::Define { source: "buffer data = zeros(4)\ninstr a() { oversample(4) { osc(sine, 440hz).tanh } + spawn(osc(square, 400hz), dur: 10ms, max: 8) { osc(sine, 220hz) * env[1 | 2ms 0] } + data[0] }".into(), at: At::Now, dir: None, tag: None }]).unwrap();
    s.send_events(
        0,
        vec![
            Op::Note {
                to: None,
                target: "a".into(),
                at: At::Now,
                args: Default::default(),
                dur: Some(Time::Seconds(0.05)),
                id: None,
                tag: None,
            },
            Op::BufferWrite {
                name: "data".into(),
                at: At::Sample(33),
                offset: 0,
                values: vec![0.1],
                tag: None,
            },
        ],
    )
    .unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = vec![0.0; 8192];
    s.prepare_realtime();
    measured(&mut r, out.len(), &mut out);
    assert!(out.iter().any(|x| x.abs() > 0.1));
}

#[test]
fn rich_parameters_and_dynamic_mapping_allocate_nothing_on_audio() {
    use niminal_score::{Time, Value};
    use niminal_server::{At, Op};
    let mut s = Session::new(48000.0, Layout::Mono).without_limiter();
    s.send_events(0,vec![Op::Define{source:"buffer one = zeros(8)\nbuffer two = zeros(8)\nctl level = 0.25\ninstr a(weights: [num] = [0.1, 0.2], source: buffer = one, value: num = 0) { spawn(osc(square, 100hz), dur: 1ms) { oversample(4) { osc(sine, 440hz) * env[1 | 1ms 0] } } + weights.sum + source[0] + value }".into(),at:At::Now,dir:None,tag:None}]).unwrap();
    s.send_events(
        0,
        vec![
            Op::Note {
                to: None,
                target: "a".into(),
                at: At::Now,
                args: Default::default(),
                dur: Some(Time::Seconds(0.1)),
                id: Some(1),
                tag: None,
            },
            Op::Array {
                id: 1,
                at: At::Sample(17),
                param: "weights".into(),
                values: vec![Value::Num(0.25), Value::Num(0.5)],
                glide: Some(Time::Seconds(0.001)),
                tag: None,
            },
            Op::BindBuffer {
                id: 1,
                at: At::Sample(33),
                param: "source".into(),
                buffer: "two".into(),
                tag: None,
            },
            Op::Map {
                id: 1,
                at: At::Sample(49),
                param: "value".into(),
                source: Some("level".into()),
                channel: 1,
                scale: 1.0,
                offset: 0.0,
                tag: None,
            },
            Op::Map {
                id: 1,
                at: At::Sample(81),
                param: "value".into(),
                source: None,
                channel: 1,
                scale: 1.0,
                offset: 0.0,
                tag: None,
            },
        ],
    )
    .unwrap();
    let mut renderer = s.start_realtime(RealtimeConfig::default());
    s.prepare_realtime();
    let mut output = vec![0.0; 8192];
    measured(&mut renderer, output.len(), &mut output);
    assert!(output.iter().all(|x| x.is_finite()));
}

#[test]
fn routing_swaps_and_draining_held_voices_do_no_heap_work_in_the_renderer() {
    use niminal_server::{At, Endpoint, Op, RoutingEdit as Edit};
    let session = || {
        let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
        s.send_events(
            1,
            vec![
                Op::Define {
                    source: "instr constant() { tail = 0ms\n 0.2 }".into(),
                    at: At::Now,
                    dir: None,
                    tag: None,
                },
                Op::Routing {
                    at: At::Now,
                    tag: None,
                    edits: vec![
                        Edit::Node {
                            id: "channel".into(),
                            fx: None,
                            instrument: None,
                            args: Default::default(),
                        },
                        Edit::Connect {
                            id: "out".into(),
                            from: Endpoint::Node {
                                id: "channel".into(),
                            },
                            to: Endpoint::Master,
                            gain: 1.0,
                            glide: 0.0,
                            delayed: false,
                        },
                    ],
                },
                Op::Note {
                    target: "constant".into(),
                    to: Some("channel".into()),
                    at: At::Now,
                    args: Default::default(),
                    dur: None,
                    id: Some(1),
                    tag: None,
                },
                Op::Routing {
                    at: At::Sample(91),
                    edits: vec![Edit::Drain {
                        id: "channel".into(),
                    }],
                    tag: None,
                },
            ],
        )
        .unwrap();
        s
    };
    let expected = session().process(512).remove(0);
    let mut s = session();
    let mut renderer = s.start_realtime(RealtimeConfig::default());
    s.prepare_realtime();
    let mut actual = vec![0.0; 512];
    measured(&mut renderer, 512, &mut actual);
    assert_eq!(actual, expected);
}

fn routing_session() -> Session {
    use serde_json::json;
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.send_events(
        1,
        serde_json::from_value(json!([
            {"op":"transaction","at":0,"source":"instr flat() { 0.2 }","edits":[
                {"op":"fx","name":"volume","source":"(level: num = 1) { out = it * level }"},
                {"op":"node","id":"a","fx":"volume"},
                {"op":"connect","id":"out","from":{"kind":"node","id":"a"},"to":{"kind":"master"}}
            ]},
            {"op":"note","target":"flat","to":"a","at":0,"dur":"1sec"}
        ]))
        .unwrap(),
    )
    .unwrap();
    s
}

#[test]
fn streamed_routing_controls_never_build_a_mixer_or_allocate_while_rendering() {
    use serde_json::json;
    let mut s = routing_session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let builds = s.realtime_metrics().unwrap().mixer_builds;
    let mut out = vec![0.0; 32];
    for i in 0..12 {
        s.send_events(
            1,
            serde_json::from_value(json!([{"op":"routing","at":r.clock()+8,"edits":[
                {"op":"set","id":"a","args":{"level":if i%2==0 {0.5} else {1.0}}},
                {"op":"gain","id":"out","value":0.5,"glide":0.0001}
            ]}]))
            .unwrap(),
        )
        .unwrap();
        s.prepare_realtime();
        measured(&mut r, 32, &mut out);
        s.sync_realtime();
        assert!(
            out[20..]
                .iter()
                .all(|v| (*v - if i % 2 == 0 { 0.05 } else { 0.1 }).abs() < 1e-6)
        );
    }
    assert_eq!(s.realtime_metrics().unwrap().mixer_builds, builds);
    assert!(
        s.routing_meters(false)[0]["nodes"][0]["peaks"][0]
            .as_f64()
            .unwrap()
            > 0.0
    );
}

#[test]
fn crossfades_and_actual_transaction_acknowledgements_do_no_heap_work_on_audio() {
    use serde_json::json;
    let mut s = routing_session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let ops: Vec<niminal_server::Op> = serde_json::from_value(json!([
        {"op":"transaction","at":17,"edits":[
            {"op":"fx","name":"twice","source":"(level: num = 1) { out = it * level * 2 }"},
            {"op":"replace","id":"a","fx":"twice","fade":8.0/48000.0}
        ]},
        {"op":"routing","at":21,"edits":[{"op":"bus","id":"unused","channels":1}]}
    ]))
    .unwrap();
    let receipt = s.send_events(1, ops.clone()).unwrap().transactions[0].id;
    let mut expected = routing_session();
    expected.send_events(1, ops).unwrap();
    let expected = expected.process(64).remove(0);
    s.prepare_realtime();
    let mut out = vec![0.0; 17];
    measured(&mut r, 17, &mut out);
    s.sync_realtime();
    assert_eq!(
        s.transaction_status(1)
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == receipt)
            .unwrap()["status"],
        "queued"
    );
    let mut tail = vec![0.0; 47];
    measured(&mut r, 47, &mut tail);
    out.extend(tail);
    s.sync_realtime();
    assert_eq!(out, expected);
    let status = s.transaction_status(1);
    let applied = status
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == receipt)
        .unwrap();
    assert_eq!(applied["status"], "applied");
    assert_eq!(applied["applied_at"], 17);
}

#[test]
fn a_graph_missed_during_starvation_is_applied_and_acknowledged_when_planning_resumes() {
    use serde_json::json;
    let mut s = routing_session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let result = s
        .send_events(
            1,
            serde_json::from_value(json!([{"op":"routing","at":32,"edits":[
                {"op":"fx","name":"double","source":"() { out = it * 2 }"},
                {"op":"replace","id":"a","fx":"double"}
            ]}]))
            .unwrap(),
        )
        .unwrap();
    let receipt = result.transactions[0].id;
    let mut before = vec![0.0; 128];
    measured(&mut r, 128, &mut before);
    s.prepare_realtime();
    assert_eq!(
        s.transaction_status(1)
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == receipt)
            .unwrap()["status"],
        "queued"
    );
    let mut after = vec![0.0; 32];
    measured(&mut r, 32, &mut after);
    s.sync_realtime();
    assert!(after.iter().all(|v| (*v - 0.4).abs() < 1e-6));
    let status = s.transaction_status(1);
    assert_eq!(
        status
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == receipt)
            .unwrap()["applied_at"],
        128
    );
}

#[test]
fn panic_during_a_fx_crossfade_matches_offline_without_heap_work_on_audio() {
    use niminal_server::{At, Op};
    use serde_json::json;
    let setup = |scheduled: bool| {
        let mut s = routing_session();
        s.send_events(1, serde_json::from_value(json!([
            {"op":"routing","at":0,"edits":[
                {"op":"fx","name":"echo","source":"() { out = it.delay(time: 1ms, feedback: 0.9) }"},
                {"op":"replace","id":"a","fx":"echo"}
            ]},
            {"op":"routing","at":128,"edits":[
                {"op":"fx","name":"double","source":"() { out = it * 2 }"},
                {"op":"replace","id":"a","fx":"double","fade":0.02}
            ]}
        ])).unwrap()).unwrap();
        if scheduled {
            s.send_events(
                1,
                vec![Op::Panic {
                    at: At::Sample(256),
                    tag: None,
                }],
            )
            .unwrap();
        }
        s
    };
    let note = || {
        serde_json::from_value(json!([
            {"op":"note","target":"flat","to":"a","at":320,"dur":"1sec"}
        ]))
        .unwrap()
    };
    for scheduled in [false, true] {
        let mut offline = setup(scheduled);
        let mut expected = offline.process(256).remove(0);
        if !scheduled {
            offline.panic();
        }
        offline.send_events(1, note()).unwrap();
        expected.extend(offline.process(256).remove(0));
        assert!(expected[128..256].iter().any(|v| *v > 0.4));
        assert!(expected[256..320].iter().all(|v| *v == 0.0));
        assert!(expected[320..].iter().all(|v| *v == 0.4));

        let mut s = setup(scheduled);
        let mut renderer = s.start_realtime(RealtimeConfig::default());
        let mut actual = vec![0.0; 512];
        measured(&mut renderer, 256, &mut actual[..256]);
        if !scheduled {
            s.panic();
        }
        s.send_events(1, note()).unwrap();
        s.prepare_realtime();
        measured(&mut renderer, 256, &mut actual[256..]);
        assert_eq!(actual, expected, "scheduled panic: {scheduled}");
    }
}

#[test]
fn late_events_use_definitions_waiting_at_the_current_sample() {
    use serde_json::json;
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    let mut renderer = s.start_realtime(RealtimeConfig::default());
    let mut audio = vec![0.0; 64];
    measured(&mut renderer, 64, &mut audio);

    // Compilation/transport missed the requested sample. The new graph waits at 64,
    // but its renderer has not consumed it when the client's next message arrives.
    let graph = s.send_events(1, serde_json::from_value(json!([
        {"op":"transaction","at":0,"source":"ctl level = 0.0.smooth(0ms)\ninstr flat() { level }","edits":[
            {"op":"node","id":"voices"},
            {"op":"connect","id":"out","from":{"kind":"node","id":"voices"},"to":{"kind":"master"}}
        ]}
    ])).unwrap()).unwrap();
    assert_eq!(graph.transactions[0].at, 64);
    let sent = s
        .send_events(
            1,
            serde_json::from_value(json!([
                {"op":"control","name":"level","at":0,"value":0.5,"glide":"0ms"},
                {"op":"note","target":"flat","to":"voices","at":0,"dur":"1sec"}
            ]))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(sent.accepted, 2);
    assert_eq!(sent.late, 2);
    assert_eq!(sent.first, Some(0));
    s.prepare_realtime();
    measured(&mut renderer, 64, &mut audio);
    assert!(audio.iter().all(|sample| *sample == 0.5));
}
