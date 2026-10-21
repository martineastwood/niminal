//! Sharing a block's voices among threads changes nothing about the sound, and costs no allocation.

use niminal_server::{At, Op, Policy, Pool, RealtimeConfig, Session};
use niminal_lang::Layout;
use niminal_score::{Time, Value};
use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::*};

struct Allocator;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static HEAP_WORK: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
thread_local! {
    /// Only the test thread and the pool's workers are watched, not whatever else the process does.
    /// 1 for the test thread, 2 for a worker.
    static WATCHED: Cell<usize> = const { Cell::new(0) };
}
fn note_heap_work() {
    if ACTIVE.load(Relaxed)
        && let Ok(who @ 1..) = WATCHED.try_with(Cell::get)
    {
        HEAP_WORK[who - 1].fetch_add(1, Relaxed);
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        note_heap_work();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        note_heap_work();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, size: usize) -> *mut u8 {
        note_heap_work();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

/// Voices that send to a bus, one with a tail that dies away into tiny numbers, and a bus effect.
const SETUP: &str = "bus space\ninstr pad(freq: hz) {\n  out = (osc(saw, freq) + osc(saw, freq * 1.01)).lpf(cutoff: freq * 4, res: 0.3) * env[0 3ms 1 | 400ms.exp 0]\n  space += out.gain(-9db)\n}\nnode pads { instrument = pad }\nprocessor room {\n  out = space.delay(time: 130ms, feedback: 0.5).lpf(cutoff: 3khz)\n}\n";

fn notes(node: &str, count: usize) -> Vec<Op> {
    (0..count)
        .map(|i| Op::Note {
            to: None,
            target: node.into(),
            // off the block grid, so that blocks are split at odd places
            at: At::Sample(7 + 131 * i as u64),
            dur: Some(Time::Seconds(0.05 + 0.002 * i as f64)),
            id: None,
            tag: None,
            args: [("freq".to_string(), Value::Hz(100.0 + 7.0 * i as f64))].into(),
        })
        .collect()
}

fn session(workers: usize, init: impl Fn() + Send + Sync + 'static) -> Session {
    let mut s = Session::new(48_000.0, Layout::Stereo).without_limiter();
    if workers > 0 {
        s.set_fork(Some(Arc::new(Pool::new(workers, Policy::Always, init))));
    }
    s.send_events(0, vec![define(SETUP)]).unwrap();
    s.send_events(0, notes("pads", 60)).unwrap();
    s
}

fn define(source: &str) -> Op {
    Op::Define {
        source: source.into(),
        at: At::Now,
        dir: None,
        tag: None,
    }
}

/// Render `source` with `notes` notes on each of `nodes`, with this many helper threads.
fn render(source: &str, nodes: &[&str], notes_each: usize, workers: usize) -> Vec<Vec<f32>> {
    let mut s = Session::new(48_000.0, Layout::Stereo).without_limiter();
    if workers > 0 {
        s.set_fork(Some(Arc::new(Pool::new(workers, Policy::Always, || {}))));
    }
    s.send_events(0, vec![define(source)]).unwrap();
    for node in nodes {
        s.send_events(0, notes(node, notes_each)).unwrap();
    }
    s.process(24_000)
}

#[test]
fn the_sound_is_the_same_however_many_threads_share_the_voices() {
    let frames = 48_000;
    let serial = session(0, || {}).process(frames);
    assert!(serial[0].iter().any(|x| x.abs() > 0.05), "silent");
    for workers in [1, 2, 3, 7] {
        assert!(
            session(workers, || {}).process(frames) == serial,
            "{workers} workers sound different"
        );
    }

    // live rendering, with the workers watched for heap use
    let mut live = session(3, || WATCHED.with(|w| w.set(2)));
    let mut renderer = live.start_realtime(RealtimeConfig::default());
    WATCHED.with(|w| w.set(1));
    let mut out: Vec<Vec<f32>> = (0..2).map(|_| Vec::with_capacity(frames + 4096)).collect();
    HEAP_WORK.iter().for_each(|n| n.store(0, Relaxed));
    for _ in 0..frames / 4096 {
        // planning is the planner thread's work, and not what is measured
        live.prepare_realtime();
        ACTIVE.store(true, SeqCst);
        renderer.render(4096, &mut |block, n| {
            for (o, b) in out.iter_mut().zip(block) {
                o.extend_from_slice(&b[..n]);
            }
        });
        ACTIVE.store(false, SeqCst);
    }
    let frames = out[0].len();
    assert!(out.iter().zip(&serial).all(|(o, s)| o[..] == s[..frames]), "live rendering with workers sounds different");
    let (renderer_thread, workers) = (HEAP_WORK[0].load(Relaxed), HEAP_WORK[1].load(Relaxed));
    assert_eq!(renderer_thread, 0, "the rendering thread used the heap");
    assert_eq!(workers, 0, "the workers used the heap");
}

/// Ten nodes of ten voices: no node has many, but a wave takes chunks from all of them.
#[test]
fn chunks_of_different_nodes_share_waves_without_changing_the_sound() {
    let mut source = String::from(SETUP.split("node pads").next().unwrap());
    let nodes: Vec<String> = (0..10).map(|t| format!("t{t}")).collect();
    for t in &nodes {
        source += &format!("node {t} {{ instrument = pad }}\n");
    }
    source += "processor room {\n  out = space.delay(time: 130ms, feedback: 0.5).lpf(cutoff: 3khz)\n}\n";
    let names: Vec<&str> = nodes.iter().map(String::as_str).collect();
    let serial = render(&source, &names, 10, 0);
    assert!(serial[0].iter().any(|x| x.abs() > 0.05), "silent");
    for workers in [1, 3, 7] {
        assert!(render(&source, &names, 10, workers) == serial, "{workers} workers sound different");
    }
}

/// Voices that write a shared buffer are never run side by side, so what ends up in it is the same:
/// forty voices each store their own number, and a reader sounds whichever is left.
#[test]
fn voices_that_write_a_buffer_still_make_the_same_sound() {
    let source = "buffer last = zeros(1)\ninstr scribe(freq: hz, tag: num = 0) {\n  if note.start { last[0] = tag }\n  osc(sine, freq) * env[0 2ms 1 | 100ms 0] * 0.01\n}\ninstr look() { osc(sine, 220hz) * env[0 2ms 1 | 300ms 0] * 0.01 + last[0] * 0.001 }\nnode writers { instrument = scribe }\nnode reader { instrument = look }\n";
    let run = |workers| {
        let mut s = Session::new(48_000.0, Layout::Stereo).without_limiter();
        if workers > 0 {
            s.set_fork(Some(Arc::new(Pool::new(workers, Policy::Always, || {}))));
        }
        s.send_events(0, vec![define(source)]).unwrap();
        let mut together = notes("writers", 40);
        for (i, op) in together.iter_mut().enumerate() {
            if let Op::Note { at, args, .. } = op {
                *at = At::Sample(64);
                args.insert("tag".into(), Value::Num(i as f64));
            }
        }
        s.send_events(0, together).unwrap();
        let mut look = notes("reader", 1);
        if let Op::Note { at, args, .. } = &mut look[0] {
            *at = At::Sample(2000);
            args.clear();
        }
        s.send_events(0, look).unwrap();
        s.process(4096)
    };
    let serial = run(0);
    assert!(serial[0].iter().any(|x| x.abs() > 0.01), "silent");
    for workers in [1, 3, 7] {
        for _ in 0..5 {
            assert!(run(workers) == serial, "{workers} workers sound different");
        }
    }
}
