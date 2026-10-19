use niminal_engine::ops::{Gain, Mul};
use niminal_engine::{BLOCK, ChainInput, GraphBuilder, Mixer, Route, Src, TrackDef, execution_order};
use std::sync::Arc;

const SR: f32 = 48_000.0;

/// An instrument that outputs `level` and sends `send` to `bus`.
fn source(level: f32, send: Option<(usize, f32)>) -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    if let Some((bus, amount)) = send {
        g.send(bus, Src::Const(amount));
    }
    let out = g.add(Gain, &[("x", Src::Const(level)), ("gain", Src::Const(1.0))]).unwrap();
    Arc::new(g.build(out))
}

/// A chain that reads one input and multiplies it.
fn amp(factor: f32) -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    let input = g.input();
    let out = g.add(Gain, &[("x", input), ("gain", Src::Const(factor))]).unwrap();
    Arc::new(g.build(out))
}

fn master_track() -> TrackDef {
    TrackDef {
        controls: Vec::new(),
        chain: None,
        inputs: vec![],
        route: Route::Master,
        voice_sends: vec![],
        voice_buffer_writes: vec![],
        voice_buffer_reads: vec![],
    }
}

fn reader(bus: usize, factor: f32) -> TrackDef {
    TrackDef {
        controls: Vec::new(),
        chain: Some(amp(factor)),
        inputs: vec![ChainInput::Bus { bus, channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
        voice_buffer_writes: vec![],
        voice_buffer_reads: vec![],
    }
}

fn block(m: &mut Mixer) -> Vec<f32> {
    let mut out = [[0.0; BLOCK]; 1];
    m.process(&mut out, BLOCK);
    out[0].to_vec()
}

/// Enough blocks for a released voice with no envelope to fade out and end.
fn fade(m: &mut Mixer) {
    for _ in 0..16 {
        block(m);
    }
}

#[test]
fn voices_sum_to_the_master() {
    let mut m = Mixer::new(vec![master_track(), master_track()], vec![], 1, SR).unwrap();
    m.note_on(0, source(0.25, None), &[], None);
    m.note_on(1, source(0.5, None), &[], None);
    assert!(block(&mut m).iter().all(|s| (s - 0.75).abs() < 1e-6));
}

#[test]
fn a_chain_processes_its_tracks_summed_voices() {
    let def = TrackDef {
        controls: Vec::new(),
        chain: Some(amp(2.0)),
        inputs: vec![ChainInput::It { channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
        voice_buffer_writes: vec![],
        voice_buffer_reads: vec![],
    };
    let mut m = Mixer::new(vec![def], vec![], 1, SR).unwrap();
    m.note_on(0, source(0.1, None), &[], None);
    m.note_on(0, source(0.2, None), &[], None);
    assert!(block(&mut m).iter().all(|s| (s - 0.6).abs() < 1e-6));
}

#[test]
fn sends_reach_their_reader_in_the_same_block_whatever_the_declaration_order() {
    // The reader is declared first; it must still run after the writer.
    let writer = TrackDef {
        controls: Vec::new(),
        voice_sends: vec![0],
        ..master_track()
    };
    let mut m = Mixer::new(vec![reader(0, 2.0), writer], vec![1], 1, SR).unwrap();
    m.note_on(1, source(0.0, Some((0, 0.25))), &[], None);
    let out = block(&mut m);
    assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6), "{:?}", &out[..4]);
}

#[test]
fn a_buffer_writer_runs_before_its_reader_whatever_the_declaration_order() {
    let writer = TrackDef {
        controls: Vec::new(),
        voice_buffer_writes: vec![0],
        ..master_track()
    };
    let reader = TrackDef {
        controls: Vec::new(),
        voice_buffer_reads: vec![0],
        ..master_track()
    };
    assert_eq!(execution_order(&[reader.clone(), writer.clone()]).unwrap(), vec![1, 0]);
    // Writing and reading the same buffer on one track is fine.
    let both = TrackDef {
        controls: Vec::new(),
        voice_buffer_writes: vec![0],
        voice_buffer_reads: vec![0],
        ..master_track()
    };
    assert_eq!(execution_order(&[both]).unwrap(), vec![0]);
}

#[test]
fn buses_are_cleared_every_block() {
    let writer = TrackDef {
        controls: Vec::new(),
        voice_sends: vec![0],
        ..master_track()
    };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
    let id = m.note_on(0, source(0.0, Some((0, 1.0))), &[], None);
    assert!(block(&mut m).iter().all(|s| *s == 1.0));
    m.release(id);
    // the voice fades out and is gone (no envelope), so nothing is written and nothing lingers
    fade(&mut m);
    assert!(block(&mut m).iter().all(|s| *s == 0.0));
}

#[test]
fn several_sends_to_one_bus_are_summed() {
    let writer = TrackDef {
        controls: Vec::new(),
        voice_sends: vec![0],
        ..master_track()
    };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, 0.25))), &[], None);
    m.note_on(0, source(0.0, Some((0, 0.5))), &[], None);
    assert!(block(&mut m).iter().all(|s| (s - 0.75).abs() < 1e-6));
}

