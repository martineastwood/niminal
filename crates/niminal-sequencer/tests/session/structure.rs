//! Controls that move what a pattern plays: `window`, `phase` and `density` read a `ctl` as the notes are asked for.
use crate::common::*;
use niminal_sequencer::ControlSignal;

const BEAT: usize = BAR / 4;

/// The pitch of each beat of the bar that starts at `bar` (a bar is four beats of a four-note motif), in hertz.
fn bar_pitches(out: &[f32], bar: usize) -> Vec<u32> {
    (0..4)
        .map(|beat| {
            let from = (bar * BAR + beat * BEAT) + 2_400;
            let slice = &out[from..from + BEAT - 4_800];
            // zero crossings of a sine, over the slice, are its frequency
            (crossings(slice) as f32 / (slice.len() as f32 / SR)).round() as u32
        })
        .collect()
}

fn cc(s: &mut Offline, cc: u8, value: u8) {
    assert_eq!(
        s.control_input(&ControlSignal::MidiCc {
            cc,
            channel: 1,
            value
        }),
        1
    );
}

fn near(got: &[u32], want: &[u32]) {
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert!(g.abs_diff(*w) <= 6, "{got:?} is not {want:?}");
    }
}

const C4: u32 = 262;
const E4: u32 = 330;
const G4: u32 = 392;
const B4: u32 = 494;

fn piece(position: &str, notes: &str) -> String {
    format!(
        "{SETUP}ctl position = midi.cc(20).range(0..4){position}\nctl size = midi.cc(21).range(1..4)\n\
         ctl amount = midi.cc(22).range(0..1)\n\
         clip run {{ instrument: pluck\n  notes: {notes} }}\nrun.start\n"
    )
}

#[test]
fn a_knob_moves_the_window_of_the_running_pattern() {
    let mut s = session();
    s.eval(
        &piece("", "[c4 e4 g4 b4].window(position, size, steps: 4)"),
        Some("now"),
    )
    .unwrap();
    // the whole motif, then a window of one note, moved along as it plays
    cc(&mut s, 21, 127);
    let mut out = run(&mut s, BAR);
    // half a bar before the next one, so what is asked for ahead is not yet played
    out.extend(run(&mut s, BAR - 2 * BEAT));
    cc(&mut s, 20, 64); // a start of 2 of 4 steps
    out.extend(run(&mut s, 2 * BEAT));
    out.extend(run(&mut s, 2 * BAR));
    near(&bar_pitches(&out, 0), &[C4, E4, G4, B4]);
    // the knob moved half way through bar 1: what was asked for ahead has already gone out, and what is next has not
    let bar1 = bar_pitches(&out, 1);
    near(&bar1[..2], &[C4, E4]);
    assert!(bar1[3].abs_diff(E4) <= 6, "{bar1:?}");
    near(&bar_pitches(&out, 2), &[G4, B4, C4, E4]);
}

#[test]
fn a_quantized_knob_waits_for_its_boundary_and_what_is_before_it_is_as_it_was() {
    let mut s = session();
    s.eval(
        &piece(".quantize(1bar)", "[c4 e4 g4 b4].window(position, size, steps: 4)"),
        Some("now"),
    )
    .unwrap();
    cc(&mut s, 21, 127);
    let mut out = run(&mut s, BAR / 2);
    // sent in the middle of the first bar, it lands on the bar line
    cc(&mut s, 20, 64);
    out.extend(run(&mut s, 3 * BAR));
    near(&bar_pitches(&out, 0), &[C4, E4, G4, B4]);
    near(&bar_pitches(&out, 1), &[G4, B4, C4, E4]);
}

#[test]
fn density_turned_up_only_adds_notes() {
    let mut s = session();
    s.eval(&piece("", "[c4 e4 g4 b4].density(amount)"), Some("now")).unwrap();
    let mut sounding = Vec::new();
    for knob in [0u8, 40, 80, 127] {
        cc(&mut s, 22, knob);
        // let the lookahead pass, then listen to a bar
        run(&mut s, 4_800 + 1);
        let out = run(&mut s, BAR);
        let _ = run(&mut s, BAR - 4_801 + 1);
        let heard: Vec<bool> = (0..4)
            .map(|beat| rms(&out[beat * BEAT..beat * BEAT + BEAT]) > 0.01)
            .collect();
        sounding.push(heard);
    }
    assert_eq!(sounding[0], [false; 4]);
    assert_eq!(sounding[3], [true; 4]);
    // each step up keeps every note that was already sounding
    for pair in sounding.windows(2) {
        assert!(
            pair[0].iter().zip(&pair[1]).all(|(before, after)| !before || *after),
            "{sounding:?}"
        );
    }
}

#[test]
fn a_piece_sent_again_keeps_the_knobs_where_they_were() {
    let mut s = session();
    let notes = "[c4 e4 g4 b4].window(position, size, steps: 4)";
    s.eval(&piece("", notes), Some("now")).unwrap();
    cc(&mut s, 21, 127);
    cc(&mut s, 20, 64);
    // something else in the piece changes; the controls are sent again as they were written
    s.eval(&piece("", &format!("{notes}.fast(1)")), Some("now")).unwrap();
    let out = run(&mut s, 2 * BAR);
    near(&bar_pitches(&out, 0), &[G4, B4, C4, E4]);
}

#[test]
fn a_quantized_knob_turned_just_before_a_boundary_waits_for_the_next_one_it_can_make() {
    let mut s = session();
    s.eval(
        &piece(".quantize(1bar)", "[c4 e4 g4 b4].window(position, size, steps: 4)"),
        Some("now"),
    )
    .unwrap();
    cc(&mut s, 21, 127);
    // 50ms before the bar line: the notes up to it, and past it, have already gone to the engine
    let mut out = run(&mut s, BAR - 2_400);
    cc(&mut s, 20, 64);
    out.extend(run(&mut s, 3 * BAR + 2_400));
    // bar 1 plays as it was, all of it, and the change lands on bar 2 for the sound and the pattern alike
    near(&bar_pitches(&out, 1), &[C4, E4, G4, B4]);
    near(&bar_pitches(&out, 2), &[G4, B4, C4, E4]);
}
