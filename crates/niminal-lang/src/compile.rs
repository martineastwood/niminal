//! Program-level compilation: tempo, opcodes, instruments and notes.

use std::collections::BTreeMap;

use niminal_score::{Event, Tempo, Time, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::kernel::compile_opcode;
use crate::lower::{compile_instr, unknown_unit};
use crate::opcodes::Registry;
use crate::parser;
use crate::program::{Instrument, Program};

type Res<T> = Result<T, Diagnostic>;

const DEFAULT_BPM: f64 = 120.0;

pub fn compile(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let items = parser::parse(source).map_err(|d| vec![d])?;
    let mut errors = Vec::new();

    let tempo = match tempo_of(&items) {
        Ok(t) => t,
        Err(d) => {
            errors.push(d);
            Tempo { bpm: DEFAULT_BPM }
        }
    };

    let mut registry = Registry::builtin();
    for item in &items {
        let Item::Opcode(def) = item else { continue };
        match compile_opcode(def, &registry, tempo) {
            Ok((spec, body_error)) => {
                errors.extend(body_error);
                registry.add(spec);
            }
            Err(d) => errors.push(d),
        }
    }

    let mut instruments: Vec<Instrument> = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    for item in &items {
        let Item::Instr(def) = item else { continue };
        let name = def.name.name.as_str();
        if instruments.iter().any(|i| i.name == name) || failed.contains(&name) {
            errors.push(Diagnostic::new(format!("instrument `{name}` is defined twice"), def.name.span));
            continue;
        }
        match compile_instr(def, tempo, &registry) {
            Ok(i) => instruments.push(i),
            Err(d) => {
                errors.push(d);
                failed.push(name);
            }
        }
    }

    let mut notes = Vec::new();
    for item in &items {
        let Item::Note(note) = item else { continue };
        if failed.contains(&note.target.name.as_str()) {
            continue; // already reported against the instrument
        }
        match check_note(note, &instruments, tempo) {
            Ok(e) => notes.push(e),
            Err(d) => errors.push(d),
        }
    }

    if errors.is_empty() { Ok(Program { tempo, instruments, notes }) } else { Err(errors) }
}

// ---- program-level statements -------------------------------------------

fn tempo_of(items: &[Item]) -> Res<Tempo> {
    let mut tempo = None;
    for item in items {
        let Item::Tempo(e) = item else { continue };
        if tempo.is_some() {
            return Err(Diagnostic::new("tempo is set more than once", e.span));
        }
        match &e.kind {
            ExprKind::Num { value, unit: Some(u) } if u == "bpm" && *value > 0.0 => {
                tempo = Some(Tempo { bpm: *value });
            }
            _ => {
                return Err(Diagnostic::new("tempo must be a positive number of bpm", e.span)
                    .with_help("for example `tempo 120bpm`"));
            }
        }
    }
    Ok(tempo.unwrap_or(Tempo { bpm: DEFAULT_BPM }))
}

fn check_note(note: &NoteStmt, instruments: &[Instrument], tempo: Tempo) -> Res<Event> {
    let name = &note.target;
    let Some(instr) = instruments.iter().find(|i| i.name == name.name) else {
        let mut d = Diagnostic::new(format!("no instrument named `{}`", name.name), name.span);
        if let Some(c) = closest(&name.name, instruments.iter().map(|i| i.name.as_str())) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        return Err(d);
    };

    let at = note.at.as_ref().map_or(Ok(Time::Beats(0.0)), literal_time)?;
    let dur = literal_time(&note.dur)?;

    let mut args = BTreeMap::new();
    for arg in &note.args {
        let Some(arg_name) = &arg.name else {
            return Err(Diagnostic::new("name the arguments of a note", arg.value.span)
                .with_help("for example `lead(freq: c4)`"));
        };
        let (_, param) = instr.param(&arg_name.name).map_err(|e| arg_error(e, arg_name.span))?;
        let value = literal_value(&arg.value)?;
        instr.param_value(param, value, tempo).map_err(|e| arg_error(e, arg.value.span))?;
        if args.insert(arg_name.name.clone(), value).is_some() {
            return Err(Diagnostic::new(format!("argument `{}` given more than once", arg_name.name), arg_name.span));
        }
    }
    instr.bind_args(&args, tempo).map_err(|e| arg_error(e, note.span))?;

    Ok(Event { target: name.name.clone(), at, dur, args })
}

fn arg_error(e: crate::program::ArgError, span: Span) -> Diagnostic {
    let d = Diagnostic::new(e.message, span);
    match e.help {
        Some(h) => d.with_help(h),
        None => d,
    }
}

fn literal_time(e: &Expr) -> Res<Time> {
    if let ExprKind::Num { value, unit: Some(u) } = &e.kind {
        match u.as_str() {
            "sec" => return Ok(Time::Seconds(*value)),
            "ms" => return Ok(Time::Seconds(value / 1000.0)),
            "beat" | "beats" => return Ok(Time::Beats(*value)),
            _ => {}
        }
    }
    Err(Diagnostic::new("expected a time", e.span).with_help("for example `1beat`, `2beats` or `250ms`"))
}

/// Note arguments are plain values for now: a number with a unit, or a note name.
fn literal_value(e: &Expr) -> Res<Value> {
    let bad = || {
        Diagnostic::new("note arguments must be literal values", e.span)
            .with_help("for example `440hz`, `-6db`, `0.5` or `a3`")
    };
    match &e.kind {
        ExprKind::Num { value, unit } => match unit.as_deref() {
            None => Ok(Value::Num(*value)),
            Some("hz") => Ok(Value::Hz(*value)),
            Some("khz") => Ok(Value::Hz(value * 1000.0)),
            Some("db") => Ok(Value::Db(*value)),
            Some("sec") => Ok(Value::Seconds(*value)),
            Some("ms") => Ok(Value::Seconds(value / 1000.0)),
            Some("beat" | "beats") => Ok(Value::Beats(*value)),
            Some(u) => Err(unknown_unit(u, e.span)),
        },
        ExprKind::Name(n) => match n.parse::<Value>() {
            Ok(v @ Value::Note(_)) => Ok(v),
            _ => Err(bad()),
        },
        ExprKind::Neg(inner) => match literal_value(inner)? {
            Value::Num(n) => Ok(Value::Num(-n)),
            Value::Hz(n) => Ok(Value::Hz(-n)),
            Value::Db(n) => Ok(Value::Db(-n)),
            Value::Seconds(n) => Ok(Value::Seconds(-n)),
            Value::Beats(n) => Ok(Value::Beats(-n)),
            Value::Note(_) => Err(bad()),
        },
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::Param;
    use crate::unit::Unit;

    fn ok(src: &str) -> Program {
        compile(src).unwrap_or_else(|errs| {
            panic!("{}", errs.iter().map(|d| d.render("t", src)).collect::<String>());
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

    const SAW_LEAD: &str = "
instr saw_lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level
}
saw_lead(freq: a3) for 1beat
";

    #[test]
    fn compiles_the_basic_synth() {
        let p = ok(SAW_LEAD);
        let i = p.instrument("saw_lead").unwrap();
        assert_eq!(i.params.len(), 2);
        assert_eq!(i.params[0], Param { name: "freq".into(), unit: Unit::Hz, range: None, default: None });
        let amp = i.params[1].default.unwrap();
        assert!((amp - 0.501_187).abs() < 1e-5, "-6db is stored as a gain factor: {amp}");

        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].dur, Time::Beats(1.0));
        assert_eq!(p.notes[0].args["freq"], Value::Note(57));
    }

    #[test]
    fn arithmetic_with_units() {
        ok("instr a(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq * 2).lpf(cutoff: freq * (2 + bright * 6)) }");
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
        assert_eq!(msg("tempo 120bpm\ntempo 90bpm"), "tempo is set more than once");
        assert_eq!(msg("tempo 120"), "tempo must be a positive number of bpm");
        assert_eq!(msg("instr a() { osc(sine, 100bpm) }"), "`bpm` can only be used with `tempo`");
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
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: sine) }"), "`sine` is a waveform, not a value");
    }

    #[test]
    fn wrong_units_are_errors() {
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: 6db) }"), "`cutoff` expects hz, found db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).gain(440hz) }"), "`gain` expects db or a plain factor, found hz");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f + 6db) }"), "can't add hz and db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f * f) }"), "can't multiply hz and hz");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f + 1) }"), "can't add hz and a plain number");
        assert_eq!(msg("instr a(f: hz) { f }"), "an instrument outputs a plain signal, but this is hz");
        assert_eq!(msg("instr a(f: hz = 6db) { osc(saw, f) }"), "`f` expects hz, found db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).gain(f * 2db) }"), "can't multiply hz and db");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f / 0) }"), "division by zero");
    }

    #[test]
    fn name_errors_suggest_fixes() {
        let d = first_error("instr a(freq: hz) { osc(saw, frq) }");
        assert_eq!(d.message, "`frq` is not defined");
        assert_eq!(d.help.as_deref(), Some("did you mean `freq`?"));
        assert_eq!(help("instr a(f: hz) { osc(saw, f).lpf(cuttoff: 1khz) }").as_deref(), Some("did you mean `cutoff`?"));
        assert_eq!(help("instr a(f: hz) { osc(saw, f).lfp(cutoff: 1khz) }").as_deref(), Some("did you mean `lpf`?"));
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).wobble() }"), "unknown opcode `wobble`");
        assert_eq!(msg("instr a(f: hz) { saw }"), "`saw` is a waveform, not a value");
        assert_eq!(msg("instr a(f: hz) { osc(wobble, f) }"), "expected a waveform");
        assert_eq!(msg("instr a(f: hertz) { osc(saw, f) }"), "unknown parameter type `hertz`");
        assert_eq!(msg("instr a(f: hz) { osc(saw, 440herz) }"), "unknown unit `herz`");
    }

    #[test]
    fn argument_binding_rules() {
        let d = first_error("instr a(f: hz) { osc(saw, f).lpf(2khz) }");
        assert_eq!(d.message, "`lpf` takes its other arguments by name");
        assert_eq!(d.help.as_deref(), Some("write `cutoff: ...`"));
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(res: 0.1) }"), "`lpf` needs an argument `cutoff`");
        assert_eq!(msg("instr a(f: hz) { osc(saw) }"), "`osc` needs an argument `freq`");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f, f) }"), "`osc` takes its other arguments by name");
        assert_eq!(msg("instr a(f: hz) { osc(saw, freq: f, freq: f) }"), "`osc` argument `freq` given more than once");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f, freq: f) }"), "`osc` argument `freq` given more than once");
        // the receiver fills the first parameter, so naming it as well clashes
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).lpf(x: f, cutoff: 1hz) }"), "`lpf` argument `x` given more than once");
    }

    #[test]
    fn instrument_shape_errors() {
        assert_eq!(msg("instr a() { }"), "instrument `a` is empty");
        assert_eq!(msg("instr a() { x = 0.5 }"), "an instrument must end with the signal it outputs");
        assert_eq!(msg("instr a() { 0.5\n0.5 }"), "this value is never used");
        assert_eq!(msg("instr a(f: hz, f: hz) { 0.5 }"), "parameter `f` is declared twice");
        assert_eq!(msg("instr a() { 0.5 }\ninstr a() { 0.5 }"), "instrument `a` is defined twice");
        assert_eq!(msg("instr a(b: 0..1 = 2) { b }"), "the default for `b` is outside 0..1");
        assert_eq!(msg("instr a(b: 1..0) { b }"), "the lower bound of a range must be below the upper bound");
    }

    #[test]
    fn envelope_unit_rules() {
        ok("instr a() { osc(sine, env[200hz 10ms 6khz 300ms.exp 900hz]) }");
        assert!(msg("instr a() { osc(sine, env[0 10ms 6khz]) }").starts_with("envelope levels must all have the same unit"));
        assert_eq!(msg("instr a() { env[-6db 10ms 0db] }"), "db levels in envelopes aren't supported yet");
    }

    #[test]
    fn note_checking() {
        let base = "instr lead(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq) }\n";
        let note = |n: &str| format!("{base}{n}");

        let p = ok(&note("at 2beats lead(freq: c4, bright: 0.2) for 250ms"));
        assert_eq!(p.notes[0].at, Time::Beats(2.0));
        assert_eq!(p.notes[0].dur, Time::Seconds(0.25));

        let d = first_error(&note("lead(freq: 440) for 1beat"));
        assert_eq!(d.message, "argument `freq`: expected a frequency (hz or a note name), got `440`");
        assert_eq!(d.help.as_deref(), Some("did you mean `440hz`?"));

        assert_eq!(msg(&note("lead() for 1beat")), "`lead` needs an argument `freq`");
        assert_eq!(msg(&note("lead(freq: c4, bright: 1.5) for 1beat")), "argument `bright` must be between 0 and 1, got 1.5");
        assert_eq!(msg(&note("lead(freq: c4, brigt: 1) for 1beat")), "`lead` has no parameter `brigt`");
        assert_eq!(help(&note("lead(freq: c4, brigt: 1) for 1beat")).as_deref(), Some("did you mean `bright`?"));
        assert_eq!(msg(&note("lede(freq: c4) for 1beat")), "no instrument named `lede`");
        assert_eq!(msg(&note("lead(c4) for 1beat")), "name the arguments of a note");
        assert_eq!(msg(&note("lead(freq: c4) for 440hz")), "expected a time");
        assert_eq!(msg(&note("lead(freq: c4, freq: c5) for 1beat")), "argument `freq` given more than once");
        assert_eq!(msg(&note("lead(freq: c4 + 1st) for 1beat")), "note arguments must be literal values");
    }

    const SPEC_OPCODES: &str = "
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}

