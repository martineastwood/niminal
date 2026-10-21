//! Tests of compiling a whole composition, definitions and all.

use crate::ast::*;
use crate::compile::*;
use niminal_lang::{Diagnostic, Layout, Param, Unit};
use niminal_score::{Event, Time, Value};

fn ok(src: &str) -> Composition {
    compile(src).unwrap_or_else(|errs| {
        panic!(
            "{}",
            errs.iter().map(|d| d.render("t", src)).collect::<String>()
        );
    })
}

fn errors(src: &str) -> Vec<Diagnostic> {
    compile(src).err().expect("expected compile errors")
}

fn first_error(src: &str) -> Diagnostic {
    errors(src).remove(0)
}

fn msg(src: &str) -> String {
    first_error(src).message
}

fn help(src: &str) -> Option<String> {
    first_error(src).help
}

#[test]
fn controls_can_be_driven_from_midi_and_osc() {
    let p = ok(
        "ctl cutoff = midi.cc(21).range(200hz..4khz).smooth(10ms)\nctl width = osc_in(\"/fx/width\").range(0..1)\nctl vol = midi.cc(7, 2).range(-60db..0db)",
    );
    let (cutoff, width, vol) = (&p.controls[0], &p.controls[1], &p.controls[2]);
    let binding = p.input("cutoff").unwrap();
    assert_eq!(
        binding.source,
        InputSource::MidiCc {
            cc: 21,
            channel: None
        }
    );
    assert_eq!(cutoff.smooth_seconds, 0.01);
    assert!((binding.map_input(cutoff, 0.0) - 200.0).abs() < 1e-3);
    assert!((binding.map_input(cutoff, 1.0) - 4000.0).abs() < 1e-2);
    // hz sweeps are geometric: halfway is the geometric mean
    assert!((binding.map_input(cutoff, 0.5) - (200.0f64 * 4000.0).sqrt()).abs() < 0.1);
    assert_eq!(
        p.input("width").unwrap().source,
        InputSource::Osc("/fx/width".into())
    );
    let learning = ok("ctl drive = midi.learn.range(0..1)");
    assert_eq!(
        learning.input("drive").unwrap().source,
        InputSource::MidiLearn
    );
    assert!((p.input("width").unwrap().map_input(width, 0.25) - 0.25).abs() < 1e-6);
    // db sweeps are even in decibels, and come back in db
    assert!((p.input("vol").unwrap().map_input(vol, 0.5) + 30.0).abs() < 1e-3);
    assert_eq!(
        p.input("vol").unwrap().source,
        InputSource::MidiCc {
            cc: 7,
            channel: Some(2)
        }
    );
}

#[test]
fn bad_control_inputs_are_explained() {
    assert_eq!(
        msg("ctl a = midi.cc(200)"),
        "expected a CC number from 0 to 127, found a number"
    );
    assert_eq!(
        msg("ctl a = midi.note(1)"),
        "`midi.note` isn't supported; use `midi.cc(number)` or `midi.learn`"
    );
    assert_eq!(
        msg("ctl a = midi.cc(1).range(200hz..1)"),
        "the range's ends need the same unit: hz and a plain number"
    );
    assert_eq!(
        msg("ctl a = midi.cc(1).range(1..0)"),
        "the upper end of a range must be above the lower end"
    );
    assert_eq!(
        msg("ctl a = osc_in(\"fx\")"),
        "an OSC address starts with `/`"
    );
}

const SAW_LEAD: &str = "\ninstr saw_lead(freq: hz, amp: db = -6db) {\n  level = env[0 5ms 1 200ms 0.6 | 300ms 0]\n  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level\n}\nsaw_lead(freq: a3) for 1beat\n";

#[test]
fn compiles_the_basic_synth() {
    let p = ok(SAW_LEAD);
    let i = p.instrument("saw_lead").unwrap();
    assert_eq!(i.params.len(), 3, "freq, amp and the implicit gain");
    assert_eq!(
        i.params[0],
        Param {
            name: "freq".into(),
            unit: Unit::Hz,
            range: None,
            default: None
        }
    );
    let amp = i.params[1].default.unwrap();
    assert!(
        (amp - 0.501_187).abs() < 1e-5,
        "-6db is stored as a gain factor: {amp}"
    );

    assert_eq!(p.notes.len(), 1);
    assert_eq!(p.notes[0].dur, Time::Beats(1.0));
    assert_eq!(p.notes[0].args["freq"], Value::Note(57));
}

#[test]
fn arithmetic_with_units() {
    ok(
        "instr a(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq * 2).lpf(cutoff: freq * (2 + bright * 6)) }",
    );
    ok("instr a(freq: hz, amp: db = -6db) { osc(sine, freq).gain(amp - 6db).gain(0.5) }");
    ok("instr a(freq: hz) { osc(sine, freq + 3hz - 1hz) }");
    ok("instr a(freq: hz) { osc(sine, freq / 2) }");
    ok("instr a() { 0.5 }");
}

#[test]
fn tempo_converts_beats_inside_instruments() {
    let src = "instr a() { osc(sine, 100hz) * env[0 1beat 1] }\na() for 1beat";
    assert_eq!(ok(&format!("tempo 60bpm\n{src}")).tempo.bpm, 60.0);
    assert_eq!(ok(src).tempo.bpm, 120.0);
    assert_eq!(
        msg("tempo 120bpm\ntempo 90bpm"),
        "tempo is set more than once"
    );
    assert_eq!(msg("tempo 120"), "tempo must be a positive number of bpm");
    assert_eq!(
        msg("instr a() { osc(sine, 100bpm) }"),
        "`bpm` can only be used with `tempo`"
    );
}

#[test]
fn bare_literal_in_a_unit_slot_gets_a_quick_fix() {
    let src = "instr a(f: hz) { osc(saw, f).lpf(cutoff: 800) }";
    let d = first_error(src);
    assert_eq!(d.message, "`cutoff` expects hz, found a plain number");
    assert_eq!(d.help.as_deref(), Some("did you mean `800hz`?"));
    assert_eq!(&src[d.span.start..d.span.end], "800");

    let d = first_error("instr a() { osc(saw, 440) }");
    assert_eq!(d.help.as_deref(), Some("did you mean `440hz`?"));
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: sine) }"),
        "`sine` is a waveform, not a value"
    );
}

#[test]
fn wrong_units_are_errors() {
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: 6db) }"),
        "`cutoff` expects hz, found db"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).gain(440hz) }"),
        "`gain` expects db or a plain factor, found hz"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f + 6db) }"),
        "can't add hz and db"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f * f) }"),
        "can't multiply hz and hz"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f + 1) }"),
        "can't add hz and a plain number"
    );
    assert_eq!(
        msg("instr a(f: hz) { f }"),
        "an instrument outputs a plain signal, but this is hz"
    );
    assert_eq!(
        msg("instr a(f: hz = 6db) { osc(saw, f) }"),
        "`f` expects hz, found db"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).gain(f * 2db) }"),
        "can't multiply hz and db"
    );
    // dividing by zero gives 0, as it does while playing
    ok("instr a(f: hz) { osc(saw, f / 0) }");
}

#[test]
fn name_errors_suggest_fixes() {
    let d = first_error("instr a(freq: hz) { osc(saw, frq) }");
    assert_eq!(d.message, "`frq` is not defined");
    assert_eq!(d.help.as_deref(), Some("did you mean `freq`?"));
    assert_eq!(
        help("instr a(f: hz) { osc(saw, f).lpf(cuttoff: 1khz) }").as_deref(),
        Some("did you mean `cutoff`?")
    );
    assert_eq!(
        help("instr a(f: hz) { osc(saw, f).lfp(cutoff: 1khz) }").as_deref(),
        Some("did you mean `lpf`?")
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).wobble() }"),
        "unknown function `wobble`"
    );
    assert_eq!(
        msg("instr a(f: hz) { saw }"),
        "`saw` is a waveform, not a value"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(wobble, f) }"),
        "expected a waveform or a table"
    );
    assert_eq!(
        msg("instr a(f: hertz) { osc(saw, f) }"),
        "unknown type `hertz`"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, 440herz) }"),
        "unknown unit `herz`"
    );
}

#[test]
fn argument_binding_rules() {
    // leading required parameters may be given in order; the rest are named
    ok("instr a(f: hz) { osc(saw, f).lpf(2khz) }");
    let d = first_error("instr a(f: hz) { osc(saw, f).lpf(2khz, 0.3) }");
    assert_eq!(d.message, "`lpf` takes its other arguments by name");
    assert_eq!(d.help.as_deref(), Some("write `res: ...`"));
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).lpf(res: 0.1) }"),
        "`lpf` needs an argument `cutoff`"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw) }"),
        "`osc` needs an argument `freq`"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f, f) }"),
        "`osc` takes its other arguments by name"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, freq: f, freq: f) }"),
        "`osc` argument `freq` given more than once"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f, freq: f) }"),
        "`osc` argument `freq` given more than once"
    );
    // the receiver fills the first parameter, so naming it as well clashes
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).lpf(x: f, cutoff: 1hz) }"),
        "`lpf` argument `x` given more than once"
    );
}

#[test]
fn instrument_shape_errors() {
    assert_eq!(msg("instr a() { }"), "instrument `a` is empty");
    assert_eq!(
        msg("instr a() { x = 0.5 }"),
        "an instrument must end with the signal it outputs"
    );
    assert_eq!(msg("instr a() { 0.5\n0.5 }"), "this value is never used");
    assert_eq!(
        msg("instr a(f: hz, f: hz) { 0.5 }"),
        "parameter `f` is declared twice"
    );
    assert_eq!(
        msg("instr a() { 0.5 }\ninstr a() { 0.5 }"),
        "`a` is defined twice"
    );
    assert_eq!(
        msg("instr a(b: 0..1 = 2) { b }"),
        "the default for `b` is outside 0..1"
    );
    assert_eq!(
        msg("instr a(b: 1..0) { b }"),
        "the lower end of a range must be below the upper end"
    );
}

#[test]
fn envelope_unit_rules() {
    ok("instr a() { osc(sine, env[200hz 10ms 6khz 300ms.exp 900hz]) }");
    assert!(
        msg("instr a() { osc(sine, env[0 10ms 6khz]) }")
            .starts_with("envelope levels must all have the same unit")
    );
    assert_eq!(
        msg("instr a() { env[-6db 10ms 0db] }"),
        "db levels in envelopes aren't supported yet"
    );
}

