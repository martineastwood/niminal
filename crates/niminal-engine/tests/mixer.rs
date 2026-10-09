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
    TrackDef { chain: None, inputs: vec![], route: Route::Master, voice_sends: vec![] }
}

fn reader(bus: usize, factor: f32) -> TrackDef {
    TrackDef {
        chain: Some(amp(factor)),
        inputs: vec![ChainInput::Bus { bus, channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
    }
}

fn block(m: &mut Mixer) -> Vec<f32> {
    let mut out = [[0.0; BLOCK]; 1];
    m.process(&mut out, BLOCK);
    out[0].to_vec()
}

#[test]
fn voices_sum_to_the_master() {
    let mut m = Mixer::new(vec![master_track(), master_track()], vec![], 1, SR).unwrap();
    m.note_on(0, source(0.25, None), &[]);
    m.note_on(1, source(0.5, None), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.75).abs() < 1e-6));
}

#[test]
fn a_chain_processes_its_tracks_summed_voices() {
    let def = TrackDef {
        chain: Some(amp(2.0)),
        inputs: vec![ChainInput::It { channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
    };
    let mut m = Mixer::new(vec![def], vec![], 1, SR).unwrap();
    m.note_on(0, source(0.1, None), &[]);
    m.note_on(0, source(0.2, None), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.6).abs() < 1e-6));
}

#[test]
fn sends_reach_their_reader_in_the_same_block_whatever_the_declaration_order() {
    // The reader is declared first; it must still run after the writer.
    let writer = TrackDef { voice_sends: vec![0], ..master_track() };
    let mut m = Mixer::new(vec![reader(0, 2.0), writer], vec![1], 1, SR).unwrap();
    m.note_on(1, source(0.0, Some((0, 0.25))), &[]);
    let out = block(&mut m);
    assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6), "{:?}", &out[..4]);
}

#[test]
fn buses_are_cleared_every_block() {
    let writer = TrackDef { voice_sends: vec![0], ..master_track() };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
    let id = m.note_on(0, source(0.0, Some((0, 1.0))), &[]);
    assert!(block(&mut m).iter().all(|s| *s == 1.0));
    m.release(id);
    // the voice is gone (no envelope), so nothing is written and nothing lingers
    let _ = block(&mut m);
    assert!(block(&mut m).iter().all(|s| *s == 0.0));
}

#[test]
fn several_sends_to_one_bus_are_summed() {
    let writer = TrackDef { voice_sends: vec![0], ..master_track() };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, 0.25))), &[]);
    m.note_on(0, source(0.0, Some((0, 0.5))), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.75).abs() < 1e-6));
}

#[test]
fn a_track_can_route_to_a_bus_instead_of_the_master() {
    let to_bus = TrackDef { route: Route::Bus(0), ..master_track() };
    let mut m = Mixer::new(vec![reader(0, 3.0), to_bus], vec![1], 1, SR).unwrap();
    m.note_on(1, source(0.2, None), &[]);
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
        chain: Some(Arc::new(g.build(silent))),
        inputs: vec![ChainInput::It { channel: 0 }],
        route: Route::Master,
        voice_sends: vec![],
    };
    let mut m = Mixer::new(vec![reader(0, 1.0), chain], vec![1], 1, SR).unwrap();
    m.note_on(1, source(0.4, None), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.4).abs() < 1e-6));
}

#[test]
fn feedback_between_tracks_is_reported_with_the_tracks_involved() {
    // 0 reads bus 0 and routes to bus 1; 1 reads bus 1 and routes to bus 0
    let a = TrackDef { route: Route::Bus(1), ..reader(0, 1.0) };
    let b = TrackDef { route: Route::Bus(0), ..reader(1, 1.0) };
    let cycle = execution_order(&[a.clone(), b.clone()]).unwrap_err();
    assert_eq!(cycle.len(), 2);
    assert!(cycle.contains(&0) && cycle.contains(&1));
    assert!(Mixer::new(vec![a, b], vec![1, 1], 1, SR).is_err());

    // a track that reads the bus it writes is a loop of one
    let c = TrackDef { route: Route::Bus(0), ..reader(0, 1.0) };
    assert_eq!(execution_order(&[master_track(), c]).unwrap_err(), vec![1]);
}

