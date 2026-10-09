use niminal_daemon::{RealtimeConfig, RealtimeRenderer, Session};
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
        MEASURE.with(|on| if on.get() { ALLOCS.with(|n| n.set(n.get() + 1)); });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        MEASURE.with(|on| if on.get() { FREES.with(|n| n.set(n.get() + 1)); });
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, size: usize) -> *mut u8 {
        MEASURE.with(|on| if on.get() { ALLOCS.with(|n| n.set(n.get() + 1)); FREES.with(|n| n.set(n.get() + 1)); });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

const SETUP: &str = "instr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\ntrack lead { instrument = tone }";
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
    r.render(n, &mut |b, n| { out[pos..pos+n].copy_from_slice(&b[0][..n]); pos += n; });
    MEASURE.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 0, "render allocated");
    assert_eq!(FREES.with(Cell::get), 0, "render destroyed heap storage");
}

#[test]
fn prepared_audio_matches_direct_rendering_and_retires_without_heap_work() {
    let source = "play lead = [c4 e4 g4].fast(64)";
    let mut expected = session();
    expected.eval(source, Some("now")).unwrap();
    let expected = expected.process(48_000).remove(0);
    let mut s = session();
    s.eval(source, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 48_000];
    for chunk in actual.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert_eq!(actual, expected);
    assert_eq!(r.metrics().starved_frames, 0);
    assert_eq!(r.metrics().late_events, 0);
}

#[test]
fn live_graph_edits_and_panic_do_no_heap_work() {
    let mut s = session();
    s.eval("play lead = [c4 e4].fast(8)", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    measured(&mut r, 256, &mut out);
    s.eval("track lead { instrument = tone
 out = it.lpf(cutoff: 2000hz) }", Some("now")).unwrap();
    s.prepare_realtime();
    measured(&mut r, 256, &mut out);
    s.prepare_realtime();
    measured(&mut r, 256, &mut out);
    assert!(out.iter().any(|v| v.abs() > 0.1));
    s.panic();
    measured(&mut r, 256, &mut out);
    assert_eq!(out, [0.0; 256]);
    s.prepare_realtime();
    measured(&mut r, 256, &mut out);
    assert_eq!(out, [0.0; 256]);
}

#[test]
fn notes_wait_for_the_exact_sample_and_future_preparation_is_separate() {
    let mut s = session();
    s.eval("at 10ms lead(freq: 440hz) for 20ms", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    assert_eq!(s.prepared_until(), 8192);
    assert_eq!(r.clock(), 0);
    let mut out = [0.0; 1024];
    measured(&mut r, 1024, &mut out);
    assert_eq!(out[..480], [0.0; 480]);
    assert!(out[481..].iter().any(|v| v.abs() > 0.1));
    assert_eq!(r.metrics().late_events, 0);
}

#[test]
fn delayed_notes_do_not_hold_up_earlier_notes_in_later_windows() {
    let source = "clip c { notes: [c4 e4 g4 b4].fast(16)
 nudge: [100ms 0ms] }\nplay lead = c";
    let mut direct = session();
    direct.eval(source, Some("now")).unwrap();
    let expected = direct.process(12_000).remove(0);
    let mut s = session();
    s.eval(source, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 12_000];
    for chunk in actual.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert_eq!(actual, expected);
}

#[test]
fn empty_event_queue_keeps_dsp_running_and_reports_starvation() {
    let mut s = session();
    s.eval("lead(freq: 440hz) for 1beat", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig { lookahead_frames: 512, ..RealtimeConfig::default() });
    let mut out = [0.0; 4096];
    measured(&mut r, 4096, &mut out);
    assert!(out[512..].iter().any(|v| v.abs() > 0.1));
    assert!(r.metrics().starved_frames >= 3584);
    s.prepare_realtime();
    measured(&mut r, 256, &mut out[..256]);
}

#[test]
fn revisions_are_not_lost_when_the_ready_queue_is_full() {
    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig { queued_windows: 1, lookahead_frames: 512, ..RealtimeConfig::default() });
    s.eval("lead(freq: 440hz) for 1beat", Some("now")).unwrap();
    s.prepare_realtime(); // Stale window still occupies the sole slot.
    let mut out = [0.0; 128];
    measured(&mut r, 128, &mut out);
    s.prepare_realtime();
    measured(&mut r, 128, &mut out);
    assert!(out.iter().any(|v| v.abs() > 0.1));
    assert!(r.metrics().starved_frames > 0);
}

#[test]
fn cancelled_quantized_changes_never_reach_the_renderer() {
    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    measured(&mut r, 256, &mut out);
    let pending = s.eval("play lead = [c4]", Some("next beat")).unwrap();
    s.prepare_realtime();
    assert_eq!(s.cancel(pending.id), 1);
    for _ in 0..110 {
        s.prepare_realtime();
        measured(&mut r, 256, &mut out);
        assert_eq!(out, [0.0; 256]);
    }
}

#[test]
fn quantized_definitions_and_notes_land_inside_prepared_windows() {
    let mut s = session();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = vec![0.0; 24_512];
    measured(&mut r, 128, &mut out[..128]);
    let accepted = s.eval("instr new(freq: hz) { osc(saw, freq) }\ntrack newtrack { instrument = new }\nplay newtrack = [c4]", Some("next beat")).unwrap();
    assert_eq!(accepted.lands_at, 24_000);
    let mut pos = 128;
    while pos < out.len() {
        s.prepare_realtime();
        let n = 256.min(out.len() - pos);
        measured(&mut r, n, &mut out[pos..pos+n]);
        pos += n;
    }
    assert_eq!(out[..24_000], vec![0.0; 24_000]);
    assert!(out[24_000..].iter().any(|v| v.abs() > 0.1));
    assert_eq!(r.metrics().late_events, 0);
    s.sync_realtime();
    assert!(s.pending().is_empty());
}

