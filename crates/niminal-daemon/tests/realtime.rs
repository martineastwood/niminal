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

#[test]
fn prepared_audio_matches_direct_rendering_and_retires_without_heap_work() {
    let source = "lead.play([c4 e4 g4].fast(64))";
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
    s.eval("lead.play([c4 e4].fast(8))", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    measured(&mut r, 256, &mut out);
    s.eval(
        "track lead { instrument = tone
 out = it.lpf(cutoff: 2000hz) }",
        Some("now"),
    )
    .unwrap();
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
    s.eval("at 10ms lead(freq: 440hz) for 20ms", Some("now"))
        .unwrap();
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
 nudge: [100ms 0ms] }\nlead.play(c)";
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
    let mut r = s.start_realtime(RealtimeConfig {
        lookahead_frames: 512,
        ..RealtimeConfig::default()
    });
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
    let mut r = s.start_realtime(RealtimeConfig {
        queued_windows: 1,
        lookahead_frames: 512,
        ..RealtimeConfig::default()
    });
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
    let pending = s.eval("lead.play([c4])", Some("next beat")).unwrap();
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
    let accepted = s.eval("instr new(freq: hz) { osc(saw, freq) }\ntrack newtrack { instrument = new }\nnewtrack.play([c4])", Some("next beat")).unwrap();
    assert_eq!(accepted.lands_at, 24_000);
    let mut pos = 128;
    while pos < out.len() {
        s.prepare_realtime();
        let n = 256.min(out.len() - pos);
        measured(&mut r, n, &mut out[pos..pos + n]);
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
            s.eval(
                "track hall { out = space.reverb(room: 0.8, damp: 0.5) * 1.0 }",
                Some("now"),
            )
            .unwrap();
        }
        s.prepare_realtime();
        let n = 256.min(actual.len() - pos);
        measured(&mut r, n, &mut actual[pos..pos + n]);
        pos += n;
    }
    assert_eq!(actual, expected);
}

