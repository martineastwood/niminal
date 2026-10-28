//! Compiling a composition: its sound-server definitions go to the language compiler, and what is left
//! (clips, scenes, commands and notes) becomes the timed events a client sends.

use std::ops::Deref;

use niminal_lang::{
    CompileOptions, ControlDef, Diagnostic, Instrument, NodeInfo, Program, resolve_target,
};
use niminal_lang::{Span, closest};
use niminal_score::{Event, Tempo, Time, Tuning};

use crate::ast::*;
use crate::dsp::{DspSource, project};
use crate::literal::{arg_error, literal_args, literal_time};
use crate::parser::parse_spanned;
use crate::perform::{Context, compile_performance};
use crate::performance::{Arrangement, Quantize, Scheduled};

type Res<T> = Result<T, Diagnostic>;

const DEFAULT_BPM: f64 = 120.0;

/// `config { quantize: { ... } }`: when each kind of change lands, where the project says.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct QuantizeConfig {
    pub default: Option<Quantize>,
    pub play: Option<Quantize>,
    pub instrument: Option<Quantize>,
    pub clip: Option<Quantize>,
    pub control: Option<Quantize>,
    pub tempo: Option<Quantize>,
}

/// A control driven from outside: a message value from 0 to 1 maps onto `lo..hi`
/// (engine units; geometric for hz and db, so sweeps are even by ear).
#[derive(Clone, Debug, PartialEq)]
pub struct ControlBinding {
    pub source: InputSource,
    pub lo: f32,
    pub hi: f32,
}

impl ControlBinding {
    /// The control's value, in its unit's own suffix (so `ControlDef::convert` accepts it), for a
    /// normalised input `t` from 0 to 1.
    pub fn map_input(&self, control: &ControlDef, t: f64) -> f64 {
        interpolate(control, self.lo, self.hi, t)
    }
}

/// The value `t` (0 to 1) of the way from `lo` to `hi` (engine units), in the control's unit suffix: even by ear
/// for hz and db, so sweeps sound the same all the way along.
pub fn interpolate(control: &ControlDef, lo: f32, hi: f32, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let (lo, hi) = (f64::from(lo), f64::from(hi));
    let geometric = matches!(
        control.unit,
        niminal_lang::Unit::Hz | niminal_lang::Unit::Db
    ) && lo > 0.0
        && hi > 0.0;
    let engine = if geometric {
        lo * (hi / lo).powf(t)
    } else {
        lo + (hi - lo) * t
    };
    if control.unit == niminal_lang::Unit::Db {
        20.0 * engine.log10()
    } else {
        engine
    }
}

/// One control a macro moves: it is at `lo` when the macro is at 0 and at `hi` when it is at 1 (engine units).
#[derive(Clone, Debug, PartialEq)]
pub struct MacroTarget {
    pub control: String,
    pub lo: f32,
    pub hi: f32,
}

/// A macro: a control from 0 to 1 that moves other controls along with it.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroDef {
    pub name: String,
    pub targets: Vec<MacroTarget>,
}

/// Commands to run when something arrives from outside, as they are written.
#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub source: InputSource,
    pub commands: Vec<String>,
}

/// A composition: the program its definitions make, and what the client sequences on it.
#[derive(Clone)]
pub struct Composition {
    pub program: Program,
    /// The text it was compiled from.
    pub source: String,
    /// The tempo the composition plays at; the program itself has none.
    pub tempo: Tempo,
    pub quantize: QuantizeConfig,
    /// The controls that MIDI or OSC drive, by control name.
    pub inputs: Vec<(String, ControlBinding)>,
    /// The macros: controls that move others.
    pub macros: Vec<MacroDef>,
    /// Commands that run when a key, controller or OSC message arrives: `on midi.note(36) beat.start`.
    pub triggers: Vec<Trigger>,
    /// Notes written directly in the source.
    pub notes: Vec<Event>,
    /// Commands that play clips and scenes, in source order.
    pub performance: Vec<Scheduled>,
    pub arrangements: Vec<Arrangement>,
    /// Every clip the source defines, by name, whether or not a command plays it.
    pub clips: std::collections::BTreeMap<String, crate::Clip>,
}

impl Deref for Composition {
    type Target = Program;
    fn deref(&self) -> &Program {
        &self.program
    }
}