#[test]
fn compatible_effect_memory_survives_a_prepared_swap() {
    const SOURCE: &str = "bus space\ninstr click(freq: hz) { space += osc(sine, 1000hz) * env[1 0.5ms 0] }\ntrack hall { out = space.reverb(room: 0.8, damp: 0.5) }\ntrack src { instrument = click }\nsrc(freq: 100hz) for 1/8beat";
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(SOURCE, Some("now")).unwrap();
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(SOURCE, Some("now")).unwrap();
    let expected = direct.process(12_000).remove(0);
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 12_000];
    let mut pos = 0;
    while pos < actual.len() {
        if pos == 4096 {
            s.eval("track hall { out = space.reverb(room: 0.8, damp: 0.5) * 1.0 }", Some("now")).unwrap();
        }
        s.prepare_realtime();
        let n = 256.min(actual.len() - pos);
        measured(&mut r, n, &mut actual[pos..pos+n]);
        pos += n;
    }
    assert_eq!(actual, expected);
}

#[test]
fn overload_is_bounded_and_does_no_heap_work_during_rendering() {
    let mut s = session();
    s.eval(&"lead(freq: 440hz) for 1beat
".repeat(32), Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig { max_voices: 2, ..RealtimeConfig::default() });
    let mut out = [0.0; 256];
    for _ in 0..100 {
        s.prepare_realtime();
        measured(&mut r, 256, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }
    assert!(r.metrics().capacity_drops > 0);
}

#[test]
fn repeated_edits_keep_audio_finite_and_render_without_heap_work() {
    let mut s = session();
    s.eval("play lead = [c4 e4 g4 b4].fast(32)", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    for i in 0..100 {
        if i % 5 == 0 {
            s.eval(&format!("track lead {{ instrument = tone\n out = it.lpf(cutoff: {}hz) }}", 1000 + i * 20), Some("now")).unwrap();
        }
        s.prepare_realtime();
        measured(&mut r, 256, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }
    let metrics = r.metrics();
    eprintln!("256-frame edit stress: {metrics:?}");
    assert_eq!(metrics.capacity_drops, 0);
    assert_eq!(metrics.late_events, 0);
}

#[test]
fn samples_polyphony_effects_and_quantized_edits_share_the_prepared_path() {
    let dir = std::env::temp_dir().join(format!("niminal-prepared-samples-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("kit")).unwrap();
    let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut wav = hound::WavWriter::create(dir.join("kit/kick.wav"), spec).unwrap();
    for _ in 0..4800 { wav.write_sample(32i16).unwrap(); }
    wav.finalize().unwrap();
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.set_sample_dir(&dir);
    let mut source = String::from("kit k = \"kit\"\ninstr poly(freq: hz) { osc(saw, freq) * 0.001 }\n");
    for i in 0..8 {
        source.push_str(&format!("track lead{i} {{ instrument = poly\n out = it.lpf(cutoff: 2000hz).reverb(room: 0.5, damp: 0.5) }}\nplay lead{i} = [c4 e4 g4 b4].fast(64)\n"));
    }
    for i in 0..4 {
        source.push_str(&format!("track drums{i} {{ instrument = k }}\nplay drums{i} = [kick].fast(128)\n"));
    }
    source.push_str(&"poly(freq: 220hz) for 2beats\n".repeat(128));
    s.eval(&source, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    for i in 0..400 {
        if i % 50 == 0 {
            s.eval(&format!("track lead0 {{ instrument = poly\n out = it.lpf(cutoff: {}hz).reverb(room: 0.5, damp: 0.5) }}", 2000 + i), Some("next beat")).unwrap();
        }
        s.prepare_realtime();
        measured(&mut r, 256, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }
    let metrics = r.metrics();
    eprintln!("12 tracks, samples, 128 held voices and quantized edit stress: {metrics:?}");
    assert_eq!(metrics.capacity_drops, 0);
    assert_eq!(metrics.late_events, 0);
    assert_eq!(metrics.starved_frames, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retirement_backpressure_retains_resources_until_the_planner_returns() {
    let mut s = session();
    s.eval("play lead = [c4 e4].fast(128)", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig {
        lookahead_frames: 16_384, max_voices: 1, ..RealtimeConfig::default()
    });
    let mut out = [0.0; 256];
    // Consume the whole lookahead without the planner draining retired storage.
    for _ in 0..64 { measured(&mut r, 256, &mut out); }
    assert!(r.metrics().retire_pressure > 0);
    s.prepare_realtime();
    for _ in 0..8 {
        measured(&mut r, 256, &mut out);
        s.prepare_realtime();
    }
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn a_scheduled_panic_discards_explicit_notes_prepared_for_later() {
    let source = "at 20ms panic\nat 100ms lead(freq: 440hz) for 1beat";
    let mut s = session();
    s.eval(source, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    for _ in 0..40 {
        s.prepare_realtime();
        measured(&mut r, 256, &mut out);
        assert_eq!(out, [0.0; 256]);
    }
}
