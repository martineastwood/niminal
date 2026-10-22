use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

fn voice(source: &str, name: &str) -> Voice {
    let program = compile(source).unwrap_or_else(|errors| panic!("{errors:?}"));
    let instrument = program.instruments.iter().find(|i| i.name == name).unwrap();
    Voice::new(instrument.graph.clone(), 48_000.0)
}

/// `blocks` whole blocks of output.
fn render(voice: &mut Voice, blocks: usize) -> Vec<f32> {
    let block = block_size();
    let mut output = vec![0.0; blocks * block];
    for chunk in output.chunks_mut(block) {
        voice.process(chunk);
    }
    output
}

fn crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
}

fn error(source: &str) -> String {
    match compile(source) {
        Ok(_) => panic!("expected an error"),
        Err(errors) => errors[0].message.clone(),
    }
}

#[test]
fn a_control_scope_holds_one_value_through_each_block() {
    let out = render(&mut voice("instr a() { control { osc(sine, 100hz) } }", "a"), 40);
    let block = block_size();
    for chunk in out.chunks(block) {
        assert!(chunk.iter().all(|v| *v == chunk[0]), "stepped within a block");
    }
    assert!(out[block..].iter().any(|v| *v != out[block]), "moves between blocks");
}

#[test]
fn an_oscillator_in_a_control_scope_keeps_its_frequency() {
    // 750 blocks is a second at 48khz and 64 samples a block; a 5hz LFO makes 10 crossings
    let blocks = 48_000 / block_size();
    let out = render(&mut voice("instr a() { control { osc(sine, 5hz) } }", "a"), blocks);
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let n = crossings(&held);
    assert!((9..=11).contains(&n), "{n} crossings");
}

#[test]
fn stateful_functions_can_use_control_scopes_too() {
    let source = "fn slow(p: num) {
        state n = 0
        n = n + 1
        control { osc(sine, 5hz) } + n * 0 + p * 0
    }
    instr a() { slow(1) }";
    let blocks = 48_000 / block_size();
    let out = render(&mut voice(source, "a"), blocks);
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let n = crossings(&held);
    assert!((9..=11).contains(&n), "{n} crossings");
}

#[test]
fn a_control_scope_reads_its_inputs_at_the_start_of_the_block() {
    let mut v = voice("instr a(x: num = 1) { control { x * 2 } }", "a");
    assert!(render(&mut v, 2).iter().all(|s| *s == 2.0));
    v.set_param(0, 3.0);
    assert!(render(&mut v, 2).iter().all(|s| *s == 6.0));
}

#[test]
fn smooth_ramps_across_the_block_to_the_new_value() {
    let mut v = voice("instr a(x: num = 0) { control { x }.smooth }", "a");
    render(&mut v, 2);
    v.set_param(0, 1.0);
    let block = block_size();
    let out = render(&mut v, 2);
    assert!(out[0] > 0.0 && out[0] < 0.1, "{}", out[0]);
    assert!(out[..block].windows(2).all(|w| w[1] > w[0]), "rises");
    assert!((out[block - 1] - 1.0).abs() < 1e-6);
    assert!(out[block..].iter().all(|s| (*s - 1.0).abs() < 1e-6));
}

#[test]
fn smooth_with_a_time_is_a_lag() {
    let mut v = voice("instr a(x: num = 0) { control { x }.smooth(5ms) }", "a");
    render(&mut v, 2);
    v.set_param(0, 1.0);
    let out = render(&mut v, 20);
    assert!(out[0] < 0.1);
    assert!(out.windows(2).all(|w| w[1] >= w[0]));
    assert!(out[out.len() - 1] > 0.9);
}

#[test]
fn hold_is_the_stepped_value() {
    let out = render(&mut voice("instr a() { control { osc(sine, 5hz) }.hold }", "a"), 4);
    assert_eq!(out[0], out[block_size() - 1]);
}

