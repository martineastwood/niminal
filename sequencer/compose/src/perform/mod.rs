//! Evaluating the performance layer: pattern literals and their operations,
//! clips, scenes, and the commands that play them.

mod music;
use music::*;
mod values;
mod ops;
mod clip_ops;
mod args;
mod clips;
mod commands;
mod grid;
use grid::*;
mod convert;
pub(crate) use convert::*;

use std::collections::{BTreeMap, HashMap};

use niminal_engine::ops::Curve;
use niminal_pattern::{
    Hit, MAX_REPEATS, Param, Pattern, Rational, Strength, Structure, grid, grid_at, parse_with,
    unit_random,
};
use niminal_score::{Tempo, Time, Value};

use crate::compile::Names;
use crate::performance::*;
use niminal_lang::GAIN_PARAM;
use niminal_lang::Unit;
use niminal_lang::ast::*;
use niminal_lang::{Diagnostic, Span, closest};
use niminal_lang::{Instrument, NodeInfo};

type Res<T> = Result<T, Diagnostic>;

/// How a segment of an envelope bends.
fn curve_of(kind: Option<CurveKind>) -> Curve {
    match kind {
        None => Curve::Linear,
        Some(CurveKind::Exp) => Curve::Exp,
        Some(CurveKind::Log) => Curve::Log,
        Some(CurveKind::Custom(k)) => Curve::Custom(k as f32),
    }
}

/// A change to a pattern, such as the `reverse` in `every(2, reverse)`.
type Change<'a, T> = Box<dyn Fn(Pattern<T>) -> Pattern<T> + 'a>;

/// The parameter that notes in a clip's `notes` lane set.
pub const PITCH_PARAM: &str = "freq";
/// Lanes every note understands, whatever the instrument.
pub(crate) const UNIVERSAL_LANES: [&str; 4] = ["dur", "nudge", GAIN_PARAM, "pan"];

/// How much louder an accent (`X`) and how much quieter a ghost note (`o`) are than a hit (`x`).
const ACCENT_DB: f64 = 3.0;
const GHOST_DB: f64 = -12.0;

#[derive(Clone)]
enum PerfValue {
    Pattern(Pattern<Step>),
    Grid(Pattern<Hit>),
    Clip(Clip),
    Scene(Scene),
    /// `env[...]`: only a lane can hold one.
    Env(EnvLane),
    /// A number with a unit, or a note, used while building clips and scores.
    Num(Value),
    /// A list from `for` or `[a, b]` in a score context.
    List(Vec<PerfValue>),
    /// A score function from `def`.
    Def(DefFn),
    Rest,
}

#[derive(Clone)]
struct DefFn {
    name: String,
    params: Vec<ParamDef>,
    body: Vec<Stmt>,
}

impl PerfValue {
    fn describe(&self) -> &'static str {
        match self {
            PerfValue::Pattern(_) => "a pattern",
            PerfValue::Grid(_) => "a grid",
            PerfValue::Clip(_) => "a clip",
            PerfValue::Scene(_) => "a scene",
            PerfValue::Env(_) => "an envelope",
            PerfValue::Num(_) => "a number",
            PerfValue::List(_) => "a list",
            PerfValue::Def(_) => "a score function",
            PerfValue::Rest => "`~`",
        }
    }

    fn as_num(&self) -> Option<Value> {
        match self {
            PerfValue::Num(v) => Some(*v),
            PerfValue::Pattern(p) => p
                .query(niminal_pattern::Span::new(Rational::ZERO, Rational::ONE))
                .into_iter()
                .next()
                .map(|h| h.value.value),
            _ => None,
        }
    }
}

/// The kinds of pattern the generic operations (`fast`, `every`...) work on.
trait Kind: Clone + Send + Sync + 'static {
    fn wrap(p: Pattern<Self>) -> PerfValue;
    fn unwrap(v: PerfValue) -> Option<Pattern<Self>>;
}

