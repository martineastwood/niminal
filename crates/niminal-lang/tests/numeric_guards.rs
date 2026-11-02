//! Maths with no real answer gives 0, never NaN, whichever way the program is compiled.

use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

fn render(body: &str) -> (Vec<f32>, bool) {
    let program = compile(&format!("instr a() {{ {body} }}")).expect("compiles");
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut out = vec![0.0; 8 * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    (out, voice.is_poisoned())
}

#[test]
fn nothing_without_a_real_answer_makes_nan() {
    // a signal (graph opcode), a constant (folded while compiling) and inside a stateful function (kernel)
    for body in [
        "pow(osc(sine, 220hz) - 2, 0.5)",
        "pow(0, -1) + osc(sine, 220hz)",
        "pow(0 - 2, 0.5) + osc(sine, 220hz)",
        "osc(sine, 220hz) / 0 + sqrt(osc(sine, 220hz) - 2) + log(osc(sine, 220hz) * 0)",
    ] {
        let (out, poisoned) = render(body);
        assert!(!poisoned, "{body} poisoned the voice");
        assert!(out.iter().all(|v| v.is_finite()), "{body}");
    }
}

#[test]
fn kernels_follow_the_same_rule() {
    let source = "fn bad(x) {\n  state s = 0\n  s = s + 0.01\n  pow(x - 2, 0.5) + s\n}\ninstr a() { osc(sine, 220hz).bad }";
    let program = compile(source).expect("compiles");
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48_000.0);
    let mut out = vec![0.0; 4 * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    assert!(!voice.is_poisoned());
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn ordinary_powers_are_unchanged() {
    let (out, _) = render("pow(2, 3) * 0.1 + pow(0.5, 2) * 0.1");
    assert!(
        (out[block_size() * 4] - 0.825).abs() < 1e-6,
        "{}",
        out[block_size() * 4]
    );
}
