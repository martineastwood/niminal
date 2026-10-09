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
        inputs: vec![ChainInput::Bus(bus)],
        route: Route::Master,
        voice_sends: vec![],
    }
}

fn block(m: &mut Mixer) -> Vec<f32> {
    let mut out = vec![0.0; BLOCK];
    m.process(&mut out);
    out
}

#[test]
fn voices_sum_to_the_master() {
    let mut m = Mixer::new(vec![master_track(), master_track()], 0, SR).unwrap();
    m.note_on(0, source(0.25, None), &[]);
    m.note_on(1, source(0.5, None), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.75).abs() < 1e-6));
}

#[test]
fn a_chain_processes_its_tracks_summed_voices() {
    let def = TrackDef {
        chain: Some(amp(2.0)),
        inputs: vec![ChainInput::It],
        route: Route::Master,
        voice_sends: vec![],
    };
    let mut m = Mixer::new(vec![def], 0, SR).unwrap();
    m.note_on(0, source(0.1, None), &[]);
    m.note_on(0, source(0.2, None), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.6).abs() < 1e-6));
}

#[test]
fn sends_reach_their_reader_in_the_same_block_whatever_the_declaration_order() {
    // The reader is declared first; it must still run after the writer.
    let writer = TrackDef { voice_sends: vec![0], ..master_track() };
    let mut m = Mixer::new(vec![reader(0, 2.0), writer], 1, SR).unwrap();
    m.note_on(1, source(0.0, Some((0, 0.25))), &[]);
    let out = block(&mut m);
    assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6), "{:?}", &out[..4]);
}

#[test]
fn buses_are_cleared_every_block() {
    let writer = TrackDef { voice_sends: vec![0], ..master_track() };
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], 1, SR).unwrap();
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
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, 0.25))), &[]);
    m.note_on(0, source(0.0, Some((0, 0.5))), &[]);
    assert!(block(&mut m).iter().all(|s| (s - 0.75).abs() < 1e-6));
}

#[test]
fn a_track_can_route_to_a_bus_instead_of_the_master() {
    let to_bus = TrackDef { route: Route::Bus(0), ..master_track() };
    let mut m = Mixer::new(vec![reader(0, 3.0), to_bus], 1, SR).unwrap();
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
        inputs: vec![ChainInput::It],
        route: Route::Master,
        voice_sends: vec![],
    };
    let mut m = Mixer::new(vec![reader(0, 1.0), chain], 1, SR).unwrap();
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
    assert!(Mixer::new(vec![a, b], 2, SR).is_err());

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
    let mut m = Mixer::new(vec![writer, reader(0, 1.0)], 1, SR).unwrap();
    m.note_on(0, source(0.0, Some((0, f32::NAN))), &[]);
    let out = block(&mut m);
    assert!(out.iter().all(|s| *s == 0.0));
    assert_eq!(m.take_silenced(), 1);
    assert_eq!(m.take_silenced(), 0);
    assert_eq!(m.active_voices(), 0);
}

#[test]
fn releasing_a_finished_voice_is_harmless() {
    let mut m = Mixer::new(vec![master_track()], 0, SR).unwrap();
    let id = m.note_on(0, source(0.5, None), &[]);
    m.release(id);
    block(&mut m);
    assert_eq!(m.active_voices(), 0);
    m.release(id);
}
