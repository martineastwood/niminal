//! A control step other than the default. The step is fixed for the life of the process, so these
//! tests live in a file of their own.

use niminal_engine::{Voice, block_size, set_control_step};
use niminal_lang::compile;

const STEP: usize = 23;

fn voice(source: &str) -> Voice {
    set_control_step(STEP).unwrap();
    let program = compile(source).unwrap_or_else(|errors| panic!("{errors:?}"));
    Voice::new(program.instruments[0].graph.clone(), 48_000.0)
}

fn render(voice: &mut Voice, total: usize, chunk: usize) -> Vec<f32> {
    let mut output = vec![0.0; total];
    for part in output.chunks_mut(chunk) {
        voice.process(part);
    }
    output
}

fn crossings(x: &[f32]) -> usize {
    x.windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count()
}

#[test]
fn an_odd_step_changes_the_value_inside_a_block() {
    assert!(block_size() > STEP);
    let out = render(&mut voice("instr a() { lfo(20hz) }"), 4096, block_size());
    for step in out.chunks(STEP) {
        assert!(step.iter().all(|v| *v == step[0]), "held through a step");
    }
    assert_ne!(out[0], out[STEP], "moves between steps in one block");
}

#[test]
fn the_frequency_follows_the_step() {
    let out = render(&mut voice("instr a() { lfo(5hz) }"), 48_000, block_size());
    let held: Vec<f32> = out.iter().step_by(STEP).copied().collect();
    let n = crossings(&held);
    assert!((9..=11).contains(&n), "{n} crossings");
    // in a stateful fn the value is read once per block, but the scope still keeps time
    let source = "fn f() { state n = 0\n n = n + 1\n lfo(5hz) + n * 0 }\ninstr a() { f() }";
    let out = render(&mut voice(source), 48_000, block_size());
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let n = crossings(&held);
    assert!((9..=11).contains(&n), "{n} crossings in a fn");
}

#[test]
fn the_sound_does_not_depend_on_how_the_calls_are_split() {
    let source = "instr a() { lfo(7hz).smooth }";
    let whole = render(&mut voice(source), 4096, block_size());
    for chunk in [4, 7, 23, 64] {
        assert_eq!(
            render(&mut voice(source), 4096, chunk),
            whole,
            "chunks of {chunk}"
        );
    }
}

#[test]
fn the_setting_is_checked() {
    assert!(set_control_step(0).is_err());
    assert!(set_control_step(2000).is_err());
}
