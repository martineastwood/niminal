use niminal_engine::{Buses, Voice, block_size};
use niminal_lang::compile;

fn voice(source: &str) -> Voice {
    let program = compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("composition", source))
                .collect::<String>()
        )
    });
    Voice::new(program.instruments[0].graph.clone(), 48_000.0)
}

fn render(source: &str, frames: usize, block: usize) -> Vec<f32> {
    let mut voice = voice(source);
    let mut buses = Buses::new(&[]);
    let mut result = Vec::new();
    for start in (0..frames).step_by(block) {
        let n = block.min(frames - start);
        let mut out = vec![vec![0.0; block_size()]; 1];
        voice.process_blocks(&mut out, n, &mut buses);
        result.extend_from_slice(&out[0][..n]);
    }
    result
}

#[test]
fn callbacks_capture_callers_values_and_can_be_forwarded() {
    let source = "fn apply(x, make: fn) { make(x) }\nfn forward(x, make: fn) { apply(x, make) }\ninstr a(x: num = 3) { factor = x * 2\nforward(x, fn(v) { v * factor }) }";
    assert!(render(source, 70, 13).iter().all(|v| *v == 18.0));
    let source =
        "fn twice(x) { x * 2 }\nfn apply(x, make: fn = twice) { make(x) }\ninstr a() { apply(3) }";
    assert!(render(source, 70, 32).iter().all(|v| *v == 6.0));
}

#[test]
fn callbacks_have_independent_state_in_graphs_and_kernels() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "fn apply(x, make: fn) {{ {state}make(x) + make(x * 2) }}\ninstr a(x: num = 1) {{ apply(x, fn(v) {{ state n = 0\nn = n + v\nn }}) }}"
        );
        let out = render(&source, 70, 13);
        for (i, v) in out.iter().enumerate() {
            assert_eq!(*v, (i + 1) as f32 * 3.0, "sample {i}");
        }
    }
}

#[test]
fn kernel_callbacks_capture_live_signals_and_preserve_units_and_arrays() {
    let source = "fn apply(x, make: fn) { state unused = 0\nmake(x) }\ninstr a(freq: hz = 100hz) { ratio = 3\napply([freq, freq * 2], fn(v) { v * ratio }).sum.num }";
    assert!(render(source, 70, 13).iter().all(|v| *v == 900.0));
}

#[test]
fn resets_restart_nested_state_and_leave_external_signals_running() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "fn counter() {{ state n = 0\nn = n + 1\nn }}\nfn f(t) {{ {state}outside = counter()\ninside = reset_on(t) {{ counter() }}\ninside - outside }}\ninstr a() {{ f((note.time.num * sample_rate.num + 0.5).floor.mod(5) == 2) }}"
        );
        let out = render(&source, 70, 13);
        for (i, v) in out.iter().enumerate() {
            let reset = if i < 2 { 0 } else { ((i - 2) / 5) * 5 + 2 };
            assert_eq!(*v, -(reset as f32), "sample {i}");
        }
        assert_eq!(out, render(&source, 70, 32));
    }
}

#[test]
fn reset_scope_can_declare_state_and_resets_buffers_and_prev() {
    let source = "instr a() { t = (note.time.num * sample_rate.num + 0.5).floor.mod(5) == 2\nreset_on(t) { state buf = [0, 0]\nstate n = 0\nn = n + 1\nbuf[0] = n\nbuf[0] + prev(n) } }";
    let out = render(source, 12, 7);
    assert_eq!(
        out,
        [1.0, 3.0, 1.0, 3.0, 5.0, 7.0, 9.0, 1.0, 3.0, 5.0, 7.0, 9.0]
    );
}

