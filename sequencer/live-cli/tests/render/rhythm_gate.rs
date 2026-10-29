//! Sample-level timing of rhythmic audio gates, independently of note retriggering.
use crate::common::*;

fn audio(expression: &str, bpm: f64) -> Vec<f32> {
    let src = format!(
        "tempo {bpm}bpm\nbus source\ninstr steady() {{ source += env[1 | 5ms 0] }}\nsteady() for 2sec\nfx chopped {{ out = source * ({expression}) }}"
    );
    let p = program(&src);
    let schedule = p.schedule();
    let mut session =
        niminal_live::Offline::new(SR as f32, niminal_lang::Layout::Mono).without_limiter();
    session.play_program(p, schedule).unwrap();
    let mut out = session.process((1.1 * SR) as usize);
    let notices = session.take_notices();
    assert!(notices.is_empty(), "{notices:?}");
    out.remove(0)
}

fn level(out: &[f32], seconds: f64, expected: f32) {
    let got = out[(seconds * SR).round() as usize];
    assert!(
        (got - expected).abs() < 0.003,
        "at {seconds}s: {got}, expected {expected}"
    );
}

#[test]
fn width_attack_and_release_fit_inside_each_pulse() {
    let out = audio(
        "0.5.gate(grid[xxxx].over(1beat), width: 60%, attack: 5ms, release: 10ms)",
        120.0,
    );
    // A sixteenth is 125ms: attack 0..5, hold 5..65, release 65..75, closed 75..125.
    for (time, expected) in [
        (0.0025, 0.25),
        (0.005, 0.5),
        (0.060, 0.5),
        (0.070, 0.25),
        (0.075, 0.0),
        (0.100, 0.0),
        (0.1275, 0.25),
    ] {
        level(&out, time, expected);
    }
}

#[test]
fn long_edges_shrink_proportionally_to_a_triangle() {
    let out = audio(
        "0.5.gate(grid[xxxx].over(1beat), width: 40%, attack: 40ms, release: 60ms)",
        120.0,
    );
    // 50ms pulse: the requested 100ms of edges becomes a 20ms attack and 30ms release.
    for (time, expected) in [
        (0.010, 0.25),
        (0.020, 0.5),
        (0.035, 0.25),
        (0.050, 0.0),
        (0.075, 0.0),
    ] {
        level(&out, time, expected);
    }
}

#[test]
fn full_width_hits_join_and_shape_the_whole_run() {
    let out = audio(
        "0.5.gate(grid[.xx.].over(1beat), width: 100%, attack: 200ms, release: 100ms)",
        120.0,
    );
    // Two joined steps form a 250ms pulse starting at 125ms. Edges shrink 5/6.
    level(&out, 0.100, 0.0);
    level(&out, 0.225, 0.3);
    level(&out, 0.250, 0.375); // No closing/reopening at the step boundary.
    level(&out, 0.3333333, 0.25);
    level(&out, 0.400, 0.0);
}

#[test]
fn joined_hits_wrap_without_retriggering_at_the_bar_boundary() {
    let out = audio(
        "0.5.gate(grid[x..x].over(1beat), attack: 10ms, release: 10ms)",
        120.0,
    );
    for time in [0.499, 0.500, 0.501, 0.510] {
        level(&out, time, 0.5);
    }
    level(&out, 0.200, 0.0);
}

#[test]
fn a_dsp_grid_is_a_level_signal_and_uses_the_live_tempo() {
    for bpm in [60.0, 120.0, 180.0] {
        let out = audio("grid[x.oX].over(1beat).width(50%) * 0.25", bpm);
        let step = 60.0 / bpm / 4.0;
        level(&out, step * 0.25, 0.25);
        level(&out, step * 0.75, 0.0);
        level(&out, step * 1.25, 0.0);
        level(&out, step * 2.25, 0.25 * 10f32.powf(-12.0 / 20.0));
        level(&out, step * 3.25, 0.25 * 10f32.powf(3.0 / 20.0));
    }
}

#[test]
fn zero_width_silent_grids_and_continuous_grids_are_well_defined() {
    for expr in ["0.5.gate(grid[xxxx], width: 0%)", "0.5.gate(grid[....])"] {
        let out = audio(expr, 120.0);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1e-6));
    }
    let out = audio("0.5.gate(grid[xxxx], attack: 20ms, release: 20ms)", 120.0);
    for time in [0.010, 0.125, 0.500, 1.0] {
        level(&out, time, 0.5);
    }
}

#[test]
fn invalid_gate_settings_have_useful_errors() {
    for (expr, expected) in [
        ("0.5.gate(grid[])", "at least one step"),
        ("0.5.gate(grid[xx], width: 101%)", "between 0% and 100%"),
        ("0.5.gate(grid[xx], release: -2ms)", "must be nonnegative"),
        ("0.5.gate(grid[xx], attack: 1)", "time"),
        ("0.5.gate(grid[xx], width: 20%, width: 30%)", "given twice"),
        ("grid[xx].over(0beats)", "positive beats or bars"),
        ("grid[xx].over(1sec)", "positive beats or bars"),
        ("grid[xx].width(20%).width(30%)", "given twice"),
    ] {
        let src = format!("fx chopped {{ out = {expr} }}");
        let errors = compile(&src).err().expect("expected a gate error");
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{expr}: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn gate_settings_can_follow_controls_without_name_collisions() {
    let src = r#"
        tempo 120bpm
        ctl width = 60%
        ctl attack = 5ms
        ctl release = 10ms
        instr pad() {
            out = 0.5.gate(grid[xxxx].over(1beat), width: width, attack: attack, release: release) * env[1 | 5ms 0]
        }
        pad() for 2sec
    "#;
    let p = program(src);
    let out = mono(
        render_with(
            &p,
            &[],
            &RenderOptions {
                limiter: false,
                until: Some(1.0),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    level(&out, 0.0025, 0.25);
    level(&out, 0.070, 0.25);
    level(&out, 0.100, 0.0);
}

#[test]
fn an_fx_generator_reopens_after_a_long_silent_run() {
    let p = program(
        "tempo 120bpm\nfx chopped { out = 0.5.gate(grid[.xx.].over(1beat), attack: 5ms, release: 10ms) }",
    );
    let schedule = p.schedule();
    let mut session =
        niminal_live::Offline::new(SR as f32, niminal_lang::Layout::Mono).without_limiter();
    session.play_program(p, schedule).unwrap();
    let out = session.process(SR as usize).remove(0);
    for time in [0.150, 0.250, 0.650, 0.750] {
        level(&out, time, 0.5);
    }
    for time in [0.050, 0.450, 0.550, 0.950] {
        level(&out, time, 0.0);
    }
}