impl Kind for Step {
    fn wrap(p: Pattern<Step>) -> PerfValue {
        PerfValue::Pattern(p)
    }
    fn unwrap(v: PerfValue) -> Option<Pattern<Step>> {
        match v {
            PerfValue::Pattern(p) => Some(p),
            _ => None,
        }
    }
}

impl Kind for Hit {
    fn wrap(p: Pattern<Hit>) -> PerfValue {
        PerfValue::Grid(p)
    }
    fn unwrap(v: PerfValue) -> Option<Pattern<Hit>> {
        match v {
            PerfValue::Grid(p) => Some(p),
            _ => None,
        }
    }
}

/// Everything the performance layer's source defines.
pub(crate) struct Compiled {
    pub actions: Vec<Scheduled>,
    pub arrangements: Vec<Arrangement>,
    /// Every clip the source defines, by name, whether or not a command plays it. Editors draw
    /// these as lanes before they are started.
    pub clips: BTreeMap<String, Clip>,
}

pub(crate) struct Context<'a> {
    /// The macros, which a clip lane can move.
    pub macros: &'a [crate::compile::MacroDef],
    pub tempo: Tempo,
    pub names: &'a Names<'a>,
    /// The `notes` line of each node, which changes the notes of every clip played on it.
    pub instruments: &'a [Instrument],
    pub nodes: &'a [NodeInfo],
    /// Nodes whose definition failed to compile; mentions of them are not
    /// reported again.
    pub failed: &'a [&'a str],
    /// The names of the samples in kits, which a pattern may use as steps.
    pub sample_names: &'a [String],
    /// Where `load("…")` looks for `.nms` files.
    pub samples: &'a niminal_lang::Samples,
    /// Where each definition was sent from, if the host wants notes to remember where they were
    /// written: (start, end) in the source, the evaluation that sent it, and where it began there.
    pub origins: Option<&'a [(usize, usize, u64, usize)]>,
    /// Leave out a grid row whose drum the kit lacks, noting it in `skipped`, instead of refusing it.
    pub lenient: bool,
    pub skipped: std::cell::RefCell<Vec<crate::compile::Skipped>>,
}

struct Perform<'a> {
    cx: &'a Context<'a>,
    bindings: HashMap<String, PerfValue>,
    /// The arrangements compiled so far, which `song.start` and `song.stop` name.
    arrangements: HashMap<String, Arrangement>,
}

pub(crate) fn compile_performance(
    items: &[Item],
    cx: &Context,
) -> Result<Compiled, Vec<Diagnostic>> {
    let mut p = Perform {
        cx,
        bindings: HashMap::new(),
        arrangements: HashMap::new(),
    };
    let mut errors = Vec::new();

    // Definitions first, in order, so commands may come before what they play.
    for item in items {
        let result = match item {
            Item::Bind { name, value } => p.eval(value, None).and_then(|v| p.define(name, v)),
            Item::Clip(block) => p
                .clip(block)
                .and_then(|c| p.define(&block.name, PerfValue::Clip(c))),
            Item::Scene(block) => p
                .scene(block)
                .and_then(|s| p.define(&block.name, PerfValue::Scene(s))),
            Item::Def(def) => p.define(
                &def.name,
                PerfValue::Def(DefFn {
                    name: def.name.name.clone(),
                    params: def.params.clone(),
                    body: def.body.clone(),
                }),
            ),
            _ => continue,
        };
        if let Err(d) = result {
            errors.push(d);
        }
    }

    // Arrangements next, so that commands can start them.
    let mut arrangements: Vec<Arrangement> = Vec::new();
    for item in items {
        let Item::Arrangement(decl) = item else {
            continue;
        };
        if arrangements.iter().any(|a| a.name == decl.name.name) {
            errors.push(Diagnostic::new(
                format!("arrangement `{}` is defined twice", decl.name.name),
                decl.name.span,
            ));
            continue;
        }
        match p.arrangement(decl) {
            Ok(a) => {
                p.arrangements.insert(a.name.clone(), a.clone());
                arrangements.push(a);
            }
            Err(d) => errors.push(d),
        }
    }

    let mut actions = Vec::new();
    for item in items {
        let Item::Command(stmt) = item else { continue };
        match p.command(stmt) {
            Ok(mut a) => actions.append(&mut a),
            Err(d) => errors.push(d),
        }
    }

    let clips = p
        .bindings
        .into_iter()
        .filter_map(|(name, value)| match value {
            PerfValue::Clip(clip) => Some((name, clip)),
            _ => None,
        })
        .collect();

    if errors.is_empty() {
        Ok(Compiled {
            actions,
            arrangements,
            clips,
        })
    } else {
        Err(errors)
    }
}