impl Composition {
    /// Resolve a musical note while keeping engine graph terminology out of diagnostics.
    pub fn plan(&self, event: &Event) -> Result<niminal_lang::NotePlan, niminal_lang::ArgError> {
        self.program.plan(event).map_err(|mut error| {
            error.message = error
                .message
                .replace("instrument or node", "instrument or clip");
            error
        })
    }

    /// What drives the control called `name`, if anything.
    pub fn input(&self, name: &str) -> Option<&ControlBinding> {
        self.inputs.iter().find(|(n, _)| n == name).map(|(_, b)| b)
    }

    /// The macro called `name`, if it is one.
    pub fn macro_of(&self, name: &str) -> Option<&MacroDef> {
        self.macros.iter().find(|m| m.name == name)
    }
}

/// What the names of a composition are, for telling a binding from a clash with a definition.
pub(crate) struct Names<'a> {
    kinds: Vec<(String, &'static str)>,
    pub controls: &'a [ControlDef],
}

impl Names<'_> {
    pub fn kind_of(&self, name: &str) -> Option<&'static str> {
        self.kinds.iter().find(|(n, _)| n == name).map(|(_, k)| *k)
    }
}

fn names_of<'a>(items: &[Item], controls: &'a [ControlDef]) -> Names<'a> {
    let mut kinds: Vec<(String, &'static str)> = Vec::new();
    for item in items {
        let (ident, kind) = match item {
            Item::Control { name, .. } => (name, "control"),
            Item::Macro(d) => (&d.name, "control"),
            Item::Instr(d) => (&d.name, "instrument"),
            Item::MidiOut(d) => (&d.name, "instrument"),
            Item::Fn(d) => (&d.name, "function"),
            Item::Const { name, .. } => (name, "constant"),
            Item::Bus(d) => (&d.name, "bus"),
            Item::Buffer(d) => (&d.name, "buffer"),
            Item::Fx(d) => (&d.name, "fx"),
            Item::Sample(d) => (&d.name, if d.is_kit { "kit" } else { "sample" }),
            Item::Arrangement(d) => (&d.name, "arrangement"),
            Item::Def(d) => (&d.name, "score function"),
            _ => continue,
        };
        if !kinds.iter().any(|(n, _)| *n == ident.name) {
            kinds.push((ident.name.clone(), kind));
        }
    }
    Names { kinds, controls }
}

pub fn compile(source: &str) -> Result<Composition, Vec<Diagnostic>> {
    compile_with(source, &CompileOptions::default())
}