#[test]
fn note_checking() {
    let base = "instr lead(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq) }\n";
    let note = |n: &str| format!("{base}{n}");

    let p = ok(&note("at 2beats lead(freq: c4, bright: 0.2) for 250ms"));
    assert_eq!(p.notes[0].at, Time::Beats(2.0));
    assert_eq!(p.notes[0].dur, Time::Seconds(0.25));

    let d = first_error(&note("lead(freq: 440) for 1beat"));
    assert_eq!(
        d.message,
        "argument `freq`: expected a frequency (hz or a note name), got `440`"
    );
    assert_eq!(d.help.as_deref(), Some("did you mean `440hz`?"));

    assert_eq!(
        msg(&note("lead() for 1beat")),
        "`lead` needs an argument `freq`"
    );
    assert_eq!(
        msg(&note("lead(freq: c4, bright: 1.5) for 1beat")),
        "argument `bright` must be between 0 and 1, got 1.5"
    );
    assert_eq!(
        msg(&note("lead(freq: c4, brigt: 1) for 1beat")),
        "`lead` has no parameter `brigt`"
    );
    assert_eq!(
        help(&note("lead(freq: c4, brigt: 1) for 1beat")).as_deref(),
        Some("did you mean `bright`?")
    );
    assert_eq!(
        msg(&note("lede(freq: c4) for 1beat")),
        "no instrument or clip named `lede`"
    );
    assert_eq!(
        msg(&note("lead(c4) for 1beat")),
        "name the arguments of a note"
    );
    assert_eq!(msg(&note("lead(freq: c4) for 440hz")), "expected a time");
    assert_eq!(
        msg(&note("lead(freq: c4, freq: c5) for 1beat")),
        "argument `freq` given more than once"
    );
    // Constant arithmetic folds: c4 + 1st is midi 61 (c#4).
    let transposed = ok(&note("lead(freq: c4 + 1st) for 1beat"));
    assert_eq!(transposed.notes[0].args.get("freq"), Some(&Value::Note(61)));
    assert_eq!(
        msg(&note("lead(freq: c4 + bright) for 1beat")),
        "note arguments must be literal values"
    );
}

const SPEC_OPCODES: &str = "\nfn one_pole(x, cutoff: hz) {\n  state y = 0.0\n  a = exp(-2 * pi * cutoff / sample_rate)\n  y = x * (1 - a) + y * a\n  y\n}\n\nfn drive(x, amount: 0..1 = 0.5) {\n  (x * (1 + amount * 9)).tanh\n}\n\nfn my_dc_block(x) {\n  state x1 = 0.0\n  state y1 = 0.0\n  y = x - x1 + 0.995 * y1\n  x1 = x\n  y1 = y\n  y\n}\n\nfn fold(x, amount: 0..1 = 0.5) {\n  (x * (1 + amount * 8)).sin\n}\n";

#[test]
fn the_specs_example_functions_compile_and_can_be_used() {
    ok(&format!(
        "{SPEC_OPCODES}\ninstr a(freq: hz) {{ osc(saw, freq).drive(amount: 0.3).one_pole(cutoff: 800hz) }}\ninstr b(freq: hz) {{ osc(sine, freq).fold(amount: env[0 2sec 1 2sec 0]).my_dc_block }}\ninstr c(freq: hz) {{ drive(osc(saw, freq)) }}\na(freq: 110hz) for 1beat"
    ));
}

#[test]
fn function_arguments_are_checked_like_any_other() {
    let with = |call: &str| format!("{SPEC_OPCODES}\ninstr a(freq: hz) {{ {call} }}");
    assert_eq!(
        msg(&with("osc(saw, freq).one_pole(cutoff: 800)")),
        "`cutoff` expects hz, found a plain number"
    );
    assert_eq!(
        help(&with("osc(saw, freq).one_pole(cutoff: 800)")).as_deref(),
        Some("did you mean `800hz`?")
    );
    assert_eq!(
        msg(&with("osc(saw, freq).one_pole()")),
        "`one_pole` needs an argument `cutoff`"
    );
    assert_eq!(
        msg(&with("osc(saw, freq).drive(amont: 0.3)")),
        "`drive` has no argument named `amont`"
    );
    assert_eq!(
        msg(&with("osc(saw, freq).drive(0.3)")),
        "`drive` takes its other arguments by name"
    );
    ok(&with("osc(saw, freq).drive"));
    ok(&with("osc(saw, freq).one_pole(800hz)"));
}

#[test]
fn function_body_errors() {
    let body = |b: &str| format!("fn f(x: num, amount: 0..1 = 0.5, cutoff: hz = 100hz) {{ {b} }}");
    let state = |b: &str| body(&format!("state s = 0.0\n{b}"));
    assert_eq!(
        msg(&body("x * undefined_thing")),
        "`undefined_thing` is not defined"
    );
    assert_eq!(
        msg(&body("exp(cutoff)")),
        "`exp` expects a plain number, found hz"
    );
    assert_eq!(msg(&body("x + cutoff")), "can't add a plain number and hz");
    // a function can give any unit
    ok(&body("cutoff"));
    ok(&state("x.lpf(cutoff: 100hz) + s"));
    assert_eq!(
        msg(&state("x.pan(azimuth: 10deg) + s")),
        "`pan` can't be used inside a function with state"
    );
    assert_eq!(msg(&state("x.exxp + s")), "unknown function `exxp`");
    assert_eq!(
        help(&state("x.exxp + s")).as_deref(),
        Some("did you mean `exp`?")
    );
    assert_eq!(msg(&state("exp(x, x) + s")), "`exp` takes one value");
    assert_eq!(
        msg(&state("env[0 1sec 1] + s")),
        "envelopes can't be used inside a function with state"
    );
    assert_eq!(msg(&body("")), "`f` must end with the value it gives");
    // a state starts at a value known at note start, here the input's first sample
    ok(&body("state s = x\ns"));
    assert_eq!(
        msg(&state("s = cutoff\ns")),
        "`s` holds a plain number, but this is hz"
    );
    assert_eq!(msg(&body("state x = 0.0\nx")), "`x` is already defined");
    assert_eq!(msg(&state("y = 1")), "`f` must end with the value it gives");
    assert_eq!(msg(&body("x\nx")), "this value is never used");
    ok("fn f(x) -> hz { x * 2 }\ninstr a(freq: hz) { osc(sine, f(freq * 0.5)) }");
    assert_eq!(
        msg("fn f(x) -> hz { x * 2 }\ninstr a(freq: hz) { osc(sine, freq * f(0.5)) }"),
        "`f` gives a plain number, but its result is declared as hz"
    );
}

#[test]
fn function_signature_errors() {
    assert_eq!(
        msg("fn lpf(x) { x }"),
        "`lpf` is a built-in function, so it can't be defined again"
    );
    assert_eq!(msg("fn f(x) { x }\nfn f(x) { x }"), "`f` is defined twice");
    assert_eq!(msg("fn f(x, x) { x }"), "parameter `x` is declared twice");
    assert_eq!(
        msg("fn f(x: num, a: 0..1 = 3) { x }\ninstr i(freq: hz) { osc(saw, freq).f }"),
        "`a` is between 0 and 1, but this is 3"
    );
    assert_eq!(msg("fn f(x, a: furlongs) { x }"), "unknown type `furlongs`");
    assert_eq!(msg("opcode f(x) { x }"), "`opcode` is now `fn`");
}

#[test]
fn state_and_untyped_parameters_belong_to_the_right_definitions() {
    assert_eq!(
        msg("instr a() { state y = 0.0\n0.5 }"),
        "`state` can only be used inside a `fn`"
    );
    assert_eq!(
        msg("instr a(freq) { osc(saw, freq) }"),
        "parameter `freq` needs a type"
    );
}

#[test]
fn a_broken_function_is_reported_once_even_if_it_is_used() {
    let errs = errors(
        "\nfn bad(x) { x + undefined_thing }\ninstr a(f: hz) { osc(saw, f).bad }\na(f: 100hz) for 1beat\n",
    );
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].message, "`undefined_thing` is not defined");
    // even when nothing calls it
    assert_eq!(
        msg("fn bad(x) { x + undefined_thing }"),
        "`undefined_thing` is not defined"
    );
}

#[test]
fn rates_on_parameters_are_checked() {
    // `const` needs a value known while compiling
    ok(
        "fn rep(x, n: const int = 2) { (for i in 0..<n { x }).sum }\ninstr a(f: hz) { osc(saw, f).rep(n: 3) }",
    );
    assert_eq!(
        msg(
            "fn rep(x, n: const int = 2) { x * n }\ninstr a(f: hz) { osc(saw, f).rep(n: f / 1hz) }"
        ),
        "`n` must be known while compiling"
    );
    assert_eq!(
        msg("fn rep(x, n: const int = 2) { x * n }\ninstr a(f: hz) { osc(saw, f).rep(n: 2.5) }"),
        "`n` expects a whole number, found 2.5"
    );
    // Structural parameters need defaults; scalar parameters retain their rate.
    assert_eq!(
        msg("instr a(f: const hz) { osc(saw, f) }"),
        "structural parameter `f` needs a default"
    );
    ok("instr a(f: init hz, cut: control hz = 1khz) { osc(saw, f).lpf(cut) }");
    assert_eq!(
        msg("fn f(mode: init (a | b) = a) { 1 }"),
        "a choice is always known while compiling"
    );
}

#[test]
fn choices_and_arrays_specialize_functions_and_instruments() {
    ok("instr a(mode: (x | y) = x) { if mode == x { 0.5 } else { 0.25 } }");
    ok(
        "instr a(freq: hz, partials: [num] = [1, 0.5], count: const int = 2) { (for i in 0..<count { osc(sine, freq * (i + 1)) * partials[i] }).sum }\ninstr b(freq: hz) { a(freq, partials: [1, 0.2, 0.1], count: 3) }",
    );
    ok(
        "fn sine(f: hz) { osc(sine, f) }\ninstr a(freq: hz, make: fn = sine) { make(freq) }\ninstr b(freq: hz) { a(freq, make: fn(f) { osc(saw, f) }) }",
    );
    assert_eq!(
        msg("fn f(m: (a | b) = c) { 1 }"),
        "the default for `m` is one of a, b"
    );
    assert_eq!(
        msg("fn f(m: (a) = a) { 1 }"),
        "a choice lists at least two names"
    );
    ok("fn f(xs: [num]) { xs.sum }\ninstr a() { f([0.1, 0.2]) * osc(sine, 100hz) }");
    ok("fn f(xs: [num]) { xs.sum }\ninstr a() { f(0.5) * osc(sine, 100hz) }");
}

#[test]
fn functions_have_no_side_effects() {
    assert_eq!(
        msg("bus b\nfn f(x) { b += x\n x }\ninstr a(f: hz) { osc(saw, f).f }"),
        "a function can't send to the bus `b`"
    );
    assert_eq!(
        msg("fn f(x: num) { x * note.time.num }"),
        "`note` is only available in an instrument"
    );
    // a function's locals may share a name with an instrument, which it can't read
    ok(
        "instr y(f: hz) { osc(saw, f) }\nfn g(x) {\n y = x * 2\n y\n}\ninstr a(f: hz) { osc(saw, f).g }",
    );
}

#[test]
fn statements_in_blocks() {
    ok("instr a(f: hz) {\n g = 1\n if f > 1khz { g = 0.5 }\n osc(saw, f) * g\n}");
    ok("instr a(f: hz) {\n if f > 1khz { out = osc(saw, f) } else { out = osc(sine, f) }\n}");
    assert_eq!(
        msg("bus b\ninstr a(f: hz) {\n if f > 1khz { b += osc(saw, f) }\n osc(saw, f)\n}"),
        "a send inside an `if` that changes while playing would always be made"
    );
    // a name first bound in a block stays in it
    assert!(
        msg("instr a(f: hz) {\n if f > 1khz { g = 0.5 }\n osc(saw, f) * g\n}")
            .contains("`g` is not defined")
    );
    assert_eq!(
        msg("fn f(x) {\n if x > 0 { state s = 0 }\n x\n}\ninstr a(f: hz) { osc(saw, f).f }"),
        "state is declared at the top of a function, not inside a block"
    );
    assert_eq!(
        msg("instr a() { if 1 > 0 { 0.5 } }"),
        "an `if` used as a value needs an `else`"
    );
}

