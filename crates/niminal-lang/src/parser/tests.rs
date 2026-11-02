//! Parser tests.

use super::parse_composition as parse;
use super::*;

fn parse_ok(src: &str) -> Vec<Item> {
    parse(src).unwrap_or_else(|e| panic!("{}", e.render("t", src)))
}

fn err(src: &str) -> String {
    parse(src).unwrap_err().message
}

fn only_instr(src: &str) -> InstrDef {
    match parse_ok(src).remove(0) {
        Item::Instr(i) => i,
        other => panic!("{other:?}"),
    }
}

const SAW_LEAD: &str = "\ninstr saw_lead(freq: hz, amp: db = -6db) {\n  level = env[0 5ms 1 200ms 0.6 | 300ms 0]\n  osc(saw, freq)\n    .lpf(cutoff: 2khz, res: 0.2)\n    .gain(amp) * level\n}\n\nsaw_lead(freq: a3) for 1beat\n";

#[test]
fn fx_is_reserved_and_parses_as_a_persistent_declaration() {
    let items = parse_ok("fx room { out = in }");
    assert!(matches!(&items[0], Item::Fx(_)));
    assert!(err("fx = 1").contains("fx name"));
    assert!(crate::ide::RESERVED_WORDS.contains(&"fx"));
    assert!(!crate::ide::RESERVED_WORDS.contains(&"processor"));
    assert!(parse("const processor = 1").is_ok());
}

#[test]
fn parses_the_basic_synth() {
    let items = parse_ok(SAW_LEAD);
    assert_eq!(items.len(), 2);
    let Item::Instr(i) = &items[0] else { panic!() };
    assert_eq!(i.name.name, "saw_lead");
    assert_eq!(i.params.len(), 2);
    assert!(i.params[0].default.is_none());
    assert!(i.params[0].ty.is_some());
    assert!(i.params[1].default.is_some());
    assert_eq!(i.body.len(), 2);
    let Item::Note(n) = &items[1] else { panic!() };
    assert_eq!(n.target.name, "saw_lead");
    assert_eq!(n.args[0].name.as_ref().unwrap().name, "freq");
}

#[test]
fn ufcs_desugars_to_calls_with_a_receiver() {
    let i = only_instr("instr a(f: hz) { osc(saw, f).lpf(cutoff: 1khz) }");
    let Stmt::Expr(e) = &i.body[0] else { panic!() };
    let ExprKind::Call { name, args } = &e.kind else {
        panic!()
    };
    assert_eq!(name.name, "lpf");
    assert!(args[0].receiver);
    assert!(matches!(&args[0].value.kind, ExprKind::Call { name, .. } if name.name == "osc"));
    assert_eq!(args[1].name.as_ref().unwrap().name, "cutoff");
}

#[test]
fn paren_less_method_calls_work() {
    let i = only_instr("instr a(f: hz) { f.neg }");
    let Stmt::Expr(e) = &i.body[0] else { panic!() };
    let ExprKind::Call { args, .. } = &e.kind else {
        panic!()
    };
    assert_eq!(args.len(), 1);
}

#[test]
fn operator_precedence() {
    let i = only_instr("instr a(f: hz) { 1 + 2 * 3 - 4 }");
    let Stmt::Expr(e) = &i.body[0] else { panic!() };
    // (1 + (2*3)) - 4
    let ExprKind::Binary {
        op: BinOp::Sub,
        lhs,
        ..
    } = &e.kind
    else {
        panic!("{e:?}")
    };
    let ExprKind::Binary {
        op: BinOp::Add,
        rhs,
        ..
    } = &lhs.kind
    else {
        panic!()
    };
    assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Mul, .. }));
}

#[test]
fn range_typed_parameters() {
    let i = only_instr("instr a(bright: 0..1 = 0.5, cut: 20hz..20khz) { bright }");
    assert!(matches!(i.params[0].ty, Some(TypeSpec::Range { .. })));
    assert!(matches!(i.params[1].ty, Some(TypeSpec::Range { .. })));
}

