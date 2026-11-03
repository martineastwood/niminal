//! Tests for compiling whole programs.

use super::*;

fn ok(src: &str) -> Program {
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

const SPEC_OPCODES: &str = "\nfn one_pole(x, cutoff: hz) {\n  state y = 0.0\n  a = exp(-2 * pi * cutoff / sample_rate)\n  y = x * (1 - a) + y * a\n  y\n}\n\nfn drive(x, amount: 0..1 = 0.5) {\n  (x * (1 + amount * 9)).tanh\n}\n\nfn my_dc_block(x) {\n  state x1 = 0.0\n  state y1 = 0.0\n  y = x - x1 + 0.995 * y1\n  x1 = x\n  y1 = y\n  y\n}\n\nfn fold(x, amount: 0..1 = 0.5) {\n  (x * (1 + amount * 8)).sin\n}\n";

#[test]
fn the_specs_example_functions_compile_and_can_be_used() {
    ok(&format!(
        "{SPEC_OPCODES}\ninstr a(freq: hz) {{ osc(saw, freq).drive(amount: 0.3).one_pole(cutoff: 800hz) }}\ninstr b(freq: hz) {{ osc(sine, freq).fold(amount: env[0 2sec 1 2sec 0]).my_dc_block }}\ninstr c(freq: hz) {{ drive(osc(saw, freq)) }}"
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
        msg(&state("x.pan(angle: 10deg) + s")),
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
    let errs = errors("\nfn bad(x) { x + undefined_thing }\ninstr a(f: hz) { osc(saw, f).bad }\n");
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
fn settings_are_written_in_the_body_and_with_is_gone() {
    ok("mono a(freq: hz) { legato = true\nglide = 50ms\n osc(saw, freq) }");
    assert!(
        msg("instr a(f: hz) { quantize = 1bar\n osc(saw, f) }").contains("belong to the client")
    );
    assert_eq!(
        msg("instr a(f: hz) with(tail: 2sec) { osc(saw, f) }"),
        "`with` is gone"
    );
    assert_eq!(
        msg("sample kick = \"kick.wav\""),
        "`sample` is no longer a keyword"
    );
}

#[test]
fn samples_are_bindings_with_chained_settings() {
    assert_eq!(msg("kick = \"kick.wav\""), "expected a sample or a kit");
    assert_eq!(msg("drums = kit(organ)"), "`kit` takes a folder in quotes");
    assert_eq!(
        msg("kick = sample(\"kick.wav\").root(c4, 2)"),
        "`root` takes one value, as in `.root(...)`"
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

#[test]
fn routing_a_node_to_a_bus() {
    let p = ok(
        "\nbus send\nnode a { instrument = i\n output = send }\nfx b { out = send }\ninstr i(f: hz) { osc(sine, f) }",
    );
    assert_eq!(p.nodes[1].def.route, niminal_engine::Route::Bus(0));
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
        "`it` is only available inside a function given to `map`"
    );
    assert_eq!(
        msg("instr a(f: hz) { in = 1 }"),
        "`in` is an input and can't be assigned"
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
    // an instrument is only reached by calling it, so a parameter or local of its name is its own
    ok(&with("instr a(other: hz) { osc(saw, other) }"));
    ok(&with("instr a(f: hz) { other = osc(saw, f)\nother * 0.5 }"));
    ok(&with("instr a(f: hz) { other = other(f: f)\nother * 0.5 }"));
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
fn a_node_shares_the_name_of_its_instrument_whichever_comes_first() {
    let instr = "instr bass(freq: hz) { osc(sine, freq) }";
    let node = "node bass { instrument = bass }";
    ok(&format!("{instr}\n{node}"));
    ok(&format!("{node}\n{instr}"));
    // a node that plays something else does not share it
    assert!(
        msg(&format!("{instr}\ninstr other(freq: hz) {{ osc(saw, freq) }}\nnode bass {{ instrument = other }}"))
            .contains("`bass` is already the name of an instrument")
    );
}

#[test]
fn collections_and_fx_declarations_have_distinct_roles() {
    let base = "instr tone(freq: hz = 440hz) { osc(sine, freq) }\n";
    ok(&format!("{base}node lead {{ instrument = tone }}"));
    ok("fx room(cut: hz = 2khz) { out = in.lpf(cutoff: cut) }");
    assert!(
        msg(&format!("{base}fx room {{ instrument = tone }}"))
            .contains("cannot select an instrument")
    );
    assert!(
        msg(&format!(
            "{base}node lead {{ instrument = tone\n out = in.lpf(cutoff: 2khz) }}"
        ))
        .contains("put DSP in an fx")
    );
    assert!(
        msg(&format!(
            "{base}node lead(cut: hz = 2khz) {{ instrument = tone }}"
        ))
        .contains("no DSP parameters")
    );
    assert!(compile("track lead { instrument = tone }").is_err());
    assert!(compile("bus room { out = in }").is_err());
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
    let errs = errors("\ninstr bad(f: hz) { osc(saw, 440) }\nnode t { instrument = bad }\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
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
        "config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f).pan(angle: 20deg, spread: 0.3) }",
    );
    assert_eq!(p.instruments[0].graph.channels(), 6);
    ok(
        "config { channels: stereo }\ninstr a(f: hz, az: deg = 10deg) { osc(saw, f).pan(angle: az) }",
    );

    let d =
        first_error("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(angle: 20) }");
    assert_eq!(d.message, "`angle` expects an angle, found a plain number");
    assert_eq!(d.help.as_deref(), Some("did you mean `20deg`?"));
    assert_eq!(
        msg("config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)].pan }"),
        "`pan` takes a one-channel signal, but `x` has 2"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f).pan(angle: 5hz) }"),
        "`angle` expects an angle, found hz"
    );
    assert_eq!(
        msg("instr a(f: hz) { osc(saw, f) * 20deg }"),
        "an instrument outputs a plain signal, but this is an angle"
    );
}

#[test]
fn angles_follow_the_unit_rules() {
    ok("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(angle: 20deg * 2 - 5deg) }");
    assert_eq!(
        msg("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(angle: 20deg + 1hz) }"),
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
    // 5.1 bus read by a stereo master node folds down
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
        "\nconfig { channels: stereo }\nbus space: stereo\nbus lfe: mono\ninstr a(f: hz) { out = osc(saw, f).pan(angle: -20deg)\nspace += out\nlfe += out }\nfx room { out = space.reverb(room: 0.8) }\nfx sub { out = lfe.lpf(cutoff: 100hz) }",
    );
    assert_eq!(p.bus_layouts, [Layout::Stereo, Layout::Mono]);
    assert_eq!(p.nodes[1].def.inputs.len(), 2, "a stereo bus is two inputs");
    assert_eq!(p.nodes[2].def.inputs.len(), 1);
    // the sub node works in mono but delivers stereo to the master
    assert_eq!(p.nodes[2].def.chain.as_ref().unwrap().channels(), 2);
    // routing to a bus delivers in that bus's layout
    let routed = ok(
        "\nconfig { channels: stereo }\nbus mono_bus: mono\nnode t { instrument = i\noutput = mono_bus }\nfx u { out = mono_bus }\ninstr i(f: hz) { osc(saw, f).pan }",
    );
    assert_eq!(routed.nodes[1].def.chain.as_ref().unwrap().channels(), 1);
}

// ---- the performance layer ------------------------------------------------

/// (start in cycles, value) of every event beginning in the first `cycles` cycles.

#[test]
fn osc_out_sends_a_nodes_signal() {
    let node = |body: &str| {
        format!(
            "instr pluck(freq: hz) {{ osc(saw, freq) }}\nbus lead_audio\nfx lead_fx {{{body}}}\nnode lead {{ instrument = pluck\noutput = lead_audio}}\n"
        )
    };
    let p = ok(&node("out = in.osc_out(\"/vj/level\", port: 9000)"));
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
        "out = in.osc_out(\"/a\", host: \"10.0.0.2\", port: 9001, rate: 5hz).osc_out(\"/b\", port: 9001)",
    ));
    assert_eq!(
        p.taps
            .iter()
            .map(|t| (t.address.as_str(), t.host.as_str(), t.rate))
            .collect::<Vec<_>>(),
        [("/a", "10.0.0.2", 5.0), ("/b", "127.0.0.1", 30.0)]
    );
    assert_eq!(
        msg(&node("out = in.osc_out(\"vj\", port: 9000)")),
        "an OSC address starts with `/`"
    );
    assert_eq!(
        msg(&node("out = in.osc_out(\"/a\")")),
        "`osc_out` needs a `port:`"
    );
    assert_eq!(
        msg(&node("out = in.osc_out(port: 9000)")),
        "`osc_out` needs an address"
    );
    assert_eq!(
        msg(&node("out = in.osc_out(\"/a\", port: 0)")),
        "a port is a whole number from 1 to 65535"
    );
    assert_eq!(
        msg(&node("out = in.osc_out(\"/a\", port: 9000, rate: 900hz)")),
        "`rate` is from 0.5hz to 200hz"
    );
    assert_eq!(
        msg(&node("out = in.osc_out(\"/a\", prot: 9000)")),
        "`osc_out` has no argument named `prot`"
    );
    assert_eq!(
        msg(&node("out = in.osc_out(5, port: 9000)")),
        "an OSC address is text in quotes"
    );
    ok("instr a(f: hz) { osc(saw, f).osc_out(\"/a\", port: 9000) }");
    assert_eq!(
        msg("fn a(x) { x.osc_out(\"/a\", port: 9000) }"),
        "`osc_out` can't be used inside a function"
    );
}

#[test]
fn mixer_lines_are_checked() {
    let collection = |body: &str| {
        format!(
            "instr pluck(freq: hz) {{ osc(saw, freq) }}\nnode lead {{ instrument = pluck\n{body}\n}}"
        )
    };
    ok(&collection("gain = -3db\n pan = 20deg"));
    assert!(msg(&collection("out = in.lpf(cutoff: 2khz)")).contains("put DSP in an fx"));
    assert_eq!(
        msg(&collection("gain = -3db\n gain = -1db")),
        "a node has only one `gain` line"
    );
    assert!(msg(&collection("gain = 20deg")).contains("expects"));
    assert!(msg(&collection("pan = 20")).contains("angle"));
}

#[test]
fn reports_errors_from_several_instruments_without_cascading() {
    let errs = errors("\ninstr a() { osc(saw, 440) }\ninstr b() { osc(saw, 440) }\n");
    assert_eq!(
        errs.len(),
        2,
        "one per broken instrument, none for the note: {errs:?}"
    );
}

#[test]
fn a_node_parameter_is_a_control_the_node_can_read() {
    let src = "instr p(freq: hz) { osc(saw, freq) }\nbus hall_audio\nfx hall_fx(tone: hz = 2khz, drive: 0..1 = 0.2) {out = hall_audio.lpf(cutoff: tone).gain(drive)}\nnode hall {  instrument = p\noutput = hall_audio}\n";
    let p = ok(src);
    let names: Vec<&str> = p.controls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["hall_fx::tone", "hall_fx::drive"]);
    assert_eq!(p.controls[0].initial, 2000.0);
    assert_eq!(
        msg("fx t(tone: hz) { out = in }"),
        "fx parameter `tone` needs a starting value"
    );
    assert_eq!(
        help("fx t(tone: hz) { out = in }").as_deref(),
        Some("for example `fx bass(tone: hz = 2khz)`")
    );
}
#[test]
fn named_kit_selection_survives_the_wire_and_keeps_voice_rules() {
    let p = ok(r#"
        instr bd(freq: hz = 55hz) { max = 2
            out = osc(sine, freq) * perc(decay: 80ms)
        }
        instr hh(freq: hz = 8khz) { choke = hats
            out = noise() * perc(decay: 20ms)
        }
        drums = kit { kick: bd(freq: 60hz), hat: hh }
        node beat { instrument = drums }
    "#);
    let id = p.sample_names.iter().position(|n| n == "kick").unwrap() as u32;
    let mut event = niminal_score::Event {
        target: "beat".into(),
        at: Time::Seconds(0.0),
        dur: Time::Seconds(0.1),
        args: BTreeMap::from([("sample".into(), Value::Sample(id))]),
    };
    let plan = p.plan(&event).unwrap();
    assert_eq!(p.instruments[plan.instrument].name, "bd");
    assert_eq!(plan.rule.as_ref().unwrap().max, 2);
    let freq = p.instruments[plan.instrument].param("freq").unwrap().0;
    assert!(plan.params.contains(&(freq, 60.0)));
    event.args = p.dsp_args(&event).unwrap();
    assert_eq!(event.args["sample"], Value::Num(0.0));
    assert_eq!(p.plan(&event).unwrap(), plan);
    event.args.insert("sample".into(), Value::Num(1.0));
    let hat = p.plan(&event).unwrap();
    assert_eq!(p.instruments[hat.instrument].name, "hh");
    assert!(hat.choke.is_some());
    event.args.insert("sample".into(), Value::Num(2.0));
    assert!(
        p.plan(&event)
            .unwrap_err()
            .to_string()
            .contains("out of range")
    );
}