#[test]
fn a_tail_is_a_time() {
    ok("instr a(f: hz) { tail = 2sec\n osc(saw, f) }");
    assert_eq!(
        msg("instr a(f: hz) { tail = 2\n osc(saw, f) }"),
        "`tail` is a time from 0 to 600sec, such as `2sec`"
    );
}

#[test]
fn constants() {
    let p = ok(
        "const k = 2\nconst ratios = (1..3).map(it * k)\ninstr a(f: hz) { osc(saw, f * ratios).sum * 0.1 }",
    );
    assert_eq!(p.instruments[0].graph.channels(), 1);
    assert_eq!(msg("const k = 1\nconst k = 2"), "`k` is defined twice");
    assert_eq!(
        msg("const k = 1\ninstr a(f: hz) { k = 2\n osc(saw, f) }"),
        "`k` is already the name of a constant, so it can't be used for a local"
    );
}

#[test]
fn db_gains_apply_to_plain_values() {
    ok("instr a(f: hz, amp: db = -6db) { osc(saw, f) * amp }");
    ok("fn g(x, amp: db = -6db) { x * amp }\ninstr a(f: hz) { osc(saw, f).g(amp: -3db) }");
}

const ROUTING: &str = "\nbus space\n\ninstr pluck(freq: hz, bright: 0..1 = 0.5) {\n  out = osc(saw, freq).lpf(cutoff: 900hz + bright * 1khz)\n  space += out.gain(-14db)\n}\n\nfx room {\n  out = space.lpf(cutoff: 4khz).hpf(cutoff: 150hz)\n}\n\nbus lead_audio\nfx lead_fx {out = lead_audio.gain(0.5)\n  space += out.gain(-12db)}\nclip lead { instrument: pluck(bright: 0.6) }\n";

#[test]
fn buses_and_nodes_compile() {
    let p = ok(&format!(
        "{ROUTING}\nlead(freq: c4) for 1beat\npluck(freq: c4) for 1beat"
    ));
    assert_eq!(p.buses, ["space", "lead_audio"]);
    assert_eq!(
        p.nodes.len(),
        4,
        "the implicit collection, two fx instances and the clip"
    );
    assert_eq!(p.nodes[3].name, "lead");
    assert_eq!(p.nodes[3].instrument, Some(0));
    assert_eq!(p.nodes[3].defaults["bright"], Value::Num(0.6));
    assert_eq!(
        p.nodes[0].def.voice_sends,
        [0],
        "bare instruments may send to the bus"
    );
    assert_eq!(p.notes.len(), 2);

    // the plan for a part note uses the node's defaults, and the note wins over them
    let plan = |e: &Event| p.plan(e).unwrap();
    let bright = p.instruments[0]
        .params
        .iter()
        .position(|q| q.name == "bright")
        .unwrap();
    let on_node = plan(&p.notes[0]);
    assert_eq!(on_node.node, 3);
    assert!(on_node.params.contains(&(bright, 0.6)));
    let bare = plan(&p.notes[1]);
    assert_eq!(bare.node, 0);
    assert!(
        !bare.params.iter().any(|(i, _)| *i == bright),
        "no default applied on the implicit node"
    );
    let overridden = p.plan(&Event {
        args: [
            ("freq".to_string(), Value::Note(60)),
            ("bright".to_string(), Value::Num(0.1)),
        ]
        .into(),
        ..p.notes[0].clone()
    });
    assert!(overridden.unwrap().params.contains(&(bright, 0.1)));
}

#[test]
fn routing_a_node_to_a_bus() {
    let p = ok(
        "\nbus fx\nclip a { instrument: i }\nfx b { out = fx }\ninstr i(f: hz) { voice = osc(sine, f)\nfx += voice }\na(f: 100hz) for 1beat",
    );
    assert_eq!(p.nodes[1].def.voice_sends, [0]);
    assert_eq!(
        p.nodes[2].def.inputs,
        [niminal_engine::ChainInput::Bus { bus: 0, channel: 0 }]
    );
}

#[test]
fn out_and_it_rules() {
    // an instrument either sets out or ends with a value, not both
    assert_eq!(
        msg("instr a(f: hz) { out = osc(saw, f)\nosc(sine, f) }"),
        "this instrument sets `out`, so its last line can't also be a value"
    );
    ok("instr a(f: hz) { out = osc(saw, f)\nout += osc(sine, f / 2) }");
    assert_eq!(
        msg("instr a(f: hz) { out += out\nout }"),
        "`out` has no value yet"
    );
    assert_eq!(
        msg("instr a(f: hz) { out = f }"),
        "`out` carries a plain signal, but this is hz"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, it) }"),
        "`it` is only available inside a node, or a function given to `map`"
    );
    assert_eq!(
        msg("instr a(f: hz) { it = 1 }"),
        "`it` is an input and can't be assigned"
    );
    assert_eq!(
        msg("instr a(f: hz) { instrument = 1\n0.5 }"),
        "`instrument` is only available in a node"
    );
    // only sends: silent but valid
    ok("bus b\ninstr a(f: hz) { b += osc(saw, f) }");
}

#[test]
fn bus_rules() {
    let with = |body: &str| format!("bus space\n{body}");
    assert_eq!(
        msg(&with("instr a(f: hz) { osc(saw, space) }")),
        "`space` is a bus, and only an fx can read a bus"
    );
    assert_eq!(
        help(&with("instr a(f: hz) { osc(saw, space) }")).as_deref(),
        Some("an instrument can send to it with `space += ...`")
    );
    assert_eq!(
        msg(&with(
            "instr a(f: hz) { x = osc(saw, f)\nx += osc(saw, f)\nx }"
        )),
        "`+=` can only add to `out` or a bus, not `x`"
    );
    assert_eq!(
        help(&with(
            "instr a(f: hz) { x = osc(saw, f)\nx += osc(saw, f)\nx }"
        ))
        .as_deref(),
        Some("to change a local, write `x = x + ...`")
    );
    assert_eq!(
        help(&with("instr a(f: hz) { spaec += osc(saw, f) }")).as_deref(),
        Some("did you mean the bus `space`?")
    );
    assert_eq!(
        msg(&with("instr a(f: hz) { space += f\n0.5 }")),
        "`space` carries a plain signal, but this is hz"
    );
    assert_eq!(msg("bus a\nbus a"), "bus `a` is defined twice");
    assert_eq!(msg("bus a: wobble"), "unknown channel layout `wobble`");
    assert_eq!(
        msg("bus a: surround(9.1)"),
        "unsupported surround layout `9.1`"
    );
    ok("bus a: mono\nbus b: stereo\nbus c: surround(5.1)\nbus d: channels(3)");
}

#[test]
fn locals_cannot_shadow_top_level_names() {
    let with = |body: &str| format!("bus space\ninstr other(f: hz) {{ osc(sine, f) }}\n{body}");
    assert_eq!(
        msg(&with("instr a(f: hz) { space = 1\nosc(saw, f) }")),
        "`space` is already the name of a bus, so it can't be used for a local"
    );
    assert_eq!(
        msg(&with("instr a(other: hz) { osc(saw, other) }")),
        "`other` is already the name of an instrument, so it can't be used for a local"
    );
    assert_eq!(
        msg(&with("instr a(f: hz) { space = 1 }")),
        "`space` is already the name of a bus, so it can't be used for a local"
    );
    assert_eq!(
        msg("bus x\ninstr x(f: hz) { osc(saw, f) }"),
        "`x` is already the name of a bus"
    );
}

#[test]
fn collections_and_fxs_have_distinct_roles() {
    let base = "instr tone(freq: hz = 440hz) { osc(sine, freq) }\n";
    ok(&format!("{base}clip lead {{ instrument: tone }}"));
    ok("fx room(cut: hz = 2khz) { out = it.lpf(cutoff: cut) }");
    assert!(
        msg(&format!("{base}fx room {{ instrument = tone }}"))
            .contains("cannot select an instrument")
    );
    assert!(
        compile(&format!(
            "{base}clip lead {{ instrument: tone\n output: room }}"
        ))
        .is_err()
    );

    assert!(compile("track lead { instrument = tone }").is_err());
    assert!(compile("bus room { out = it }").is_err());
}

#[test]
fn feedback_between_nodes_is_an_error_naming_them() {
    let src = "\nbus x\nbus y\nfx a { out = x.lpf(cutoff: 1khz).to(y) }\nfx b { out = y.lpf(cutoff: 1khz).to(x) }\n";
    let d = first_error(src);
    assert!(
        d.message == "nodes feed back through buses or buffers: a -> b -> a"
            || d.message == "nodes feed back through buses or buffers: b -> a -> b",
        "{}",
        d.message
    );
    assert!(d.help.unwrap().contains("`late`"));

    // a late send breaks the loop
    ok("bus x\nbus y\nfx a { out = x.lpf(cutoff: 1khz).to(y) }\nfx b { out = y\n x += late(out) }");
    // outside a send, `prev` is one sample earlier
    ok("bus x\nfx a { out = prev(x) }");
    let d = first_error("bus x\nfx a { out = x.to(x) }");
    assert_eq!(
        d.message,
        "nodes feed back through buses or buffers: a -> a"
    );
}

#[test]
fn a_broken_instrument_does_not_cascade_into_its_nodes_and_notes() {
    let errs = errors(
        "\ninstr bad(f: hz) { osc(saw, 440) }\nclip t { instrument: bad }\nt() for 1beat\nbad() for 1beat\n",
    );
    assert_eq!(errs.len(), 1, "{errs:?}");
}

#[test]
fn delay_and_reverb_compile_with_units_checked() {
    ok(
        "instr a(f: hz) { osc(saw, f).delay(time: 3/16beat, feedback: 0.4).reverb(room: 0.8, damp: 0.3) }",
    );
    ok("instr a(f: hz) { osc(saw, f).delay(time: 250ms).reverb }");

    let d = first_error("instr a(f: hz) { osc(saw, f).delay(time: 0.2) }");
    assert_eq!(d.message, "`time` expects a time, found a plain number");
    assert_eq!(d.help.as_deref(), Some("did you mean `0.2sec`?"));
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).delay(time: 1beat, feedback: 1sec) }"),
        "`feedback` expects a plain number, found a time"
    );
}

#[test]
fn a_changing_delay_time_needs_a_max() {
    let e = msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t) }");
    assert_eq!(
        e,
        "`delay` needs a constant `time`, or a `max:` that bounds a changing one"
    );
    ok("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 2sec) }");
    assert_eq!(
        msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 31sec) }"),
        "a delay can be at most 30 seconds long"
    );
    assert_eq!(
        msg("instr a() { osc(saw, 100hz).delay(time: 40sec) }"),
        "a delay can be at most 30 seconds long"
    );
    assert_eq!(
        msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 5) }"),
        "`max` expects a time, found a plain number"
    );
}

