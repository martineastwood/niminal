use niminal_daemon::{RealtimeConfig, RealtimeRenderer, Session};
mod support;
use support::Eval;
use niminal_lang::Layout;
use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::cell::Cell;

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
    "instr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\ntrack lead { instrument = tone }";
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

fn streamed_notes(count: usize, seconds: f64) -> Vec<niminal_daemon::Op> {
    use niminal_daemon::{At, Op};
    use niminal_score::{Time, Value};
    (0..count)
        .map(|i| Op::Note {
            to: None,
            target: "lead".into(),
            at: At::Sample(((0.05 + seconds * i as f64 / count as f64) * 48000.0).round() as u64),
            dur: Some(Time::Seconds(0.02)),
            id: None,
            tag: None,
            args: [("freq".to_string(), Value::Hz(200.0 + (i % 12) as f64 * 40.0))].into(),
        })
        .collect()
}

/// Stream `events` in batches, each sent `ahead` frames before its first note, while the renderer plays.
/// Returns the audio and what the renderer reported.
fn stream_while_playing(events: &[niminal_daemon::Op], ahead: u64, frames: usize) -> (Vec<f32>, niminal_daemon::RenderMetrics) {
    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = vec![0.0; frames];
    let mut sent = 0;
    for chunk in out.chunks_mut(256) {
        let clock = r.clock();
        while sent < events.len() {
            let start = match &events[sent] { niminal_daemon::Op::Note { at: niminal_daemon::At::Sample(t), .. } => *t, _ => unreachable!() };
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

fn offline(events: &[niminal_daemon::Op], frames: usize) -> Vec<f32> {
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

fn voice_script() -> Vec<(u64, niminal_daemon::Op)> {
    use niminal_daemon::{At, Op};
    use niminal_score::{Time, Value};
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
        (14_400, Op::Set { id: 1, at: At::Sample(14_400), args: [("cut".to_string(), Value::Hz(6000.0))].into(), glide: Some(Time::Seconds(0.2)), tag: None }),
        (20_000, Op::Set { id: 2, at: At::Sample(20_000), args: [("cut".to_string(), Value::Hz(300.0))].into(), glide: None, tag: None }),
        (28_800, Op::Off { id: 1, at: At::Sample(28_800), tag: None }),
        held(3, 30_000, 220.0, None),
        (33_000, Op::Off { id: 3, at: At::Sample(33_000), tag: None }),
    ]
}

fn organ_session() -> Session {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval("instr organ(freq: hz, cut: hz = 100hz) { osc(saw, freq).lpf(cutoff: cut) * env[1 | 100ms 0] * 0.3 }", Some("now")).unwrap();
    s
}

#[test]
fn held_notes_sets_and_offs_sound_the_same_prepared_as_offline_wherever_they_are_sent() {
    let script = voice_script();
    let frames = 60_000;
    let mut offline = organ_session();
    offline.send_events(1, script.iter().map(|(_, op)| op.clone()).collect()).unwrap();
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

fn bars(hz: f64, first: u32, count: u32, tag: &str) -> Vec<niminal_daemon::Op> {
    use niminal_daemon::{At, Op};
    use niminal_score::{Time, Value};
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
    use niminal_daemon::At;
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
    use niminal_daemon::{At, Op};
    use niminal_score::{Time, Value};
    const SINE: &str = "instr organ(freq: hz, cut: hz = 100hz) { osc(sine, freq) * env[1 | 100ms 0] * 0.3 }";
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
        (0, Op::Define { source: SAW.into(), at: At::Sample(72_000), dir: None, tag: None }),
        (30_000, note(At::Sample(192_000), 440.0)),
        (60_000, note(At::Sample(96_000), 110.0)),
        (60_000, note(At::Sample(150_000), 550.0)),
    ];
    let session = || {
        let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
        s.eval(&format!("tempo 120bpm\n{SINE}"), Some("now")).unwrap();
        s
    };
    let frames = 4 * 48_000;
    let mut offline = session();
    let mut expected = Vec::new();
    let mut sent = 0;
    while expected.len() < frames {
        // the same things at the same clock as the prepared run sends them
        while sent < script.len() && script[sent].0 <= expected.len() as u64 {
            offline.send_events(1, vec![script[sent].1.clone()]).unwrap();
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
    use niminal_daemon::{At, Op};
    let script = |at: u64| Op::Panic { at: At::Sample(at), tag: None };
    let held = voice_script();
    let frames = 60_000;
    let mut offline = organ_session();
    offline.send_events(1, held.iter().map(|(_, op)| op.clone()).collect()).unwrap();
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
    use niminal_daemon::{At, Op};
    use niminal_score::Time;
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
    use niminal_daemon::{At, Op};
    use niminal_score::{Time,Value};
    let mut s=Session::new(48000.0,Layout::Mono).without_limiter();
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
    use niminal_daemon::{At, Endpoint, Op, RoutingEdit as Edit};
    let session = || {
        let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
        s.send_events(
            1,
            vec![
                Op::Define {
                    source: "instr constant() with(tail: 0ms) { 0.2 }".into(),
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
                            processor: None,
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