#[test]
fn graph_reset_retriggers_builtin_oscillators_and_envelopes() {
    let source = "instr a() { reset_on((note.time.num * sample_rate.num + 0.5).floor.mod(8) == 3) { osc(saw, 100hz) * env[1 1sec 0] } }";
    let out = render(source, 50, 13);
    assert_eq!(out[0], out[3]);
    assert_eq!(out[3..11], out[11..19]);
    assert_eq!(out, render(source, 50, 32));
}

#[test]
fn sustained_positive_trigger_is_one_reset_and_new_edges_reset_again() {
    let source = "instr a() { reset_on((note.time.num * sample_rate.num + 0.5).floor.mod(8) < 4) { state n = 0\nn = n + 1\nn } }";
    let out = render(source, 20, 7);
    assert_eq!(&out[..8], &[1., 2., 3., 4., 5., 6., 7., 8.]);
    assert_eq!(&out[8..16], &[1., 2., 3., 4., 5., 6., 7., 8.]);
}

#[test]
fn completion_waits_for_release_and_every_layer() {
    let source =
        "instr a() { out = 1.finish_when(note.time > 1ms)\nout += 2.finish_when(note.time > 2ms) }";
    let mut v = voice(source);
    let mut buses = Buses::new(&[]);
    let mut out = vec![vec![0.0; block_size()]; 1];
    v.process_blocks(&mut out, 32, &mut buses);
    assert!(!v.is_finished());
    v.release();
    v.process_blocks(&mut out, 32, &mut buses);
    assert!(!v.is_finished());
    assert_eq!(out[0][31], 2.0);
    for _ in 0..3 {
        v.process_blocks(&mut out, 32, &mut buses);
    }
    assert!(v.is_finished());
    assert!(out[0].iter().all(|x| *x == 0.0));
}

#[test]
fn completion_in_custom_functions_controls_voice_lifetime() {
    let source = "fn custom(t) { state unused = 0\n1.finish_when(t > 1ms) }\ninstr a() { custom(note.time) }";
    let mut v = voice(source);
    v.release();
    let mut buses = Buses::new(&[]);
    let mut out = vec![vec![0.0; block_size()]; 1];
    v.process_blocks(&mut out, 32, &mut buses);
    assert!(!v.is_finished());
    v.process_blocks(&mut out, 32, &mut buses);
    assert!(v.is_finished());
}

#[test]
fn callback_arguments_are_checked_at_the_call_site() {
    for source in [
        "fn apply(x, make: fn) { make(x) }\ninstr a() { apply(1, 2) }",
        "fn apply(x, make: fn) { make(x) }\ninstr a() { apply(1, fn(x,y) { x+y }) }",
        "fn apply(x, make: fn) { make(x) }\ninstr a() { apply(1, fn(x) { x + 1hz }) }",
        "instr a(make: fn) { 0 }",
    ] {
        assert!(compile(source).is_err(), "{source}");
    }
}

#[test]
fn reset_reinitializes_from_current_inputs() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "fn f(x, t) {{ {state}reset_on(t) {{ state n = x\nn = n + 1\nn }} }}\ninstr a() {{ x = (note.time.num * sample_rate.num + 0.5).floor\nf(x, x.mod(5) == 2) }}"
        );
        assert_eq!(
            render(&source, 12, 7),
            [1., 2., 3., 4., 5., 6., 7., 8., 9., 10., 11., 12.]
        );
    }
}

#[test]
fn nested_reset_scopes_restart_their_own_subgraphs() {
    let source = "fn count() { state n = 0\nn = n + 1\nn }\nfn f(t) { state unused = 0\nreset_on(t.mod(10) == 7) { out = count()\nout += reset_on(t.mod(4) == 2) { count() } } }\ninstr a() { f((note.time.num * sample_rate.num + 0.5).floor) }";
    let out = render(source, 24, 7);
    let mut outer = 0;
    let mut inner = 0;
    let mut last_inner_trigger = false;
    for (i, &v) in out.iter().enumerate() {
        if i % 10 == 7 {
            outer = 0;
            inner = 0;
            last_inner_trigger = false;
        }
        let trigger = i % 4 == 2;
        if trigger && !last_inner_trigger {
            inner = 0;
        }
        last_inner_trigger = trigger;
        outer += 1;
        inner += 1;
        assert_eq!(v, (outer + inner) as f32, "sample {i}");
    }
    assert_eq!(out, render(source, 24, 32));
}