#[test]
fn config_sets_the_master_layout_and_instruments_follow_it() {
    let src = |config: &str| format!("{config}\ninstr a(f: hz) {{ osc(saw, f) }}");
    assert_eq!(ok(&src("")).master, Layout::Mono);
    for (config, layout, channels) in [
        ("config { channels: stereo }", Layout::Stereo, 2),
        ("config { channels: quad }", Layout::Quad, 4),
        ("config { channels: surround(5.1) }", Layout::Surround51, 6),
        ("config { channels: surround(7.1) }", Layout::Surround71, 8),
    ] {
        let p = ok(&src(config));
        assert_eq!(p.master, layout);
        assert_eq!(p.instruments[0].graph.channels(), channels, "{config}");
    }
    // an unnamed layout has no automatic conversion from mono, but a list of that many works
    assert_eq!(
        msg(&src("config { channels: channels(3) }")),
        "can't convert mono to ch[3] automatically"
    );
    let three = ok(
        "config { channels: channels(3) }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f)] }",
    );
    assert_eq!(three.instruments[0].graph.channels(), 3);
    assert_eq!(
        msg(&src("config { channels: wobble }")),
        "unknown channel layout `wobble`"
    );
    assert_eq!(msg(&src("config { speed: 2 }")), "unknown setting `speed`");
    assert_eq!(
        msg(&src(
            "config { channels: stereo }\nconfig { channels: mono }"
        )),
        "a project has only one `config`"
    );
    assert_eq!(
        msg(&src("config { channels: channels(40) }")),
        "`channels` takes a count from 1 to 16"
    );
}

#[test]
fn pan_places_a_mono_signal_in_the_master_layout() {
    let p = ok(
        "config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg, spread: 0.3) }",
    );
    assert_eq!(p.instruments[0].graph.channels(), 6);
    ok(
        "config { channels: stereo }\ninstr a(f: hz, az: deg = 10deg) { osc(saw, f).pan(azimuth: az) }",
    );

    let d =
        first_error("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20) }");
    assert_eq!(
        d.message,
        "`azimuth` expects an angle, found a plain number"
    );
    assert_eq!(d.help.as_deref(), Some("did you mean `20deg`?"));
    assert_eq!(
        msg("config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)].pan }"),
        "`pan` takes a one-channel signal, but `x` has 2"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).pan(azimuth: 5hz) }"),
        "`azimuth` expects an angle, found hz"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f) * 20deg }"),
        "an instrument outputs a plain signal, but this is an angle"
    );
}

#[test]
fn angles_follow_the_unit_rules() {
    ok(
        "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg * 2 - 5deg) }",
    );
    assert_eq!(
        msg(
            "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg + 1hz) }"
        ),
        "can't add an angle and hz"
    );
}

#[test]
fn channel_lists_and_destructuring() {
    let stereo = |body: &str| format!("config {{ channels: stereo }}\ninstr a(f: hz) {{ {body} }}");
    let p = ok(&stereo("[osc(saw, f), osc(sine, f)]"));
    assert_eq!(p.instruments[0].graph.channels(), 2);

    ok(&stereo(
        "[l, r] = osc(saw, f).pan\nmid = (l + r) * 0.5\n[mid + l, mid - r]",
    ));
    // an opcode applied to a stereo signal runs once per channel
    ok(&stereo(
        "[osc(saw, f), osc(saw, f)].lpf(cutoff: 800hz).delay(time: 100ms).reverb",
    ));
    // different cutoffs per channel
    ok(&stereo("osc(saw, f).pan.lpf(cutoff: [800hz, 1200hz])"));

    assert_eq!(
        msg(&stereo("[l, r, c] = osc(saw, f).pan\nl")),
        "this signal has 2 channel(s), but 3 name(s) are given"
    );
    assert_eq!(
        msg(&stereo("[osc(saw, f).pan, osc(saw, f)]")),
        "a channel array takes one channel per entry, but this has 2"
    );
    assert_eq!(
        msg(&stereo("[f, f]")),
        "an instrument outputs a plain signal, but this is hz"
    );
    assert_eq!(
        msg(&stereo("[osc(saw, f), f]")),
        "the entries of a channel array must have the same unit: this is hz, the first was a plain number"
    );
    assert_eq!(
        msg(&stereo(
            "[osc(saw, f), osc(saw, f)] + [osc(saw, f), osc(saw, f), osc(saw, f)]"
        )),
        "can't combine an array of 2 with an array of 3"
    );
    assert_eq!(
        msg(&stereo("osc(saw, [f, f]).lpf(cutoff: [1khz, 2khz, 3khz])")),
        "opcode array arguments must have matching shapes"
    );
    assert_eq!(
        msg(&stereo("[l, r] = osc(saw, f)\nl")),
        "this signal has 1 channel(s), but 2 name(s) are given"
    );
}

#[test]
fn lists_map_and_sum() {
    let mono = |body: &str| format!("instr a(f: hz) {{ {body} }}");
    // a supersaw: seven detuned oscillators added into one signal
    let p = ok(&mono(
        "(0..7).map(fn(i) { osc(saw, f * (1 + (i - 3) * 0.004)) }).sum * 0.2",
    ));
    assert_eq!(p.instruments[0].graph.channels(), 1);
    // a list literal maps too, and the parameter can be used in a unit
    ok(&mono(
        "[1, 2, 3].map(fn(h) { osc(sine, f * h) * (1 / h) }).sum",
    ));
    // mapping to a list keeps it a list
    let p = ok(
        "config { channels: stereo }\ninstr a(f: hz) { [1, 1.01].map(fn(r) { osc(saw, f * r) }) }",
    );
    assert_eq!(p.instruments[0].graph.channels(), 2);

    assert_eq!(
        msg(&mono("(3..<3).map(fn(i) { osc(saw, f) }).sum")),
        "this array is empty"
    );
    // `..` includes its end
    ok(&mono("(1..8).map(fn(n) { osc(sine, f * n) / n }).sum"));
    // a list is not limited by the channel count until it becomes a signal
    ok(&mono(
        "(0..40).map(fn(i) { osc(saw, f * (1 + i * 0.001)) }).sum",
    ));
    assert_eq!(
        msg(&mono("(0..17).map(fn(i) { osc(saw, f) })")),
        "audio output supports at most 16 channels"
    );
    // an expression of `it` is a function of one parameter
    ok(&mono("[1, 2].map(osc(saw, f * it)).sum"));
    assert_eq!(msg(&mono("fn(x) { x }")), "a function is not a signal");
    assert_eq!(
        msg(&mono("[1, 2].map(fn(f) { osc(saw, f) }).sum")),
        "`f` is already in use, so it can't be this function's parameter"
    );
    // the parameter is not visible after the map
    assert!(msg(&mono("[1].map(fn(x) { x }).sum + x")).contains("x"));
    // functions take and give lists
    ok(
        "fn spread(f, n: const int = 3) { for i in 0..<n { f * (1 + i * 0.01) } }\ninstr a(f: hz) { osc(saw, spread(f)).sum }",
    );
}

#[test]
fn indexing_and_for_loops() {
    let mono = |body: &str| format!("instr a(f: hz) {{ {body} }}");
    // `for` is `map` with the function written inline
    let p = ok(&mono(
        "(for i in 0..7 { osc(saw, f * (1 + (i - 3) * 0.004)) }).sum * 0.2",
    ));
    assert_eq!(p.instruments[0].graph.channels(), 1);
    ok(&mono("for h in [1, 2, 3] { osc(sine, f * h) }.sum"));
    // indexing picks one entry; the index can come from a loop
    ok(&mono("bank = [osc(saw, f), osc(sine, f)]\nbank[1] * 0.5"));
    ok(&mono(
        "bank = [osc(saw, f), osc(sine, f)]\n(for i in 0..2 { bank[i] * (i + 1) }).sum",
    ));
    // a unit survives an index
    ok(&mono("osc(saw, [f, f * 2][1])"));

    // indexes wrap, so they never fail
    ok(&mono("[osc(saw, f), osc(sine, f)][2]"));
    ok(&mono("[osc(saw, f), osc(sine, f)][-1]"));
    // an index that changes switches between the entries
    ok(&mono("[osc(saw, f), osc(sine, f)][osc(square, 2hz).abs]"));
    ok(&mono("osc(saw, [100hz, 200hz, 300hz][osc(saw, 1hz) * 3])"));
    assert_eq!(
        msg(&mono("[osc(saw, f)][f]")),
        "an index is a plain number, found hz"
    );
    ok("fn o(x: num) { x[0] }");
}

#[test]
fn lists_are_their_own_values() {
    let mono = |body: &str| format!("instr a(f: hz) {{ {body} }}");
    // lists of lists, `len`, and arithmetic on lists
    ok(&mono(
        "rows = for r in 0..3 { for c in 0..4 { osc(saw, f * (1 + (r * 4 + c) * 0.001)) } }\nrows[1][2] + rows[2].sum * (1 / rows.len)",
    ));
    ok(&mono("([1, 2, 3] * 2).sum * 0.1 + osc(saw, f)"));
    // a list holds entries of different units; only a signal needs one
    ok(&mono("pair = [f, 0.5]\nosc(saw, pair[0]) * pair[1]"));
    // `channels` makes a list of a signal's channels
    let p = ok(
        "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan.channels.map(fn(c) { c * 0.5 }) }",
    );
    assert_eq!(p.instruments[0].graph.channels(), 2);
    ok(
        "config { channels: stereo }\ninstr a(f: hz) { [l, r] = [osc(saw, f), osc(sine, f)]\n[r, l] }",
    );

    ok(&mono("osc(saw, f).pan.map(fn(c) { c }).sum"));
    ok(&mono("osc(saw, f)[0]"));
    assert_eq!(
        msg(&mono(
            "[[osc(saw, f), osc(saw, f)], [osc(saw, f), osc(saw, f)]]"
        )),
        "an array of arrays can't be used as a signal"
    );
    assert_eq!(
        msg(&mono("[l, r, c] = [osc(saw, f), osc(sine, f)]\nl")),
        "this array has 2 entries, but 3 name(s) are given"
    );
}

#[test]
fn layouts_convert_automatically_where_the_spec_says_they_do() {
    // mono into 5.1 is centred; stereo into 5.1 takes the front pair
    ok("config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f) }");
    ok("config { channels: surround(5.1) }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)] }");
    // 5.1 bus read by a stereo master part folds down
    ok(
        "config { channels: stereo }\nbus hall: surround(5.1)\ninstr a(f: hz) { out = osc(saw, f)\nhall += out }\nfx t { out = hall }",
    );
    // unnamed channel lists are taken as whatever layout has that many
    ok(
        "config { channels: quad }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f), osc(saw, f)] }",
    );

    let d = first_error(
        "config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f)] }",
    );
    assert_eq!(d.message, "can't convert ch[3] to stereo automatically");
    let d = first_error(
        "config { channels: surround(5.1) }\nbus q: quad\ninstr a(f: hz) { q += [osc(saw, f), osc(saw, f), osc(saw, f), osc(saw, f)].to_layout(surround(7.1)) }",
    );
    assert_eq!(
        d.message,
        "can't convert ch[4] to surround(7.1) automatically"
    );
}

#[test]
fn to_layout_converts_explicitly() {
    ok(
        "config { channels: surround(5.1) }\nbus s: stereo\ninstr a(f: hz) { out = osc(saw, f).pan\ns += out.to_layout(stereo) }",
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).to_layout(wobble) }"),
        "unknown channel layout `wobble`"
    );
    ok("instr a(f: hz) { osc(saw, f).to_layout(mono) }");
}