pub(crate) fn value_of(value: f64, unit: Option<&str>, span: Span) -> Res<Value> {
    Ok(match unit {
        None => Value::Num(value),
        Some("hz") => Value::Hz(value),
        Some("khz") => Value::Hz(value * 1000.0),
        Some("db") => Value::Db(value),
        Some("sec") => Value::Seconds(value),
        Some("ms") => Value::Seconds(value / 1000.0),
        Some("beat" | "beats") => Value::Beats(value),
        Some("bar" | "bars") => Value::Bars(value),
        Some("deg") => Value::Degrees(value),
        Some("st") => Value::Semitones(value),
        Some("ct") => Value::Semitones(value / 100.0),
        Some("%") => Value::Num(value / 100.0),
        Some(u) => return Err(niminal_lang::unknown_unit(u, span)),
    })
}

/// `env[300hz 1bar.exp 3khz]` as a lane. A lane has no note to release, so it has no sustain point.
/// A step of an envelope that must be written as a number: lanes and tempo curves have no note to read
/// values from.
fn lit_step(v: &EnvValue) -> Res<&EnvNum> {
    v.lit().ok_or_else(|| {
        Diagnostic::new("this envelope's steps are written as numbers", v.span())
            .with_help("only an instrument's envelopes can read names, at note start")
    })
}

/// Where a macro's `target` is when the macro is at `level` (0 to 1), as a value in the control's unit.
fn macro_value(target: &crate::compile::MacroTarget, level: f64) -> Value {
    use niminal_lang::Unit;
    let x = target.value(level);
    match target.unit {
        Unit::Hz => Value::Hz(x),
        Unit::Db => Value::Db(x),
        Unit::Time => Value::Seconds(x),
        Unit::Angle => Value::Degrees(x),
        Unit::Semitones => Value::Semitones(x),
        Unit::Num => Value::Num(x),
    }
}

/// A macro's envelope as one for each thing it moves, glides and curves kept, levels turned into where each
/// control is.
fn fan_out_env(
    def: &crate::compile::MacroDef,
    curve: &EnvLane,
    curves: &mut Vec<(String, EnvLane)>,
) {
    for target in &def.targets {
        let at = |v: &Value| macro_value(target, v.as_number().unwrap_or(0.0));
        curves.push((
            target.control.clone(),
            EnvLane {
                start: at(&curve.start),
                segments: curve
                    .segments
                    .iter()
                    .map(|(time, shape, level)| (*time, *shape, at(level)))
                    .collect(),
                looped: curve.looped,
                unit: curve.unit,
            },
        ));
    }
}

fn env_lane(lit: &EnvLit) -> Res<EnvLane> {
    if lit.sustain_at.is_some() {
        return Err(Diagnostic::new(
            "a lane has no release, so its envelope can't have a sustain point `|`",
            lit.start.span(),
        )
        .with_help("take out the `|`, or add `.loop` to repeat the shape"));
    }
    let level = |v: &EnvValue| {
        let n = lit_step(v)?;
        value_of(n.value, n.unit.as_deref(), n.span)
    };
    let mut segments = Vec::with_capacity(lit.segments.len());
    for seg in &lit.segments {
        let d = lit_step(&seg.dur)?;
        let dur = value_of(d.value, d.unit.as_deref(), d.span)?
            .as_time()
            .map_err(|e| Diagnostic::new(e.to_string(), d.span))?;
        segments.push((dur, curve_of(seg.curve), level(&seg.level)?));
    }
    Ok(EnvLane {
        start: level(&lit.start)?,
        segments,
        looped: lit.looped,
        unit: None,
    })
}

