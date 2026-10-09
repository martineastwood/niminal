//! Program-level compilation: tempo, opcodes, instruments and notes.

use std::collections::BTreeMap;

use niminal_engine::{Route, TrackDef, execution_order};
use niminal_score::{Event, Tempo, Time, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::kernel::compile_opcode;
use crate::layout::Layout;
use crate::lower::{Names, compile_chain, compile_instr, layout_from_expr, unknown_unit};
use crate::opcodes::Registry;
use crate::parser;
use crate::program::{Instrument, Program, TrackInfo, resolve_target};

type Res<T> = Result<T, Diagnostic>;

const DEFAULT_BPM: f64 = 120.0;

pub fn compile(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let items = parser::parse(source).map_err(|d| vec![d])?;
    let mut errors = Vec::new();

    let tempo = match tempo_of(&items) {
        Ok(t) => t,
        Err(d) => {
            errors.push(d);
            Tempo::new(DEFAULT_BPM)
        }
    };

    // Top-level names first, so locals can be kept from shadowing them.
    let mut names = Names::default();
    for item in &items {
        let (ident, kind) = match item {
            Item::Instr(d) => (&d.name, "instrument"),
            Item::Opcode(d) => (&d.name, "opcode"),
            Item::Bus(d) => (&d.name, "bus"),
            Item::Track(d) => (&d.name, "track"),
            Item::Tempo(_) | Item::Note(_) | Item::Config(_) => continue,
        };
        match names.kind_of(&ident.name) {
            None => names.add(&ident.name, kind),
            Some(existing) if existing != kind => {
                errors.push(Diagnostic::new(format!("`{}` is already the name of a {existing}", ident.name), ident.span));
            }
            Some(_) => {}
        }
    }

    match master_layout(&items) {
        Ok(layout) => names.master = layout,
        Err(d) => errors.push(d),
    }

    let mut registry = Registry::builtin();
    for item in &items {
        let Item::Opcode(def) = item else { continue };
        match compile_opcode(def, &registry, &names, tempo) {
            Ok((spec, body_error)) => {
                errors.extend(body_error);
                registry.add(spec);
            }
            Err(d) => errors.push(d),
        }
    }

    let mut seen_buses: Vec<&str> = Vec::new();
    for item in &items {
        let Item::Bus(bus) = item else { continue };
        if seen_buses.contains(&bus.name.name.as_str()) {
            errors.push(Diagnostic::new(format!("bus `{}` is defined twice", bus.name.name), bus.name.span));
            continue;
        }
        seen_buses.push(&bus.name.name);
        if let Some(expr) = &bus.layout {
            match layout_from_expr(expr) {
                Ok(layout) => {
                    if let Some(i) = names.bus(&bus.name.name) {
                        names.bus_layouts[i] = layout;
                    }
                }
                Err(d) => errors.push(d),
            }
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
        match compile_instr(def, tempo, &registry, &names) {
            Ok(i) => instruments.push(i),
            Err(d) => {
                errors.push(d);
                failed.push(name);
            }
        }
    }

    // Track 0 plays bare instruments, so it may send to any bus an instrument does.
    let implicit = TrackInfo {
        name: String::new(),
        instrument: None,
        defaults: BTreeMap::new(),
        def: TrackDef {
            chain: None,
            inputs: Vec::new(),
            route: Route::Master,
            voice_sends: {
                let mut sends: Vec<usize> = instruments.iter().flat_map(|i| i.graph.send_buses()).collect();
                sends.sort_unstable();
                sends.dedup();
                sends
            },
        },
    };
    let mut tracks = vec![implicit];
    let mut track_spans = vec![Span::default()];
    for item in &items {
        let Item::Track(decl) = item else { continue };
        let name = decl.name.name.as_str();
        if tracks.iter().skip(1).any(|t| t.name == name) || failed.contains(&name) {
            errors.push(Diagnostic::new(format!("track `{name}` is defined twice"), decl.name.span));
            continue;
        }
        match compile_track(decl, &instruments, &failed, tempo, &registry, &names) {
            Ok(Some(t)) => {
                tracks.push(t);
                track_spans.push(decl.name.span);
            }
            Ok(None) => failed.push(name),
            Err(d) => {
                errors.push(d);
                failed.push(name);
            }
        }
    }

    let defs: Vec<TrackDef> = tracks.iter().map(|t| t.def.clone()).collect();
    if let Err(cycle) = execution_order(&defs) {
        let path: Vec<&str> = cycle.iter().chain(cycle.first()).map(|&i| tracks[i].name.as_str()).collect();
        errors.push(
            Diagnostic::new(format!("tracks feed back through buses: {}", path.join(" -> ")), track_spans[cycle[0]])
                .with_help("a bus has to be written before it is read; breaking the loop with `prev` isn't available yet"),
        );
    }

    let mut notes = Vec::new();
    for item in &items {
        let Item::Note(note) = item else { continue };
        if failed.contains(&note.target.name.as_str()) {
            continue; // already reported against the instrument or track
        }
        match check_note(note, &instruments, &tracks, tempo) {
            Ok(e) => notes.push(e),
            Err(d) => errors.push(d),
        }
    }

    if errors.is_empty() {
        Ok(Program {
            tempo,
            master: names.master,
            instruments,
            buses: names.buses,
            bus_layouts: names.bus_layouts,
            tracks,
            notes,
        })
    } else {
        Err(errors)
    }
}

/// A track's chain, plus the instrument named by its `instrument = ...` line.
/// `None` when that instrument failed to compile, which is already reported.
fn compile_track(
    decl: &TrackDecl,
    instruments: &[Instrument],
    failed: &[&str],
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Option<TrackInfo>> {
    let mut instrument_line: Option<&Expr> = None;
    let mut rest: Vec<&Stmt> = Vec::new();
    for stmt in &decl.body {
        match stmt {
            Stmt::Bind { name, value } if name.name == "instrument" => {
                if instrument_line.is_some() {
                    return Err(Diagnostic::new("a track has only one instrument", name.span));
                }
                instrument_line = Some(value);
            }
            other => rest.push(other),
        }
    }

    let mut instrument = None;
    let mut defaults = BTreeMap::new();
    if let Some(value) = instrument_line {
        let (name, span, args): (&str, Span, &[Arg]) = match &value.kind {
            ExprKind::Name(n) => (n, value.span, &[]),
            ExprKind::Call { name, args } if !args.iter().any(|a| a.receiver) => (&name.name, name.span, args),
            _ => {
                return Err(Diagnostic::new("expected an instrument here", value.span)
                    .with_help("for example `instrument = pluck` or `instrument = pluck(bright: 0.6)`"));
            }
        };
        if failed.contains(&name) {
            return Ok(None);
        }
        let Some(index) = instruments.iter().position(|i| i.name == name) else {
            let mut d = Diagnostic::new(format!("no instrument named `{name}`"), span);
            if let Some(c) = closest(name, instruments.iter().map(|i| i.name.as_str())) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        defaults = literal_args(args, &instruments[index], tempo)?;
        instrument = Some(index);
    }

    let chain = compile_chain(&decl.name, &rest, instrument.is_some(), tempo, registry, names)?;
    let voice_sends = instrument.map(|i| instruments[i].graph.send_buses()).unwrap_or_default();
    Ok(Some(TrackInfo {
        name: decl.name.name.clone(),
        instrument,
        defaults,
        def: TrackDef { chain: chain.graph, inputs: chain.inputs, route: chain.route, voice_sends },
    }))
}

// ---- program-level statements -------------------------------------------

/// The master layout from the project's `config`, mono if there is none.
fn master_layout(items: &[Item]) -> Res<Layout> {
    let mut layout = None;
    let mut seen = false;
    for item in items {
        let Item::Config(config) = item else { continue };
        if seen {
            return Err(Diagnostic::new("a project has only one `config`", config.span));
        }
        seen = true;
        for (key, value) in &config.entries {
            match key.name.as_str() {
                "channels" => layout = Some(layout_from_expr(value)?),
                other => {
                    let mut d = Diagnostic::new(format!("unknown setting `{other}`"), key.span);
                    d = d.with_help("the settings so far are: channels");
                    return Err(d);
                }
            }
        }
    }
    Ok(layout.unwrap_or(Layout::Mono))
}

fn tempo_of(items: &[Item]) -> Res<Tempo> {
    let mut tempo = None;
    for item in items {
        let Item::Tempo(e) = item else { continue };
        if tempo.is_some() {
            return Err(Diagnostic::new("tempo is set more than once", e.span));
        }
        match &e.kind {
            ExprKind::Num { value, unit: Some(u) } if u == "bpm" && *value > 0.0 => {
                tempo = Some(Tempo::new(*value));
            }
            _ => {
                return Err(Diagnostic::new("tempo must be a positive number of bpm", e.span)
                    .with_help("for example `tempo 120bpm`"));
            }
        }
    }
    Ok(tempo.unwrap_or(Tempo::new(DEFAULT_BPM)))
}

/// Literal named arguments, checked against `instr`'s parameters.
fn literal_args(args: &[Arg], instr: &Instrument, tempo: Tempo) -> Res<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for arg in args {
        let Some(arg_name) = &arg.name else {
            return Err(Diagnostic::new("name the arguments of a note", arg.value.span)
                .with_help("for example `lead(freq: c4)`"));
        };
        let (_, param) = instr.param(&arg_name.name).map_err(|e| arg_error(e, arg_name.span))?;
        let value = literal_value(&arg.value)?;
        instr.param_value(param, value, tempo).map_err(|e| arg_error(e, arg.value.span))?;
        if out.insert(arg_name.name.clone(), value).is_some() {
            return Err(Diagnostic::new(format!("argument `{}` given more than once", arg_name.name), arg_name.span));
        }
    }
    Ok(out)
}

fn check_note(note: &NoteStmt, instruments: &[Instrument], tracks: &[TrackInfo], tempo: Tempo) -> Res<Event> {
    let name = &note.target;
    let target = resolve_target(instruments, tracks, &name.name).map_err(|e| arg_error(e, name.span))?;
    let instr = &instruments[target.instrument];

    let at = note.at.as_ref().map_or(Ok(Time::Beats(0.0)), literal_time)?;
    let dur = literal_time(&note.dur)?;

    let args = literal_args(&note.args, instr, tempo)?;
    let mut merged = target.defaults.cloned().unwrap_or_default();
    merged.extend(args.iter().map(|(k, v)| (k.clone(), *v)));
    instr.bind_args(&merged, tempo).map_err(|e| arg_error(e, note.span))?;

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
            "bar" | "bars" => return Ok(Time::Bars(*value)),
            _ => {}
        }
    }
    Err(Diagnostic::new("expected a time", e.span).with_help("for example `1beat`, `2bars` or `250ms`"))
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
            Some("bar" | "bars") => Ok(Value::Bars(*value)),
            Some("st") => Ok(Value::Semitones(*value)),
            Some("deg") => Ok(Value::Degrees(*value)),
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
            Value::Bars(n) => Ok(Value::Bars(-n)),
            Value::Semitones(n) => Ok(Value::Semitones(-n)),
            Value::Degrees(n) => Ok(Value::Degrees(-n)),
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
        assert_eq!(msg(&note("lede(freq: c4) for 1beat")), "no instrument or track named `lede`");
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

    const ROUTING: &str = "
bus space

instr pluck(freq: hz, bright: 0..1 = 0.5) {
  out = osc(saw, freq).lpf(cutoff: 900hz + bright * 1khz)
  space += out.gain(-14db)
}

track room {
  out = space.lpf(cutoff: 4khz).hpf(cutoff: 150hz)
}

track lead {
  instrument = pluck(bright: 0.6)
  out = it.gain(0.5)
  space += out.gain(-12db)
}
";

    #[test]
    fn buses_and_tracks_compile() {
        let p = ok(&format!("{ROUTING}\nlead(freq: c4) for 1beat\npluck(freq: c4) for 1beat"));
        assert_eq!(p.buses, ["space"]);
        assert_eq!(p.tracks.len(), 3, "the implicit track, room and lead");
        assert_eq!(p.tracks[2].name, "lead");
        assert_eq!(p.tracks[2].instrument, Some(0));
        assert_eq!(p.tracks[2].defaults["bright"], Value::Num(0.6));
        assert_eq!(p.tracks[0].def.voice_sends, [0], "bare instruments may send to the bus");
        assert_eq!(p.notes.len(), 2);

        // the plan for a track note uses the track's defaults, and the note wins over them
        let plan = |e: &Event| p.plan(e).unwrap();
        let bright = p.instruments[0].params.iter().position(|q| q.name == "bright").unwrap();
        let on_track = plan(&p.notes[0]);
        assert_eq!(on_track.track, 2);
        assert!(on_track.params.contains(&(bright, 0.6)));
        let bare = plan(&p.notes[1]);
        assert_eq!(bare.track, 0);
        assert!(!bare.params.iter().any(|(i, _)| *i == bright), "no default applied on the implicit track");
        let overridden = p.plan(&Event {
            args: [("freq".to_string(), Value::Note(60)), ("bright".to_string(), Value::Num(0.1))].into(),
            ..p.notes[0].clone()
        });
        assert!(overridden.unwrap().params.contains(&(bright, 0.1)));
    }

    #[test]
    fn routing_a_track_to_a_bus() {
        let p = ok("
bus fx
track a { instrument = i\n out = it.to(fx) }
track b { out = fx }
instr i(f: hz) { osc(sine, f) }
a(f: 100hz) for 1beat");
        assert_eq!(p.tracks[1].def.route, niminal_engine::Route::Bus(0));
        assert_eq!(p.tracks[2].def.inputs, [niminal_engine::ChainInput::Bus { bus: 0, channel: 0 }]);
    }

    #[test]
    fn out_and_it_rules() {
        // an instrument either sets out or ends with a value, not both
        assert_eq!(
            msg("instr a(f: hz) { out = osc(saw, f)\nosc(sine, f) }"),
            "this instrument sets `out`, so its last line can't also be a value"
        );
        ok("instr a(f: hz) { out = osc(saw, f)\nout += osc(sine, f / 2) }");
        assert_eq!(msg("instr a(f: hz) { out += out\nout }"), "`out` has no value yet");
        assert_eq!(msg("instr a(f: hz) { out = f }"), "`out` carries a plain signal, but this is hz");
        assert_eq!(msg("instr a(f: hz) { osc(saw, it) }"), "`it` is only available inside a track");
        assert_eq!(msg("instr a(f: hz) { it = 1 }"), "`it` is an input and can't be assigned");
        assert_eq!(msg("instr a(f: hz) { instrument = 1\n0.5 }"), "`instrument` is only available in a track");
        // only sends: silent but valid
        ok("bus b\ninstr a(f: hz) { b += osc(saw, f) }");
    }

    #[test]
    fn bus_rules() {
        let with = |body: &str| format!("bus space\n{body}");
        assert_eq!(
            msg(&with("instr a(f: hz) { osc(saw, space) }")),
            "`space` is a bus, and only a track can read a bus"
        );
        assert_eq!(
            help(&with("instr a(f: hz) { osc(saw, space) }")).as_deref(),
            Some("an instrument can send to it with `space += ...`")
        );
        assert_eq!(msg(&with("instr a(f: hz) { x = osc(saw, f)\nx += osc(saw, f)\nx }")), "`+=` can only add to `out` or a bus, not `x`");
        assert_eq!(
            help(&with("instr a(f: hz) { x = osc(saw, f)\nx += osc(saw, f)\nx }")).as_deref(),
            Some("to change a local, write `x = x + ...`")
        );
        assert_eq!(help(&with("instr a(f: hz) { spaec += osc(saw, f) }")).as_deref(), Some("did you mean the bus `space`?"));
        assert_eq!(msg(&with("instr a(f: hz) { space += f\n0.5 }")), "`space` carries a plain signal, but this is hz");
        assert_eq!(msg("bus a\nbus a"), "bus `a` is defined twice");
        assert_eq!(msg("bus a: wobble"), "unknown channel layout `wobble`");
        assert_eq!(msg("bus a: surround(9.1)"), "unsupported surround layout `9.1`");
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
            "`other` is already the name of a instrument, so it can't be used for a local"
        );
        assert_eq!(msg(&with("instr a(f: hz) { space = 1 }")), "`space` is already the name of a bus, so it can't be used for a local");
        assert_eq!(msg("bus x\ninstr x(f: hz) { osc(saw, f) }"), "`x` is already the name of a bus");
    }

    #[test]
    fn track_errors() {
        let base = "bus space\ninstr i(f: hz, b: 0..1 = 0.5) { osc(saw, f) }\n";
        let track = |body: &str| format!("{base}track t {{ {body} }}");
        assert_eq!(msg(&track("")), "track `t` does nothing");
        assert_eq!(msg(&track("x = 1")), "track `t` never sets `out`");
        ok(&track("out = it")); // valid, though silent: nothing feeds `it`
        assert_eq!(msg(&track("instrument = i\ninstrument = i")), "a track has only one instrument");
        assert_eq!(msg(&track("instrument = j")), "no instrument named `j`");
        assert_eq!(msg(&track("instrument = i(q: 1)")), "`i` has no parameter `q`");
        assert_eq!(msg(&track("instrument = i(b: 2)")), "argument `b` must be between 0 and 1, got 2");
        assert_eq!(msg(&track("instrument = i\n0.5")), "a track makes sound through `out`");
        assert_eq!(msg(&track("instrument = i\nout = it.to(spce)")), "`spce` is not defined");
        assert_eq!(msg(&track("instrument = i\nout = it.to(1)")), "`to` takes the name of a bus");
        assert_eq!(msg(&track("instrument = i\nout = it.gain(1).to(space).gain(1)")), "`to` can only end the line that sets a track's `out`");
        assert_eq!(msg(&format!("{base}track t {{ instrument = i }}\ntrack t {{ instrument = i }}")), "track `t` is defined twice");
        // a track with an instrument and nothing else just plays it
        ok(&track("instrument = i"));
        // an effect track with no instrument can't be played
        let e = first_error(&format!("{base}track fx {{ out = space }}\nfx(f: 100hz) for 1beat"));
        assert_eq!(e.message, "track `fx` has no instrument to play");
    }

    #[test]
    fn feedback_between_tracks_is_an_error_naming_them() {
        let src = "
bus x
bus y
track a { out = x.lpf(cutoff: 1khz).to(y) }
track b { out = y.lpf(cutoff: 1khz).to(x) }
";
        let d = first_error(src);
        assert!(d.message == "tracks feed back through buses: a -> b -> a" || d.message == "tracks feed back through buses: b -> a -> b", "{}", d.message);
        assert!(d.help.unwrap().contains("`prev`"));

        let d = first_error("bus x\ntrack a { out = x.to(x) }");
        assert_eq!(d.message, "tracks feed back through buses: a -> a");
    }

    #[test]
    fn a_broken_instrument_does_not_cascade_into_its_tracks_and_notes() {
        let errs = errors("
instr bad(f: hz) { osc(saw, 440) }
track t { instrument = bad }
t() for 1beat
bad() for 1beat
");
        assert_eq!(errs.len(), 1, "{errs:?}");
    }

    #[test]
    fn delay_and_reverb_compile_with_units_checked() {
        ok("instr a(f: hz) { osc(saw, f).delay(time: 3/16beat, feedback: 0.4).reverb(room: 0.8, damp: 0.3) }");
        ok("instr a(f: hz) { osc(saw, f).delay(time: 250ms).reverb }");

        let d = first_error("instr a(f: hz) { osc(saw, f).delay(time: 0.2) }");
        assert_eq!(d.message, "`time` expects a time, found a plain number");
        assert_eq!(d.help.as_deref(), Some("did you mean `0.2sec`?"));
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).delay(time: 1beat, feedback: 1sec) }"), "`feedback` expects a plain number, found a time");
    }

    #[test]
    fn a_changing_delay_time_needs_a_max() {
        let e = msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t) }");
        assert_eq!(e, "`delay` needs a constant `time`, or a `max:` that bounds a changing one");
        ok("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 2sec) }");
        assert_eq!(
            msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 31sec) }"),
            "a delay can be at most 30 seconds long"
        );
        assert_eq!(msg("instr a() { osc(saw, 100hz).delay(time: 40sec) }"), "a delay can be at most 30 seconds long");
        assert_eq!(msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 5) }"), "`max` expects a time, found a plain number");
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
        let three = ok("config { channels: channels(3) }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f)] }");
        assert_eq!(three.instruments[0].graph.channels(), 3);
        assert_eq!(msg(&src("config { channels: wobble }")), "unknown channel layout `wobble`");
        assert_eq!(msg(&src("config { speed: 2 }")), "unknown setting `speed`");
        assert_eq!(msg(&src("config { channels: stereo }\nconfig { channels: mono }")), "a project has only one `config`");
        assert_eq!(msg(&src("config { channels: channels(40) }")), "`channels` takes a count from 1 to 16");
    }

    #[test]
    fn pan_places_a_mono_signal_in_the_master_layout() {
        let p = ok("config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg, spread: 0.3) }");
        assert_eq!(p.instruments[0].graph.channels(), 6);
        ok("config { channels: stereo }\ninstr a(f: hz, az: deg = 10deg) { osc(saw, f).pan(azimuth: az) }");

        let d = first_error("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20) }");
        assert_eq!(d.message, "`azimuth` expects an angle, found a plain number");
        assert_eq!(d.help.as_deref(), Some("did you mean `20deg`?"));
        assert_eq!(
            msg("config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)].pan }"),
            "`pan` takes a one-channel signal, but `x` has 2"
        );
        assert_eq!(msg("instr a(f: hz) { osc(saw, f).pan(azimuth: 5hz) }"), "`azimuth` expects an angle, found hz");
        assert_eq!(msg("instr a(f: hz) { osc(saw, f) * 20deg }"), "an instrument outputs a plain signal, but this is an angle");
    }

    #[test]
    fn angles_follow_the_unit_rules() {
        ok("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg * 2 - 5deg) }");
        assert_eq!(
            msg("config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg + 1hz) }"),
            "can't add an angle and hz"
        );
    }

    #[test]
    fn channel_lists_and_destructuring() {
        let stereo = |body: &str| format!("config {{ channels: stereo }}\ninstr a(f: hz) {{ {body} }}");
        let p = ok(&stereo("[osc(saw, f), osc(sine, f)]"));
        assert_eq!(p.instruments[0].graph.channels(), 2);

        ok(&stereo("[l, r] = osc(saw, f).pan\nmid = (l + r) * 0.5\n[mid + l, mid - r]"));
        // an opcode applied to a stereo signal runs once per channel
        ok(&stereo("[osc(saw, f), osc(saw, f)].lpf(cutoff: 800hz).delay(time: 100ms).reverb"));
        // different cutoffs per channel
        ok(&stereo("osc(saw, f).pan.lpf(cutoff: [800hz, 1200hz])"));

        assert_eq!(
            msg(&stereo("[l, r, c] = osc(saw, f).pan\nl")),
            "this signal has 2 channel(s), but 3 name(s) are given"
        );
        assert_eq!(
            msg(&stereo("[osc(saw, f).pan, osc(saw, f)]")),
            "a channel list takes one channel per entry, but this has 2"
        );
        assert_eq!(msg(&stereo("[f, f]")), "an instrument outputs a plain signal, but this is hz");
        assert_eq!(
            msg(&stereo("[osc(saw, f), f]")),
            "the entries of a channel list must have the same unit: this is hz, the first was a plain number"
        );
        assert_eq!(
            msg(&stereo("[osc(saw, f), osc(saw, f)] + [osc(saw, f), osc(saw, f), osc(saw, f)]")),
            "can't combine a 2-channel signal with a 3-channel one"
        );
        assert_eq!(
            msg(&stereo("osc(saw, [f, f]).lpf(cutoff: [1khz, 2khz, 3khz])")),
            "`lpf`'s arguments have 2 and 3 channels, which can't be combined"
        );
        assert_eq!(msg(&stereo("[l, r] = osc(saw, f)\nl")), "this signal has 1 channel(s), but 2 name(s) are given");
    }

    #[test]
    fn layouts_convert_automatically_where_the_spec_says_they_do() {
        // mono into 5.1 is centred; stereo into 5.1 takes the front pair
        ok("config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f) }");
        ok("config { channels: surround(5.1) }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)] }");
        // 5.1 bus read by a stereo master track folds down
        ok("config { channels: stereo }\nbus hall: surround(5.1)\ninstr a(f: hz) { out = osc(saw, f)\nhall += out }\ntrack t { out = hall }");
        // unnamed channel lists are taken as whatever layout has that many
        ok("config { channels: quad }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f), osc(saw, f)] }");

        let d = first_error("config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f)] }");
        assert_eq!(d.message, "can't convert ch[3] to stereo automatically");
        let d = first_error("config { channels: surround(5.1) }\nbus q: quad\ninstr a(f: hz) { q += [osc(saw, f), osc(saw, f), osc(saw, f), osc(saw, f)].to_layout(surround(7.1)) }");
        assert_eq!(d.message, "can't convert ch[4] to surround(7.1) automatically");
    }

    #[test]
    fn to_layout_converts_explicitly() {
        ok("config { channels: surround(5.1) }\nbus s: stereo\ninstr a(f: hz) { out = osc(saw, f).pan\ns += out.to_layout(stereo) }");
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).to_layout(wobble) }"),
            "unknown channel layout `wobble`"
        );
        ok("instr a(f: hz) { osc(saw, f).to_layout(mono) }");
    }

    #[test]
    fn buses_and_tracks_carry_their_layouts() {
        let p = ok("
config { channels: stereo }
bus space: stereo
bus lfe: mono
instr a(f: hz) { out = osc(saw, f).pan(azimuth: -20deg)\nspace += out\nlfe += out }
track room { out = space.reverb(room: 0.8) }
track sub { out = lfe.lpf(cutoff: 100hz) }
a(f: 100hz) for 1beat");
        assert_eq!(p.bus_layouts, [Layout::Stereo, Layout::Mono]);
        assert_eq!(p.tracks[1].def.inputs.len(), 2, "a stereo bus is two inputs");
        assert_eq!(p.tracks[2].def.inputs.len(), 1);
        // the sub track works in mono but delivers stereo to the master
        assert_eq!(p.tracks[2].def.chain.as_ref().unwrap().channels(), 2);
        // routing to a bus delivers in that bus's layout
        let routed = ok("
config { channels: stereo }
bus mono_bus: mono
track t { instrument = i\nout = it.to(mono_bus) }
track u { out = mono_bus }
instr i(f: hz) { osc(saw, f).pan }");
        assert_eq!(routed.tracks[1].def.chain.as_ref().unwrap().channels(), 1);
    }

    #[test]
    fn bars_and_semitones() {
        let p = ok("instr a(f: hz) { osc(sine, f + 7st) * env[1 1bar 0] }");
        assert_eq!(p.instruments.len(), 1);
        ok("instr a(f: hz, up: st = 7st) { osc(sine, f + up) }");
        ok("instr a(f: hz) { osc(sine, f - 12st + 3st) }");
        ok("instr a(f: hz) { osc(sine, c4 + 12st) }");
        ok("instr a(f: hz) { osc(sine, 7st + f) }");
        assert_eq!(msg("instr a(f: hz) { osc(sine, f + 7) }"), "can't add hz and a plain number");
        assert_eq!(msg("instr a(f: hz) { osc(sine, 7st - f) }"), "can't subtract an interval and hz");
        assert_eq!(msg("instr a(f: hz) { osc(sine, f * 2st) }"), "can't multiply hz and an interval");
        let notes = ok("instr a() { osc(sine, 100hz) }\na() for 2bars\nat 1bar a() for 1bar");
        assert_eq!(notes.notes[0].dur, Time::Bars(2.0));
        assert_eq!(notes.notes[1].at, Time::Bars(1.0));
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