#[test]
fn buses_and_nodes_carry_their_layouts() {
    let p = ok(
        "\nconfig { channels: stereo }\nbus space: stereo\nbus lfe: mono\ninstr a(f: hz) { out = osc(saw, f).pan(azimuth: -20deg)\nspace += out\nlfe += out }\nfx room { out = space.reverb(room: 0.8) }\nfx sub { out = lfe.lpf(cutoff: 100hz) }\na(f: 100hz) for 1beat",
    );
    assert_eq!(p.bus_layouts, [Layout::Stereo, Layout::Mono]);
    assert_eq!(p.nodes[1].def.inputs.len(), 2, "a stereo bus is two inputs");
    assert_eq!(p.nodes[2].def.inputs.len(), 1);
    // the sub part works in mono but delivers stereo to the master
    assert_eq!(p.nodes[2].def.chain.as_ref().unwrap().channels(), 2);
    // routing to a bus delivers in that bus's layout
    let routed = ok(
        "\nconfig { channels: stereo }\nbus mono_bus: mono\nclip t { instrument: i }\nfx u { out = mono_bus }\ninstr i(f: hz) { voice = osc(saw, f).pan\nmono_bus += voice.to_layout(mono) }",
    );
    assert_eq!(routed.nodes[1].def.voice_sends, [0]);
}

#[test]
fn bars_and_semitones() {
    let p = ok("instr a(f: hz) { osc(sine, f + 7st) * env[1 1bar 0] }");
    assert_eq!(p.instruments.len(), 1);
    ok("instr a(f: hz, up: st = 7st) { osc(sine, f + up) }");
    ok("instr a(f: hz) { osc(sine, f - 12st + 3st) }");
    ok("instr a(f: hz) { osc(sine, c4 + 12st) }");
    ok("instr a(f: hz) { osc(sine, 7st + f) }");
    assert_eq!(
        msg("instr a(f: hz) { osc(sine, f + 7) }"),
        "can't add hz and a plain number"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(sine, 7st - f) }"),
        "can't subtract an interval and hz"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(sine, f * 2st) }"),
        "can't multiply hz and an interval"
    );
    let notes = ok("instr a() { osc(sine, 100hz) }\na() for 2bars\nat 1bar a() for 1bar");
    assert_eq!(notes.notes[0].dur, Time::Bars(2.0));
    assert_eq!(notes.notes[1].at, Time::Bars(1.0));
}

// ---- the performance layer ------------------------------------------------

use crate::performance::{Action, Clip, QuantizeRelation, QuantizeUnit};
use niminal_pattern::{Pattern, Rational, Span as CycleSpan};

const STAGE: &str = "\ninstr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: freq * (2 + bright * 6)) }\nclip lead { instrument: pluck }\nclip bass { instrument: pluck }\nclip pad { instrument: pluck }\n";

fn stage(body: &str) -> String {
    format!("{STAGE}{body}")
}

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d)
}

/// (start in cycles, value) of every event beginning in the first `cycles` cycles.
fn events(p: &Pattern<crate::performance::Step>, cycles: i64) -> Vec<(Rational, Value)> {
    let mut v: Vec<_> = p
        .onsets(CycleSpan::new(Rational::ZERO, Rational::int(cycles)))
        .into_iter()
        .map(|h| (h.whole.unwrap().start, h.value.value))
        .collect();
    v.sort_by_key(|e| e.0);
    v
}

fn played(src: &str) -> Clip {
    let p = ok(&stage(src));
    match &p.performance[0].action {
        Action::Update { clip, .. } => clip.clone(),
        _ => panic!("expected a play command"),
    }
}

fn notes_of(src: &str, cycles: i64) -> Vec<(Rational, Value)> {
    events(played(src).lane("notes").unwrap(), cycles)
}

fn midi(m: i32) -> Value {
    Value::Note(m)
}

fn values_of(src: &str, cycles: i64) -> Vec<Value> {
    notes_of(src, cycles).into_iter().map(|e| e.1).collect()
}

#[test]
fn mini_notation_has_weights_repeats_polymeter_and_euclid() {
    assert_eq!(
        values_of("lead.__update([c4!2 e4])", 1),
        [midi(60), midi(60), midi(64)]
    );
    assert_eq!(notes_of("lead.__update([c4@3 e4])", 1)[1].0, r(3, 4));
    assert_eq!(notes_of("lead.__update([c4(3,8)])", 1).len(), 3);
    assert_eq!(
        values_of("lead.__update([{c4 e4 g4}%2])", 2),
        [midi(60), midi(64), midi(67), midi(60)]
    );
    assert_eq!(notes_of("lead.__update([c4/2 e4])", 2).len(), 3);
}

#[test]
fn numeric_arguments_can_be_patterns() {
    assert_eq!(
        notes_of("lead.__update([c4 e4].fast([<1 2>]))", 2).len(),
        2 + 4
    );
    assert_eq!(
        notes_of("lead.__update([c4].euclid(hits: [<3 5>], steps: 8))", 2).len(),
        8
    );
    assert_eq!(
        notes_of("lead.__update([c4 e4].every([<1 2>], it.fast(2)))", 2).len(),
        4 + 2
    );
    assert_eq!(
        notes_of("lead.__update([c4].repeat_each([1 2]))", 1).len(),
        2
    );
}

#[test]
fn new_pattern_operations() {
    assert!(notes_of("lead.__update([c4*8].sometimes(50%, it.fast(2)))", 8).len() > 64);
    assert_eq!(
        values_of("lead.__update([c4 e4 g4 b4].rotate(4))", 2)[4..],
        [midi(64), midi(67), midi(71), midi(60)]
    );
    assert_eq!(
        values_of("lead.__update([c4 e4 g4 b4].repeat_start(50%))", 1),
        [midi(60), midi(64), midi(60), midi(64)]
    );
    assert_eq!(
        values_of("lead.__update([c4 e4].palindrome)", 2),
        [midi(60), midi(64), midi(64), midi(60)]
    );
    assert_eq!(
        notes_of(
            "lead.__update([c4 e4 g4 b4].within(0%, 50%, it.fast(2)))",
            1
        )
        .len(),
        6
    );
    let mut swung = notes_of("lead.__update([c4*8].swing(amount: 25%, every: 1/4))", 1);
    swung.sort_by_key(|e| e.0);
    assert_eq!(
        swung[1].0,
        r(3, 16),
        "the second eighth of each quarter waits a quarter of a quarter"
    );
    let mut shuffled = values_of("lead.__update([c4 e4 g4 b4].shuffle(4))", 1);
    shuffled.sort_by(|a, b| format!("{a}").cmp(&format!("{b}")));
    assert_eq!(shuffled.len(), 4);
    assert_eq!(
        notes_of("lead.__update([c4 e4 g4 b4].in_turn(4, it.fast(2)))", 1).len(),
        5
    );
    assert_eq!(
        notes_of("lead.__update([c4 e4 g4 b4].pick_random(4))", 1).len(),
        4
    );
}

#[test]
fn osc_out_sends_a_nodes_signal() {
    let node = |body: &str| {
        format!(
            "tempo 120bpm\ninstr pluck(freq: hz) {{ osc(saw, freq) }}\nbus lead_audio\nfx lead_fx {{{body}}}\nclip lead {{ instrument: pluck }}\n"
        )
    };
    let p = ok(&node("out = it.osc_out(\"/vj/level\", port: 9000)"));
    assert_eq!(p.taps.len(), 1);
    assert_eq!(
        (
            p.taps[0].address.as_str(),
            p.taps[0].host.as_str(),
            p.taps[0].port,
            p.taps[0].rate
        ),
        ("/vj/level", "127.0.0.1", 9000, 30.0)
    );
    let p = ok(&node(
        "out = it.osc_out(\"/a\", host: \"10.0.0.2\", port: 9001, rate: 5hz).osc_out(\"/b\", port: 9001)",
    ));
    assert_eq!(
        p.taps
            .iter()
            .map(|t| (t.address.as_str(), t.host.as_str(), t.rate))
            .collect::<Vec<_>>(),
        [("/a", "10.0.0.2", 5.0), ("/b", "127.0.0.1", 30.0)]
    );
    assert_eq!(
        msg(&node("out = it.osc_out(\"vj\", port: 9000)")),
        "an OSC address starts with `/`"
    );
    assert_eq!(
        msg(&node("out = it.osc_out(\"/a\")")),
        "`osc_out` needs a `port:`"
    );
    assert_eq!(
        msg(&node("out = it.osc_out(port: 9000)")),
        "`osc_out` needs an address"
    );
    assert_eq!(
        msg(&node("out = it.osc_out(\"/a\", port: 0)")),
        "a port is a whole number from 1 to 65535"
    );
    assert_eq!(
        msg(&node("out = it.osc_out(\"/a\", port: 9000, rate: 900hz)")),
        "`rate` is from 0.5hz to 200hz"
    );
    assert_eq!(
        msg(&node("out = it.osc_out(\"/a\", prot: 9000)")),
        "`osc_out` has no argument named `prot`"
    );
    assert_eq!(
        msg(&node("out = it.osc_out(5, port: 9000)")),
        "an OSC address is text in quotes"
    );
    ok("instr a(f: hz) { osc(saw, f).osc_out(\"/a\", port: 9000) }");
    assert_eq!(
        msg("fn a(x) { x.osc_out(\"/a\", port: 9000) }"),
        "`osc_out` can't be used inside a function"
    );
}

#[test]
fn midi_out_is_an_instrument_that_sends_notes() {
    let src = "tempo 120bpm\nmidi_out drums(port: \"IAC\", channel: 10)\nclip beat { instrument: drums }\nbeat.__update([c4 g4])";
    let p = ok(src);
    let instrument = p.instrument("drums").expect("it is an instrument");
    let target = instrument.midi.as_ref().expect("it goes to a port");
    assert_eq!((&*target.port, target.channel), ("IAC", 10));
    let events = p.schedule().events(0.0, 2.0).unwrap();
    let plans: Vec<_> = events
        .iter()
        .map(|e| p.plan(e).unwrap().midi.unwrap())
        .collect();
    assert_eq!(
        plans
            .iter()
            .map(|m| (m.note, m.channel))
            .collect::<Vec<_>>(),
        [(60, 10), (67, 10)]
    );
    // a gain sets the velocity: 0db is the loudest, -30db the softest
    let loud = p
        .plan(&niminal_score::Event {
            target: "drums".into(),
            at: Time::Seconds(0.0),
            dur: Time::Seconds(1.0),
            args: [
                ("freq".to_string(), Value::Note(60)),
                ("gain".to_string(), Value::Db(0.0)),
            ]
            .into(),
        })
        .unwrap()
        .midi
        .unwrap();
    let soft = p
        .plan(&niminal_score::Event {
            target: "drums".into(),
            at: Time::Seconds(0.0),
            dur: Time::Seconds(1.0),
            args: [
                ("freq".to_string(), Value::Note(60)),
                ("gain".to_string(), Value::Db(-30.0)),
            ]
            .into(),
        })
        .unwrap()
        .midi
        .unwrap();
    assert_eq!((loud.velocity, soft.velocity), (127, 1));
    // the channel defaults to 1
    let p = ok("midi_out synth(port: \"x\")");
    assert_eq!(
        p.instrument("synth")
            .unwrap()
            .midi
            .as_ref()
            .unwrap()
            .channel,
        1
    );
    assert_eq!(msg("midi_out a()"), "`a` needs a `port:`");
    assert_eq!(
        msg("midi_out a(port: 3)"),
        "the port is the name of a MIDI output, in quotes"
    );
    assert_eq!(
        msg("midi_out a(port: \"x\", channel: 17)"),
        "a MIDI channel is a whole number from 1 to 16"
    );
    assert_eq!(
        msg("midi_out a(port: \"x\", chanel: 2)"),
        "`midi_out` has no argument `chanel`"
    );
    assert_eq!(
        msg("midi_out a(port: \"x\")\ninstr a(f: hz) { osc(sine, f) }"),
        "`a` is defined twice"
    );
}

