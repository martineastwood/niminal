use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, ..Default::default() }).unwrap())
}

#[test]
fn prev_is_the_value_one_sample_ago_so_feedback_is_a_loop_in_one_line() {
    // y = x + y/2 from a steady 1 settles at 2
    let out = raw("fn loop(x, g: 0..1 = 0.5) {\n y = x + prev(y) * g\n y\n}\ninstr a() { env[1 | 10ms 0].loop(g: 0.5) * 0.25 }\na() for 1beat");
    let at = (0.2 * SR) as usize;
    assert!((out[at] - 0.5).abs() < 1e-4, "{}", out[at]);
    assert!(out[2].abs() < 0.5 && out[2] > 0.25, "it builds up over the first samples: {}", out[2]);
}

#[test]
fn prev_of_an_expression_delays_it_a_sample() {
    // out(n) = x(n-1): a ramp from env, delayed one sample, is one step behind
    let direct = raw("fn same(x) { x }\ninstr a() { env[0 100ms 1 | 10ms 0].same() * 0.5 }\na() for 1beat");
    let delayed = raw("fn one_behind(x) { prev(x) }\ninstr a() { env[0 100ms 1 | 10ms 0].one_behind() * 0.5 }\na() for 1beat");
    for i in 100..400 {
        assert!((delayed[i] - direct[i - 1]).abs() < 1e-6, "sample {i}");
    }
}

#[test]
fn prev_of_a_name_that_is_never_defined_is_an_error() {
    let errors = compile("fn bad(x) {\n x + prev(nothing)\n}").err().unwrap();
    assert!(errors[0].message.contains("never defined"), "{}", errors[0].message);
}