#[test]
fn functions_with_state_untyped_parameters_and_results() {
    let items = parse_ok(
        "\nfn one_pole(x, cutoff: hz) {\n  state y = 0.0\n  a = exp(-2 * pi * cutoff / sample_rate)\n  y = x * (1 - a) + y * a\n  y\n}",
    );
    let Item::Fn(f) = &items[0] else { panic!() };
    assert_eq!(f.name.name, "one_pole");
    assert!(f.params[0].ty.is_none());
    assert!(f.params[1].ty.is_some());
    assert!(matches!(f.body[0], Stmt::State { .. }));
    assert_eq!(f.body.len(), 4);
    assert_eq!(f.result, FnResult::Inferred);
    let Item::Fn(f) = &parse_ok("fn mtof(n: num) -> hz { 440hz }")[0] else {
        panic!()
    };
    assert!(matches!(&f.result, FnResult::Unit(u) if u.name == "hz"));
    let Item::Fn(f) = &parse_ok("fn s(x) -> (low, high) { low = x\n high = x }")[0] else {
        panic!()
    };
    assert!(matches!(&f.result, FnResult::Named(n) if n.len() == 2));
    assert_eq!(err("fn (x) {}"), "expected a function name, found `(`");
    assert_eq!(err("opcode o(x) { x }"), "`opcode` is now `fn`");
}

#[test]
fn rates_choices_and_list_types() {
    let Item::Fn(f) = &parse_ok(
        "fn f(a: init hz, b: control = 1, size: const int = 8, mode: (low | high) = low, taps: [num]) { a }",
    )[0] else {
        panic!()
    };
    assert_eq!(f.params[0].rate.map(|r| r.0), Some(Rate::Init));
    assert!(matches!(&f.params[0].ty, Some(TypeSpec::Unit(u)) if u.name == "hz"));
    assert_eq!(f.params[1].rate.map(|r| r.0), Some(Rate::Control));
    assert!(f.params[1].ty.is_none());
    assert_eq!(f.params[2].rate.map(|r| r.0), Some(Rate::Const));
    assert!(matches!(&f.params[3].ty, Some(TypeSpec::Choice(n)) if n.len() == 2));
    assert!(matches!(&f.params[4].ty, Some(TypeSpec::Array(..))));
}