#[test]
fn a_cycle_is_found_even_when_a_track_merely_sits_downstream_of_it() {
    // track 0 reads bus 1 only to listen; tracks 1 and 2 loop through buses 0 and 1.
    let listener = reader(1, 1.0);
    let a = TrackDef { route: Route::Bus(1), ..reader(0, 1.0) };
    let b = TrackDef { route: Route::Bus(0), ..reader(1, 1.0) };
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
    let writer = TrackDef { voice_sends: vec![0], ..master_track() };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, f32::NAN))), &[]);
    let out = block(&mut m);
    assert!(out.iter().all(|s| *s == 0.0));
    assert_eq!(m.take_silenced(), 1);
    assert_eq!(m.take_silenced(), 0);
    assert_eq!(m.active_voices(), 0);
}

#[test]
fn releasing_a_finished_voice_is_harmless() {
    let mut m = Mixer::new(vec![master_track()], vec![], 1, SR).unwrap();
    let id = m.note_on(0, source(0.5, None), &[]);
    m.release(id);
    block(&mut m);
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
        let master = TrackDef { chain: None, inputs: vec![], route: Route::Master, voice_sends: vec![] };
        let mut m = Mixer::new(vec![master], vec![], 2, SR).unwrap();
        m.note_on(0, two_channels(0.25, 0.5), &[]);
        m.note_on(0, two_channels(0.25, 0.0), &[]);
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
            chain: Some(Arc::new(gr.build_channels(&[l, r]))),
            inputs: vec![
                ChainInput::Bus { bus: 0, channel: 0 },
                ChainInput::Bus { bus: 0, channel: 1 },
            ],
            route: Route::Master,
            voice_sends: vec![],
        };
        let writer = TrackDef { voice_sends: vec![0], chain: None, inputs: vec![], route: Route::Master };

        let mut m = Mixer::new(vec![reader, writer], vec![2], 2, SR).unwrap();
        m.note_on(1, sender, &[]);
        let [l, r] = stereo_block(&mut m);
        assert!(l.iter().all(|s| (s - 0.5).abs() < 1e-6), "{:?}", &l[..2]);
        assert!(r.iter().all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    #[should_panic(expected = "master's channel count")]
    fn an_instrument_with_the_wrong_channel_count_is_rejected() {
        let master = TrackDef { chain: None, inputs: vec![], route: Route::Master, voice_sends: vec![] };
        let mut m = Mixer::new(vec![master], vec![], 2, SR).unwrap();
        m.note_on(0, source(0.5, None), &[]);
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
            chain: Some(echo_chain()),
            inputs: vec![ChainInput::It { channel: 0 }],
            route: Route::Master,
            voice_sends: vec![],
        };
        let mut m = Mixer::new(vec![track], vec![], 1, SR).unwrap();
        m.note_on(0, source(0.5, None), &[]);
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
        m.note_on(0, source(0.25, None), &[]);
        let later: Vec<f32> = (0..3).flat_map(|_| block(&mut m)).collect();
        assert!(later.iter().any(|s| *s != 0.0), "the 1ms delay is 48 samples, more than one block");
    }

    #[test]
    fn panic_empties_the_buses() {
        let writer = TrackDef { voice_sends: vec![0], ..master_track() };
        let mut m = Mixer::new(vec![writer, reader(0, 1.0)], vec![1], 1, SR).unwrap();
        m.note_on(0, source(0.0, Some((0, 1.0))), &[]);
        assert!(block(&mut m).iter().any(|s| *s != 0.0));
        m.panic();
        assert!(block(&mut m).iter().all(|s| *s == 0.0));
    }
}
