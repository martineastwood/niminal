//! A line in an instrument's body that sets a number nobody reads is a setting spelt wrong.
#![allow(clippy::unwrap_used)]

use niminal_lang::compile;

fn error(source: &str) -> String {
    let errors = compile(source).err().expect("should not compile");
    let e = &errors[0];
    format!("{} {}", e.message, e.help.clone().unwrap_or_default())
}

#[test]
fn a_misspelt_setting_is_reported_with_the_one_meant() {
    let e = error("instr pad(freq: hz = 220hz) {\n  maxx = 4\n  osc(saw, freq)\n}");
    assert!(e.contains("`maxx` is set but never used"), "{e}");
    assert!(e.contains("did you mean the setting `max`"), "{e}");
}

#[test]
fn a_name_far_from_every_setting_lists_them() {
    let e = error("instr pad(freq: hz = 220hz) {\n  sustain_time = 2sec\n  osc(saw, freq)\n}");
    assert!(e.contains("`sustain_time` is set but never used"), "{e}");
    assert!(e.contains("`tail`"), "{e}");
}

#[test]
fn constants_that_are_read_still_compile() {
    let source = "instr pad(freq: hz = 220hz) {\n  mix = 0.5\n  depth = -3\n  tail = 1sec\n  osc(saw, freq) * mix + depth * 0\n}";
    assert!(compile(source).is_ok());
    // read only inside a nested block
    let source = "instr pad(freq: hz = 220hz) {\n  amount = 0.5\n  x = if freq > 100hz { amount } else { 0 }\n  osc(saw, freq) * x\n}";
    assert!(compile(source).is_ok());
}

#[test]
fn a_constant_result_is_not_an_unused_setting() {
    assert!(compile("instr a() {\n  out = 0.5\n}").is_ok());
}