#[test]
fn a_track_can_route_to_a_bus_instead_of_the_master() {
    let to_bus = TrackDef {
        controls: Vec::new(),
        route: Route::Bus(0),
        ..master_track()
    };
    let mut m = Mixer::new(vec![reader(0, 3.0), to_bus], vec![1], 1, SR).unwrap();
    m.note_on(1, source(0.2, None), &[], None);
    // the routed track is silent on the master; only the reader is heard
    assert!(block(&mut m).iter().all(|s| (s - 0.6).abs() < 1e-6));
}

#[test]
fn chains_can_send_too() {
    // chain: send its input to bus 0, output nothing
    let mut g = GraphBuilder::new();
    let input = g.input();
    g.send(0, input);
    let silent = g.add(Mul, &[("a", input), ("b", Src::Const(0.0))]).unwrap();
    let chain = TrackDef {
        controls: Vec::new(),
        chain: Some(Arc::new(g.build(silent))),
        inputs: vec![ChainInput::It { channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
        voice_buffer_writes: vec![],
        voice_buffer_reads: vec![],
    };
    let mut m = Mixer::new(vec![reader(0, 1.0), chain], vec![1], 1, SR).unwrap();
    m.note_on(1, source(0.4, None), &[], None);
    assert!(block(&mut m).iter().all(|s| (s - 0.4).abs() < 1e-6));
}

#[test]
fn feedback_between_tracks_is_reported_with_the_tracks_involved() {
    // 0 reads bus 0 and routes to bus 1; 1 reads bus 1 and routes to bus 0
    let a = TrackDef {
        controls: Vec::new(),
        route: Route::Bus(1),
        ..reader(0, 1.0)
    };
    let b = TrackDef {
        controls: Vec::new(),
        route: Route::Bus(0),
        ..reader(1, 1.0)
    };
    let cycle = execution_order(&[a.clone(), b.clone()]).unwrap_err();
    assert_eq!(cycle.len(), 2);
    assert!(cycle.contains(&0) && cycle.contains(&1));
    assert!(Mixer::new(vec![a, b], vec![1, 1], 1, SR).is_err());

    // a track that reads the bus it writes is a loop of one
    let c = TrackDef {
        controls: Vec::new(),
        route: Route::Bus(0),
        ..reader(0, 1.0)
    };
    assert_eq!(execution_order(&[master_track(), c]).unwrap_err(), vec![1]);
}

#[test]
fn a_cycle_is_found_even_when_a_track_merely_sits_downstream_of_it() {
    // track 0 reads bus 1 only to listen; tracks 1 and 2 loop through buses 0 and 1.
    let listener = reader(1, 1.0);
    let a = TrackDef {
        controls: Vec::new(),
        route: Route::Bus(1),
        ..reader(0, 1.0)
    };
    let b = TrackDef {
        controls: Vec::new(),
        route: Route::Bus(0),
        ..reader(1, 1.0)
    };
    let cycle = execution_order(&[listener, a, b]).unwrap_err();
    assert_eq!(cycle.len(), 2);
    assert!(cycle.contains(&1) && cycle.contains(&2));
}

#[test]
fn independent_tracks_keep_declaration_order() {
    let tracks = vec![master_track(), master_track(), master_track()];
    assert_eq!(execution_order(&tracks).unwrap(), vec![0, 1, 2]);
}

#[test]
fn nan_in_a_send_silences_the_voice_and_does_not_poison_the_bus() {
    let writer = TrackDef {
        controls: Vec::new(),
        voice_sends: vec![0],
        ..master_track()
    };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, f32::NAN))), &[], None);
    let out = block(&mut m);
    assert!(out.iter().all(|s| *s == 0.0));
    assert_eq!(m.take_silenced(), 1);
    assert_eq!(m.take_silenced(), 0);
    assert_eq!(m.active_voices(), 0);
}

