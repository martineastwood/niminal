use super::*;
use niminal_engine::{GraphBuilder, Opcode, Port, ProcessCtx};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;

#[derive(Clone)]
struct Fault {
    clone: Arc<AtomicBool>,
    process: Arc<AtomicBool>,
}
impl Opcode for Fault {
    fn name(&self) -> &str {
        "fault"
    }
    fn ports(&self) -> &[Port] {
        &[]
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        assert!(
            !self.clone.swap(false, Ordering::Relaxed),
            "injected planner failure"
        );
        Box::new(self.clone())
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], out: &mut [f32]) {
        assert!(
            !self.process.swap(false, Ordering::Relaxed),
            "injected DSP failure"
        );
        out.fill(0.125);
    }
}

fn fault_session() -> (Session, RealtimeRenderer, Fault) {
    let fault = Fault {
        clone: Arc::default(),
        process: Arc::default(),
    };
    let mut graph = GraphBuilder::new();
    let out = graph.add(fault.clone(), &[]).unwrap();
    let mut session = Session::new(48_000., Layout::Mono);
    session
        .send_events(
            0,
            vec![Op::Define {
                source: "instr tone() { 0.125 }".into(),
                at: At::Now,
                dir: None,
                tag: None,
            }],
        )
        .unwrap();
    let graph = Arc::new(graph.build(out));
    let program = Arc::make_mut(&mut session.program);
    program.nodes[0].def.chain = Some(graph.clone());
    program.instruments[0].graph = graph;
    session.mixer = session.program.mixer(48_000.);
    let renderer = session.start_realtime(RealtimeConfig::default());
    (session, renderer, fault)
}

fn audible(renderer: &mut RealtimeRenderer) {
    let mut peak = 0f32;
    renderer.render(256, &mut |block, n| {
        assert!(block[0][..n].iter().all(|v| v.is_finite()));
        for sample in &block[0][..n] {
            peak = peak.max(sample.abs());
        }
    });
    assert!(peak > 0.01, "audio resumes after recovery");
}

#[test]
fn a_dsp_panic_does_not_destroy_the_render_buffer_or_prevent_recovery() {
    let (mut session, mut renderer, fault) = fault_session();
    fault.process.store(true, Ordering::Relaxed);
    assert!(catch_unwind(AssertUnwindSafe(|| renderer.render(64, &mut |_, _| {}))).is_err());
    renderer.request_panic();
    session.prepare_realtime();
    audible(&mut renderer);
    assert_eq!(renderer.clock(), 256);
}

#[test]
fn a_sink_panic_also_preserves_the_render_buffer() {
    let (mut session, mut renderer, _) = fault_session();
    assert!(
        catch_unwind(AssertUnwindSafe(
            || renderer.render(64, &mut |_, _| panic!("sink failed"))
        ))
        .is_err()
    );
    renderer.request_panic();
    session.prepare_realtime();
    audible(&mut renderer);
}

#[test]
fn a_planner_panic_invalidates_windows_and_builds_a_clean_mixer() {
    let (mut session, mut renderer, fault) = fault_session();
    session.invalidate_live();
    let revision = renderer.state.revision.load(Ordering::Acquire);
    fault.clone.store(true, Ordering::Relaxed);
    session.prepare_realtime();
    assert_eq!(renderer.metrics().planner_panics, 1);
    assert!(renderer.state.revision.load(Ordering::Acquire) > revision);
    let builds = renderer.metrics().mixer_builds;
    session.prepare_realtime();
    assert!(renderer.metrics().mixer_builds > builds);
    audible(&mut renderer);
}