opcode drive(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 9)).tanh
}

opcode dc_block(x) {
  state x1 = 0.0
  state y1 = 0.0
  y = x - x1 + 0.995 * y1
  x1 = x
  y1 = y
  y
}

opcode fold(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 8)).sin
}
";

    #[test]
    fn the_specs_example_opcodes_compile_and_can_be_used() {
        ok(&format!(
            "{SPEC_OPCODES}
instr a(freq: hz) {{ osc(saw, freq).drive(amount: 0.3).one_pole(cutoff: 800hz) }}
instr b(freq: hz) {{ osc(sine, freq).fold(amount: env[0 2sec 1 2sec 0]).dc_block }}
instr c(freq: hz) {{ drive(osc(saw, freq)) }}
a(freq: 110hz) for 1beat"
        ));
    }

    #[test]
    fn opcode_arguments_are_checked_like_any_other() {
        let with = |call: &str| format!("{SPEC_OPCODES}\ninstr a(freq: hz) {{ {call} }}");
        assert_eq!(msg(&with("osc(saw, freq).one_pole(cutoff: 800)")), "`cutoff` expects hz, found a plain number");
        assert_eq!(help(&with("osc(saw, freq).one_pole(cutoff: 800)")).as_deref(), Some("did you mean `800hz`?"));
        assert_eq!(msg(&with("osc(saw, freq).one_pole()")), "`one_pole` needs an argument `cutoff`");
        assert_eq!(msg(&with("osc(saw, freq).drive(amont: 0.3)")), "`drive` has no argument named `amont`");
        assert_eq!(msg(&with("osc(saw, freq).drive(0.3)")), "`drive` takes its other arguments by name");
        ok(&with("osc(saw, freq).drive"));
    }

    #[test]
    fn opcode_body_errors() {
        let body = |b: &str| format!("opcode f(x, amount: 0..1 = 0.5, cutoff: hz = 100hz) {{ {b} }}");
        assert_eq!(msg(&body("x * undefined_thing")), "`undefined_thing` is not defined");
        assert_eq!(msg(&body("exp(cutoff)")), "`exp` expects a plain number, found hz");
        assert_eq!(msg(&body("x + cutoff")), "can't add a plain number and hz");
        assert_eq!(msg(&body("cutoff")), "an opcode outputs a plain signal, but this is hz");
        assert_eq!(msg(&body("x.lpf(cutoff: 100hz)")), "`lpf` can't be used inside an opcode");
        assert_eq!(msg(&body("x.exxp")), "`exxp` can't be used inside an opcode");
        assert_eq!(help(&body("x.exxp")).as_deref(), Some("did you mean `exp`?"));
        assert_eq!(msg(&body("exp(x, x)")), "`exp` takes one argument");
        assert_eq!(msg(&body("env[0 1sec 1]")), "envelopes can't be used inside an opcode");
        assert_eq!(msg(&body("")), "opcode `f` is empty");
        assert_eq!(msg(&body("state s = x\ns")), "the starting value of a state must be a number");
        assert_eq!(msg(&body("state s = 0.0\ns = cutoff\ns")), "`s` holds a plain number, but this is hz");
        assert_eq!(msg(&body("state x = 0.0\nx")), "`x` is already defined");
        assert_eq!(msg(&body("x = 1")), "an opcode must end with the value it outputs");
        assert_eq!(msg(&body("x\nx")), "this value is never used");
    }

    #[test]
    fn opcode_signature_errors() {
        assert_eq!(msg("opcode lpf(x) { x }"), "`lpf` is already an opcode");
        assert_eq!(msg("opcode f(x) { x }\nopcode f(x) { x }"), "`f` is already an opcode");
        assert_eq!(msg("opcode f(x, x) { x }"), "parameter `x` is declared twice");
        assert_eq!(msg("opcode f(x, a: 0..1 = 3) { x }"), "the default for `a` is outside 0..1");
        assert_eq!(msg("opcode f(x, a: furlongs) { x }"), "unknown parameter type `furlongs`");
    }

    #[test]
    fn state_and_untyped_parameters_belong_to_the_right_definitions() {
        assert_eq!(msg("instr a() { state y = 0.0\n0.5 }"), "`state` can only be used inside an `opcode`");
        assert_eq!(msg("instr a(freq) { osc(saw, freq) }"), "parameter `freq` needs a type");
    }

    #[test]
    fn a_broken_opcode_is_reported_once_even_if_it_is_used() {
        let errs = errors("
opcode bad(x) { x + undefined_thing }
instr a(f: hz) { osc(saw, f).bad }
a(f: 100hz) for 1beat
");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].message, "`undefined_thing` is not defined");
    }

    #[test]
    fn db_gains_apply_to_plain_values() {
        ok("instr a(f: hz, amp: db = -6db) { osc(saw, f) * amp }");
        ok("opcode g(x, amp: db = -6db) { x * amp }\ninstr a(f: hz) { osc(saw, f).g(amp: -3db) }");
    }

    #[test]
    fn reports_errors_from_several_instruments_without_cascading() {
        let errs = errors("
instr a() { osc(saw, 440) }
instr b() { osc(saw, 440) }
a() for 1beat
");
        assert_eq!(errs.len(), 2, "one per broken instrument, none for the note: {errs:?}");
    }
}