pub fn compile_with(
    source: &str,
    options: &CompileOptions,
) -> Result<Composition, Vec<Diagnostic>> {
    let parsed = parse_spanned(source).map_err(|d| vec![d])?;
    let mut items: Vec<Item> = parsed.iter().map(|(item, _)| item.clone()).collect();
    items = expand_score_fors(items).map_err(|d| vec![d])?;
    let mut errors = Vec::new();

    let meter = meter_of(&items).unwrap_or_else(|d| {
        errors.push(d);
        (4.0, 4)
    });
    let tempo = tempo_of(&items, meter, Tuning::default()).unwrap_or_else(|d| {
        errors.push(d);
        Tempo {
            bpm: DEFAULT_BPM,
            beats_per_bar: meter.0,
            beat_unit: meter.1,
            tuning: Tuning::default(),
        }
    });
    // `launch: midi.note(36)` on a clip or scene is a trigger that starts it, and is not a lane or a member.
    let mut launches = Vec::new();
    for item in &mut items {
        if let Item::Clip(block) | Item::Scene(block) = item
            && let Some(at) = block
                .entries
                .iter()
                .position(|(key, _)| key.name == "launch")
        {
            let (_, value) = block.entries.remove(at);
            match launch_source(&value, "launch") {
                Ok(source) => launches.push(Trigger {
                    source,
                    commands: vec![format!("{}.start", block.name.name)],
                }),
                Err(d) => errors.push(d),
            }
        }
    }
    // Instrument selection is lowered to DSP nodes, leaving just the clip lanes.
    for item in &mut items {
        if let Item::Clip(block) = item
            && let Some(at) = block
                .entries
                .iter()
                .position(|(key, _)| key.name == "instrument")
        {
            block.entries.remove(at);
        }
    }

    let dsp = project(source, &parsed, tempo, true).map_err(|d| vec![d])?;
    let mut program = match niminal_lang::compile_with(&dsp.text, options) {
        Ok(program) => program,
        Err(found) => {
            errors.extend(found.into_iter().map(|d| locate(&dsp, d)));
            return Err(errors);
        }
    };
    // the program was made from the DSP text, which leaves out comments, timing and notes, so what
    // it says about places in the text is moved to where those places are in the composition
    for instrument in &mut program.instruments {
        for hint in &mut instrument.rate_hints {
            hint.span = dsp.locate(hint.span);
        }
    }
    let tempo = Tempo {
        tuning: program.tempo.tuning,
        ..tempo
    };

    let names = names_of(&items, &program.controls);
    let cx = Context {
        tempo,
        names: &names,
        instruments: &program.instruments,
        nodes: &program.nodes,
        failed: &[],
        sample_names: &program.sample_names,
        samples: &options.samples,
        origins: options.origins.as_deref(),
    };
    let quantize = quantize_config(&items).unwrap_or_else(|d| {
        errors.push(d);
        Default::default()
    });
    let (performance, arrangements, clips) = match compile_performance(&items, &cx) {
        Ok(compiled) => (compiled.actions, compiled.arrangements, compiled.clips),
        Err(mut found) => {
            errors.append(&mut found);
            (Vec::new(), Vec::new(), Default::default())
        }
    };

    let mut notes = Vec::new();
    for item in &items {
        let Item::Note(note) = item else { continue };
        match check_note(note, &program.instruments, &program.nodes, tempo) {
            Ok(e) => notes.push(e),
            Err(d) => errors.push(d),
        }
    }
    let inputs = match control_inputs(source, &items, &program, options) {
        Ok(inputs) => inputs,
        Err(d) => {
            errors.push(d);
            Vec::new()
        }
    };

    let macros = match compile_macros(&items, &program, tempo) {
        Ok(macros) => macros,
        Err(mut found) => {
            errors.append(&mut found);
            Vec::new()
        }
    };

    let triggers = match triggers(source, &items, launches) {
        Ok(triggers) => triggers,
        Err(d) => {
            errors.push(d);
            Vec::new()
        }
    };

    errors.retain(|d| !d.message.is_empty());
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Composition {
        program,
        source: source.to_string(),
        tempo,
        quantize,
        inputs,
        macros,
        triggers,
        notes,
        performance,
        arrangements,
        clips,
    })
}

/// A diagnostic about the DSP text, placed in the composition it was made from.
fn locate(dsp: &DspSource, mut d: Diagnostic) -> Diagnostic {
    d.span = dsp.locate(d.span);
    d.message = d
        .message
        .replace("a node has only one", "a clip has only one");
    for name in ["width", "attack", "release"] {
        d.message = d
            .message
            .replace(&format!("`__rg_{name}`"), &format!("`{name}`"));
    }
    d.message = d.message.replace("`__niminal_rhythm_gate`", "`gate`");
    d
}