#[test]
fn ranges_logic_and_remainder() {
    let i = only_instr("instr a() { x = 0..<8\n y = 1..8\n z = a > 1 and not b or c\n z }");
    assert!(matches!(
        &i.body[0],
        Stmt::Bind {
            value: Expr {
                kind: ExprKind::Range {
                    inclusive: false,
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        &i.body[1],
        Stmt::Bind {
            value: Expr {
                kind: ExprKind::Range {
                    inclusive: true,
                    ..
                },
                ..
            },
            ..
        }
    ));
    // `or` binds loosest
    assert!(matches!(
        &i.body[2],
        Stmt::Bind {
            value: Expr {
                kind: ExprKind::Logic {
                    op: LogicOp::Or,
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert_eq!(err("instr a() { 3 % 2 }"), "`%` only means percent");
}

#[test]
fn blocks_in_if_and_for_and_index_writes() {
    let i = only_instr(
        "instr a() {\n  if x > 0 {\n    y = 1\n    z = 2\n  }\n  for n in 1..8 { out += n }\n  buf[i + 1] = 3\n  v = if c { 1 } else if d { 2 } else { 3 }\n  v\n}",
    );
    assert!(
        matches!(&i.body[0], Stmt::Expr(Expr { kind: ExprKind::If { otherwise: None, then, .. }, .. }) if then.len() == 2)
    );
    assert!(matches!(
        &i.body[1],
        Stmt::Expr(Expr {
            kind: ExprKind::For { .. },
            ..
        })
    ));
    assert!(matches!(&i.body[2], Stmt::IndexAssign { .. }));
    assert!(
        matches!(&i.body[3], Stmt::Bind { value: Expr { kind: ExprKind::If { otherwise: Some(o), .. }, .. }, .. } if o.len() == 1)
    );
}

#[test]
fn constants_are_lists_not_patterns() {
    let items = parse_ok("const taps = [0.25, 0.5, 0.25]");
    assert!(
        matches!(&items[0], Item::Const { value: Expr { kind: ExprKind::Array(v), .. }, .. } if v.len() == 3)
    );
}

fn env_of(src: &str) -> EnvLit {
    let i = only_instr(&format!("instr a() {{ {src} }}"));
    match &i.body[0] {
        Stmt::Expr(Expr {
            kind: ExprKind::Env(e),
            ..
        }) => e.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn env_literal_with_sustain_and_curves() {
    let e = env_of("env[0 5ms 1 120ms.exp 0.4 | 600ms.curve(-4) 0]");
    assert_eq!(e.segments.len(), 3);
    assert_eq!(e.sustain_at, Some(2));
    assert_eq!(e.segments[1].curve, Some(CurveKind::Exp));
    assert_eq!(e.segments[2].curve, Some(CurveKind::Custom(-4.0)));
    assert_eq!(e.segments[2].level.lit().unwrap().value, 0.0);
}

#[test]
fn env_without_sustain_and_with_units_and_fractions() {
    let e = env_of("env[200hz 10ms 6khz 1/8beat.log 900hz]");
    assert_eq!(e.sustain_at, None);
    assert_eq!(e.segments[1].dur.lit().unwrap().value, 0.125);
    assert_eq!(e.start.lit().unwrap().unit.as_deref(), Some("hz"));
    let e = env_of("env[-18db 2sec -6db]");
    assert_eq!(e.start.lit().unwrap().value, -18.0);
}

#[test]
fn env_steps_from_names_and_expressions() {
    let e = env_of("env[0 attack 1 | (release * 2).exp 0]");
    assert!(
        matches!(&e.segments[0].dur, EnvValue::Expr(n) if matches!(n.kind, ExprKind::Name(_)))
    );
    assert!(
        matches!(&e.segments[1].dur, EnvValue::Expr(n) if matches!(n.kind, ExprKind::Binary { .. }))
    );
    assert_eq!(e.segments[1].curve, Some(CurveKind::Exp));
}

#[test]
fn env_can_loop_but_not_with_a_sustain_point() {
    assert!(!env_of("env[0 5ms 1 5ms 0]").looped);
    let e = env_of("env[0 5ms 1 5ms 0].loop");
    assert!(e.looped);
    assert_eq!(e.segments.len(), 2);
    assert!(
        err("instr a() { env[0 5ms 1 | 5ms 0].loop }").contains("can't have a sustain point")
    );
}

#[test]
fn if_then_else_is_a_one_line_value() {
    let items = parse_ok(
        "instr a() { x = if 1 > 0 then 4khz else if 2 > 3 then 1khz else 800hz\n y = if 1 > 0 then 1\n else 2 }",
    );
    let Item::Instr(i) = &items[0] else { panic!() };
    assert!(
        matches!(&i.body[0], Stmt::Bind { value: Expr { kind: ExprKind::If { otherwise: Some(o), .. }, .. }, .. } if o.len() == 1)
    );
    assert!(matches!(
        &i.body[1],
        Stmt::Bind {
            value: Expr {
                kind: ExprKind::If {
                    otherwise: Some(_),
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert!(err("instr a() { x = if 1 > 0 then 2 }").contains("needs both values"));
}

fn server_err(src: &str) -> String {
    super::parse(src).unwrap_err().message
}

#[test]
fn the_server_takes_definitions_and_refuses_composition_items() {
    let items = super::parse(
        "instr a(f: hz) { osc(sine, f) }\nnode n { instrument = a }\nnode m = a\nfx r { out = 1 }\nbus b\nctl x = 0.5",
    )
    .unwrap();
    assert_eq!(items.len(), 6);
    assert!(matches!(items[1], Item::Node(_)) && matches!(items[2], Item::Node(_)));
    for src in [
        "clip c { instrument: a }",
        "scene s { }",
        "tempo 90bpm",
        "meter 3/4",
        "a.start",
        "c3 d3",
        "x = 1",
        "ctl x = midi.cc(7).range(0..1)",
    ] {
        assert!(super::parse(src).is_err(), "the server accepted `{src}`");
    }
    assert!(server_err("clip c { instrument: a }").contains("a definition"));
}

#[test]
fn a_composition_refuses_node_which_is_the_servers() {
    assert!(err("node n { instrument = a }").contains("sound server"));
    assert!(parse("clip c { instrument: a }").is_ok());
}

#[test]
fn env_takes_a_note_field_as_a_duration() {
    let env = env_of("env[0 note.dur 1 | 5ms.exp 0]");
    assert_eq!(env.segments.len(), 2);
}

#[test]
fn env_alternation_errors() {
    assert_eq!(err("instr a() { env[0 1 1] }"), "expected a duration here");
    assert_eq!(
        err("instr a() { env[0 5ms 10ms 1] }"),
        "expected a level here, found a duration"
    );
    assert!(err("instr a() { env[0 5ms 1 | 5ms 0 | 5ms 0] }").contains("only one sustain"));
    assert!(err("instr a() { env[0 5ms] }").starts_with("expected a level"));
    assert_eq!(
        err("instr a() { env[0 5ms.wobble 1] }"),
        "unknown curve `wobble`"
    );
}

#[test]
fn note_statements() {
    let items = parse_ok("at 2beats lead(freq: c4, amp: -3db) for 1/2beat");
    let Item::Note(n) = &items[0] else { panic!() };
    assert!(matches!(n.at, Some(AtPos::Time(_))));
    assert_eq!(n.args.len(), 2);
}

#[test]
fn buses_nodes_and_add_assign() {
    let items = parse_ok(
        "\nbus space\nbus hall: stereo\nbus lead_audio\nfx lead_fx {out = lead_audio.lpf(cutoff: 1khz)\n  space += out.gain(-12db)}\nclip lead { instrument: pluck(bright: 0.6) }",
    );
    let Item::Bus(b) = &items[0] else { panic!() };
    assert_eq!((b.name.name.as_str(), b.layout.is_none()), ("space", true));
    let Item::Bus(b) = &items[1] else { panic!() };
    assert!(matches!(&b.layout.as_ref().unwrap().kind, ExprKind::Name(n) if n == "stereo"));
    let Item::Fx(t) = &items[3] else { panic!() };
    assert_eq!(t.name.name, "lead_fx");
    assert_eq!(t.body.len(), 2);
    assert!(matches!(&t.body[1], Stmt::AddAssign { name, .. } if name.name == "space"));
}

#[test]
fn channel_lists_destructuring_and_config() {
    let items = parse_ok(
        "\nconfig { channels: surround(5.1) }\ninstr a(f: hz) {\n  [l, r] = osc(saw, f).pan(angle: 20deg)\n  mid = (l + r) * 0.5\n  [mid + l, mid - r]\n}",
    );
    let Item::Config(c) = &items[0] else { panic!() };
    assert_eq!(c.entries[0].0.name, "channels");
    assert!(
        matches!(&c.entries[0].1.kind, ExprKind::Call { name, .. } if name.name == "surround")
    );
    let Item::Instr(i) = &items[1] else { panic!() };
    assert!(matches!(&i.body[0], Stmt::Destructure { names, .. } if names.len() == 2));
    let Stmt::Expr(e) = &i.body[2] else { panic!() };
    assert!(matches!(&e.kind, ExprKind::Array(items) if items.len() == 2));
    // a bracketed expression that is not followed by `=` is a value, not a pattern
    assert!(matches!(
        parse_ok("config { a: 1, b: 2 }")[0],
        Item::Config(_)
    ));
}

#[test]
fn tempo_statement() {
    assert!(matches!(parse_ok("tempo 120bpm")[0], Item::Tempo(_)));
}

#[test]
fn syntax_errors_say_what_was_expected() {
    assert_eq!(
        err("instr (a) {}"),
        "expected an instrument name, found `(`"
    );
    assert_eq!(
        err("lead(freq: c4)"),
        "expected `for` and a duration, such as `for 1beat`, found the end of the file"
    );
    assert_eq!(err("instr a() { 1 + }"), "expected a value, found `}`");
    assert_eq!(err("instr a() { f(1 2) }"), "expected `)`, found a number");
}
#[test]
fn a_match_needs_an_arm_to_test_and_ends_with_else() {
    assert!(parse("fn f(m: (a | b) = a) { match m { a: 1, b | c: 2, else: 3 } }").is_ok());
    assert_eq!(
        err("fn f(m: (a | b) = a) { match m { else: 3 } }"),
        "a `match` needs an arm to test"
    );
    assert!(
        err("fn f(m: (a | b) = a) { match m { else: 3, a: 1 } }")
            .contains("`else` is the last arm")
    );
}