#[test]
fn part_declarations_give_a_migration_error() {
    for src in [
        "part lead { instrument = pluck }",
        "part lead = pluck(bright: 0.2)",
    ] {
        let d = first_error(src);
        assert_eq!(d.message, "`part` has been removed");
        assert_eq!(
            d.help.as_deref(),
            Some("write `clip x { instrument: y }` instead")
        );
        assert_eq!(&src[d.span.start..d.span.end], "part");
    }
}

#[test]
fn where_changes_only_the_notes_that_pass() {
    assert_eq!(
        values_of(
            "lead.__update([c3 e4 g2 c5].where(it < c4, it.transpose(+12st)))",
            1
        ),
        [midi(60), midi(64), midi(55), midi(72)]
    );
    assert_eq!(
        values_of(
            "lead.__update([c3 e4].where(it.freq >= e4, it.transpose(-12st)))",
            1
        ),
        [midi(48), midi(52)]
    );
    assert!(msg(&stage("lead.__update([c3].where(c3, it))")).contains("takes a test on the note"));
}

/// The notes of a clip with the controls each carries, in time order.
fn steps_of(src: &str, cycles: i64) -> Vec<(Rational, crate::performance::Step)> {
    let clip = played(src);
    let mut v: Vec<_> = clip
        .lane("notes")
        .unwrap()
        .onsets(CycleSpan::new(Rational::ZERO, Rational::int(cycles)))
        .into_iter()
        .map(|h| (h.whole.unwrap().start, h.value))
        .collect();
    v.sort_by_key(|e| e.0);
    v
}

#[test]
fn notes_carry_their_own_controls() {
    let s = steps_of("lead.__update([c4 e4].gain([-6db -12db]).pan(-30deg))", 1);
    assert_eq!((s[0].1.gain, s[1].1.gain), (Some(-6.0), Some(-12.0)));
    assert_eq!((s[0].1.pan, s[1].1.pan), (Some(-30.0), Some(-30.0)));
    // a control keeps the structure of the notes, not of its own pattern
    let s = steps_of("lead.__update([c4 e4 g4 b4].pan([-30deg 30deg]))", 1);
    assert_eq!(
        s.iter().map(|e| e.1.pan.unwrap()).collect::<Vec<_>>(),
        [-30.0, -30.0, 30.0, 30.0]
    );
    // and survives other changes to the notes
    let s = steps_of(
        "lead.__update([c4 e4].pan(20deg).transpose(+12st).reverse)",
        1,
    );
    assert_eq!((s[0].1.value, s[0].1.pan), (midi(76), Some(20.0)));
    assert_eq!(
        steps_of("lead.__update([c4].gain(0.5))", 1)[0]
            .1
            .gain
            .map(|g| (g * 10.0).round()),
        Some(-60.0)
    );
    assert!(
        msg(&stage("lead.__update([c4].pan(3hz))")).contains("`pan` takes a direction in degrees")
    );
    assert!(
        msg(&stage("lead.__update([c4].begin(2))")).contains("`begin` takes a share of the sample")
    );
}

#[test]
fn left_right_places_the_original_and_the_change_apart() {
    let s = steps_of("lead.__update([c4 e4].left_right(it.reverse))", 1);
    assert_eq!(s.len(), 4);
    let left: Vec<_> = s
        .iter()
        .filter(|e| e.1.pan == Some(-30.0))
        .map(|e| e.1.value)
        .collect();
    let right: Vec<_> = s
        .iter()
        .filter(|e| e.1.pan == Some(30.0))
        .map(|e| e.1.value)
        .collect();
    assert_eq!(left, [midi(60), midi(64)]);
    assert_eq!(right, [midi(64), midi(60)]);
    let wide = steps_of("lead.__update([c4].left_right(it, angle: 60deg))", 1);
    assert!(wide.iter().any(|e| e.1.pan == Some(60.0)));
}

#[test]
fn chop_walks_through_the_sample() {
    let s = steps_of("lead.__update([c4 e4].chop(4))", 1);
    assert_eq!(s.len(), 8);
    assert_eq!(
        s.iter().map(|e| e.1.begin.unwrap()).collect::<Vec<_>>(),
        [0.0, 0.25, 0.5, 0.75, 0.0, 0.25, 0.5, 0.75]
    );
    assert_eq!(s[1].0, r(1, 8), "each piece takes its share of the note");
    let s = steps_of("lead.__update([c4 e4].interleave_chops(2))", 1);
    assert_eq!(
        s.iter()
            .map(|e| (e.0, e.1.begin.unwrap()))
            .collect::<Vec<_>>(),
        [
            (r(0, 1), 0.0),
            (r(1, 4), 0.0),
            (r(1, 2), 0.5),
            (r(3, 4), 0.5)
        ]
    );
    let s = steps_of("lead.__update([c4].begin(50%).chop(2))", 1);
    assert_eq!(
        s.iter().map(|e| e.1.begin.unwrap()).collect::<Vec<_>>(),
        [0.5, 0.75],
        "pieces divide what is left of the sample"
    );
}

#[test]
fn grids_invert() {
    let hits = notes_of("lead.__update([c4].rhythm(grid[x.x. x...].invert))", 1);
    assert_eq!(
        hits.iter().map(|e| e.0).collect::<Vec<_>>(),
        [r(1, 8), r(3, 8), r(5, 8), r(3, 4), r(7, 8)]
    );
    assert!(msg(&stage("lead.__update([c4].invert)")).contains("`invert` works on grids"));
}

#[test]
fn signals_scale_and_sample_into_values() {
    let v = notes_of("lead.__update(saw.range(100hz, 200hz).to_steps(4))", 1);
    assert_eq!(v.len(), 4);
    let Value::Hz(h) = v[0].1 else { panic!() };
    assert!((h - 112.5).abs() < 1e-9, "{h}");
    let Value::Hz(h) = v[3].1 else { panic!() };
    assert!((h - 187.5).abs() < 1e-9, "{h}");
    assert!(msg(&stage("lead.__update(sine.range(1hz, 2db))")).contains("same unit"));
}

#[test]
fn composition_tools() {
    assert_eq!(
        values_of("lead.__update([0 1 2 7].in_scale(minor, root: c3))", 1),
        [midi(48), midi(50), midi(51), midi(60)]
    );
    assert_eq!(
        values_of("lead.__update([-1].in_scale(major, root: c4))", 1),
        [midi(59)]
    );
    let chord = notes_of("lead.__update([c4].chord(minor7))", 1);
    assert_eq!(chord.iter().map(|e| e.1).collect::<Vec<_>>().len(), 4);
    assert!(chord.iter().all(|e| e.0 == r(0, 1)));
    assert_eq!(
        values_of("lead.__update([c4].chord(major).arpeggiate(up))", 1),
        [midi(60), midi(64), midi(67)]
    );
    assert_eq!(
        values_of("lead.__update([c4].chord(major).arpeggiate(updown))", 1),
        [midi(60), midi(64), midi(67), midi(64)]
    );
    assert_eq!(
        values_of("lead.__update([c4].chord(major).arpeggiate(converge))", 1),
        [midi(60), midi(67), midi(64)]
    );
    assert_eq!(
        values_of("lead.__update([c4 e4 g4].invert_melody(around: c4))", 1),
        [midi(60), midi(56), midi(53)]
    );
    // the second chord is moved to sit by the first
    let led = values_of("lead.__update([[c4,e4,g4] [f4,a4,c5]].voice_lead)", 1);
    assert_eq!(
        led.iter()
            .filter(|v| matches!(v, Value::Note(m) if *m >= 72))
            .count(),
        0,
        "{led:?}"
    );
    // markov and random_walk keep the rhythm and start where the source does
    let m = notes_of("lead.__update([c4 e4 g4 e4].markov)", 4);
    assert_eq!(m.len(), 16);
    assert_eq!(m[4].1, midi(60));
    let w = values_of("lead.__update([c4*8].random_walk(step: 2st))", 3);
    assert_eq!(w[0], midi(60));
    assert!(
        w.iter()
            .all(|v| matches!(v, Value::Note(m) if (m - 60).abs() <= 24))
    );
    assert!(w.iter().any(|v| *v != midi(60)));
    assert!(msg(&stage("lead.__update([0].in_scale(nonsense))")).contains("isn't a scale"));
    assert!(msg(&stage("lead.__update([c4].chord(nonsense))")).contains("isn't a chord"));
}

#[test]
fn a_pattern_of_notes_plays_on_a_node() {
    let p = ok(&stage("riff = [c4 ~ e4 g4]\nlead.__update(riff)"));
    assert_eq!(p.performance.len(), 1);
    let Action::Update { node, clip } = &p.performance[0].action else {
        panic!()
    };
    assert_eq!(*node, 1, "node 0 is the implicit one");
    assert_eq!(clip.length, Rational::ONE);
    assert_eq!(
        events(clip.lane("notes").unwrap(), 1),
        [
            (r(0, 1), midi(60)),
            (r(1, 2), midi(64)),
            (r(3, 4), midi(67))
        ]
    );
    assert_eq!(p.performance[0].at, Time::Beats(0.0));
}

#[test]
fn a_pattern_can_be_written_inline_and_used_before_it_is_defined() {
    assert_eq!(notes_of("lead.__update([a3 b3])", 1).len(), 2);
    let p = ok(&stage("lead.__update(riff)\nriff = [c4]"));
    assert_eq!(p.performance.len(), 1);
}

#[test]
fn atoms_carry_units_and_note_names() {
    let c = played("lead.__update([c#4 -6db 440hz 1/8beat 0.5 7st])");
    let values: Vec<_> = events(c.lane("notes").unwrap(), 1)
        .into_iter()
        .map(|e| e.1)
        .collect();
    assert_eq!(
        values,
        [
            Value::Note(61),
            Value::Db(-6.0),
            Value::Hz(440.0),
            Value::Beats(0.125),
            Value::Num(0.5),
            Value::Semitones(7.0)
        ]
    );
}

#[test]
fn time_operations() {
    assert_eq!(notes_of("lead.__update([c4 e4].fast(2))", 1).len(), 4);
    let slow = notes_of("lead.__update([c4 e4].slow(2))", 2);
    assert_eq!(slow, [(r(0, 1), midi(60)), (r(1, 1), midi(64))]);
    assert_eq!(
        notes_of("lead.__update([c4 e4 g4].reverse)", 1)[0].1,
        midi(67)
    );
    let shifted = notes_of("lead.__update([c4 e4].shift(1/4))", 1);
    assert_eq!(
        shifted.iter().map(|e| e.0).collect::<Vec<_>>(),
        [r(1, 4), r(3, 4)]
    );
    // beats are a share of a bar: a quarter of 4/4 is a quarter cycle
    let by_beat = notes_of("lead.__update([c4 e4].shift(1beat))", 1);
    assert_eq!(by_beat[0].0, r(1, 4));
    let over = notes_of("lead.__update([c4 e4].over(2bars))", 2);
    assert_eq!(over.len(), 2);
    assert_eq!(over[1].0, r(1, 1));
}

#[test]
fn every_applies_a_change_on_some_cycles() {
    let n = notes_of("lead.__update([c4 e4 g4 b4].every(2, it.reverse))", 2);
    assert_eq!(n[0].1, midi(71), "reversed on cycle 0");
    assert_eq!(n[4].1, midi(60), "plain on cycle 1");
    // the change can use any operation, including others that use `it`
    assert_eq!(
        notes_of("lead.__update([c4 e4].every(1, it.fast(2).reverse))", 1).len(),
        4
    );
}