/// `midi.note(36)`, `midi.note(c1, 10)`, `midi.cc(64)` or `osc_in("/go")`, written as an expression.
fn launch_source(e: &Expr, word: &str) -> Res<InputSource> {
    let bad = || {
        Diagnostic::new(
            format!("`{word}` takes a key, a controller or an OSC address"),
            e.span,
        )
        .with_help(format!("for example `{word}: midi.cc(21)` or `{word}: osc_in(\"/verse\")`"))
    };
    let ExprKind::Call { name, args } = &e.kind else {
        return Err(bad());
    };
    let number = |a: &Arg, max: u8, what: &str| -> Res<u8> {
        let value = match &a.value.kind {
            ExprKind::Num { value, unit: None } if value.fract() == 0.0 => *value as i64,
            ExprKind::Name(n) => match n.parse::<niminal_score::Value>() {
                Ok(niminal_score::Value::Note(m)) => i64::from(m),
                _ => return Err(bad()),
            },
            _ => return Err(bad()),
        };
        u8::try_from(value)
            .ok()
            .filter(|v| *v <= max)
            .ok_or_else(|| Diagnostic::new(format!("{what} is from 0 to {max}"), a.value.span))
    };
    match (name.name.as_str(), args.as_slice()) {
        ("osc_in", [address]) if !address.receiver => match &address.value.kind {
            ExprKind::Str(a) if a.starts_with('/') => Ok(InputSource::Osc(a.clone())),
            _ => Err(Diagnostic::new(
                "an OSC address starts with `/`",
                address.value.span,
            )),
        },
        (which @ ("note" | "cc"), [receiver, first, rest @ ..])
            if receiver.receiver
                && matches!(&receiver.value.kind, ExprKind::Name(m) if m == "midi")
                && rest.len() <= 1 =>
        {
            let (max, what) = if which == "note" {
                (127, "a note number")
            } else {
                (127, "a CC number")
            };
            let number_value = number(first, max, what)?;
            let channel = match rest {
                [c] => match number(c, 16, "a channel")? {
                    0 => return Err(Diagnostic::new("MIDI channels count from 1", c.value.span)),
                    c => Some(c),
                },
                _ => None,
            };
            Ok(if which == "note" {
                InputSource::MidiNote {
                    note: number_value,
                    channel,
                }
            } else {
                InputSource::MidiCc {
                    cc: number_value,
                    channel,
                }
            })
        }
        _ => Err(bad()),
    }
}

/// The `on` lines: what to run when each source fires. Each must be a command.
fn triggers(source: &str, items: &[Item], launches: Vec<Trigger>) -> Res<Vec<Trigger>> {
    let mut out: Vec<Trigger> = Vec::new();
    for launch in launches {
        match out.iter_mut().find(|t| t.source == launch.source) {
            Some(existing) => existing.commands.extend(launch.commands),
            None => out.push(launch),
        }
    }
    for item in items {
        let Item::On(decl) = item else { continue };
        let mut commands = Vec::new();
        for span in &decl.commands {
            let text = &source[span.start..span.end];
            let not_a_command = || {
                Diagnostic::new("an `on` runs commands", *span)
                    .with_help("for example `on midi.note(36) beat.start`")
            };
            let statements = crate::analysis::analyze(text).map_err(|_| not_a_command())?;
            if statements.is_empty()
                || statements
                    .iter()
                    .any(|s| s.kind() != crate::StatementKind::Command)
            {
                return Err(not_a_command());
            }
            commands.push(text.to_string());
        }
        match out.iter_mut().find(|t| t.source == decl.source) {
            Some(existing) => existing.commands.extend(commands),
            None => out.push(Trigger {
                source: decl.source.clone(),
                commands,
            }),
        }
    }
    Ok(out)
}