/// A pattern atom as a value: a note name, a number with a unit, or the name
/// of a sample in a kit.
fn atom_value(atom: &str, sample_names: &[String]) -> Result<Value, String> {
    atom.parse::<Value>().or_else(|e| {
        // `kick` or `drums.kick` (the qualified form wins over a note of the same name)
        let member = atom.rsplit_once('.').map_or(atom, |(_, m)| m);
        match sample_names.iter().position(|n| n == member) {
            Some(id) => Ok(Value::Sample(id as u32)),
            None if atom
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') =>
            {
                let mut message = format!("`{atom}` isn't a note, a value, or a sample in a kit");
                let slices = sample_names
                    .iter()
                    .filter(|n| {
                        n.strip_prefix('s')
                            .is_some_and(|d| d.parse::<u32>().is_ok())
                    })
                    .count();
                let is_slice = member
                    .strip_prefix('s')
                    .is_some_and(|d| d.parse::<u32>().is_ok());
                if is_slice && slices > 0 {
                    // a slice past the end, such as `s7` of a sample cut at the 6 hits `detect:` found
                    message.push_str(&format!(
                        " (the sliced sample has {slices} pieces, `s0` to `s{}`)",
                        slices - 1
                    ));
                } else if let Some(c) = closest(member, sample_names.iter().map(String::as_str)) {
                    message.push_str(&format!(" (did you mean `{c}`?)"));
                }
                Err(message)
            }
            None => Err(e.to_string()),
        }
    })
}

/// How high a value sounds, for ordering: a MIDI note number (other values sort as themselves).
fn pitch(v: &Value) -> f64 {
    match *v {
        Value::Note(m) => f64::from(m),
        Value::Hz(h) => 69.0 + 12.0 * (h / 440.0).log2(),
        Value::Num(n) | Value::Semitones(n) => n,
        _ => 0.0,
    }
}

/// The controls a note carries with it.
#[derive(Clone, Copy)]
enum Control {
    Gain,
    Pan,
    Begin,
}

impl Control {
    fn param(self) -> &'static str {
        match self {
            Control::Gain => "level",
            Control::Pan => "angle",
            Control::Begin => "share",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Control::Gain => "a level in db, such as `-6db`",
            Control::Pan => "a direction in degrees, such as `-30deg`",
            Control::Begin => "a share of the sample from 0% to 100%",
        }
    }

    /// The control's number, if `v` is the right kind of value for it.
    fn read(self, v: Value) -> Option<f64> {
        match (self, v) {
            (Control::Gain, Value::Db(db)) => Some(db),
            (Control::Gain, Value::Num(n)) if n > 0.0 => Some(20.0 * n.log10()),
            (Control::Pan, Value::Degrees(d)) => Some(d),
            (Control::Begin, Value::Num(n)) if (0.0..=1.0).contains(&n) => Some(n),
            _ => None,
        }
    }

    fn set(self, s: Step, x: f64) -> Step {
        match self {
            Control::Gain => Step { gain: Some(x), ..s },
            Control::Pan => Step { pan: Some(x), ..s },
            Control::Begin => Step {
                begin: Some(x),
                ..s
            },
        }
    }
}

/// Where piece `i` of `n` starts in the stretch of a sample that begins at `begin` (a share, or the start).
fn slice_start(begin: Option<f64>, i: i64, n: i64) -> f64 {
    let from = begin.unwrap_or(0.0);
    from + (1.0 - from) * i as f64 / n as f64
}

/// A change to a step's value that leaves its controls alone.
fn on_value(
    f: impl Fn(Value) -> Value + Send + Sync + 'static,
) -> impl Fn(Step) -> Step + Send + Sync + 'static {
    move |s| s.with(f(s.value))
}