#[test]
fn releasing_a_finished_voice_is_harmless() {
    let mut m = Mixer::new(vec![master_track()], vec![], 1, SR).unwrap();
    let id = m.note_on(0, source(0.5, None), &[], None);
    m.release(id);
    fade(&mut m);
    assert_eq!(m.active_voices(), 0);
    m.release(id);
}

mod stereo {
    use super::*;
    use niminal_engine::{Graph, Voice};

    /// An instrument that outputs `left` and `right` as constants.
    fn two_channels(left: f32, right: f32) -> Arc<Graph> {
        let mut g = GraphBuilder::new();
        let l = g.add(Gain, &[("x", Src::Const(left))]).unwrap();
        let r = g.add(Gain, &[("x", Src::Const(right))]).unwrap();
        Arc::new(g.build_channels(&[l, r]))
    }

    fn stereo_block(m: &mut Mixer) -> [Vec<f32>; 2] {
        let mut out = [[0.0; BLOCK]; 2];
        m.process(&mut out, BLOCK);
        [out[0].to_vec(), out[1].to_vec()]
    }

    #[test]
    fn a_voice_renders_every_channel() {
        let mut v = Voice::new(two_channels(0.25, -0.5), SR);
        assert_eq!(v.channels(), 2);
        let mut out = [[0.0; BLOCK]; 2];
        v.process_blocks(&mut out, BLOCK, &mut niminal_engine::Buses::new(&[]));
        assert!(out[0].iter().all(|s| *s == 0.25));
        assert!(out[1].iter().all(|s| *s == -0.5));
    }

