//! Compiling a composition: its sound-server definitions go to the language compiler, and what is left
//! (clips, scenes, commands and notes) becomes the timed events a client sends.

use std::collections::HashMap;
use std::ops::Deref;

use niminal_lang::{CompileOptions, ControlDef, Diagnostic, Instrument, Program, TrackInfo, resolve_target};
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
    pub track: Option<Quantize>,
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
        let t = t.clamp(0.0, 1.0);
        let (lo, hi) = (f64::from(self.lo), f64::from(self.hi));
        let geometric = matches!(control.unit, niminal_lang::Unit::Hz | niminal_lang::Unit::Db) && lo > 0.0 && hi > 0.0;
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
    /// Notes written directly in the source.
    pub notes: Vec<Event>,
    /// Commands that play clips and scenes, in source order.
    pub performance: Vec<Scheduled>,
    pub arrangements: Vec<Arrangement>,
}

impl Deref for Composition {
    type Target = Program;
    fn deref(&self) -> &Program {
        &self.program
    }
}

impl Composition {
    /// What drives the control called `name`, if anything.
    pub fn input(&self, name: &str) -> Option<&ControlBinding> {
        self.inputs.iter().find(|(n, _)| n == name).map(|(_, b)| b)
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
    let clip_names: Vec<&str> = items
        .iter()
        .filter_map(|i| if let Item::Clip(b) = i { Some(b.name.name.as_str()) } else { None })
        .collect();
    let mut kinds: Vec<(String, &'static str)> = Vec::new();
    for item in items {
        let (ident, kind) = match item {
            Item::Control { name, .. } => (name, "control"),
            Item::Instr(d) => (&d.name, "instrument"),
            Item::MidiOut(d) => (&d.name, "instrument"),
            Item::Fn(d) => (&d.name, "function"),
            Item::Const { name, .. } => (name, "constant"),
            Item::Bus(d) => (&d.name, "bus"),
            Item::Buffer(d) => (&d.name, "buffer"),
            // a clip with an `instrument:` is a track of its own name; the clip may share it
            Item::Track(d) => (&d.name, if clip_names.contains(&d.name.name.as_str()) { "part" } else { "track" }),
            Item::Sample(d) => (&d.name, if d.is_kit { "kit" } else { "sample" }),
            Item::Arrangement(d) => (&d.name, "arrangement"),
            Item::Def(d) => (&d.name, "score function"),
            _ => continue,
        };
        if !kinds.iter().any(|(n, _)| *n == ident.name) {
            kinds.push((ident.name.clone(), kind));
        }
    }
    // a clip with an `instrument:` is also a track of that name
    for item in items {
        if let Item::Clip(b) = item
            && b.entries.iter().any(|(k, _)| k.name == "instrument")
            && !kinds.iter().any(|(n, _)| *n == b.name.name)
        {
            kinds.push((b.name.name.clone(), "part"));
        }
    }
    Names { kinds, controls }
}

pub fn compile(source: &str) -> Result<Composition, Vec<Diagnostic>> {
    compile_with(source, &CompileOptions::default())
}

pub fn compile_with(source: &str, options: &CompileOptions) -> Result<Composition, Vec<Diagnostic>> {
    let parsed = parse_spanned(source).map_err(|d| vec![d])?;
    let mut items: Vec<Item> = parsed.iter().map(|(item, _)| item.clone()).collect();
    items = expand_score_fors(items).map_err(|d| vec![d])?;
    let mut errors = Vec::new();

    let beats_per_bar = meter_of(&items).unwrap_or_else(|d| {
        errors.push(d);
        4.0
    });
    let tempo = tempo_of(&items, beats_per_bar, Tuning::default()).unwrap_or_else(|d| {
        errors.push(d);
        Tempo { bpm: DEFAULT_BPM, beats_per_bar, tuning: Tuning::default() }
    });
    let implicit = implicit_tracks(&items);
    // the clips' `instrument:` is the track they stand for, not something to play
    for item in &mut items {
        if let Item::Clip(block) = item
            && let Some(at) = block.entries.iter().position(|(key, _)| key.name == "instrument")
        {
            block.entries.remove(at);
        }
    }

    let dsp = project(source, &parsed, tempo, &implicit, true).map_err(|d| vec![d])?;
    let program = match niminal_lang::compile_with(&dsp.text, options) {
        Ok(program) => program,
        Err(found) => {
            errors.extend(found.into_iter().map(|d| locate(&dsp, d)));
            return Err(errors);
        }
    };
    let tempo = Tempo { tuning: program.tempo.tuning, ..tempo };

    let names = names_of(&items, &program.controls);
    let track_notes = match track_notes(&items) {
        Ok(notes) => notes,
        Err(d) => {
            errors.push(d);
            HashMap::new()
        }
    };
    let cx = Context {
        tempo,
        names: &names,
        track_notes: &track_notes,
        instruments: &program.instruments,
        tracks: &program.tracks,
        failed: &[],
        sample_names: &program.sample_names,
        samples: &options.samples,
    };
    let quantize = quantize_config(&items).unwrap_or_else(|d| {
        errors.push(d);
        Default::default()
    });
    let (performance, arrangements) = match compile_performance(&items, &cx) {
        Ok(compiled) => (compiled.actions, compiled.arrangements),
        Err(mut found) => {
            errors.append(&mut found);
            (Vec::new(), Vec::new())
        }
    };

    let mut notes = Vec::new();
    for item in &items {
        let Item::Note(note) = item else { continue };
        match check_note(note, &program.instruments, &program.tracks, tempo) {
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

    errors.retain(|d| !d.message.is_empty());
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Composition { program, source: source.to_string(), tempo, quantize, inputs, notes, performance, arrangements })
}

/// The `notes = ...` line of each track that has one.
fn track_notes(items: &[Item]) -> Res<HashMap<String, Expr>> {
    let mut out = HashMap::new();
    for item in items {
        let Item::Track(track) = item else { continue };
        for stmt in &track.body {
            let Stmt::Bind { name, value } = stmt else { continue };
            if name.name != "notes" {
                continue;
            }
            if out.insert(track.name.name.clone(), value.clone()).is_some() {
                return Err(Diagnostic::new("a track has only one `notes` line", name.span));
            }
        }
    }
    Ok(out)
}

/// A diagnostic about the DSP text, placed in the composition it was made from.
fn locate(dsp: &DspSource, mut d: Diagnostic) -> Diagnostic {
    d.span = dsp.locate(d.span);
    d
}

/// The controls that MIDI or OSC drive: `ctl cutoff = midi.cc(21).range(200hz..4khz)`.
fn control_inputs(
    source: &str,
    items: &[Item],
    program: &Program,
    options: &CompileOptions,
) -> Res<Vec<(String, ControlBinding)>> {
    let mut out = Vec::new();
    let driven: Vec<(&Ident, &ControlInput)> = items
        .iter()
        .filter_map(|item| match item {
            Item::Control { name, input: Some(input), .. } => Some((name, input)),
            _ => None,
        })
        .collect();
    if driven.is_empty() {
        return Ok(out);
    }
    // the upper ends of the ranges are worked out by the compiler, among the definitions they may use
    let mut text = String::new();
    for (n, (_, input)) in driven.iter().enumerate() {
        text.push_str(&format!("
ctl __input_hi_{n} = {}", &source[input.hi.span.start..input.hi.span.end]));
    }
    let base = &program.source;
    let found = niminal_lang::compile_with(&format!("{base}{text}"), options)
        .map_err(|mut found| found.remove(0))
        .map_err(|mut d| {
            d.span = driven[0].1.hi.span;
            d
        })?;
    for (n, (name, input)) in driven.iter().enumerate() {
        let def = program.controls.iter().find(|c| c.name == name.name).expect("a control is compiled");
        let hi = found
            .controls
            .iter()
            .find(|c| c.name == format!("__input_hi_{n}"))
            .expect("a control is compiled");
        if hi.unit != def.unit {
            return Err(Diagnostic::new(
                format!(
                    "the range's ends need the same unit: {} and {}",
                    def.unit.describe(),
                    hi.unit.describe()
                ),
                input.hi.span,
            ));
        }
        if !hi.initial.is_finite() || hi.initial <= def.initial {
            return Err(Diagnostic::new(
                "the upper end of a range must be above the lower end",
                input.hi.span,
            ));
        }
        out.push((
            name.name.clone(),
            ControlBinding { source: input.source.clone(), lo: def.initial, hi: hi.initial },
        ));
    }
    Ok(out)
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
            if let Command::Play { what, .. } = &mut c.command {
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

/// Naming an instrument where a part goes (`pluck.play([c4 e4])`, or in a scene) makes a track of that name
/// playing it, so there is nothing to declare for the simple case. The names of those instruments.
fn implicit_tracks(items: &[Item]) -> Vec<String> {
    let mut used: Vec<Ident> = Vec::new();
    for item in items {
        match item {
            Item::Command(c) => match &c.command {
                Command::Play { track: t, .. }
                | Command::Stop(t)
                | Command::Mute(t)
                | Command::Unmute(t)
                | Command::Solo(t)
                | Command::Unsolo(Some(t))
                | Command::Edit { track: t, .. }
                | Command::Start { name: t, .. } => used.push(t.clone()),
                _ => {}
            },
            Item::Scene(block) => used.extend(block.entries.iter().map(|(t, _)| t.clone())),
            _ => {}
        }
    }
    let mut names: Vec<String> = Vec::new();
    for name in used {
        let is_instrument = items
            .iter()
            .any(|i| matches!(i, Item::Instr(d) if d.name.name == name.name));
        let taken = items.iter().any(|i| match i {
            Item::Track(t) => t.name.name == name.name,
            Item::Clip(b) => b.name.name == name.name,
            Item::Scene(b) => b.name.name == name.name,
            Item::Bind { name: n, .. } => n.name == name.name,
            _ => false,
        });
        if is_instrument && !taken && !names.contains(&name.name) {
            names.push(name.name);
        }
    }
    names
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
                        "track" => &mut config.track,
                        "ctl" => &mut config.control,
                        "tempo" => &mut config.tempo,
                        other => {
                            let mut d = Diagnostic::new(
                                format!("unknown kind of change `{other}`"),
                                key.span,
                            );
                            d = d.with_help(
                                "the kinds are: default, play, instr, track, ctl, tempo",
                            );
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
            Item::Track(t) => {
                crate::perform::definition_quantum(&t.options)?;
            }
            _ => {}
        }
    }
    Ok(config)
}

/// Beats per bar from `meter 3/4`: the top over the bottom, in quarter-note beats.
fn meter_of(items: &[Item]) -> Res<f64> {
    let mut beats_per_bar = None;
    for item in items {
        let Item::Meter(e) = item else { continue };
        if beats_per_bar.is_some() {
            return Err(Diagnostic::new("meter is set more than once", e.span));
        }
        beats_per_bar = Some(meter_beats(e)?);
    }
    Ok(beats_per_bar.unwrap_or(4.0))
}

/// Beats per bar from a fraction such as `3/4`.
pub(crate) fn meter_beats(e: &Expr) -> Res<f64> {
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
    Ok(top * 4.0 / bottom)
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

fn tempo_of(items: &[Item], beats_per_bar: f64, tuning: Tuning) -> Res<Tempo> {
    let mut tempo = None;
    for item in items {
        let Item::Tempo(stmt) = item else { continue };
        if tempo.is_some() {
            return Err(Diagnostic::new("tempo is set more than once", stmt.span));
        }
        // `@ next bar` and `over 2bars` say how a live session gets there; a file starts at the tempo.
        tempo = Some(Tempo {
            bpm: bpm_of(&stmt.value)?,
            beats_per_bar,
            tuning,
        });
    }
    Ok(tempo.unwrap_or(Tempo {
        bpm: DEFAULT_BPM,
        beats_per_bar,
        tuning,
    }))
}

fn check_note(
    note: &NoteStmt,
    instruments: &[Instrument],
    tracks: &[TrackInfo],
    tempo: Tempo,
) -> Res<Event> {
    let name = &note.target;
    let target =
        resolve_target(instruments, tracks, &name.name).map_err(|e| arg_error(e, name.span))?;
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