#[test]
fn value_operations() {
    let up = notes_of("lead.__update([c4 e4].transpose(+5st))", 1);
    assert_eq!(
        up.iter().map(|e| e.1).collect::<Vec<_>>(),
        [midi(65), midi(69)]
    );
    let down = notes_of("lead.__update([c4].transpose(-12st))", 1);
    assert_eq!(down[0].1, midi(48));
    let detuned = notes_of("lead.__update([a4].transpose(0.5st))", 1);
    let Value::Hz(hz) = detuned[0].1 else {
        panic!("a fractional interval gives a frequency")
    };
    assert!((hz - 440.0 * 2f64.powf(0.5 / 12.0)).abs() < 1e-9);

    let e = notes_of("lead.__update([c2].euclid(hits: 3, steps: 8))", 1);
    assert_eq!(
        e.iter().map(|e| e.0).collect::<Vec<_>>(),
        [r(0, 1), r(3, 8), r(3, 4)]
    );
    let rhythm = notes_of("lead.__update([c4 e4 g4].rhythm(grid[x.x. x.x.]))", 1);
    assert_eq!(rhythm.len(), 4);
    assert_eq!(rhythm[0].0, r(0, 1));
    let masked = notes_of("lead.__update([c4 e4 g4 b4].mask(grid[x.x.]))", 1);
    assert_eq!(
        masked.iter().map(|e| e.1).collect::<Vec<_>>(),
        [midi(60), midi(67)]
    );
    assert_eq!(
        notes_of("lead.__update([c4 e4].repeat_each(2))", 1).len(),
        4
    );
    let layered = notes_of(
        "lead.__update([c4].layer(after: 1/2, it.transpose(+12st)))",
        1,
    );
    assert_eq!(
        layered.iter().map(|e| e.1).collect::<Vec<_>>(),
        [midi(60), midi(72)]
    );
}

#[test]
fn thinning_is_seeded_by_the_source() {
    let src = "lead.__update([c4*16].thin(50%))";
    let a = notes_of(src, 8).len();
    assert_eq!(
        a,
        notes_of(src, 8).len(),
        "the same source thins the same way"
    );
    assert!((40..90).contains(&a), "about half of 128 survive: {a}");
    assert_eq!(notes_of("lead.__update([c4*4].thin(0%))", 1).len(), 4);
    assert!(notes_of("lead.__update([c4*4].thin(100%))", 1).is_empty());
}

#[test]
fn clips_have_a_length_and_lanes() {
    let p = ok(&stage(
        "\nclip riff {\n  length: 2bars\n  notes: [c2 ~ eb2 g1]\n  bright: [0.2 0.8]\n  gain: [0db -6db]\n}\nbass.__update(riff)",
    ));
    let Action::Update { clip, .. } = &p.performance[0].action else {
        panic!()
    };
    assert_eq!(clip.length, Rational::int(2));
    assert_eq!(clip.name, "riff");
    assert_eq!(clip.lanes.len(), 3);
    // a lane spans the whole clip: two steps over two bars
    assert_eq!(
        events(clip.lane("bright").unwrap(), 2),
        [(r(0, 1), Value::Num(0.2)), (r(1, 1), Value::Num(0.8))]
    );
    // length in beats is a share of a bar
    assert_eq!(
        played("clip c { length: 6beats\nnotes: [c4] }\nlead.__update(c)").length,
        r(3, 2)
    );
}

#[test]
fn operations_apply_to_whole_clips() {
    let sped = played(
        "clip c { length: 2bars\nnotes: [c4 e4]\nbright: [0.1 0.9] }\nlead.__update(c.fast(2))",
    );
    assert_eq!(sped.length, Rational::ONE, "fast shortens the clip");
    assert_eq!(
        events(sped.lane("notes").unwrap(), 1).len(),
        2,
        "the lanes are squeezed into the shorter clip"
    );
    assert_eq!(
        events(sped.lane("bright").unwrap(), 1).len(),
        2,
        "every lane speeds up together"
    );

    let stretched = played(
        "clip c { length: 2bars\nnotes: [c4 e4]\nbright: [0.1 0.9] }\nlead.__update(c.over(4bars))",
    );
    assert_eq!(stretched.length, Rational::int(4));
    assert_eq!(
        events(stretched.lane("notes").unwrap(), 4).len(),
        2,
        "over stretches the lanes with the clip"
    );

    let up = played("clip c { notes: [c4]\nbright: [0.3] }\nlead.__update(c.transpose(+12st))");
    assert_eq!(events(up.lane("notes").unwrap(), 1)[0].1, midi(72));
    assert_eq!(
        events(up.lane("bright").unwrap(), 1)[0].1,
        Value::Num(0.3),
        "only the notes move"
    );
}