#[test]
fn rates_are_checked() {
    assert!(
        error("instr a() { osc(sine, 100hz).hold }").contains("at most once per block"),
        "hold needs a control value"
    );
    assert!(error("instr a() { osc(sine, 100hz).smooth }").contains("at most once per block"));
    let m = error("fn f() {\n state s = 0\n s = s + 1\n control { s } }\ninstr a() { f() }");
    assert!(m.contains("control"), "{m}");
}

#[test]
fn bindings_in_an_instrument_body_report_their_rate() {
    let source = "instr a(freq: hz = 440hz) {\n  slow = control { osc(sine, 2hz) }\n  fast = osc(sine, freq)\n  fast * slow\n}";
    let program = compile(source).unwrap();
    let hints: Vec<(&str, &str)> = program.instruments[0]
        .rate_hints
        .iter()
        .map(|h| (&source[h.span.start..h.span.end], h.rate))
        .collect();
    assert_eq!(hints, [("slow", "control"), ("fast", "audio")]);
}

#[test]
fn a_control_fn_runs_once_per_block_wherever_it_is_called() {
    let source = "control fn lfo(rate: hz) { osc(sine, rate) }\ninstr a() { lfo(5hz) }";
    let blocks = 48_000 / block_size();
    let out = render(&mut voice(source, "a"), blocks);
    for chunk in out.chunks(block_size()) {
        assert!(chunk.iter().all(|v| *v == chunk[0]), "stepped within a block");
    }
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let n = crossings(&held);
    assert!((9..=11).contains(&n), "{n} crossings");
}

#[test]
fn a_control_fn_can_have_state_and_branches() {
    let source = "control fn late(step: num) {
        state n = 0
        n = n + step
        if n > 3 { 1 } else { 0 }
    }
    instr a() { late(1) }";
    let out = render(&mut voice(source, "a"), 6);
    let per_block: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    assert_eq!(per_block, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
}

#[test]
fn a_control_fn_works_inside_a_stateful_fn() {
    let source = "control fn lfo(rate: hz) { osc(sine, rate) }
    fn wob(x) { state n = 0\n n = n + 1\n x * (1 + lfo(5hz)) + n * 0 }
    instr a() { wob(1) }";
    let blocks = 48_000 / block_size();
    let out = render(&mut voice(source, "a"), blocks);
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let n = crossings(&held.iter().map(|v| v - 1.0).collect::<Vec<_>>());
    assert!((9..=11).contains(&n), "{n} crossings");
}

#[test]
fn a_control_fn_has_one_result() {
    let m = error("control fn f(x) -> (a, b) { a = x\n b = x }");
    assert!(m.contains("one result"), "{m}");
}

#[test]
fn the_prelude_has_control_rate_modulators() {
    let blocks = 48_000 / block_size();
    let out = render(&mut voice("instr a() { lfo(5hz) }", "a"), blocks);
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    assert!((9..=11).contains(&crossings(&held)));
    assert!(held.iter().all(|v| v.abs() <= 1.0));

    let out = render(&mut voice("instr a() { lfo(2hz, shape: square) }", "a"), blocks);
    let flat = out.iter().filter(|v| v.abs() > 0.9).count();
    assert!(flat * 10 > out.len() * 9, "square sits at the extremes");

    let out = render(&mut voice("instr a() { random_steps(4hz) }", "a"), blocks);
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let changes = held.windows(2).filter(|w| w[0] != w[1]).count();
    assert!((3..=5).contains(&changes), "{changes} changes");

    let out = render(&mut voice("instr a() { random_glide(4hz) }", "a"), blocks);
    let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
    let changes = held.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(changes > 50, "glides between values: {changes} changes");
}