/// The controls that MIDI or OSC drive: `ctl cutoff = midi.cc(21).range(200hz..4khz)`.
fn control_inputs(
    source: &str,
    items: &[Item],
    program: &Program,
    options: &CompileOptions,
) -> Res<Vec<(String, ControlBinding)>> {
    let mut out = Vec::new();
    // a macro's `input:` drives the macro's own control, over 0 to 1
    let mut from_macros: Vec<(Ident, ControlInput)> = Vec::new();
    for item in items {
        if let Item::Macro(block) = item
            && let Some((_, value)) = block.entries.iter().find(|(key, _)| key.name == "input")
        {
            from_macros.push((
                block.name.clone(),
                ControlInput {
                    source: launch_source(value, "input")?,
                    hi: Expr {
                        kind: ExprKind::Num {
                            value: 1.0,
                            unit: None,
                        },
                        span: value.span,
                    },
                },
            ));
        }
    }
    let driven: Vec<(&Ident, &ControlInput)> = items
        .iter()
        .filter_map(|item| match item {
            Item::Control {
                name,
                input: Some(input),
                ..
            } => Some((name, input)),
            _ => None,
        })
        .chain(from_macros.iter().map(|(name, input)| (name, input)))
        .collect();
    if driven.is_empty() {
        return Ok(out);
    }
    // a plain number is its own upper end; anything else (`4khz`, a name, a sum) is worked out by the
    // compiler, among the definitions it may use, which compiles the program again
    let literal: Vec<Option<f32>> = driven
        .iter()
        .map(|(name, input)| {
            let def = program.controls.iter().find(|c| c.name == name.name)?;
            match (&input.hi.kind, def.unit) {
                (ExprKind::Num { value, unit: None }, niminal_lang::Unit::Num) => Some(*value as f32),
                _ => None,
            }
        })
        .collect();
    let found = if literal.iter().all(Option::is_some) {
        None
    } else {
        let mut text = String::new();
        for (n, (_, input)) in driven.iter().enumerate() {
            text.push_str(&format!(
                "
ctl __input_hi_{n} = {}",
                crate::dsp::control_value(source, &input.hi)
            ));
        }
        let base = &program.source;
        Some(
            niminal_lang::compile_with(&format!("{base}{text}"), options)
                .map_err(|mut found| found.remove(0))
                .map_err(|mut d| {
                    d.span = driven[0].1.hi.span;
                    d
                })?,
        )
    };
    for (n, (name, input)) in driven.iter().enumerate() {
        let def = program
            .controls
            .iter()
            .find(|c| c.name == name.name)
            .expect("a control is compiled");
        let (hi_unit, hi_initial) = match (literal[n], &found) {
            (Some(value), _) => (def.unit, value),
            (None, Some(found)) => {
                let hi = found
                    .controls
                    .iter()
                    .find(|c| c.name == format!("__input_hi_{n}"))
                    .expect("a control is compiled");
                (hi.unit, hi.initial)
            }
            (None, None) => unreachable!("a range that is not a number is compiled"),
        };
        if hi_unit != def.unit {
            return Err(Diagnostic::new(
                format!(
                    "the range's ends need the same unit: {} and {}",
                    def.unit.describe(),
                    hi_unit.describe()
                ),
                input.hi.span,
            ));
        }
        if !hi_initial.is_finite() || hi_initial <= def.initial {
            return Err(Diagnostic::new(
                "the upper end of a range must be above the lower end",
                input.hi.span,
            ));
        }
        out.push((
            name.name.clone(),
            ControlBinding {
                source: input.source.clone(),
                lo: def.initial,
                hi: hi_initial,
            },
        ));
    }
    Ok(out)
}

/// A control name as the code writes it: `break_stutter.damp`, not the `break_stutter::damp` it is kept as.
pub(crate) fn written(control: &str) -> String {
    control.replace("::", ".")
}

/// `200hz`, `0.6` or `-12` in a range: the number and its unit.
fn endpoint(e: &Expr) -> Option<(f64, Option<&str>)> {
    match &e.kind {
        ExprKind::Num { value, unit } => Some((*value, unit.as_deref())),
        ExprKind::Neg(inner) => endpoint(inner).map(|(value, unit)| (-value, unit)),
        _ => None,
    }
}