#[test]
fn overload_is_bounded_and_does_no_heap_work_during_rendering() {
    let mut s = session();
    s.eval(
        &"lead(freq: 440hz) for 1beat
"
        .repeat(32),
        Some("now"),
    )
    .unwrap();
    let mut r = s.start_realtime(RealtimeConfig {
        max_voices: 2,
        ..RealtimeConfig::default()
    });
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
    s.eval("lead.play([c4 e4 g4 b4].fast(32))", Some("now"))
        .unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    for i in 0..100 {
        if i % 5 == 0 {
            s.eval(
                &format!(
                    "track lead {{ instrument = tone\n out = it.lpf(cutoff: {}hz) }}",
                    1000 + i * 20
                ),
                Some("now"),
            )
            .unwrap();
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
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::create(dir.join("kit/kick.wav"), spec).unwrap();
    for _ in 0..4800 {
        wav.write_sample(32i16).unwrap();
    }
    wav.finalize().unwrap();
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.set_sample_dir(&dir);
    let mut source = String::from(
        "ctl level = 1.0\nkit k = \"kit\"\ninstr poly(freq: hz) { osc(saw, freq) * 0.001 * level }\n",
    );
    for i in 0..8 {
        source.push_str(&format!("track lead{i} {{ instrument = poly\n out = it.lpf(cutoff: 2000hz).reverb(room: 0.5, damp: 0.5) }}\nlead{i}.play([c4 e4 g4 b4].fast(64))\n"));
    }
    for i in 0..4 {
        source.push_str(&format!(
            "track drums{i} {{ instrument = k }}\ndrums{i}.play([kick].fast(128))\n"
        ));
    }
    source.push_str(&"poly(freq: 220hz) for 2beats\n".repeat(128));
    s.eval(&source, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    for i in 0..400 {
        if i % 5 == 0 {
            s.set_control("level", 0.8 + (i % 20) as f64 / 100.0, None, None, None)
                .unwrap();
        }
        if i % 50 == 0 {
            s.eval(&format!("track lead0 {{ instrument = poly\n out = it.lpf(cutoff: {}hz).reverb(room: 0.5, damp: 0.5) }}", 2000 + i), Some("next beat")).unwrap();
        }
        s.prepare_realtime();
        measured(&mut r, 256, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }
    let metrics = r.metrics();
    eprintln!(
        "12 tracks, samples, 128 held voices, control automation and quantized edit stress: {metrics:?}"
    );
    assert_eq!(metrics.capacity_drops, 0);
    assert_eq!(metrics.late_events, 0);
    assert_eq!(metrics.starved_frames, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retirement_backpressure_retains_resources_until_the_planner_returns() {
    let mut s = session();
    s.eval("lead.play([c4 e4].fast(128))", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig {
        lookahead_frames: 16_384,
        window_frames: 512,
        max_voices: 1,
        ..RealtimeConfig::default()
    });
    let mut out = [0.0; 256];
    // Consume the whole lookahead without the planner draining retired storage.
    for _ in 0..64 {
        measured(&mut r, 256, &mut out);
    }
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

#[test]
fn timestamped_controls_match_offline_audio_and_do_no_heap_work() {
    const SOURCE: &str = "ctl level = 0.0\nctl other = 1.0\ninstr flat() { level * other }\nflat() for 1beat\nat 3ms flat() for 1beat";
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(SOURCE, Some("now")).unwrap();
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(SOURCE, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let horizon = s.prepared_until();
    for session in [&mut s, &mut direct] {
        session
            .set_control("level", 1.0, None, Some(37), Some(2.0))
            .unwrap();
        session
            .set_control("other", 0.5, None, Some(80), Some(1.0))
            .unwrap();
        session
            .set_control("level", 0.2, None, Some(160), Some(1.0))
            .unwrap();
    }
    assert_eq!(s.prepared_until(), horizon);
    let expected = direct.process(512).remove(0);
    let mut out = [0.0; 512];
    measured(&mut r, 512, &mut out);
    assert_eq!(out.as_slice(), expected);
    assert_eq!(r.metrics().late_events, 0);
    s.sync_realtime();
    let replay = Session::replay(
        48_000.0,
        Layout::Mono,
        Default::default(),
        s.log(),
        512,
        false,
    );
    assert_eq!(replay[0], out);
}

#[test]
fn turning_controls_does_not_invalidate_prepared_notes_or_allocate_in_render() {
    let mut s = session();
    s.eval("ctl cutoff = 2000hz\ntrack lead { instrument = tone\n out = it.lpf(cutoff: cutoff) }\nlead.play([c4 e4 g4 b4].fast(32))", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut out = [0.0; 256];
    for i in 0..100 {
        let horizon = s.prepared_until();
        s.set_control("cutoff", 500.0 + i as f64 * 20.0, None, None, None)
            .unwrap();
        assert_eq!(s.prepared_until(), horizon);
        measured(&mut r, 256, &mut out);
        s.prepare_realtime();
        assert!(out.iter().all(|v| v.is_finite()));
    }
    assert_eq!(r.metrics().starved_frames, 0);
    assert_eq!(r.metrics().late_events, 0);
}

#[test]
fn late_control_updates_record_the_sample_actually_used_for_exact_replay() {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(
        "ctl level = 0.0.smooth(0ms)\ninstr flat() { level }\nflat() for 1beat",
        Some("now"),
    )
    .unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut out = vec![0.0; 128];
        let mut pos = 0;
        r.render(128, &mut |block, n| {
            out[pos..pos + n].copy_from_slice(&block[0][..n]);
            if pos == 0 {
                ready_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            }
            pos += n;
        });
        (r, out)
    });
    ready_rx.recv().unwrap();
    // The first block has already been mixed, but its clock has not published.
    s.set_control("level", 1.0, None, None, Some(1.0)).unwrap();
    resume_tx.send(()).unwrap();
    let (r, out) = thread.join().unwrap();
    s.sync_realtime();
    assert_eq!(r.metrics().late_events, 1);
    let niminal_daemon::LogInput::Send { ops, .. } = &s.log().last().unwrap().input else {
        panic!("the control is in the log");
    };
    assert!(matches!(ops[0], niminal_daemon::Op::Control { at: niminal_daemon::At::Sample(32), .. }), "{ops:?}");
    let replay = Session::replay(
        48_000.0,
        Layout::Mono,
        Default::default(),
        s.log(),
        128,
        false,
    );
    assert_eq!(out, replay[0]);
}

#[test]
fn quantized_control_redeclarations_restore_code_values_even_when_text_is_unchanged() {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    const DECL: &str = "ctl level = 0.0.smooth(0ms)";
    s.eval(
        &format!("{DECL}\ninstr flat() {{ level }}\nflat() for 2beats"),
        Some("now"),
    )
    .unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    s.set_control("level", 0.5, None, None, None).unwrap();
    let mut out = [0.0; 256];
    measured(&mut r, 256, &mut out);
    assert_eq!(out, [0.5; 256]);
    let accepted = s.eval(DECL, Some("next beat")).unwrap();
    assert_eq!(accepted.lands_at, 24_000);
    while r.clock() < 24_000 {
        s.prepare_realtime();
        let n = (24_000 - r.clock()).min(256) as usize;
        measured(&mut r, n, &mut out[..n]);
        assert_eq!(&out[..n], &vec![0.5; n]);
    }
    s.prepare_realtime();
    measured(&mut r, 256, &mut out);
    assert_eq!(out, [0.0; 256]);
}

#[test]
fn a_control_update_waits_for_its_new_declaration_to_reach_dsp() {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    let mut r = s.start_realtime(RealtimeConfig {
        queued_windows: 1,
        lookahead_frames: 512,
        ..Default::default()
    });
    s.eval(
        "ctl level = 0.0.smooth(0ms)\ninstr flat() { level }\nflat() for 1beat",
        Some("now"),
    )
    .unwrap();
    s.set_control("level", 0.5, None, None, None).unwrap();
    s.prepare_realtime(); // The old window occupies the only queue slot.
    let mut out = [0.0; 256];
    measured(&mut r, 256, &mut out);
    s.prepare_realtime();
    measured(&mut r, 256, &mut out);
    assert_eq!(out, [0.5; 256]);
    assert_eq!(r.metrics().control_drops, 0);
    s.sync_realtime();
    assert_eq!(s.control_values()[0].1, 0.5);
}

#[test]
fn a_held_key_sounds_through_the_prepared_path_until_it_is_let_go() {
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval("tempo 120bpm\ninstr organ(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip pad { instrument: organ\n keys: midi.keys(channel: 1) }\npad.start", Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let render = |s: &mut Session, r: &mut RealtimeRenderer, frames: usize| {
        let mut out = vec![0.0; frames];
        for chunk in out.chunks_mut(256) {
            s.prepare_realtime();
            let mut pos = 0;
            r.render(chunk.len(), &mut |b, n| {
                chunk[pos..pos + n].copy_from_slice(&b[0][..n]);
                pos += n;
            });
        }
        out
    };
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert_eq!(peak(&render(&mut s, &mut r, 2048)), 0.0);
    assert_eq!(s.key_input(1, 69, 127), 1);
    let held = render(&mut s, &mut r, 48_000);
    assert!(
        peak(&held[4096..]) > 0.5,
        "sounding a second after the key went down"
    );
    s.key_input(1, 69, 0);
    render(&mut s, &mut r, 4096);
    assert!(peak(&render(&mut s, &mut r, 8192)) < 1e-3, "released");
    // a tap is heard and ends
    s.key_input(1, 60, 127);
    s.key_input(1, 60, 0);
    let tap = render(&mut s, &mut r, 4096);
    assert!(peak(&tap) > 0.01, "even a very short tap makes a sound");
    assert!(peak(&render(&mut s, &mut r, 8192)) < 1e-3);
}

#[test]
fn a_tempo_ramp_sounds_the_same_through_the_prepared_path() {
    let code = "lead.play([c4 e4 g4 b4].fast(2))";
    let ramp = "tempo 180bpm @ now over 1bar";
    let mut expected = session();
    expected.eval(code, Some("now")).unwrap();
    let mut first = expected.process(10_240).remove(0);
    expected.eval(ramp, Some("now")).unwrap();
    first.extend(expected.process(150_000).remove(0));

    let mut s = session();
    s.eval(code, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; first.len()];
    for (i, chunk) in actual.chunks_mut(256).enumerate() {
        if i * 256 == 10_240 {
            s.eval(ramp, Some("now")).unwrap();
        }
        s.prepare_realtime();
        let mut pos = 0;
        r.render(chunk.len(), &mut |b, n| {
            chunk[pos..pos + n].copy_from_slice(&b[0][..n]);
            pos += n;
        });
    }
    assert!(first.iter().any(|v| v.abs() > 0.1));
    assert_eq!(actual, first, "the same notes in the same places");
}

#[test]
fn a_legato_glide_instrument_sounds_the_same_on_the_prepared_path_without_heap_work() {
    const SOURCE: &str = "tempo 120bpm\nmono bass(freq: hz) with(legato: true, glide: 30ms, priority: highest) { osc(saw, freq) * env[0 20ms 1 | 20ms 0] }\ntrack b { instrument = bass }\nb(freq: 110hz) for 4beats\nat 1/4beat b(freq: 165hz) for 1beat\nat 1/2beat b(freq: 82hz) for 2beats";
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(SOURCE, Some("now")).unwrap();
    let expected = direct.process(60_000).remove(0);
    assert!(expected.iter().any(|v| v.abs() > 0.1));
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(SOURCE, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 60_000];
    let mut pos = 0;
    while pos < actual.len() {
        s.prepare_realtime();
        let n = 256.min(actual.len() - pos);
        measured(&mut r, n, &mut actual[pos..pos + n]);
        pos += n;
    }
    assert_eq!(actual, expected);
}

#[test]
fn bend_pressure_and_the_pedal_sound_the_same_through_the_prepared_path() {
    use niminal_daemon::Expression::{Bend, Pressure, Sustain};
    const CODE: &str = "tempo 120bpm\ninstr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0) { osc(sine, freq + bend) * (0.2 + pressure * 0.6) * env[1 | 10ms 0] }\nclip pad { instrument: organ\n keys: midi.keys(channel: 1) }\npad.start";
    let play = |s: &mut Session, step: &mut dyn FnMut(&mut Session, usize) -> Vec<f32>| {
        let mut out = step(s, 600);
        s.key_input(1, 69, 100);
        out.extend(step(s, 2048));
        s.expression_input(1, Bend, 1.0, None);
        out.extend(step(s, 2048));
        s.expression_input(1, Pressure, 0.7, None);
        s.expression_input(1, Sustain, 1.0, None);
        s.key_input(1, 69, 0);
        out.extend(step(s, 4096));
        s.expression_input(1, Sustain, 0.0, None);
        out.extend(step(s, 4096));
        out
    };
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(CODE, Some("now")).unwrap();
    let expected = play(&mut direct, &mut |s, n| s.process(n).remove(0));

    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(CODE, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let actual = play(&mut s, &mut |s, frames| {
        let mut out = vec![0.0; frames];
        for chunk in out.chunks_mut(256) {
            s.prepare_realtime();
            let mut pos = 0;
            r.render(chunk.len(), &mut |b, n| {
                chunk[pos..pos + n].copy_from_slice(&b[0][..n]);
                pos += n;
            });
        }
        out
    });
    assert_eq!(actual.len(), expected.len());
    let worst = actual
        .iter()
        .zip(&expected)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 0.05,
        "the live path follows the offline one: {worst}"
    );
}

#[test]
fn a_lane_that_sets_a_track_parameter_sounds_the_same_prepared_as_direct_and_without_heap_work() {
    let setup = "instr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }";
    let play = |lane: &str| format!("clip c {{ notes: [c5 c5]\n{lane} }}\npad.play(c)");
    let direct = |lane: &str| {
        let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
        s.eval(setup, Some("now")).unwrap();
        s.eval(&play(lane), Some("now")).unwrap();
        s.process(96_000).remove(0)
    };
    let expected = direct("cut: [300hz 8khz]");

    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(setup, Some("now")).unwrap();
    s.eval(&play("cut: [300hz 8khz]"), Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 96_000];
    for chunk in actual.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert_eq!(actual, expected);

    // and the lane did something: the second note is brighter than with the parameter left alone
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    let dull = direct("cut: [300hz]");
    assert!(rms(&actual[48_000..]) > rms(&dull[48_000..]) * 1.5);
}

#[test]
fn an_envelope_lane_moves_a_track_parameter_the_same_prepared_as_direct_and_without_heap_work() {
    let setup = "instr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }";
    // a looping, curved sweep so that many short glides and restarts cross window edges
    let play = "clip c { notes: [c5 c5 c5 c5]\ncut: env[300hz 1/4bar.exp 6khz 1/8bar.log 300hz].loop }\npad.play(c)";
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(setup, Some("now")).unwrap();
    direct.eval(play, Some("now")).unwrap();
    let expected = direct.process(96_000).remove(0);

    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(setup, Some("now")).unwrap();
    s.eval(play, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 96_000];
    for chunk in actual.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert_eq!(actual, expected);
    assert_eq!(r.metrics().late_events, 0);
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    assert!(rms(&expected) > 0.1);
}

#[test]
fn a_signal_lane_moves_a_track_parameter_the_same_prepared_as_direct_and_without_heap_work() {
    let setup = "instr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }";
    // an LFO across a bar, with no notes of its own to step on, under a held pad
    let play = "clip c { notes: [c5 c5 c5 c5]\ncut: sine.range(300hz, 6khz) }\npad.play(c)";
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(setup, Some("now")).unwrap();
    direct.eval(play, Some("now")).unwrap();
    let expected = direct.process(96_000).remove(0);

    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(setup, Some("now")).unwrap();
    s.eval(play, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let mut actual = vec![0.0; 96_000];
    for chunk in actual.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert_eq!(actual, expected);
    assert_eq!(r.metrics().late_events, 0);
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    assert!(rms(&expected) > 0.05);
}

#[test]
fn a_device_input_reaches_a_track_that_reads_it_through_the_prepared_path() {
    let code = "tempo 120bpm\ntrack mic { out = input(1) * 0.5 }";
    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(code, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let (mut tx, ring) = niminal_daemon::AudioInput::ring(1, 8192);
    r.set_audio_input(ring);
    let heard: Vec<f32> = (0..6000).map(|i| (i as f32 / 6000.0) * 0.8).collect();
    for sample in &heard {
        tx.push(*sample).unwrap();
    }
    let mut out = vec![0.0; 6000 + 512];
    for chunk in out.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    assert!(
        out[..6000]
            .iter()
            .zip(&heard)
            .all(|(o, i)| (o - i * 0.5).abs() < 1e-6),
        "what comes in goes out, halved"
    );
    assert!(
        out[6000..].iter().all(|v| *v == 0.0),
        "and silence when the device has nothing more"
    );
}

#[test]
fn midi_out_notes_leave_the_prepared_path_at_the_samples_they_belong_to() {
    let code = "tempo 120bpm\nmidi_out synth(port: \"IAC\", channel: 2)\ntrack lead { instrument = synth }\nlead.play([c4 e4 ~ g4])";
    let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
    direct.eval(code, Some("now")).unwrap();
    direct.process(100_000);
    let expected = direct.take_midi();
    assert_eq!(
        expected.len(),
        7,
        "three notes and their ends in the first bar, then the clip starts over"
    );

    let mut s = Session::new(48_000.0, Layout::Mono).without_limiter();
    s.eval(code, Some("now")).unwrap();
    let mut r = s.start_realtime(RealtimeConfig::default());
    let (tx, mut rx) = rtrb::RingBuffer::new(64);
    r.set_midi_output(tx);
    let mut out = vec![0.0; 100_000];
    for chunk in out.chunks_mut(256) {
        s.prepare_realtime();
        measured(&mut r, chunk.len(), chunk);
    }
    let mut got = Vec::new();
    while let Ok(event) = rx.pop() {
        got.push(event);
    }
    assert_eq!(
        got, expected,
        "the same notes at the same samples as the direct path"
    );
    assert!(out.iter().all(|v| *v == 0.0));
}

/// A small deterministic generator, so a failure names the seed that found it.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % below
    }
}

const TWO_TRACKS: &str = "instr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\n\
    track lead { instrument = tone }\ntrack bass { instrument = tone }";

/// The direct path is the oracle for the prepared one: whatever edits arrive and whenever they land,
/// the two must make the same audio. This is what stops them drifting apart.
#[test]
fn random_live_edits_sound_the_same_prepared_as_direct() {
    const EDITS: [&str; 14] = [
        "lead.play([c4 e4 g4 b4].fast(16))",
        "lead.play([c4 [e4 g4]].fast(8))",
        "bass.play([c2 g2 c3].fast(4))",
        "lead.mute",
        "lead.unmute",
        "bass.mute",
        "bass.unmute",
        "lead.stop",
        "bass.stop",
        "hush",
        "tone(freq: 330hz) for 1beat",
        "instr tone(freq: hz) { osc(saw, freq) * 0.5 * env[1 | 20ms 0] }",
        "bass.solo",
        "bass.unsolo",
    ];
    const WHEN: [&str; 4] = ["now", "next beat", "next bar", "now"];
    let mut audible = 0;
    for seed in 1..=12u64 {
        let mut rng = Lcg(seed);
        let mut direct = Session::new(48_000.0, Layout::Mono).without_limiter();
        let mut live = Session::new(48_000.0, Layout::Mono).without_limiter();
        for s in [&mut direct, &mut live] {
            s.eval(TWO_TRACKS, Some("now")).unwrap();
        }
        let mut r = live.start_realtime(RealtimeConfig::default());
        let (mut expected, mut actual) = (Vec::new(), Vec::new());
        for step in 0..40 {
            if rng.next(3) == 0 {
                let (edit, when) = (EDITS[rng.next(EDITS.len())], WHEN[rng.next(WHEN.len())]);
                let a = direct.eval(edit, Some(when)).map(|a| a.lands_at);
                let b = live.eval(edit, Some(when)).map(|a| a.lands_at);
                assert_eq!(
                    a.is_ok(),
                    b.is_ok(),
                    "seed {seed} step {step}: `{edit}` @ {when}"
                );
                if let (Ok(a), Ok(b)) = (a, b) {
                    assert_eq!(
                        a, b,
                        "seed {seed} step {step}: `{edit}` @ {when} lands differently"
                    );
                }
            }
            expected.extend(direct.process(2048).remove(0));
            let mut chunk = vec![0.0; 2048];
            for part in chunk.chunks_mut(256) {
                // odd seeds plan a window at a time, as the planner thread does when others are waiting
                if seed % 2 == 1 {
                    while live.prepare_realtime_step(1) {}
                } else {
                    live.prepare_realtime();
                }
                measured(&mut r, part.len(), part);
            }
            actual.extend(chunk);
        }
        let first = expected.iter().zip(&actual).position(|(e, a)| e != a);
        assert!(
            first.is_none(),
            "seed {seed}: audio differs from sample {first:?}"
        );
        assert_eq!(r.metrics().late_events, 0, "seed {seed}");
        audible += usize::from(actual.iter().any(|v| v.abs() > 0.05));
    }
    assert!(
        audible >= 8,
        "only {audible} of 12 seeds made sound, so the comparison proves little"
    );
}

fn streamed_notes(count: usize, seconds: f64) -> Vec<niminal_daemon::Op> {
    use niminal_daemon::{At, Op};
    use niminal_score::{Time, Value};
    (0..count)
        .map(|i| Op::Note {
            target: "lead".into(),
            at: At::Time(Time::Seconds(0.05 + seconds * i as f64 / count as f64)),
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
            let start = match &events[sent] { niminal_daemon::Op::Note { at: niminal_daemon::At::Time(niminal_score::Time::Seconds(t)), .. } => (t * 48_000.0) as u64, _ => unreachable!() };
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
    let held = |id, at: u64, hz, dur: Option<f64>| (at, Op::Note { target: "organ".into(), at: At::Sample(at), args: [("freq".to_string(), Value::Hz(hz))].into(), dur: dur.map(Time::Seconds), id: Some(id), tag: None });
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
            target: "organ".into(),
            at: At::Time(Time::Beats(f64::from(beat))),
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
    use niminal_score::Time;
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
                let done = s.cancel_events(1, Some("a"), At::Time(Time::Bars(1.0)), false);
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