#[test]
fn a_file_s_own_definition_replaces_the_prelude() {
    let out = render(&mut voice("fn lfo(rate: hz) { 7 }\ninstr a() { lfo(5hz) }", "a"), 2);
    assert!(out.iter().all(|v| *v == 7.0));
    let out = render(&mut voice("instr lfo() { 3 }\ninstr a() { lfo() }", "a"), 2);
    assert!(out.iter().all(|v| *v == 3.0));
}

#[test]
fn explain_suggests_control_scopes_for_slow_modulators() {
    let program = compile(
        "instr a(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: osc(sine, 0.3hz).range(300hz..3khz)) }\ninstr b(freq: hz = 110hz) { osc(saw, freq).lpf(cutoff: lfo(0.3hz).range(300hz..3khz)) }",
    )
    .unwrap();
    let report = program.explain();
    let (a, b) = report.split_once("\nb\n").unwrap();
    assert!(a.contains("could run once per block"), "{report}");
    assert!(!b.contains("could run once per block"), "{report}");
}

/// `total` samples in calls of `chunk` samples.
fn render_chunks(voice: &mut Voice, total: usize, chunk: usize) -> Vec<f32> {
    let mut output = vec![0.0; total];
    for part in output.chunks_mut(chunk) {
        voice.process(part);
    }
    output
}

#[test]
fn control_scopes_sound_the_same_at_any_block_size_up_to_a_step() {
    for source in [
        "instr a() { control { osc(sine, 7hz) }.smooth }",
        "instr a() { control { osc(sine, 7hz) * env[0 100ms 1 | 100ms 0] } }",
        "fn slow(p: num) { state n = 0\n n = n + 1\n control { osc(sine, 7hz) } + n * 0 + p * 0 }\ninstr a() { slow(1) }",
        "control fn wander(rate: hz) { random_glide(rate) }\ninstr a() { wander(5hz) }",
    ] {
        let whole = render_chunks(&mut voice(source, "a"), 8192, 64);
        for chunk in [16, 32] {
            assert_eq!(
                render_chunks(&mut voice(source, "a"), 8192, chunk),
                whole,
                "{source} at {chunk}"
            );
        }
        assert!(whole.iter().any(|v| *v != whole[0]), "{source} moves");
    }
}

#[test]
fn explain_warns_about_what_a_control_step_cannot_do() {
    let program = compile(
        "instr a(freq: hz = 110hz) { osc(saw, freq) * control { osc(sine, 900hz) * env[0 0.5ms 1 | 100ms 0] } }",
    )
    .unwrap();
    let report = program.explain();
    assert!(report.contains("above half the control rate (375hz)"), "{report}");
    assert!(report.contains("shorter than one control step"), "{report}");
}

#[test]
fn smooth_works_in_a_stateful_fn_too() {
    let source = "fn f(x: num) { state n = 0\n n = n + 1\n control { x }.smooth + n * 0 }\ninstr a(x: num = 0) { f(x) }";
    let mut v = voice(source, "a");
    render(&mut v, 2);
    v.set_param(0, 1.0);
    let block = block_size();
    let out = render(&mut v, 2);
    assert!(out[0] > 0.0 && out[0] < 0.1, "{}", out[0]);
    assert!(out[..block].windows(2).all(|w| w[1] > w[0]), "rises");
    assert!((out[block - 1] - 1.0).abs() < 1e-6);
}

#[test]
fn a_control_scope_inside_a_control_scope_does_not_slow_down() {
    let blocks = 48_000 / block_size();
    for source in [
        "instr a() { control { lfo(5hz) } }",
        "control fn twice(rate: hz) { lfo(rate) }\ninstr a() { control { twice(5hz) } }",
        "fn f() { state n = 0\n n = n + 1\n control { control { osc(sine, 5hz) } } + n * 0 }\ninstr a() { f() }",
    ] {
        let out = render(&mut voice(source, "a"), blocks);
        let held: Vec<f32> = out.iter().step_by(block_size()).copied().collect();
        let n = crossings(&held);
        assert!((9..=11).contains(&n), "{source}: {n} crossings");
    }
}