fn midi_session() -> (Session, RealtimeRenderer) {
    let mut session = Session::new(48_000., Layout::Mono);
    session
        .send_events(
            1,
            vec![Op::Define {
                source: "midi_out synth(port: \"test\", channel: 2)".into(),
                at: At::Now,
                dir: None,
                tag: None,
            }],
        )
        .unwrap();
    session
        .send_events(
            1,
            vec![
                Op::Note {
                    target: "synth".into(),
                    to: None,
                    at: At::Sample(0),
                    args: [("freq".into(), Value::Note(60))].into(),
                    dur: Some(Time::Seconds(4. / 48_000.)),
                    id: None,
                    tag: None,
                },
                Op::Note {
                    target: "synth".into(),
                    to: None,
                    at: At::Sample(1),
                    args: [("freq".into(), Value::Note(64))].into(),
                    dur: Some(Time::Seconds(4. / 48_000.)),
                    id: None,
                    tag: None,
                },
            ],
        )
        .unwrap();
    let renderer = session.start_realtime(RealtimeConfig::default());
    (session, renderer)
}

#[test]
fn a_full_midi_queue_refuses_new_notes_but_retries_the_note_off() {
    let (_session, mut renderer) = midi_session();
    let (tx, mut rx) = RingBuffer::new(1);
    renderer.set_midi_output(tx, None);
    renderer.render(8, &mut |_, _| {});
    let on = rx.pop().unwrap();
    assert!(on.on);
    assert_eq!(on.note, 60);
    assert_eq!(
        renderer.sounding.midi.len(),
        1,
        "the failed note-off stays owned"
    );
    assert!(renderer.metrics().capacity_drops > 0);
    let mut blocks = 0;
    renderer.render(256, &mut |_, _| blocks += 1);
    assert!(
        blocks <= 256 / niminal_engine::block_size() + 1,
        "an overdue MIDI off must not split every sample"
    );
    let off = rx.pop().unwrap();
    assert!(!off.on);
    assert_eq!(off.note, 60);
    assert!(renderer.sounding.midi.is_empty());
    assert!(
        rx.pop().is_err(),
        "the refused note-on was not started or released"
    );
}

#[test]
fn panic_and_reset_keep_note_offs_until_the_midi_queue_accepts_them() {
    for mode in 0..3 {
        let (session, mut renderer) = midi_session();
        let (tx, mut rx) = RingBuffer::new(1);
        renderer.set_midi_output(tx, None);
        renderer.render(1, &mut |_, _| {});
        match mode {
            0 => {
                renderer.request_panic();
                renderer.render(1, &mut |_, _| {});
            }
            1 => {
                let mut command =
                    session.reset_command(renderer.clock, &session.program, renderer.config);
                renderer.apply(&mut command.kind);
            }
            _ => {
                let command =
                    session.reset_command(renderer.clock, &session.program, renderer.config);
                let Kind::Reset { change, .. } = command.kind else {
                    unreachable!()
                };
                renderer.apply(&mut Kind::ClientPanic {
                    client: 1,
                    change,
                    transfers: Vec::new(),
                });
            }
        }
        assert_eq!(renderer.sounding.midi.len(), 1);
        assert!(rx.pop().unwrap().on);
        renderer.render(1, &mut |_, _| {});
        let off = rx.pop().unwrap();
        assert!(!off.on);
        assert_eq!(off.note, 60);
        assert!(renderer.sounding.midi.is_empty());
    }
}

#[test]
fn a_panic_while_injecting_a_note_also_restores_the_planner() {
    let (mut session, mut renderer, fault) = fault_session();
    session
        .send_events(
            0,
            vec![Op::Note {
                target: "tone".into(),
                to: None,
                at: At::Sample(32),
                args: Default::default(),
                dur: Some(Time::Seconds(0.1)),
                id: None,
                tag: None,
            }],
        )
        .unwrap();
    let revision = renderer.state.revision.load(Ordering::Acquire);
    fault.clone.store(true, Ordering::Relaxed);
    session.prepare_realtime();
    assert!(session.live.is_some());
    assert_eq!(renderer.metrics().planner_panics, 1);
    assert!(renderer.state.revision.load(Ordering::Acquire) > revision);
    session.prepare_realtime();
    audible(&mut renderer);
}
