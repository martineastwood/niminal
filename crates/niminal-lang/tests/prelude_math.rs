//! The math helpers every file can call without defining them, checked against the numbers they should give.
use niminal_engine::{Buses, Voice, block_size};
use niminal_lang::compile;

fn value(expr: &str) -> f32 {
    let source = format!("instr a() {{ {expr} }}");
    let p = compile(&source).unwrap_or_else(|errors| {
        panic!(
            "{expr}: {}",
            errors
                .iter()
                .map(|e| e.render("prelude_math", &source))
                .collect::<String>()
        )
    });
    let mut voice = Voice::new(p.instruments[0].graph.clone(), 48_000.0);
    let mut out = vec![vec![0.0; block_size()]; 1];
    voice.process_blocks(&mut out, block_size(), &mut Buses::new(&[]));
    out[0][block_size() - 1]
}

#[track_caller]
fn is(expr: &str, expected: f32) {
    let got = value(expr);
    assert!(
        (got - expected).abs() < 1e-4,
        "{expr}: expected {expected}, got {got}"
    );
}

#[test]
fn interpolation_and_ranges() {
    is("lerp(2, 6, 0.25)", 3.0);
    is("lerp(2, 6, 1.5)", 8.0);
    is("smoothstep(0.5)", 0.5);
    is("smoothstep(-1)", 0.0);
    is("smoothstep(2)", 1.0);
    is("bipolar(0.75)", 0.5);
    is("unipolar(bipolar(0.3))", 0.3);
    is("floor_to(0.37, 0.25)", 0.25);
}

#[test]
fn logarithms_and_inverse_trigonometry() {
    is("log2(8)", 3.0);
    is("log10(1000)", 3.0);
    is("atan(1)", std::f32::consts::FRAC_PI_4);
    is("asin(0.5)", std::f32::consts::FRAC_PI_6);
    is("acos(0.5)", std::f32::consts::FRAC_PI_3);
    is("acos(-1)", std::f32::consts::PI);
}

#[test]
fn midi_notes_and_frequencies() {
    is("mtof(69).num", 440.0);
    is("mtof(81).num", 880.0);
    is("ftom(440hz)", 69.0);
    is("ftom(mtof(60))", 60.0);
}

#[test]
fn a_whole_number_from_a_random_range_compiles() {
    assert!(value("random_between(0, 10).floor") >= 0.0);
    assert!(value("mtof(60 + random_between(0, 12).floor).num") >= 261.0);
}

#[test]
fn a_file_can_still_define_its_own_version() {
    let source = "fn mtof(n: num) -> hz { 1hz }\ninstr a() { mtof(60).num }";
    assert!(compile(source).is_ok());
}