/// The macros: what each moves, over what range. A macro's own entries are `input:` and `smooth:`; every other
/// entry is a control to move and the range to move it over, written `cutoff: 200hz..4khz`, in a block for an
/// effect (`break_stutter { damp: 0..0.6 }`) or with a dot (`break_stutter.damp: 0..0.6`).
fn compile_macros(items: &[Item], program: &Program, tempo: Tempo) -> Result<Vec<MacroDef>, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut out: Vec<MacroDef> = Vec::new();
    let macro_names: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            Item::Macro(block) => Some(block.name.name.as_str()),
            _ => None,
        })
        .collect();
    for item in items {
        let Item::Macro(block) = item else { continue };
        if out.iter().any(|m| m.name == block.name.name) {
            errors.push(Diagnostic::new(
                format!("macro `{}` is defined twice", block.name.name),
                block.name.span,
            ));
            continue;
        }
        let mut wanted: Vec<(String, &Expr, Span)> = Vec::new();
        for (key, value) in &block.entries {
            match key.name.as_str() {
                "input" | "smooth" => {}
                _ => match &value.kind {
                    ExprKind::FxLanes(rows) => {
                        for (row, range) in rows {
                            wanted.push((format!("{}::{}", key.name, row.name), range, row.span));
                        }
                    }
                    _ => wanted.push((key.name.clone(), value, key.span)),
                },
            }
        }
        if wanted.is_empty() {
            errors.push(
                Diagnostic::new(
                    format!("macro `{}` moves nothing", block.name.name),
                    block.name.span,
                )
                .with_help("give it a control and a range, such as `cutoff: 200hz..4khz`"),
            );
            continue;
        }
        let mut targets: Vec<MacroTarget> = Vec::new();
        for (control, range, span) in wanted {
            let shown = written(&control);
            if macro_names.contains(&&*control) {
                errors.push(Diagnostic::new(
                    format!("`{shown}` is a macro; a macro can't move another macro"),
                    span,
                ));
                continue;
            }
            let Some(def) = program.controls.iter().find(|c| c.name == control) else {
                let mut d = Diagnostic::new(format!("no control named `{shown}`"), span);
                if let Some(near) = closest(
                    &control,
                    program
                        .controls
                        .iter()
                        .map(|c| c.name.as_str())
                        .filter(|n| !n.starts_with("__")),
                ) {
                    d = d.with_help(format!("did you mean `{}`?", written(near)));
                }
                errors.push(d);
                continue;
            };
            if targets.iter().any(|t| t.control == control) {
                errors.push(Diagnostic::new(
                    format!("macro `{}` moves `{shown}` twice", block.name.name),
                    span,
                ));
                continue;
            }
            let bounds = match &range.kind {
                ExprKind::Range {
                    lo,
                    hi,
                    inclusive: true,
                } => endpoint(lo).zip(endpoint(hi)),
                _ => None,
            };
            let Some(((lo, lo_unit), (hi, hi_unit))) = bounds else {
                errors.push(
                    Diagnostic::new(
                        format!("`{shown}` needs the range the macro moves it over"),
                        range.span,
                    )
                    .with_help("two numbers, such as `200hz..4khz` or `0..0.6`; the first is where it is when the macro is at 0"),
                );
                continue;
            };
            match (def.convert(lo, lo_unit, tempo), def.convert(hi, hi_unit, tempo)) {
                (Ok(lo), Ok(hi)) => targets.push(MacroTarget { control, lo, hi }),
                (Err(why), _) | (_, Err(why)) => errors.push(Diagnostic::new(why, range.span)),
            }
        }
        out.push(MacroDef {
            name: block.name.name.clone(),
            targets,
        });
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// Expand top-level `for` / `if` blocks into repeated or chosen notes and commands.
fn expand_score_fors(items: Vec<Item>) -> Result<Vec<Item>, Diagnostic> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::For {
                var,
                iter,
                body,
                span,
            } => {
                let values = const_list(&iter)?;
                if values.len() > 10_000 {
                    return Err(Diagnostic::new(
                        "this `for` expands to too many items",
                        span,
                    ));
                }
                for value in values {
                    let mut body = body.clone();
                    for item in &mut body {
                        substitute_item(item, &var.name, &value);
                    }
                    out.extend(expand_score_fors(body)?);
                }
            }
            Item::If {
                cond,
                then,
                otherwise,
                span,
            } => {
                let take = const_truthy(&cond).map_err(|d| {
                    Diagnostic::new(d.message, span).with_help(
                        "a top-level `if` needs a condition known while compiling, such as `i > 2`",
                    )
                })?;
                let body = if take { then } else { otherwise };
                out.extend(expand_score_fors(body)?);
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

fn const_truthy(e: &Expr) -> Result<bool, Diagnostic> {
    match &e.kind {
        ExprKind::Num { value, unit: None } => Ok(*value != 0.0),
        ExprKind::Name(n) if n == "true" => Ok(true),
        ExprKind::Name(n) if n == "false" => Ok(false),
        ExprKind::Compare { op, lhs, rhs } => {
            let l = const_number(lhs)?;
            let r = const_number(rhs)?;
            Ok(match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Le => l <= r,
                CmpOp::Gt => l > r,
                CmpOp::Ge => l >= r,
            })
        }
        ExprKind::Not(inner) => Ok(!const_truthy(inner)?),
        ExprKind::Logic { op, lhs, rhs } => {
            let (l, r) = (const_truthy(lhs)?, const_truthy(rhs)?);
            Ok(match op {
                LogicOp::And => l && r,
                LogicOp::Or => l || r,
            })
        }
        _ => Err(Diagnostic::new(
            "expected a condition known while compiling",
            e.span,
        )),
    }
}

fn const_list(e: &Expr) -> Result<Vec<Expr>, Diagnostic> {
    match &e.kind {
        ExprKind::Range { lo, hi, inclusive } => {
            let lo = const_number(lo)?;
            let hi = const_number(hi)?;
            let mut out = Vec::new();
            let mut i = lo.floor() as i64;
            let end = hi.floor() as i64;
            while if *inclusive { i <= end } else { i < end } {
                out.push(Expr {
                    kind: ExprKind::Num {
                        value: i as f64,
                        unit: None,
                    },
                    span: e.span,
                });
                i += 1;
                if out.len() > 10_000 {
                    break;
                }
            }
            Ok(out)
        }
        ExprKind::Array(items) => Ok(items.clone()),
        _ => Err(Diagnostic::new(
            "a top-level `for` needs a range or list known while compiling",
            e.span,
        )
        .with_help("for example `for i in 0..<16 { ... }`")),
    }
}

fn const_number(e: &Expr) -> Result<f64, Diagnostic> {
    match &e.kind {
        ExprKind::Num { value, unit: None } => Ok(*value),
        ExprKind::Neg(inner) => Ok(-const_number(inner)?),
        _ => Err(Diagnostic::new("expected a plain number", e.span)),
    }
}

fn substitute_item(item: &mut Item, name: &str, value: &Expr) {
    match item {
        Item::Note(n) => {
            if let Some(at) = &mut n.at {
                substitute_at(at, name, value);
            }
            for arg in &mut n.args {
                substitute_expr(&mut arg.value, name, value);
            }
            substitute_expr(&mut n.dur, name, value);
        }
        Item::Command(c) => {
            if let Some(at) = &mut c.at {
                substitute_at(at, name, value);
            }
            if let Command::Update { what, .. } = &mut c.command {
                substitute_expr(what, name, value);
            }
        }
        Item::For { iter, body, .. } => {
            substitute_expr(iter, name, value);
            for item in body {
                substitute_item(item, name, value);
            }
        }
        Item::If {
            cond,
            then,
            otherwise,
            ..
        } => {
            substitute_expr(cond, name, value);
            for item in then.iter_mut().chain(otherwise.iter_mut()) {
                substitute_item(item, name, value);
            }
        }
        _ => {}
    }
}

fn substitute_at(at: &mut AtPos, name: &str, value: &Expr) {
    match at {
        AtPos::Time(e) | AtPos::After(e) => substitute_expr(e, name, value),
        AtPos::Bar { bar, beat } => {
            substitute_expr(bar, name, value);
            if let Some(b) = beat {
                substitute_expr(b, name, value);
            }
        }
    }
}

fn substitute_expr(e: &mut Expr, name: &str, value: &Expr) {
    if matches!(&e.kind, ExprKind::Name(n) if n == name) {
        *e = value.clone();
        return;
    }
    e.for_each_child_mut(&mut |child| substitute_expr(child, name, value));
}

/// `config { quantize: { default: 1bar  play: cycle } }`, and the `quantize` of definitions checked.
fn quantize_config(items: &[Item]) -> Res<QuantizeConfig> {
    let mut config = QuantizeConfig::default();
    for item in items {
        match item {
            Item::Config(c) => {
                for (key, value) in &c.quantize {
                    let slot = match key.name.as_str() {
                        "default" => &mut config.default,
                        "play" => &mut config.play,
                        "instr" => &mut config.instrument,
                        "clip" => &mut config.clip,
                        "ctl" => &mut config.control,
                        "tempo" => &mut config.tempo,
                        other => {
                            let mut d = Diagnostic::new(
                                format!("unknown kind of change `{other}`"),
                                key.span,
                            );
                            d = d
                                .with_help("the kinds are: default, play, instr, clip, ctl, tempo");
                            return Err(d);
                        }
                    };
                    if slot.is_some() {
                        return Err(Diagnostic::new(
                            format!("`{}` is given twice", key.name),
                            key.span,
                        ));
                    }
                    *slot = Some(crate::perform::quantum_of(value)?);
                }
            }
            Item::Clip(b) | Item::Scene(b) => {
                crate::perform::definition_quantum(&b.options)?;
            }
            _ => {}
        }
    }
    Ok(config)
}

/// Beats per bar and beat unit from `meter 3/4`: the top over the bottom in quarter-note beats, and the bottom.
fn meter_of(items: &[Item]) -> Res<(f64, u32)> {
    let mut meter = None;
    for item in items {
        let Item::Meter(e) = item else { continue };
        if meter.is_some() {
            return Err(Diagnostic::new("meter is set more than once", e.span));
        }
        meter = Some(meter_beats(e)?);
    }
    Ok(meter.unwrap_or((4.0, 4)))
}

/// Beats per bar and beat unit from a fraction such as `3/4`.
pub(crate) fn meter_beats(e: &Expr) -> Res<(f64, u32)> {
    let whole = |e: &Expr| match &e.kind {
        ExprKind::Num { value, unit: None } if value.fract() == 0.0 && *value >= 1.0 => {
            Some(*value)
        }
        _ => None,
    };
    let parsed = match &e.kind {
        ExprKind::Binary {
            op: BinOp::Div,
            lhs,
            rhs,
        } => whole(lhs).zip(whole(rhs)),
        _ => None,
    };
    let Some((top, bottom)) = parsed else {
        return Err(
            Diagnostic::new("meter is a fraction of whole numbers", e.span)
                .with_help("for example `meter 3/4` or `meter 7/8`"),
        );
    };
    Ok((top * 4.0 / bottom, bottom as u32))
}

/// Beats per minute from a number such as `120bpm`.
pub(crate) fn bpm_of(e: &Expr) -> Res<f64> {
    match &e.kind {
        ExprKind::Num {
            value,
            unit: Some(u),
        } if u == "bpm" && *value > 0.0 => Ok(*value),
        _ => Err(
            Diagnostic::new("tempo must be a positive number of bpm", e.span)
                .with_help("for example `tempo 120bpm`"),
        ),
    }
}

fn tempo_of(items: &[Item], meter: (f64, u32), tuning: Tuning) -> Res<Tempo> {
    let mut tempo = None;
    for item in items {
        let Item::Tempo(stmt) = item else { continue };
        if tempo.is_some() {
            return Err(Diagnostic::new("tempo is set more than once", stmt.span));
        }
        // `@ next bar` and `over 2bars` say how a live session gets there; a file starts at the tempo.
        tempo = Some(Tempo {
            bpm: bpm_of(&stmt.value)?,
            beats_per_bar: meter.0,
            beat_unit: meter.1,
            tuning,
        });
    }
    Ok(tempo.unwrap_or(Tempo {
        bpm: DEFAULT_BPM,
        beats_per_bar: meter.0,
        beat_unit: meter.1,
        tuning,
    }))
}

fn check_note(
    note: &NoteStmt,
    instruments: &[Instrument],
    nodes: &[NodeInfo],
    tempo: Tempo,
) -> Res<Event> {
    let name = &note.target;
    let target =
        resolve_target(instruments, nodes, &name.name).map_err(|e| arg_error(e, name.span))?;
    let instr = &instruments[target.instrument];

    let at = match &note.at {
        None => Time::Beats(0.0),
        Some(AtPos::Time(e) | AtPos::After(e)) => literal_time(e)?,
        Some(AtPos::Bar { bar, beat }) => bar_position(bar, beat.as_ref(), tempo)?,
    };
    let dur = literal_time(&note.dur)?;

    let args = literal_args(&note.args, instr, &[], tempo)?;
    let mut merged = target.defaults.cloned().unwrap_or_default();
    merged.extend(args.iter().map(|(k, v)| (k.clone(), *v)));
    instr
        .bind_args(&merged, tempo)
        .map_err(|e| arg_error(e, note.span))?;

    Ok(Event {
        target: name.name.clone(),
        at,
        dur,
        args,
    })
}

/// `bar 5 beat 3`, counting from 1, as a number of beats.
fn bar_position(bar: &Expr, beat: Option<&Expr>, tempo: Tempo) -> Res<Time> {
    let number = |e: &Expr| match e.kind {
        ExprKind::Num { value, unit: None } if value >= 1.0 => Ok(value),
        _ => Err(Diagnostic::new(
            "bars and beats are numbers counting from 1",
            e.span,
        )),
    };
    let beat = beat.map(number).transpose()?.unwrap_or(1.0);
    Ok(Time::Beats(
        (number(bar)? - 1.0) * tempo.beats_per_bar + (beat - 1.0),
    ))
}