#[test]
fn a_scene_starts_stops_and_mutes_each_of_its_clips() {
    let p = ok(&stage(
        "\nclip hat { instrument: pluck, notes: [c2 g2] }\nscene intro { bass, hat }\nintro.start\nintro.mute\nintro.stop",
    ));
    let kinds: Vec<_> = p
        .performance
        .iter()
        .map(|s| match &s.action {
            Action::Start { node, clip } => format!("start {node} {}", clip.name),
            Action::Mute { node } => format!("mute {node}"),
            Action::Stop { node } => format!("stop {node}"),
            _ => "other".into(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "start 2 bass",
            "start 4 hat",
            "mute 2",
            "mute 4",
            "stop 2",
            "stop 4"
        ]
    );
}

#[test]
fn a_scene_lists_clips_and_nothing_else() {
    assert_eq!(
        msg(&stage("scene s { lead: [c4] }")),
        "a scene is a list of clips, so `lead` has nothing after it"
    );
    assert_eq!(
        msg(&stage("scene s { lead, lead }")),
        "clip `lead` appears twice in this scene"
    );
}

#[test]
fn removed_forms_say_what_to_write_instead() {
    assert!(msg(&stage("lead.play([c4])")).starts_with("there is no `play`"));
    assert!(
        msg(&stage(
            "clip a { instrument: pluck }\na.start then a after 4bars"
        ))
        .starts_with("`then` is gone")
    );
    assert!(
        msg(&stage("at bar 5 lead.mute")).starts_with("a command can't be placed at a bar number")
    );
}

#[test]
fn a_clip_can_be_made_from_another() {
    let p = ok(
        "instr tone(freq: hz) { osc(sine, freq) }\nclip bass { instrument: tone(freq: 100hz)\nnotes: [c2] }\nclip higher = bass.transpose(+12st)\nclip higher_still = higher.transpose(+12st)\nhigher_still.start",
    );
    let names: Vec<_> = p.nodes.iter().skip(1).map(|n| n.name.as_str()).collect();
    assert_eq!(
        names,
        ["bass", "higher", "higher_still"],
        "each has the instrument of the first"
    );
    let Action::Start { clip, .. } = &p.performance[0].action else {
        panic!("a start")
    };
    assert_eq!(clip.name, "higher_still");
    assert_eq!(events(clip.lane("notes").unwrap(), 1)[0].1, midi(60));
    assert!(
        msg("clip a = 5").starts_with("a clip is made from another clip"),
        "{}",
        msg("clip a = 5")
    );
}

#[test]
fn naming_an_instrument_to_start_it_points_to_a_clip() {
    let d = first_error("instr pluck(freq: hz) { osc(saw, freq) }\npluck.start");
    assert_eq!(d.message, "`pluck` is an instrument, not a clip");
    let d = first_error("riff = [c4]\nriff.start");
    assert!(
        d.message.contains("is a pattern, not a clip"),
        "{}",
        d.message
    );
    let d = first_error("clip loose { notes: [c4] }\nloose.start");
    assert!(d.message.contains("has no instrument"), "{}", d.message);
}

#[test]
fn commands_carry_times_and_quantization() {
    let p = ok(&stage(
        "\ntempo 120bpm\nclip riff { notes: [c2] }\nbass.__update(riff)\nat 4bars lead.__update(riff)\nat 18beats bass.stop\nat 2beats lead.mute @ next bar\nlead.unmute @ in 2bars\nbass.solo @ 4bars\nhush @ now\npanic\nunsolo\nat 8beats bass.unsolo",
    ));
    let acts = &p.performance;
    assert_eq!(acts[1].at, Time::Bars(4.0));
    assert_eq!(
        acts[2].at,
        Time::Beats(18.0),
        "18 beats in 4/4 is 4bars and 2beats in"
    );
    assert_eq!(acts[3].at, Time::Beats(2.0));
    let q = acts[3].quantize.unwrap();
    assert_eq!(
        (q.relation, q.count, q.unit),
        (QuantizeRelation::Next, 1.0, QuantizeUnit::Bar)
    );
    let q = acts[4].quantize.unwrap();
    assert_eq!(
        (q.relation, q.count, q.unit),
        (QuantizeRelation::In, 2.0, QuantizeUnit::Bar)
    );
    let q = acts[5].quantize.unwrap();
    assert_eq!(
        (q.relation, q.count, q.unit),
        (QuantizeRelation::OnGrid, 4.0, QuantizeUnit::Bar)
    );
    assert_eq!(acts[6].quantize.unwrap().unit, QuantizeUnit::Now);
    assert!(matches!(acts[7].action, Action::Panic));
    assert!(matches!(acts[8].action, Action::Unsolo { node: None }));
    assert!(matches!(acts[9].action, Action::Unsolo { node: Some(2) }));
    assert_eq!(acts[9].at, Time::Beats(8.0));
    assert!(acts[0].quantize.is_none());
}

#[test]
fn meter_changes_what_a_bar_is() {
    let p = ok(&stage(
        "meter 3/4\nclip c { notes: [c4] }\nat 2bars lead.__update(c)\nat 1bar lead.mute",
    ));
    assert_eq!(p.tempo.beats_per_bar, 3.0);
    // a bar of 3/4 at 120bpm is a second and a half
    assert_eq!(p.performance[0].at.to_seconds(p.tempo), 3.0);
    let waltz = ok(&stage(
        "meter 7/8\nclip c { notes: [c4] }\nat 1bar lead.__update(c)",
    ));
    assert_eq!(waltz.performance[0].at.to_seconds(waltz.tempo), 1.75);
    assert_eq!(msg("meter 3"), "meter is a fraction of whole numbers");
    assert_eq!(msg("meter 3/4\nmeter 4/4"), "meter is set more than once");
    // a beat is a quarter of a bar in 4/4 and a third of one in 3/4
    let pat = played("meter 3/4\nlead.__update([c4 e4].shift(1beat))");
    assert_eq!(events(pat.lane("notes").unwrap(), 1)[0].0, r(1, 3));
}

#[test]
fn instruments_accept_a_gain_control() {
    let p = ok(&stage(""));
    let gain = p.instruments[0]
        .params
        .iter()
        .find(|q| q.name == "gain")
        .unwrap();
    assert_eq!((gain.unit, gain.default), (Unit::Db, Some(1.0)));
    assert!(p.notes.is_empty());
    let note = ok(&stage("lead(freq: c4, gain: -6db) for 1beat"));
    assert_eq!(note.notes[0].args["gain"], Value::Db(-6.0));
    // an instrument can take the note's gain over, and use it as it likes
    let p = ok("instr a(freq: hz, gain: db = 0db) { osc(saw, freq).lpf(freq * 4).gain(gain) }");
    assert_eq!(
        p.instruments[0]
            .params
            .iter()
            .filter(|q| q.name == "gain")
            .count(),
        1
    );
    assert_eq!(
        msg("instr a(gain: hz) { osc(sine, 100hz) }"),
        "an instrument's own `gain` is in db, like the one every note has"
    );
}

#[test]
fn clips_are_checked_against_the_node_they_play_on() {
    let d = first_error(&stage(
        "clip c { notes: [c4]\ntone: [1] }\nlead.__update(c)",
    ));
    assert_eq!(
        d.message,
        "clip `c` has a lane `tone`, but `pluck` has no such parameter"
    );
    let d = first_error(&stage(
        "clip c { notes: [c4]\nbrigth: [0.1] }\nlead.__update(c)",
    ));
    assert_eq!(d.help.as_deref(), Some("did you mean `bright`?"));
    ok(&stage(
        "clip c { notes: [c4]\nbright: [0.1]\ngain: [-3db]\ndur: [1/4beat]\nnudge: [10ms] }\nlead.__update(c)",
    ));
    ok(&stage(
        "clip c { notes: [c4]\npan: [-30deg 30deg] }\nlead.__update(c)",
    ));
    assert_eq!(
        msg(&stage(
            "clip c { notes: [c4]\nto: [0deg] }\nlead.__update(c)"
        )),
        "the `to` lane isn't supported yet"
    );

    let no_pitch =
        "instr drone(amp: db) { osc(sine, 100hz) }\nclip t { instrument: drone }\nt.__update([c4])";
    assert_eq!(
        msg(no_pitch),
        "`drone` has no `freq` parameter for the notes to set"
    );
    let silent = "instr a(freq: hz) { osc(sine, freq) }\nfx room { out = it }\nroom.__update([c4])";
    assert_eq!(
        msg(silent),
        "node `room` has no instrument to play a clip on"
    );
}

#[test]
fn performance_errors_are_clear_and_located() {
    // unknown names, with suggestions
    let d = first_error(&stage("leed.__update([c4])"));
    assert_eq!(d.message, "no clip named `leed`");
    assert_eq!(d.help.as_deref(), Some("did you mean `lead`?"));
    let d = first_error(&stage("riff = [c4]\nlead.__update(rif)"));
    assert_eq!(d.message, "`rif` is not defined");
    assert_eq!(d.help.as_deref(), Some("did you mean `riff`?"));
    assert_eq!(
        first_error(&stage("nothing.start")).message,
        "no scene or clip named `nothing`"
    );
    assert_eq!(
        first_error(&stage("lead.__update([c4].set(0.9))")).message,
        "`set` takes a lane and its value, as in `set(bright: 0.9)`"
    );
    assert_eq!(
        first_error(&stage("lead.__update([c4].set(length: 2))")).message,
        "a clip's length can't be set; use `over(…)`"
    );
    let d = first_error(&stage("lead.__update([c4].set(wobble: 1))"));
    assert!(d.message.contains("wobble"), "{}", d.message);
    let d = first_error(&stage("lead.__update([c4].fsat(2))"));
    assert_eq!(d.message, "unknown pattern operation `fsat`");
    assert_eq!(d.help.as_deref(), Some("did you mean `fast`?"));

    // bad atoms are pointed at, even inside brackets and alternations
    let src = stage("lead.__update([c4 [e4 kick] g4])");
    let d = first_error(&src);
    assert!(
        d.message
            .contains("`kick` isn't a note, a value, or a sample in a kit"),
        "{}",
        d.message
    );
    assert_eq!(&src[d.span.start..d.span.start + 4], "kick");
    let d = first_error(&stage("lead.__update([c4 e4)"));
    assert_eq!(d.message, "this `[` is never closed");

    // operation arguments
    assert_eq!(
        msg(&stage("lead.__update([c4].fast(0))")),
        "`fast` must be above zero"
    );
    assert_eq!(
        msg(&stage("lead.__update([c4].fast)")),
        "`fast` needs an argument `by`"
    );
    assert_eq!(
        msg(&stage("lead.__update([c4].transpose(5))")),
        "`transpose` takes an interval in semitones"
    );
    assert_eq!(
        msg(&stage("lead.__update([c4].thin(150%))")),
        "`thin` takes an amount from 0% to 100%"
    );
    assert_eq!(
        msg(&stage("lead.__update([c4].shift(1sec))")),
        "expected a length in beats or bars, found `sec`"
    );
    assert_eq!(
        msg(&stage("lead.__update([c4].every(2, it.nonsense))")),
        "unknown pattern operation `nonsense`"
    );
    assert_eq!(
        msg(&stage("lead.__update(it)")),
        "`it` is only available inside an argument, as in `every(4, it.reverse)`"
    );
    assert_eq!(
        msg(&stage("lead.__update(fast(2))")),
        "`fast` works on a pattern: write it after one, as in `riff.fast`"
    );

    // kinds of value
    assert_eq!(
        msg(&stage("g = grid[x.x.]\nlead.__update(g)")),
        "a grid has no pitches to play"
    );
    assert_eq!(
        msg(&stage("scene s { lead }\nlead.__update(s)")),
        "a scene is launched, not played"
    );
    assert_eq!(msg(&stage("scene s { nobody }")), "no clip named `nobody`");
    assert_eq!(
        msg(&stage("clip c { notes: env[0 1sec 1] }")),
        "the `notes` lane holds notes, not an envelope"
    );
    assert_eq!(
        msg(&stage("clip c { length: 0bars }")),
        "a clip needs a length above zero"
    );
    assert_eq!(
        msg(&stage("clip c { notes: [c4]\nnotes: [e4] }")),
        "lane `notes` is given twice"
    );
    assert_eq!(
        msg(&stage("g = grid[x.q]")),
        "`q` isn't a grid step: use x, X, o or ."
    );

    // names
    assert_eq!(
        msg(&stage("riff = [c4]\nriff = [e4]")),
        "`riff` is defined twice"
    );
    assert_eq!(msg(&stage("lead = [c4]")), "`lead` is defined twice");
    assert_eq!(msg(&stage("it = [c4]")), "`it` is a reserved word");
    assert_eq!(
        msg(&stage("at bar 0 lead(freq: c4) for 1beat")),
        "bars and beats are numbers counting from 1"
    );
    // a file starts at its tempo; how a live session gets there is not the compiler's concern
    assert_eq!(
        ok(&stage("tempo 90bpm @ next bar over 2bars")).tempo.bpm,
        90.0
    );
}

#[test]
fn reports_errors_from_several_instruments_without_cascading() {
    let errs =
        errors("\ninstr a() { osc(saw, 440) }\ninstr b() { osc(saw, 440) }\na() for 1beat\n");
    assert_eq!(
        errs.len(),
        2,
        "one per broken instrument, none for the note: {errs:?}"
    );
}

#[test]
fn a_node_parameter_is_a_control_the_node_can_read() {
    let src = "instr p(freq: hz) { osc(saw, freq) }\nbus hall_audio\nfx hall_fx(tone: hz = 2khz, drive: 0..1 = 0.2) {out = hall_audio.lpf(cutoff: tone).gain(drive)}\nclip hall { instrument: p }\n";
    let p = ok(src);
    let names: Vec<&str> = p.controls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["hall_fx::tone", "hall_fx::drive"]);
    assert_eq!(p.controls[0].initial, 2000.0);
    assert_eq!(
        msg("fx t(tone: hz) { out = it }"),
        "fx parameter `tone` needs a starting value"
    );
    assert_eq!(
        help("fx t(tone: hz) { out = it }").as_deref(),
        Some("for example `fx bass(tone: hz = 2khz)`")
    );
}

#[test]
fn clips_transform_their_own_notes() {
    let p = ok(&format!(
        "{STAGE}\nclip lead2 {{ instrument: pluck\n notes: [c4 e4 g4].reverse }}\nlead2.start"
    ));
    let Action::Start { clip, .. } = &p.performance[0].action else {
        panic!()
    };
    assert_eq!(events(clip.lane("notes").unwrap(), 1)[0].1, midi(67));
}

#[test]
fn clips_select_instrument_defaults() {
    let one = ok(
        "instr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq) }\nclip bass { instrument: pluck(bright: 0.2) }\nbass.__update([c2])",
    );
    assert_eq!(one.nodes[1].name, "bass");
    assert_eq!(one.nodes[1].instrument, Some(0));
    assert_eq!(one.nodes[1].defaults.len(), 1);

    // naming something that is not a clip says so
    assert_eq!(
        msg("instr pluck(freq: hz) { osc(saw, freq) }\nnope.__update([c2])"),
        "no clip named `nope`"
    );
    // An explicit clip selects an instrument too.
    ok(
        "instr pluck(freq: hz) { osc(saw, freq) }\nclip bass { instrument: pluck }\nbass.__update([c2])",
    );
}

#[test]
fn a_scene_starts_the_clips_it_names_and_nothing_else() {
    let p = ok(
        "instr tone(freq: hz) { osc(sine, freq) }\nclip bass { instrument: tone\nnotes: [c2] }\nclip lead { instrument: tone\nnotes: [c4] }\nbass.start\nscene chorus { lead }\nchorus.start",
    );
    assert_eq!(p.performance.len(), 2, "bass is left alone");
    assert!(matches!(
        &p.performance[0].action,
        Action::Start { node: 1, .. }
    ));
    let Action::Start { node: 2, clip } = &p.performance[1].action else {
        panic!("scene starts lead")
    };
    assert_eq!(events(clip.lane("notes").unwrap(), 1)[0].1, midi(60));
}

#[test]
fn non_finite_note_times_are_rejected() {
    let d = first_error("instr tone(freq: hz) { osc(sine, freq) }\ntone(freq: c4) for 1/0beat");
    assert_eq!(d.message, "a time must be finite");
}

#[test]
fn an_arrangement_can_hold_timed_lines() {
    let p = ok(
        "instr tone(freq: hz) { osc(sine, freq) }\nclip a { instrument: tone\nnotes: [c4] }\nclip b { instrument: tone\nnotes: [e4] }\nscene one { a }\nscene two { b }\narrangement song {\n  at +0bars one.start\n  at +4bars one.stop\n  at +4bars two.start\n}\nsong.start\nsong.stop",
    );
    let song = &p.arrangements[0];
    assert_eq!(song.actions.len(), 3);
    assert_eq!(song.actions[1].at, Time::Bars(4.0));
    // starting it puts its lines where it was started; stopping it drops what is still to come
    assert_eq!(p.performance.len(), 4);
    assert!(matches!(
        &p.performance[0].action,
        Action::Start { node: 1, .. }
    ));
    assert!(
        matches!(&p.performance[3].action, Action::Cancel { arrangement } if arrangement == "song")
    );
    // a tempo of its own can only be followed by a render of the arrangement
    let d = first_error(
        "instr tone(freq: hz) { osc(sine, freq) }\nclip a { instrument: tone\nnotes: [c4] }\nscene one { a }\narrangement song { sections: [one.over(2bars)]\n tempo: [90bpm] }\nsong.start",
    );
    assert!(d.message.contains("sets its own tempo"), "{}", d.message);
    assert_eq!(
        msg("instr tone(freq: hz) { osc(sine, freq) }\narrangement song {\n}"),
        "arrangement `song` has nothing in it"
    );
    assert!(
        msg("clip a { }\narrangement song {\n  at bar 3 hush\n}")
            .starts_with("expected a command placed"),
        "{}",
        msg("clip a { }\narrangement song {\n  at bar 3 hush\n}")
    );
}