/// A number in `0..1` for the `i`th random choice of a cycle.
fn chance(seed: u64, cycle: i64, i: usize) -> f64 {
    unit_random(
        seed ^ (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        Rational::int(cycle),
    )
}

/// A value `x` of the way from `lo` to `hi`, which must share a unit.
fn blend(lo: Value, hi: Value, x: f64) -> Option<Value> {
    let mix = |a: f64, b: f64| a + (b - a) * x;
    Some(match (lo, hi) {
        (Value::Num(a), Value::Num(b)) => Value::Num(mix(a, b)),
        (Value::Hz(a), Value::Hz(b)) => Value::Hz(mix(a, b)),
        (Value::Db(a), Value::Db(b)) => Value::Db(mix(a, b)),
        (Value::Seconds(a), Value::Seconds(b)) => Value::Seconds(mix(a, b)),
        (Value::Beats(a), Value::Beats(b)) => Value::Beats(mix(a, b)),
        (Value::Bars(a), Value::Bars(b)) => Value::Bars(mix(a, b)),
        (Value::Semitones(a), Value::Semitones(b)) => Value::Semitones(mix(a, b)),
        (Value::Degrees(a), Value::Degrees(b)) => Value::Degrees(mix(a, b)),
        (Value::Note(a), Value::Note(b)) => {
            Value::Note(mix(f64::from(a), f64::from(b)).round() as i32)
        }
        _ => return None,
    })
}

/// When a kind of change lands, as written in `config` and in a definition's `quantize`: `1beat`, `4bars`,
/// `cycle`, `now`, `bar`.
pub(crate) fn quantum_of(e: &Expr) -> Res<Quantize> {
    let on_grid = |count: f64, unit| {
        Ok(Quantize {
            relation: QuantizeRelation::OnGrid,
            count,
            unit,
        })
    };
    match &e.kind {
        ExprKind::Num {
            value,
            unit: Some(u),
        } if *value > 0.0 => match u.as_str() {
            "beat" | "beats" => on_grid(*value, QuantizeUnit::Beat),
            "bar" | "bars" => on_grid(*value, QuantizeUnit::Bar),
            _ => Err(quantum_error(e.span)),
        },
        ExprKind::Name(n) => match n.as_str() {
            "now" => on_grid(1.0, QuantizeUnit::Now),
            "beat" => on_grid(1.0, QuantizeUnit::Beat),
            "bar" => on_grid(1.0, QuantizeUnit::Bar),
            "cycle" => on_grid(1.0, QuantizeUnit::Cycle),
            _ => Err(quantum_error(e.span)),
        },
        _ => Err(quantum_error(e.span)),
    }
}

fn quantum_error(span: Span) -> Diagnostic {
    Diagnostic::new("expected when a change lands", span)
        .with_help("for example `now`, `beat`, `bar`, `cycle`, `1beat` or `4bars`")
}

/// The one setting a clip or scene takes besides its body: `quantize: 1bar`.
pub(crate) fn definition_quantum(options: &[Arg]) -> Res<Option<Quantize>> {
    options
        .iter()
        .find(|a| a.name.as_ref().is_some_and(|n| n.name == "quantize"))
        .map(|a| quantum_of(&a.value))
        .transpose()
}

/// A time written in source: `2beats`, `1/2beat`, `4bars`, `250ms`.
fn time_of(e: &Expr, p: &Perform) -> Res<Time> {
    let (r, unit) = p.quantity(e)?;
    let v = r.to_f64();
    match unit.as_deref() {
        Some("sec") => Ok(Time::Seconds(v)),
        Some("ms") => Ok(Time::Seconds(v / 1000.0)),
        Some("beat" | "beats") => Ok(Time::Beats(v)),
        Some("bar" | "bars") => Ok(Time::Bars(v)),
        _ => Err(Diagnostic::new("expected a time", e.span)
            .with_help("for example `2beats`, `4bars` or `250ms`, or `bar 5 beat 3`")),
    }
}
