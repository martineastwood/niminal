//! Conditional sample work preserves state order, including when branches change every sample.
#![allow(clippy::unwrap_used)]
use niminal_engine::{Buses, Voice, block_size};

fn run(source: &str, mut expected: impl FnMut(usize, f32, f32) -> f32) {
    let p = niminal_lang::compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("optimizer", source))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(p.instruments[0].graph.clone(), 48000.0);
    let mut buses = Buses::new(&[]);
    let mut out = vec![vec![0.0; block_size()]];
    for i in 0..1000 {
        let t = if i % 7 < 3 { 1.0 } else { 0.0 };
        let x = i as f32 / 100.0 - 3.0;
        voice.set_param_by_name("t", t);
        voice.set_param_by_name("x", x);
        voice.process_blocks(&mut out, 1, &mut buses);
        let want = expected(i, t, x);
        assert!(
            (out[0][0] - want).abs() < 1e-5,
            "sample {i}: {} != {want}",
            out[0][0]
        );
    }
}

#[test]
fn branch_math_and_shared_expressions_follow_sample_rate_controls() {
    run(
        "fn f(t, x) { state unused = 0\n if t > 0 { sin(x.max(0)) + sin(x.max(0)) } else { cos(x) } }\ninstr a(t: num, x: num) { f(t, x) }",
        |_, t, x| {
            if t > 0.0 {
                (f64::from(x).max(0.0).sin() * 2.0) as f32
            } else {
                f64::from(x).cos() as f32
            }
        },
    );
}

#[test]
fn branches_preserve_scalar_state_and_ordered_buffer_updates() {
    let mut total = 0.0;
    run(
        "fn f(t) { state n = 0\n state b = [0]\n if t > 0 { n = n + 1\n b[0] = b[0] + 10\n n + b[0] } else { n = n + 2\n b[0] = b[0] + 20\n n + b[0] } }\ninstr a(t: num, x: num) { f(t) }",
        |_, t, _| {
            total += if t > 0.0 { 11.0 } else { 22.0 };
            total
        },
    );
}

#[test]
fn repeated_expressions_do_not_share_stateful_calls() {
    let mut total = 0.0;
    run(
        "fn count(x) { state n = 0\n n = n + x\n n }\nfn f(t) { state unused = 0\n if t > 0 { count(1) + count(2) } else { 0 } }\ninstr a(t: num, x: num) { f(t) }",
        |_, t, _| {
            if t > 0.0 {
                total += 3.0;
                total
            } else {
                0.0
            }
        },
    );
}