    #[test]
    fn the_mixer_keeps_channels_apart() {
        let master = TrackDef {
            controls: Vec::new(),
            chain: None,
            inputs: vec![],
            route: Route::Master,
            voice_sends: vec![],
            voice_buffer_writes: vec![],
            voice_buffer_reads: vec![],
        };
        let mut m = Mixer::new(vec![master], vec![], 2, SR).unwrap();
        m.note_on(0, two_channels(0.25, 0.5), &[], None);
        m.note_on(0, two_channels(0.25, 0.0), &[], None);
        let [l, r] = stereo_block(&mut m);
        assert!(l.iter().all(|s| (s - 0.5).abs() < 1e-6));
        assert!(r.iter().all(|s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn stereo_buses_carry_both_channels_to_a_stereo_reader() {
        // voices send (0.25, 0.5) to a stereo bus; a track reads both channels,
        // doubling the left and halving the right.
        let mut gw = GraphBuilder::new();
        gw.send_channel(0, 0, Src::Const(0.25));
        gw.send_channel(0, 1, Src::Const(0.5));
        let silent = [
            gw.add(Gain, &[("x", Src::Const(0.0))]).unwrap(),
            gw.add(Gain, &[("x", Src::Const(0.0))]).unwrap(),
        ];
        let sender = Arc::new(gw.build_channels(&silent));

        let mut gr = GraphBuilder::new();
        let (il, ir) = (gr.input(), gr.input());
        let l = gr.add(Gain, &[("x", il), ("gain", Src::Const(2.0))]).unwrap();
        let r = gr.add(Gain, &[("x", ir), ("gain", Src::Const(0.5))]).unwrap();
        let reader = TrackDef {
            controls: Vec::new(),
            chain: Some(Arc::new(gr.build_channels(&[l, r]))),
            inputs: vec![
                ChainInput::Bus { bus: 0, channel: 0 },
                ChainInput::Bus { bus: 0, channel: 1 },
            ],
            route: Route::Master,
            voice_sends: vec![],
            voice_buffer_writes: vec![],
            voice_buffer_reads: vec![],
        };
        let writer = TrackDef {
            controls: Vec::new(),
            voice_sends: vec![0],
            chain: None,
            inputs: vec![],
            route: Route::Master,
            voice_buffer_writes: vec![],
            voice_buffer_reads: vec![],
        };

        let mut m = Mixer::new(vec![reader, writer], vec![2], 2, SR).unwrap();
        m.note_on(1, sender, &[], None);
        let [l, r] = stereo_block(&mut m);
        assert!(l.iter().all(|s| (s - 0.5).abs() < 1e-6), "{:?}", &l[..2]);
        assert!(r.iter().all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    #[should_panic(expected = "master's channel count")]
    fn an_instrument_with_the_wrong_channel_count_is_rejected() {
        let master = TrackDef {
            controls: Vec::new(),
            chain: None,
            inputs: vec![],
            route: Route::Master,
            voice_sends: vec![],
            voice_buffer_writes: vec![],
            voice_buffer_reads: vec![],
        };
        let mut m = Mixer::new(vec![master], vec![], 2, SR).unwrap();
        m.note_on(0, source(0.5, None), &[], None);
    }
}

mod panic {
    use super::*;
    use niminal_engine::ops::Delay;
    use niminal_engine::Graph;

    /// A chain that delays its input by `samples` with feedback.
    fn echo_chain() -> Arc<Graph> {
        let mut g = GraphBuilder::new();
        let input = g.input();
        let d = g
            .add(
                Delay::new(0.1),
                &[("x", input), ("time", Src::Const(0.001)), ("feedback", Src::Const(0.9))],
            )
            .unwrap();
        Arc::new(g.build(d))
    }

    #[test]
    fn panic_stops_voices_and_clears_the_echoes_in_a_chain() {
        let track = TrackDef {
            controls: Vec::new(),
            chain: Some(echo_chain()),
            inputs: vec![ChainInput::It { channel: 0 }],
            route: Route::Master,
            voice_sends: vec![],
            voice_buffer_writes: vec![],
            voice_buffer_reads: vec![],
        };
        let mut m = Mixer::new(vec![track], vec![], 1, SR).unwrap();
        m.note_on(0, source(0.5, None), &[], None);
        for _ in 0..10 {
            block(&mut m);
        }
        assert!(block(&mut m).iter().any(|s| s.abs() > 0.1), "sounding, with echoes");

        m.panic();
        assert_eq!(m.active_voices(), 0);
        assert!(block(&mut m).iter().all(|s| *s == 0.0), "silence within the first block");
        for _ in 0..50 {
            assert!(block(&mut m).iter().all(|s| *s == 0.0), "and the echoes are gone for good");
        }

        // the mixer still works afterwards
        m.note_on(0, source(0.25, None), &[], None);
        let later: Vec<f32> = (0..3).flat_map(|_| block(&mut m)).collect();
        assert!(later.iter().any(|s| *s != 0.0), "the 1ms delay is 48 samples, more than one block");
    }

    #[test]
    fn panic_empties_the_buses() {
        let writer = TrackDef {
            controls: Vec::new(),
            voice_sends: vec![0],
            ..master_track()
        };
        let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
        m.note_on(0, source(0.0, Some((0, 1.0))), &[], None);
        assert!(block(&mut m).iter().any(|s| *s != 0.0));
        m.panic();
        assert!(block(&mut m).iter().all(|s| *s == 0.0));
    }
}

mod adopting {
    use super::*;
    use niminal_engine::Transfer;
    use niminal_engine::ops::Delay;

    fn delay_track() -> TrackDef {
        let mut g = GraphBuilder::new();
        let input = g.input();
        let d = g
            .add(Delay::new(0.1), &[("x", input), ("time", Src::Const(0.001)), ("feedback", Src::Const(0.9))])
            .unwrap();
        TrackDef {
            controls: Vec::new(),
            chain: Some(Arc::new(g.build(d))),
            inputs: vec![ChainInput::It { channel: 0 }],
            route: Route::Master,
            voice_sends: vec![],
            voice_buffer_writes: vec![],
            voice_buffer_reads: vec![],
        }
    }

    #[test]
    fn voices_keep_sounding_on_the_new_mixer() {
        let mut old = Mixer::new(vec![master_track()], vec![], 1, SR).unwrap();
        let id = old.note_on(0, source(0.25, None), &[], None);
        block(&mut old);

        // the new mixer has an extra track first; the voice moves to the second
        let mut new = Mixer::new(vec![master_track(), master_track()], vec![], 1, SR).unwrap();
        new.adopt(
            &mut old,
            &[
                Transfer::default(),
                Transfer {
                    release: false,
                    voices_from: Some(0),
                    chain_from: None,
                },
            ],
        );
        assert_eq!(new.active_voices(), 1);
        assert_eq!(old.active_voices(), 0);
        assert!(block(&mut new).iter().all(|s| (s - 0.25).abs() < 1e-6));

        // releasing by the old id still finds it
        new.release(id);
        fade(&mut new);
        assert_eq!(new.active_voices(), 0);
    }

    #[test]
    fn a_chain_keeps_its_echoes_across_the_swap() {
        let mut old = Mixer::new(vec![delay_track()], vec![], 1, SR).unwrap();
        let id = old.note_on(0, source(0.5, None), &[], None);
        for _ in 0..4 {
            block(&mut old);
        }
        old.release(id);
        block(&mut old);

        let mut kept = Mixer::new(vec![delay_track()], vec![], 1, SR).unwrap();
        kept.adopt(
            &mut old,
            &[Transfer {
                release: false,
                voices_from: None,
                chain_from: Some(0),
            }],
        );
        let echoes: Vec<f32> = (0..3).flat_map(|_| block(&mut kept)).collect();
        assert!(echoes.iter().any(|s| s.abs() > 0.1), "the echoes carried over");

        let mut fresh_old = Mixer::new(vec![delay_track()], vec![], 1, SR).unwrap();
        let id = fresh_old.note_on(0, source(0.5, None), &[], None);
        for _ in 0..4 {
            block(&mut fresh_old);
        }
        fresh_old.release(id);
        block(&mut fresh_old);
        let mut dropped = Mixer::new(vec![delay_track()], vec![], 1, SR).unwrap();
        dropped.adopt(&mut fresh_old, &[Transfer::default()]);
        let silent: Vec<f32> = (0..3).flat_map(|_| block(&mut dropped)).collect();
        assert!(silent.iter().all(|s| *s == 0.0), "without the transfer the new chain starts empty");
    }

    #[test]
    fn serials_continue_so_ids_stay_unique() {
        let mut old = Mixer::new(vec![master_track()], vec![], 1, SR).unwrap();
        let a = old.note_on(0, source(0.1, None), &[], None);
        let mut new = Mixer::new(vec![master_track()], vec![], 1, SR).unwrap();
        new.adopt(
            &mut old,
            &[Transfer {
                release: false,
                voices_from: Some(0),
                chain_from: None,
            }],
        );
        let b = new.note_on(0, source(0.1, None), &[], None);
        assert_ne!(a, b);
        new.release(a);
        fade(&mut new);
        assert_eq!(new.active_voices(), 1, "releasing the first left the second");
    }
}

#[test]
fn a_new_note_in_a_choke_group_fades_out_the_old_one_without_touching_others() {
    let mut m = Mixer::new(vec![master_track(), master_track()], vec![], 1, SR).unwrap();
    m.note_on(0, source(0.5, None), &[], Some(1));
    m.note_on(1, source(0.25, None), &[], Some(2));
    block(&mut m);
    assert_eq!(m.active_voices(), 2);

    // the new note is in group 1, on another track
    m.note_on(1, source(0.125, None), &[], Some(1));
    let mut total = Vec::new();
    for _ in 0..12 {
        total.extend(block(&mut m));
    }
    // 5ms is 240 samples: the choked 0.5 ramps down across the first ones and is gone by 8 blocks
    assert!((total[0] - (0.5 + 0.25 + 0.125)).abs() < 1e-3, "{}", total[0]);
    assert!(total.windows(2).all(|w| w[1] <= w[0] + 1e-6), "it only gets quieter, smoothly");
    assert!((total[total.len() - 1] - 0.375).abs() < 1e-6, "{}", total[total.len() - 1]);
    assert_eq!(m.active_voices(), 2, "the choked voice has been removed");
}

#[test]
fn a_choke_fade_also_covers_what_a_voice_sends_to_a_bus() {
    let mut m = Mixer::new(vec![master_track(), reader(0, 1.0)], vec![1], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, 0.5))), &[], Some(7));
    block(&mut m);
    m.note_on(0, source(0.0, None), &[], Some(7));
    let mut tail = Vec::new();
    for _ in 0..12 {
        tail.extend(block(&mut m));
    }
    assert!(tail[0] > 0.4 && tail[tail.len() - 1].abs() < 1e-6, "{} {}", tail[0], tail[tail.len() - 1]);
}

#[test]
fn a_delayed_send_arrives_exactly_one_block_of_samples_later_whatever_the_call_sizes() {
    use niminal_engine::{BLOCK, Buses};
    // an impulse every 100 samples, written in calls of uneven length
    let mut buses = Buses::new(&[1]);
    let mut seen = Vec::new();
    let mut at = 0usize;
    for frames in [7usize, 32, 1, 20, 32, 9, 32, 32, 31, 32, 32, 32] {
        buses.begin_block(frames);
        seen.extend_from_slice(buses.get(0, 0, frames));
        let send: Vec<f32> = (0..frames).map(|i| if (at + i).is_multiple_of(100) { 1.0 } else { 0.0 }).collect();
        buses.add_delayed(0, 0, &send);
        buses.add(0, 0, &[0.0]);
        buses.end_block(frames);
        at += frames;
    }
    let arrivals: Vec<usize> = seen.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(i, _)| i).collect();
    assert_eq!(arrivals, [BLOCK, 100 + BLOCK, 200 + BLOCK], "each impulse comes out BLOCK samples after it went in");
}

/// Passes its input through, counting the samples it has been asked to process.
#[derive(Clone)]
struct Counting(Arc<std::sync::atomic::AtomicUsize>);

impl niminal_engine::Opcode for Counting {
    fn name(&self) -> &str { "counting" }
    fn ports(&self) -> &[niminal_engine::Port] {
        const PORTS: &[niminal_engine::Port] = &[niminal_engine::Port::required("x")];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn niminal_engine::Opcode> { Box::new(self.clone()) }
    fn process(&mut self, _ctx: &niminal_engine::ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        self.0.fetch_add(out.len(), std::sync::atomic::Ordering::Relaxed);
        out.copy_from_slice(inputs[0]);
    }
}

#[test]
fn a_chain_with_nothing_going_in_or_coming_out_is_left_alone_until_something_arrives() {
    use std::sync::atomic::Ordering::Relaxed;
    let processed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut g = GraphBuilder::new();
    let input = g.input();
    let out = g.add(Counting(processed.clone()), &[("x", input)]).unwrap();
    let def = TrackDef {
        controls: Vec::new(),
        chain: Some(Arc::new(g.build(out))),
        inputs: vec![ChainInput::It { channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
        voice_buffer_writes: vec![],
        voice_buffer_reads: vec![],
    };
    let mut m = Mixer::new(vec![def], vec![], 1, SR).unwrap();
    let seconds = |m: &mut Mixer, s: f32| -> f32 {
        (0..(s * SR) as usize / BLOCK).flat_map(|_| block(m)).fold(0.0, |peak, x| peak.max(x.abs()))
    };
    let id = m.note_on(0, source(0.5, None), &[], None);
    assert!(seconds(&mut m, 0.5) > 0.4);
    m.release(id);
    seconds(&mut m, 0.5);
    let resting = processed.load(Relaxed);
    assert_eq!(seconds(&mut m, 1.0), 0.0);
    assert_eq!(processed.load(Relaxed), resting, "the chain was processed while nothing went through it");
    m.note_on(0, source(0.5, None), &[], None);
    assert!(seconds(&mut m, 0.1) > 0.4, "the chain takes up again at once");
    assert!(processed.load(Relaxed) > resting);
}

/// A chain that is a reverb of its one input.
fn reverb_chain() -> Arc<niminal_engine::Graph> {
    let mut g = GraphBuilder::new();
    let input = g.input();
    let out = g.add(niminal_engine::ops::Reverb::new(), &[("x", input), ("room", Src::Const(0.5)), ("damp", Src::Const(0.5))]).unwrap();
    Arc::new(g.build(out))
}

#[test]
fn a_resting_reverb_chain_still_rings_out_its_tail_and_wakes_for_the_next_note() {
    let def = TrackDef {
        controls: Vec::new(),
        chain: Some(reverb_chain()),
        inputs: vec![ChainInput::It { channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
        voice_buffer_writes: vec![],
        voice_buffer_reads: vec![],
    };
    let mut m = Mixer::new(vec![def], vec![], 1, SR).unwrap();
    let id = m.note_on(0, source(0.5, None), &[], None);
    let heard = |m: &mut Mixer, seconds: usize| -> Vec<f32> {
        (0..seconds * SR as usize / BLOCK).flat_map(|_| block(m)).collect()
    };
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    assert!(peak(&heard(&mut m, 1)) > 1e-3, "the reverb sounds while the note is held");
    m.release(id);
    // the tail is still audible a moment after the note ends, and fades out
    assert!(peak(&heard(&mut m, 1)) > 1e-3);
    heard(&mut m, 40);
    assert_eq!(peak(&heard(&mut m, 1)), 0.0);
    m.note_on(0, source(0.5, None), &[], None);
    assert!(peak(&heard(&mut m, 1)) > 1e-3, "the next note is reverberated again");
}
