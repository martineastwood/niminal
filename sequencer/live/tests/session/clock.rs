use crate::common::*;
use niminal_live::Realtime;

#[test]
fn clock_out_starts_on_the_next_bar_and_ticks_24_to_the_beat() {
    let mut s = ready();
    s.sequencer().set_clock_out(true);
    run(&mut s, 2 * BAR);
    let sent = s.sequencer().take_clock_out();
    assert_eq!(sent[0].message, Realtime::Start);
    assert_eq!(sent[0].at % BAR as u64, 0, "on a bar line: {}", sent[0].at);
    let ticks: Vec<u64> = sent
        .iter()
        .filter(|m| m.message == Realtime::Tick)
        .map(|m| m.at)
        .collect();
    assert!(ticks.len() >= 90, "{} ticks in about a bar", ticks.len());
    assert!(
        ticks.windows(2).all(|w| w[1] - w[0] == 1_000),
        "120bpm is a tick every 1000 samples"
    );
}

#[test]
fn clock_out_follows_a_tempo_change() {
    let mut s = ready();
    s.sequencer().set_clock_out(true);
    run(&mut s, BAR + 10_000);
    let _ = s.sequencer().take_clock_out();
    s.eval("tempo 60bpm", Some("now")).unwrap();
    run(&mut s, BAR);
    let sent = s.sequencer().take_clock_out();
    let gaps: Vec<u64> = sent.windows(2).map(|w| w[1].at - w[0].at).collect();
    assert!(
        gaps.iter()
            .all(|&g| g == 1_000 || g == 2_000 || (1_900..=2_100).contains(&g))
    );
    assert_eq!(
        *gaps.last().unwrap(),
        2_000,
        "60bpm is a tick every 2000 samples: {gaps:?}"
    );
}

#[test]
fn clock_out_stops_when_switched_off() {
    let mut s = ready();
    s.sequencer().set_clock_out(true);
    run(&mut s, BAR + 1_000);
    let _ = s.sequencer().take_clock_out();
    s.sequencer().set_clock_out(false);
    run(&mut s, BAR);
    let sent = s.sequencer().take_clock_out();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message, Realtime::Stop);
}

/// Tick the sequencer's clock in as a device at `bpm` would, for `ticks` ticks.
fn tick(s: &mut Offline, bpm: f64, ticks: usize) {
    let per_tick = (60.0 * f64::from(SR) / (bpm * 24.0)).round() as usize;
    for _ in 0..ticks {
        run(s, per_tick);
        let at = s.clock() as f64;
        s.sequencer().midi_clock_at(Realtime::Tick, at);
    }
}

#[test]
fn clock_in_takes_the_tempo_and_puts_bar_one_on_the_start() {
    let mut s = ready();
    s.sequencer().set_clock_in(true);
    run(&mut s, 7_000);
    let at = s.clock() as f64;
    s.sequencer().midi_clock_at(Realtime::Start, at);
    tick(&mut s, 100.0, 24 * 16);
    let t = s.transport();
    assert!((t.bpm - 100.0).abs() < 0.5, "tempo {}", t.bpm);
    // the last tick is number 383 from the start, 383/24 beats in
    let beats = (t.bar - 1) as f64 * 4.0 + t.beat - 1.0;
    assert!((beats - 383.0 / 24.0).abs() < 0.05, "{t:?}");
}

#[test]
fn clock_in_follows_a_tempo_change_of_the_other_clock() {
    let mut s = ready();
    s.sequencer().set_clock_in(true);
    let at = s.clock() as f64;
    s.sequencer().midi_clock_at(Realtime::Start, at);
    tick(&mut s, 100.0, 24 * 8);
    tick(&mut s, 130.0, 24 * 12);
    let t = s.transport();
    assert!((t.bpm - 130.0).abs() < 1.0, "tempo {}", t.bpm);
}

#[test]
fn a_clock_that_is_not_followed_changes_nothing() {
    let mut s = ready();
    tick(&mut s, 90.0, 24 * 4);
    assert_eq!(s.transport().bpm, 120.0);
}