#[test]
fn completed_layers_keep_downstream_delay_tails_alive() {
    let source = "instr a() { (1.finish_when(note.time > 1ms)).delay(1ms, feedback: 0.5) }";
    let mut v = voice(source);
    v.release();
    let mut buses = Buses::new(&[]);
    let mut out = vec![vec![0.0; block_size()]; 1];
    for _ in 0..4 {
        v.process_blocks(&mut out, 32, &mut buses);
    }
    assert!(!v.is_finished());
    for _ in 0..40 {
        v.process_blocks(&mut out, 32, &mut buses);
    }
    assert!(v.is_finished());
}

#[test]
fn completion_latches_until_its_scope_is_reset() {
    let source = "instr a() { x = (note.time.num * sample_rate.num + 0.5).floor\nreset_on(x.mod(8) == 5) { 1.finish_when(x.mod(8) == 2) } }";
    assert_eq!(
        render(source, 16, 7),
        [
            1., 1., 0., 0., 0., 1., 1., 1., 1., 1., 0., 0., 0., 1., 1., 1.
        ]
    );
}

#[test]
fn reset_helpers_can_wrap_builtin_envelopes_and_read_instrument_note_context() {
    let source = "fn retrigger(t) { reset_on(t) { env[1 1sec 0] } }\ninstr a() { t = (note.time.num * sample_rate.num + 0.5).floor.mod(8) == 3\nretrigger(t) + reset_on(t) { note.released } }";
    let out = render(source, 24, 7);
    assert_eq!(out[0], out[3]);
    assert_eq!(out[3..11], out[11..19]);
}

#[test]
fn stateful_native_callbacks_freeze_on_untaken_branches() {
    let source = "fn f(t, make: fn) { state unused = 0\nif t < 4 { make() } else { 0 } }\ninstr a() { f((note.time.num * sample_rate.num + 0.5).floor.mod(8), fn() { osc(saw, 1khz) }) }";
    let out = render(source, 16, 7);
    assert_eq!(&out[4..8], &[0.; 4]);
    assert!((out[8] - (-1.0 + 2.0 * 4.0 / 48.0)).abs() < 1e-6);
}

#[test]
fn named_functions_and_callback_parameters_work_with_map() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "fn twice(x) {{ x * 2 }}\nfn bank(xs: [num], make: fn) {{ {state}xs.map(make).sum }}\ninstr a() {{ bank([1,2,3], twice) }}"
        );
        assert!(render(&source, 65, 13).iter().all(|v| *v == 12.0));
    }
}

#[test]
fn named_callback_results_remain_accessible_inline() {
    for state in ["", "state unused = 0\n"] {
        let source = format!(
            "fn split(x) -> (low, high) {{ low = x\nhigh = x * 2 }}\nfn apply(x, make: fn) {{ {state}make(x).high }}\ninstr a() {{ apply(3, split) }}"
        );
        assert!(render(&source, 65, 13).iter().all(|v| *v == 6.0));
    }
}

#[test]
fn explicit_tail_can_end_effects_inside_a_custom_fx_after_completion() {
    let source = "fn custom(t) { state unused = 0\n1.finish_when(t > 1ms).delay(10ms, feedback: 0.995) }\ninstr a() { tail = 2ms\n custom(note.time) }";
    let mut v = voice(source);
    v.release();
    let mut buses = Buses::new(&[]);
    let mut out = vec![vec![0.0; block_size()]; 1];
    for _ in 0..32 {
        v.process_blocks(&mut out, 32, &mut buses);
    }
    assert!(v.is_finished());
}
