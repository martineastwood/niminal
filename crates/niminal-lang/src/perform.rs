//! Evaluating the performance layer: pattern literals and their operations,
//! clips, scenes, and the commands that play them.

use std::collections::HashMap;

use niminal_engine::ops::Curve;
use niminal_pattern::{Hit, MAX_REPEATS, Param, Pattern, Rational, grid, parse_with, unit_random};
use niminal_score::{Tempo, Time, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::lower::{GAIN_PARAM, Names, curve_of};
use crate::performance::*;
use crate::program::{Instrument, TrackInfo};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

/// A change to a pattern, such as the `it.reverse` in `every(2, it.reverse)`.
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
    Rest,
}

impl PerfValue {
    fn describe(&self) -> &'static str {
        match self {
            PerfValue::Pattern(_) => "a pattern",
            PerfValue::Grid(_) => "a grid",
            PerfValue::Clip(_) => "a clip",
            PerfValue::Scene(_) => "a scene",
            PerfValue::Env(_) => "an envelope",
            PerfValue::Rest => "`~`",
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
}

pub(crate) struct Context<'a> {
    pub tempo: Tempo,
    pub names: &'a Names,
    pub instruments: &'a [Instrument],
    pub tracks: &'a [TrackInfo],
    /// Tracks whose definition failed to compile; mentions of them are not
    /// reported again.
    pub failed: &'a [&'a str],
    /// The names of the samples in kits, which a pattern may use as steps.
    pub sample_names: &'a [String],
}

struct Perform<'a> {
    cx: &'a Context<'a>,
    bindings: HashMap<String, PerfValue>,
}

pub(crate) fn compile_performance(
    items: &[Item],
    cx: &Context,
) -> Result<Compiled, Vec<Diagnostic>> {
    let mut p = Perform {
        cx,
        bindings: HashMap::new(),
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
            _ => continue,
        };
        if let Err(d) = result {
            errors.push(d);
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
            Ok(a) => arrangements.push(a),
            Err(d) => errors.push(d),
        }
    }

    if errors.is_empty() {
        Ok(Compiled {
            actions,
            arrangements,
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
        Some("%") => Value::Num(value / 100.0),
        Some(u) => return Err(crate::lower::unknown_unit(u, span)),
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
    atom.parse::<Value>()
        .or_else(|e| match sample_names.iter().position(|n| n == atom) {
            Some(id) => Ok(Value::Sample(id as u32)),
            None if atom.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                let mut message = format!("`{atom}` isn't a note, a value, or a sample in a kit");
                if let Some(c) = closest(atom, sample_names.iter().map(String::as_str)) {
                    message.push_str(&format!(" (did you mean `{c}`?)"));
                }
                Err(message)
            }
            None => Err(e.to_string()),
        })
}

/// The built-in signals: `sine`, `saw`, `tri`, `square`, `rand` and `perlin`, each from 0 to 1.
fn signal(name: &str) -> Option<Pattern<f64>> {
    Some(match name {
        "sine" => Pattern::sine(),
        "saw" => Pattern::saw(),
        "tri" => Pattern::tri(),
        "square" => Pattern::square(),
        "rand" => Pattern::rand(),
        "perlin" => Pattern::perlin(),
        _ => return None,
    })
}

/// Scales as semitones above the root, one octave.
const SCALES: [(&str, &[i32]); 14] = [
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic_minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("melodic_minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("major_pentatonic", &[0, 2, 4, 7, 9]),
    ("minor_pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
    ("whole_tone", &[0, 2, 4, 6, 8, 10]),
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

/// Chords as semitones above the root.
const CHORDS: [(&str, &[i32]); 16] = [
    ("major", &[0, 4, 7]),
    ("minor", &[0, 3, 7]),
    ("dim", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("power", &[0, 7]),
    ("major6", &[0, 4, 7, 9]),
    ("minor6", &[0, 3, 7, 9]),
    ("major7", &[0, 4, 7, 11]),
    ("minor7", &[0, 3, 7, 10]),
    ("dom7", &[0, 4, 7, 10]),
    ("dim7", &[0, 3, 6, 9]),
    ("half_dim7", &[0, 3, 6, 10]),
    ("major9", &[0, 4, 7, 11, 14]),
    ("minor9", &[0, 3, 7, 10, 14]),
];

/// How arpeggios can run, lowest note first.
const ARP_ORDERS: [(&str, &str); 6] = [
    ("up", "up"),
    ("down", "down"),
    ("updown", "updown"),
    ("downup", "downup"),
    ("converge", "converge"),
    ("diverge", "diverge"),
];

fn arpeggio_order(order: &str, mut notes: Vec<Step>) -> Vec<Step> {
    notes.sort_by(|a, b| pitch(&a.value).total_cmp(&pitch(&b.value)));
    let init = |v: &[Step]| v[..v.len().saturating_sub(1).max(1)].to_vec();
    let reversed = |v: &[Step]| v.iter().rev().cloned().collect::<Vec<_>>();
    if notes.len() < 2 {
        return notes;
    }
    // outside in: lowest, highest, next lowest...
    let converge = |v: &[Step]| {
        let (mut lo, mut hi, mut out) = (0, v.len(), Vec::new());
        while lo < hi {
            out.push(v[lo]);
            lo += 1;
            if lo < hi {
                hi -= 1;
                out.push(v[hi]);
            }
        }
        out
    };
    match order {
        "down" => reversed(&notes),
        "updown" => [init(&notes), init(&reversed(&notes))].concat(),
        "downup" => [init(&reversed(&notes)), init(&notes)].concat(),
        "converge" => converge(&notes),
        "diverge" => reversed(&converge(&notes)),
        _ => notes,
    }
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

impl Perform<'_> {
    fn define(&mut self, name: &Ident, value: PerfValue) -> Res<()> {
        if let Some(kind) = self
            .cx
            .names
            .kind_of(&name.name)
            .filter(|k| !(*k == "part" && matches!(value, PerfValue::Clip(_))))
        {
            return Err(Diagnostic::new(
                format!("`{}` is already the name of a {kind}", name.name),
                name.span,
            ));
        }
        if matches!(name.name.as_str(), "it" | "out") {
            return Err(Diagnostic::new(
                format!("`{}` is a reserved word", name.name),
                name.span,
            ));
        }
        if self.bindings.contains_key(&name.name) {
            return Err(Diagnostic::new(
                format!("`{}` is defined twice", name.name),
                name.span,
            ));
        }
        self.bindings.insert(name.name.clone(), value);
        Ok(())
    }

    // ---- expressions -----------------------------------------------------

    fn eval(&self, e: &Expr, it: Option<&PerfValue>) -> Res<PerfValue> {
        match &e.kind {
            ExprKind::Pattern(raw) => self.pattern(raw, e.span).map(PerfValue::Pattern),
            ExprKind::Grid(raw) => grid(raw)
                .map(PerfValue::Grid)
                .map_err(|err| at_offset(err.message, e.span, err.offset)),
            ExprKind::Rows(raw) => self.rows(raw, e.span).map(PerfValue::Pattern),
            ExprKind::Rest => Ok(PerfValue::Rest),
            ExprKind::Name(name) if name == "it" => it.cloned().ok_or_else(|| {
                Diagnostic::new(
                    "`it` is only available inside an argument, as in `every(4, it.reverse)`",
                    e.span,
                )
            }),
            ExprKind::Name(name) => match self.bindings.get(name) {
                Some(v) => Ok(v.clone()),
                None if signal(name).is_some() => Ok(PerfValue::Pattern(
                    signal(name)
                        .expect("checked")
                        .map(|x| Step::from(Value::Num(x))),
                )),
                // a lone note or value is a one-step pattern
                None => match atom_value(name, self.cx.sample_names) {
                    Ok(v) => Ok(PerfValue::Pattern(Pattern::pure(Step::from(v)))),
                    Err(_) => {
                        let mut d = Diagnostic::new(format!("`{name}` is not defined"), e.span);
                        if let Some(c) = closest(name, self.bindings.keys().map(String::as_str)) {
                            d = d.with_help(format!("did you mean `{c}`?"));
                        }
                        Err(d)
                    }
                },
            },
            ExprKind::Num { value, unit } => Ok(PerfValue::Pattern(Pattern::pure(Step::from(
                value_of(*value, unit.as_deref(), e.span)?,
            )))),
            ExprKind::Neg(inner) => match self.eval(inner, it)? {
                PerfValue::Pattern(p) => Ok(PerfValue::Pattern(p.map(|s| s.with(negate(s.value))))),
                other => Err(Diagnostic::new(
                    format!("can't negate {}", other.describe()),
                    e.span,
                )),
            },
            ExprKind::Call { name, args } => self.op(name, args, e.span, it),
            ExprKind::Env(lit) => env_lane(lit).map(PerfValue::Env),
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can only be an argument that asks for it",
                e.span,
            )),
            ExprKind::Compare { .. }
            | ExprKind::If { .. }
            | ExprKind::Logic { .. }
            | ExprKind::Not(_)
            | ExprKind::For { .. } => Err(Diagnostic::new(
                "comparisons, logic, `if` and `for` work in instruments, functions and tracks, not in clips and scenes",
                e.span,
            )),
            ExprKind::Channels(_)
            | ExprKind::Binary { .. }
            | ExprKind::Range { .. }
            | ExprKind::Index { .. }
            | ExprKind::Lambda { .. } => Err(Diagnostic::new(
                "expected a pattern, clip or scene here",
                e.span,
            )
            .with_help("write steps in brackets, as in `[c4 e4 g4]`")),
        }
    }

    fn pattern(&self, raw: &str, span: Span) -> Res<Pattern<Step>> {
        parse_with(raw, |atom| {
            atom_value(atom, self.cx.sample_names).map(Step::from)
        })
        .map_err(|e| at_offset(e.message, span, e.offset))
    }

    /// `grid { kick: x... snare: .x.. }`: each row plays the sample it names on its hits,
    /// and the rows play together.
    fn rows(&self, raw: &str, span: Span) -> Res<Pattern<Step>> {
        let base = span.end - 1 - raw.len();
        let mut layers = Vec::new();
        let mut line_start = 0;
        for line in raw.split('\n') {
            let at = base + line_start;
            line_start += line.len() + 1;
            let code = line.split("//").next().unwrap_or("");
            if code.trim().is_empty() {
                continue;
            }
            let here = |offset: usize, message: String| {
                Diagnostic::new(
                    message,
                    Span::new(at + offset, (at + offset + 1).min(span.end)),
                )
            };
            let Some((name, cells)) = code.split_once(':') else {
                return Err(here(0, "a grid row is `sample: x...`".into()));
            };
            let sample = name.trim();
            let Some(id) = self.cx.sample_names.iter().position(|n| n == sample) else {
                let mut message = format!("`{sample}` isn't a sample in a kit");
                if let Some(c) = closest(sample, self.cx.sample_names.iter().map(String::as_str)) {
                    message.push_str(&format!(" (did you mean `{c}`?)"));
                }
                return Err(here(name.len() - name.trim_start().len(), message));
            };
            let hits = grid(cells).map_err(|e| here(name.len() + 1 + e.offset, e.message))?;
            let id = id as u32;
            layers.push(hits.map(move |hit| {
                let sample = Step::from(Value::Sample(id));
                match hit {
                    Hit::Normal => sample,
                    Hit::Accent => Step {
                        gain: Some(ACCENT_DB),
                        ..sample
                    },
                    Hit::Ghost => Step {
                        gain: Some(GHOST_DB),
                        ..sample
                    },
                }
            }));
        }
        if layers.is_empty() {
            return Err(Diagnostic::new("a grid needs at least one row", span)
                .with_help("for example `grid { kick: x... }`"));
        }
        Ok(Pattern::stack(layers))
    }

    // ---- numbers -----------------------------------------------------------

    /// A constant number with an optional unit, evaluated exactly: `1/8beat`, `-5st`.
    fn quantity(&self, e: &Expr) -> Res<(Rational, Option<String>)> {
        match &e.kind {
            ExprKind::Num { value, unit } => Ok((Rational::from_f64(*value), unit.clone())),
            ExprKind::Neg(inner) => self.quantity(inner).map(|(r, u)| (-r, u)),
            ExprKind::Binary { op, lhs, rhs } => {
                let ((l, lu), (r, ru)) = (self.quantity(lhs)?, self.quantity(rhs)?);
                let bad = || Diagnostic::new("can't combine these units", e.span);
                match op {
                    BinOp::Add | BinOp::Sub if lu == ru => {
                        Ok((if *op == BinOp::Add { l + r } else { l - r }, lu))
                    }
                    BinOp::Mul if ru.is_none() => Ok((l * r, lu)),
                    BinOp::Mul if lu.is_none() => Ok((l * r, ru)),
                    BinOp::Div if ru.is_none() && !r.is_zero() => Ok((l / r, lu)),
                    BinOp::Div if lu == ru && !r.is_zero() => Ok((l / r, None)),
                    _ => Err(bad()),
                }
            }
            _ => Err(Diagnostic::new("expected a number", e.span)
                .with_help("for example `2`, `3/2` or `1/8beat`")),
        }
    }

    /// A plain number (no unit).
    fn plain(&self, e: &Expr) -> Res<Rational> {
        match self.quantity(e)? {
            (r, None) => Ok(r),
            (_, Some(u)) => Err(Diagnostic::new(
                format!("expected a plain number, found a value in `{u}`"),
                e.span,
            )),
        }
    }

    /// A length of time as a number of cycles (bars). Beats are a share of a
    /// bar; seconds would depend on the tempo, so they are not accepted.
    fn cycles(&self, e: &Expr) -> Res<Rational> {
        self.cycles_in(e, self.cx.tempo.beats_per_bar)
    }

    /// Like [`Perform::cycles`], for bars that are `beats_per_bar` long.
    fn cycles_in(&self, e: &Expr, beats_per_bar: f64) -> Res<Rational> {
        let (r, unit) = self.quantity(e)?;
        match unit.as_deref() {
            None | Some("bar" | "bars") => Ok(r),
            Some("beat" | "beats") => Ok(r / Rational::from_f64(beats_per_bar)),
            Some(u) => Err(Diagnostic::new(
                format!("expected a length in beats or bars, found `{u}`"),
                e.span,
            )),
        }
    }

    // ---- operations ----------------------------------------------------------

    fn op(&self, name: &Ident, args: &[Arg], span: Span, it: Option<&PerfValue>) -> Res<PerfValue> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                format!(
                    "`{}` works on a pattern: write it after one, as in `riff.{}`",
                    name.name, name.name
                ),
                name.span,
            ));
        };
        let rest = &args[1..];
        match self.eval(&receiver.value, it)? {
            // `set` gives lanes to a clip; a pattern is a clip with only its notes
            PerfValue::Pattern(p) if name.name == "set" => {
                let clip = Clip {
                    name: String::new(),
                    length: Rational::ONE,
                    endless: true,
                    lanes: vec![("notes".to_string(), p)],
                    keys: None,
                    curves: Vec::new(),
                    signals: Vec::new(),
                };
                self.on_clip(name, rest, clip, span, it)
                    .map(PerfValue::Clip)
            }
            PerfValue::Pattern(p) => self.on_pattern(name, rest, p, span, it),
            PerfValue::Grid(g) if name.name == "invert" => {
                Ok(PerfValue::Grid(g.invert(Hit::Normal)))
            }
            PerfValue::Grid(g) => match self.generic(name, rest, g, span, it)? {
                Some(g) => Ok(PerfValue::Grid(g)),
                None => Err(self.not_for(name, "a grid")),
            },
            PerfValue::Clip(clip) => self
                .on_clip(name, rest, clip, span, it)
                .map(PerfValue::Clip),
            other => Err(self.not_for(name, other.describe())),
        }
    }

    fn not_for(&self, name: &Ident, what: &str) -> Diagnostic {
        Diagnostic::new(
            format!("`{}` can't be used on {what}", name.name),
            name.span,
        )
    }

    fn on_pattern(
        &self,
        name: &Ident,
        rest: &[Arg],
        p: Pattern<Step>,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        if let Some(out) = self.generic(name, rest, p.clone(), span, it)? {
            return Ok(PerfValue::Pattern(out));
        }
        let out = match name.name.as_str() {
            "transpose" => {
                let arg = self.arg(name, rest, "by", span)?;
                let (st, unit) = self.quantity(arg)?;
                if unit.as_deref() != Some("st") {
                    return Err(Diagnostic::new(
                        "`transpose` takes an interval in semitones",
                        arg.span,
                    )
                    .with_help("for example `transpose(+5st)`"));
                }
                let st = st.to_f64();
                p.map(on_value(move |v| transpose(v, st)))
            }
            "euclid" => {
                let hits = self.num_param(self.arg(name, rest, "hits", span)?, it)?;
                let steps_arg = self.arg(name, rest, "steps", span)?;
                let steps = self.num_param(steps_arg, it)?;
                let rotation = match self.find(name, rest, "rotation") {
                    Some(e) => self.num_param(e, it)?,
                    None => Param::Const(Rational::ZERO),
                };
                if let Param::Const(s) = &steps
                    && *s > Rational::int(MAX_REPEATS)
                {
                    return Err(Diagnostic::new(
                        format!("`euclid` takes at most {MAX_REPEATS} steps"),
                        name.span,
                    ));
                }
                for (arg, what) in [(&hits, "hits"), (&steps, "steps"), (&rotation, "rotation")] {
                    if let Param::Const(r) = arg
                        && (!r.is_integer() || (*r < Rational::ZERO && what != "hits"))
                    {
                        return Err(Diagnostic::new(
                            format!("`{what}` must be a whole number"),
                            name.span,
                        ));
                    }
                }
                p.rhythm(&Pattern::<bool>::euclid_with(&hits, &steps, &rotation))
            }
            "rhythm" => p.rhythm(&self.structure(self.arg(name, rest, "structure", span)?, it)?),
            "mask" => p.mask(&self.structure(self.arg(name, rest, "mask", span)?, it)?),
            "range" => {
                let (from, to) = (
                    self.arg(name, rest, "from", span)?,
                    self.arg(name, rest, "to", span)?,
                );
                let (lo, hi) = (self.constant(from, it)?, self.constant(to, it)?);
                if blend(lo, hi, 0.0).is_none() {
                    return Err(Diagnostic::new(
                        "`range` needs two values in the same unit",
                        to.span,
                    )
                    .with_help("for example `range(300hz, 3khz)`"));
                }
                p.map(on_value(move |v| match v {
                    Value::Num(x) => blend(lo, hi, x).unwrap_or(v),
                    other => other,
                }))
            }
            "in_scale" => {
                let scale: &[i32] =
                    self.word(self.arg(name, rest, "scale", span)?, "scale", &SCALES)?;
                let root = match self.find(name, rest, "root") {
                    Some(e) => match self.constant(e, it)? {
                        Value::Note(m) => m,
                        _ => return Err(Diagnostic::new("`root` is a note, such as `c3`", e.span)),
                    },
                    None => 60,
                };
                let len = scale.len() as i64;
                p.map(on_value(move |v| match v {
                    Value::Num(degree) => {
                        let d = degree.floor() as i64;
                        Value::Note(
                            root + scale[d.rem_euclid(len) as usize]
                                + 12 * d.div_euclid(len) as i32,
                        )
                    }
                    other => other,
                }))
            }
            "chord" => {
                let shape: &[i32] =
                    self.word(self.arg(name, rest, "kind", span)?, "chord", &CHORDS)?;
                p.expand(move |s| match s.value {
                    Value::Note(m) => shape.iter().map(|i| s.with(Value::Note(m + i))).collect(),
                    _ => vec![s],
                })
            }
            "arpeggiate" => {
                let e = self.arg(name, rest, "order", span)?;
                let order: &str = self.word(e, "arpeggio order", &ARP_ORDERS)?;
                p.spread(move |notes| arpeggio_order(order, notes))
            }
            "voice_lead" => p.rewrite_cycles(|events, _| {
                // each chord is moved by octaves to sit as close as it can to the one before
                let mut out: Vec<Step> = Vec::with_capacity(events.len());
                let (mut start, mut centre) = (0, None::<f64>);
                while start < events.len() {
                    let end = (start..events.len())
                        .find(|&i| events[i].whole != events[start].whole)
                        .unwrap_or(events.len());
                    let group: Vec<Step> = events[start..end]
                        .iter()
                        .map(|h| match (h.value.value, centre) {
                            (Value::Note(m), Some(c)) => h.value.with(Value::Note(
                                m + 12 * ((c - f64::from(m)) / 12.0).round() as i32,
                            )),
                            _ => h.value,
                        })
                        .collect();
                    let notes: Vec<f64> = group
                        .iter()
                        .filter_map(|s| {
                            if let Value::Note(m) = s.value {
                                Some(f64::from(m))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !notes.is_empty() {
                        centre = Some(notes.iter().sum::<f64>() / notes.len() as f64);
                    }
                    out.extend(group);
                    start = end;
                }
                out
            }),
            "invert_melody" => {
                let around = self.constant(self.arg(name, rest, "around", span)?, it)?;
                let Value::Note(axis) = around else {
                    return Err(Diagnostic::new(
                        "`around` is a note, such as `c4`",
                        name.span,
                    ));
                };
                let axis_hz = around.as_hz().unwrap_or(440.0);
                p.map(on_value(move |v| match v {
                    Value::Note(m) => Value::Note(2 * axis - m),
                    Value::Hz(h) => Value::Hz(axis_hz * axis_hz / h),
                    other => other,
                }))
            }
            "markov" => {
                let seed = self.seed(name, rest, 0x4D41_524B)?;
                p.rewrite_cycles(move |events, cycle| {
                    // the notes follow each other as they do in this cycle, in a new order
                    let values: Vec<Value> = events.iter().map(|h| h.value.value).collect();
                    let Some(&first) = values.first() else {
                        return Vec::new();
                    };
                    let mut chain = vec![first];
                    for i in 1..values.len() {
                        let current = chain[i - 1];
                        let next: Vec<Value> = (0..values.len())
                            .filter(|&j| values[j] == current)
                            .map(|j| values[(j + 1) % values.len()])
                            .collect();
                        let pick = (chance(seed, cycle, i) * next.len() as f64) as usize;
                        chain.push(next.get(pick).copied().unwrap_or(first));
                    }
                    events
                        .iter()
                        .zip(chain)
                        .map(|(h, v)| h.value.with(v))
                        .collect()
                })
            }
            "random_walk" => {
                let e = self.arg(name, rest, "step", span)?;
                let (st, unit) = self.quantity(e)?;
                if unit.as_deref() != Some("st") || st < Rational::ZERO {
                    return Err(Diagnostic::new(
                        "`random_walk` takes the biggest step in semitones",
                        e.span,
                    )
                    .with_help("for example `random_walk(step: 2st)`"));
                }
                let step = st.floor();
                let seed = self.seed(name, rest, 0x5741_4C4B)?;
                p.rewrite_cycles(move |events, cycle| {
                    // from the first note, each note moves by up to `step` semitones either way
                    let mut at = None::<i32>;
                    events
                        .iter()
                        .enumerate()
                        .map(|(i, h)| match (h.value.value, at) {
                            (Value::Note(m), None) => {
                                at = Some(m);
                                h.value
                            }
                            (Value::Note(_), Some(prev)) => {
                                let by =
                                    (chance(seed, cycle, i) * (2 * step + 1) as f64) as i64 - step;
                                at = Some(prev + by as i32);
                                h.value.with(Value::Note(prev + by as i32))
                            }
                            _ => h.value,
                        })
                        .collect()
                })
            }
            "where" => {
                // `where(it < c3, change)`: change only the notes that pass the test
                let e = self.arg(name, rest, "when", span)?;
                let bad = || {
                    Diagnostic::new(
                        "`where` takes a test on the note, such as `it < c3`",
                        e.span,
                    )
                    .with_help("write `it` or `it.freq`, a comparison and a note or value")
                };
                let ExprKind::Compare { op, lhs, rhs } = &e.kind else {
                    return Err(bad());
                };
                let subject = match &lhs.kind {
                    ExprKind::Name(n) => n == "it",
                    ExprKind::Call { name, args } => {
                        name.name == PITCH_PARAM
                            && matches!(args.as_slice(), [a] if matches!(&a.value.kind, ExprKind::Name(n) if n == "it"))
                    }
                    _ => false,
                };
                if !subject {
                    return Err(bad());
                }
                let limit = pitch(&self.constant(rhs, it)?);
                let op = *op;
                let passes = move |s: &Step| {
                    let x = pitch(&s.value);
                    match op {
                        CmpOp::Lt => x < limit,
                        CmpOp::Gt => x > limit,
                        CmpOp::Le => x <= limit,
                        CmpOp::Ge => x >= limit,
                        CmpOp::Eq => x == limit,
                        CmpOp::Ne => x != limit,
                    }
                };
                let change = self.transformer::<Step>(self.arg(name, rest, "change", span)?, &p)?;
                Pattern::stack(vec![
                    p.filter(move |s| !passes(s)),
                    change(p.filter(passes)),
                ])
            }
            "gain" | "pan" | "begin" => {
                let control = match name.name.as_str() {
                    "gain" => Control::Gain,
                    "pan" => Control::Pan,
                    _ => Control::Begin,
                };
                let e = self.arg(name, rest, control.param(), span)?;
                let PerfValue::Pattern(values) = self.eval(e, it)? else {
                    return Err(Diagnostic::new(
                        format!("`{}` takes {}", name.name, control.describe()),
                        e.span,
                    ));
                };
                // say now, not when the note plays, if the value is of the wrong kind
                let cycle = niminal_pattern::Span::new(Rational::ZERO, Rational::ONE);
                if let Some(bad) = values
                    .query(cycle)
                    .iter()
                    .find(|h| control.read(h.value.value).is_none())
                {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` takes {}, found `{}`",
                            name.name,
                            control.describe(),
                            bad.value.value
                        ),
                        e.span,
                    ));
                }
                p.set_from(&values, move |s, o| match control.read(o.value) {
                    Some(x) => control.set(*s, x),
                    None => *s,
                })
            }
            "left_right" => {
                // the original on the left and the changed copy on the right
                let angle = match self.find(name, rest, "angle") {
                    Some(e) => match self.quantity(e)? {
                        (a, Some(u)) if u == "deg" => a.to_f64(),
                        _ => {
                            return Err(Diagnostic::new(
                                "`angle` is a direction in degrees",
                                e.span,
                            )
                            .with_help("for example `angle: 30deg`"));
                        }
                    },
                    None => 30.0,
                };
                let change = self.transformer::<Step>(self.arg(name, rest, "change", span)?, &p)?;
                let placed = |p: Pattern<Step>, pan: f64| {
                    p.map(move |s| Step {
                        pan: Some(pan),
                        ..s
                    })
                };
                Pattern::stack(vec![placed(p.clone(), -angle), placed(change(p), angle)])
            }
            "chop" => {
                let n = self.whole(self.arg(name, rest, "pieces", span)?, "chop")?;
                // each piece plays the next stretch of the sample
                p.pieces(n, |s, i, n| Step {
                    begin: Some(slice_start(s.begin, i, n)),
                    ..*s
                })
            }
            "interleave_chops" => {
                let n = self.whole(self.arg(name, rest, "pieces", span)?, "interleave_chops")?;
                // the pattern again for each stretch of the sample, in turn
                Pattern::fastcat(
                    (0..n)
                        .map(|i| {
                            p.map(move |s| Step {
                                begin: Some(slice_start(s.begin, i, n)),
                                ..s
                            })
                        })
                        .collect(),
                )
            }
            "invert" => {
                return Err(Diagnostic::new(
                    "`invert` works on grids, as in `grid[x... .x..].invert`",
                    name.span,
                ));
            }
            other => return Err(self.unknown_op(other, name.span)),
        };
        Ok(PerfValue::Pattern(out))
    }

    /// A grid (or pattern) read as on/off.
    fn structure(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Pattern<bool>> {
        match self.eval(e, it)? {
            PerfValue::Grid(g) => Ok(g.map(|_| true)),
            PerfValue::Pattern(p) => Ok(p.map(|_| true)),
            other => Err(Diagnostic::new(
                format!("expected a grid, found {}", other.describe()),
                e.span,
            )),
        }
    }

    fn on_clip(
        &self,
        name: &Ident,
        rest: &[Arg],
        clip: Clip,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<Clip> {
        if name.name == "set" {
            return self.set_lanes(rest, clip, it);
        }
        // The lanes span the clip, so `over` stretches them from the clip's length to the new one.
        if name.name == "over" {
            let e = self.arg(name, rest, "length", span)?;
            let length = self.cycles(e)?;
            if length <= Rational::ZERO {
                return Err(Diagnostic::new("`over` must be above zero", e.span));
            }
            let length = length.max(Rational::new(1, 1000));
            let factor = length / clip.length;
            let lanes = clip
                .lanes
                .into_iter()
                .map(|(lane, p)| (lane, p.slow(factor)))
                .collect();
            return Ok(Clip {
                name: clip.name,
                length,
                endless: false,
                lanes,
                keys: clip.keys,
                curves: clip.curves,
                signals: Vec::new(),
            });
        }
        // Pitch operations touch only the notes; time operations touch every lane.
        let notes_only = matches!(
            name.name.as_str(),
            "transpose"
                | "euclid"
                | "rhythm"
                | "mask"
                | "thin"
                | "sometimes"
                | "in_scale"
                | "chord"
                | "arpeggiate"
                | "voice_lead"
                | "invert_melody"
                | "markov"
                | "random_walk"
                | "where"
                | "gain"
                | "pan"
                | "begin"
                | "left_right"
                | "chop"
                | "interleave_chops"
        );
        let mut lanes = Vec::with_capacity(clip.lanes.len());
        for (lane, pattern) in &clip.lanes {
            if notes_only && lane != "notes" {
                lanes.push((lane.clone(), pattern.clone()));
                continue;
            }
            match self.on_pattern(name, rest, pattern.clone(), span, it)? {
                PerfValue::Pattern(p) => lanes.push((lane.clone(), p)),
                _ => unreachable!("operations on a pattern give a pattern"),
            }
        }
        let length = match name.name.as_str() {
            "fast" | "slow" => match self.num_param(self.arg(name, rest, "by", span)?, it)? {
                Param::Const(by) if name.name == "fast" => clip.length / by,
                Param::Const(by) => clip.length * by,
                Param::Pat(_) => clip.length,
            },
            "over" => self
                .cycles(self.arg(name, rest, "length", span)?)?
                .max(Rational::new(1, 1000)),
            _ => clip.length,
        };
        Ok(Clip {
            name: clip.name,
            length,
            endless: clip.endless && name.name != "over",
            lanes,
            keys: clip.keys,
            curves: clip.curves,
            signals: Vec::new(),
        })
    }

    /// `riff.set(bright: 0.9, gain: [0db -6db])`: these lanes, in place of the clip's own.
    fn set_lanes(&self, rest: &[Arg], mut clip: Clip, it: Option<&PerfValue>) -> Res<Clip> {
        for arg in rest {
            let Some(lane) = &arg.name else {
                return Err(Diagnostic::new(
                    "`set` takes a lane and its value, as in `set(bright: 0.9)`",
                    arg.value.span,
                ));
            };
            if lane.name == "length" {
                return Err(Diagnostic::new(
                    "a clip's length can't be set; use `over(…)`",
                    lane.span,
                ));
            }
            let pattern = match self.eval(&arg.value, it)? {
                PerfValue::Pattern(p) => p,
                PerfValue::Env(_) if lane.name == "notes" => {
                    return Err(Diagnostic::new(
                        "the `notes` lane holds notes, not an envelope",
                        arg.value.span,
                    ));
                }
                PerfValue::Env(curve) => {
                    clip.lanes.retain(|(n, _)| *n != lane.name);
                    match clip.curves.iter_mut().find(|(n, _)| *n == lane.name) {
                        Some((_, existing)) => *existing = curve,
                        None => clip.curves.push((lane.name.clone(), curve)),
                    }
                    continue;
                }
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "a lane holds a pattern, a number or an envelope, but this is {}",
                            other.describe()
                        ),
                        arg.value.span,
                    ));
                }
            };
            clip.curves.retain(|(n, _)| *n != lane.name);
            let pattern = pattern.slow(clip.length);
            match clip.lanes.iter_mut().find(|(n, _)| *n == lane.name) {
                Some((_, existing)) => *existing = pattern,
                None => clip.lanes.push((lane.name.clone(), pattern)),
            }
        }
        Ok(clip)
    }

    /// Operations that work on any kind of pattern. `None` if `name` isn't one.
    fn generic<'a, T: Kind>(
        &'a self,
        name: &Ident,
        rest: &'a [Arg],
        p: Pattern<T>,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<Option<Pattern<T>>> {
        let max = MAX_REPEATS;
        let positive = |r: Rational, what: &str, at: Span| {
            if r <= Rational::ZERO {
                Err(Diagnostic::new(format!("`{what}` must be above zero"), at))
            } else if r > Rational::int(max) || r < Rational::new(1, max) {
                Err(Diagnostic::new(
                    format!("`{what}` takes a number from 1/{max} to {max}"),
                    at,
                ))
            } else {
                Ok(r)
            }
        };
        Ok(Some(match name.name.as_str() {
            "fast" | "slow" => {
                let e = self.arg(name, rest, "by", span)?;
                let by = self.num_param(e, it)?;
                if let Param::Const(r) = &by {
                    positive(*r, &name.name, e.span)?;
                }
                let slow = name.name == "slow";
                by.apply(&p, move |p, f| {
                    let f = f.min(Rational::int(max));
                    if slow { p.slow(f) } else { p.fast(f) }
                })
            }
            "over" => {
                let e = self.arg(name, rest, "length", span)?;
                p.slow(positive(self.cycles(e)?, "over", e.span)?)
            }
            "reverse" => p.reverse(),
            "palindrome" => p.palindrome(),
            "shift" => p.shift(self.cycles(self.arg(name, rest, "by", span)?)?),
            "repeat_each" => {
                let e = self.arg(name, rest, "times", span)?;
                let times = self.num_param(e, it)?;
                if let Param::Const(n) = &times
                    && (!n.is_integer() || *n < Rational::ONE || *n > Rational::int(MAX_REPEATS))
                {
                    return Err(Diagnostic::new(
                        format!("`repeat_each` takes a whole number from 1 to {MAX_REPEATS}"),
                        name.span,
                    ));
                }
                times.apply(&p, |p, n| p.repeat_each(n.floor()))
            }
            "thin" => {
                let drop = self.chance(name, rest, "amount", span)?;
                // The same amount always thins the same events, wherever it is written.
                p.thin(drop, self.seed(name, rest, drop.to_bits())?)
            }
            "sometimes" => {
                let chance = self.chance(name, rest, "chance", span)?;
                let seed = self.seed(name, rest, chance.to_bits())?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.sometimes(chance, seed, change)
            }
            "every" => {
                let e = self.arg(name, rest, "n", span)?;
                let n = self.num_param(e, it)?;
                if let Param::Const(n) = &n
                    && (!n.is_integer() || *n < Rational::ONE)
                {
                    return Err(Diagnostic::new(
                        "`every` takes a whole number of at least 1",
                        name.span,
                    ));
                }
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.every_by(&n, change)
            }
            "layer" => {
                let after = match self.find(name, rest, "after") {
                    Some(e) => self.cycles(e)?,
                    None => Rational::ZERO,
                };
                let f = self.arg(name, rest, "change", span)?;
                let change = self.transformer::<T>(f, &p)?;
                p.layer(after, change)
            }
            "within" => {
                let from = self.fraction(self.arg(name, rest, "from", span)?)?;
                let to = self.fraction(self.arg(name, rest, "to", span)?)?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.within(from, to, change)
            }
            "swing" => {
                let amount = self.fraction(self.arg(name, rest, "amount", span)?)?;
                let e = self.arg(name, rest, "every", span)?;
                let every = self.cycles(e)?;
                if every <= Rational::ZERO || every > Rational::ONE {
                    return Err(Diagnostic::new(
                        "`swing` needs `every` to be a share of a bar, such as `1/8beat`",
                        e.span,
                    ));
                }
                p.swing(amount, Rational::ONE / every)
            }
            "rotate" => {
                let times = self.num_param(self.arg(name, rest, "parts", span)?, it)?;
                times.apply(&p, |p, n| p.rotate(n.floor().min(MAX_REPEATS)))
            }
            "in_turn" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "in_turn")?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.in_turn(n, change)
            }
            "shuffle" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "shuffle")?;
                p.shuffle(n, self.seed(name, rest, 0)?)
            }
            "pick_random" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "pick_random")?;
                p.pick_random(n, self.seed(name, rest, 0)?)
            }
            "repeat_start" => {
                let e = self.arg(name, rest, "length", span)?;
                let fraction = self.fraction(e)?;
                if fraction <= Rational::ZERO || fraction > Rational::ONE {
                    return Err(Diagnostic::new(
                        "`repeat_start` takes a share of the cycle, from above 0% to 100%",
                        e.span,
                    ));
                }
                p.repeat_start(fraction)
            }
            "to_steps" => {
                let steps = self.num_param(self.arg(name, rest, "steps", span)?, it)?;
                steps.apply(&p, |p, n| {
                    p.rhythm(
                        &Pattern::pure(true)
                            .fast(n.max(Rational::ZERO).min(Rational::int(MAX_REPEATS))),
                    )
                })
            }
            _ => return Ok(None),
        }))
    }

    /// A number argument, fixed (`2`, `3/2`) or a pattern (`<2 4>`).
    fn num_param(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Param<Rational>> {
        match self.quantity(e) {
            Ok((r, None)) => return Ok(Param::Const(r)),
            Ok((_, Some(u))) => {
                return Err(Diagnostic::new(
                    format!("expected a plain number, found a value in `{u}`"),
                    e.span,
                ));
            }
            Err(_) => {}
        }
        match self.eval(e, it)? {
            PerfValue::Pattern(p) => Ok(Param::Pat(p.filter_map(|s| match s.value {
                Value::Num(n) => Some(Rational::from_f64(n)),
                _ => None,
            }))),
            other => Err(Diagnostic::new(
                format!(
                    "expected a number or a pattern of numbers, found {}",
                    other.describe()
                ),
                e.span,
            )),
        }
    }

    /// A share of a cycle: `25%`, `1/4`, or a length such as `2beats`.
    fn fraction(&self, e: &Expr) -> Res<Rational> {
        match self.quantity(e)? {
            (r, Some(u)) if u == "%" => Ok(r / Rational::int(100)),
            (r, None) => Ok(r),
            _ => self.cycles(e),
        }
    }

    /// An amount from 0% to 100%, as a chance from 0 to 1.
    fn chance(&self, name: &Ident, rest: &[Arg], param: &str, span: Span) -> Res<f64> {
        let e = self.arg(name, rest, param, span)?;
        let (amount, unit) = self.quantity(e)?;
        let chance = match unit.as_deref() {
            Some("%") => amount.to_f64() / 100.0,
            None => amount.to_f64(),
            Some(u) => {
                return Err(Diagnostic::new(
                    format!("`{}` takes a percentage, not `{u}`", name.name),
                    e.span,
                ));
            }
        };
        if !(0.0..=1.0).contains(&chance) {
            return Err(Diagnostic::new(
                format!("`{}` takes an amount from 0% to 100%", name.name),
                e.span,
            ));
        }
        Ok(chance)
    }

    /// A whole number of pieces, from 1 to the most a step can be divided into.
    fn whole(&self, e: &Expr, what: &str) -> Res<i64> {
        let n = self.plain(e)?;
        if n.is_integer() && n >= Rational::ONE && n <= Rational::int(MAX_REPEATS) {
            Ok(n.numer())
        } else {
            Err(Diagnostic::new(
                format!("`{what}` takes a whole number from 1 to {MAX_REPEATS}"),
                e.span,
            ))
        }
    }

    /// The `seed` argument if given: a different seed chooses differently.
    fn seed(&self, name: &Ident, rest: &[Arg], default: u64) -> Res<u64> {
        match self.find(name, rest, "seed") {
            Some(e) => Ok(self.plain(e)?.floor() as u64 ^ default),
            None => Ok(default),
        }
    }

    /// A single value, such as the `c3` in `in_scale(minor, root: c3)`.
    fn constant(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Value> {
        let PerfValue::Pattern(p) = self.eval(e, it)? else {
            return Err(Diagnostic::new("expected a single value", e.span));
        };
        let at = niminal_pattern::Span::new(Rational::ZERO, Rational::ZERO);
        p.query(at)
            .into_iter()
            .next()
            .map(|h| h.value.value)
            .ok_or_else(|| Diagnostic::new("expected a single value", e.span))
    }

    /// A bare word such as `minor7`, looked up in `table`.
    fn word<'t, V: ?Sized>(
        &self,
        e: &Expr,
        what: &str,
        table: &'t [(&'static str, &'t V)],
    ) -> Res<&'t V> {
        let ExprKind::Name(word) = &e.kind else {
            return Err(Diagnostic::new(
                format!("expected the name of a {what}"),
                e.span,
            ));
        };
        table
            .iter()
            .find(|(n, _)| n == word)
            .map(|(_, v)| *v)
            .ok_or_else(|| {
                let mut d = Diagnostic::new(format!("`{word}` isn't a {what}"), e.span);
                if let Some(c) = closest(word, table.iter().map(|(n, _)| *n)) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            })
    }

    /// An argument such as `it.reverse`, as a function from pattern to pattern.
    /// It is tried once on `sample` so that mistakes are reported now.
    fn transformer<'a, T: Kind>(&'a self, e: &'a Expr, sample: &Pattern<T>) -> Res<Change<'a, T>> {
        let apply = move |p: Pattern<T>| -> Res<Pattern<T>> {
            let out = self.eval(e, Some(&T::wrap(p)))?;
            T::unwrap(out).ok_or_else(|| {
                Diagnostic::new("this change must give the same kind of pattern", e.span)
            })
        };
        apply(sample.clone())?;
        Ok(Box::new(move |p: Pattern<T>| apply(p.clone()).unwrap_or(p)))
    }

    fn unknown_op(&self, name: &str, span: Span) -> Diagnostic {
        use std::sync::OnceLock;
        static NAMES: OnceLock<Vec<String>> = OnceLock::new();
        let names = NAMES.get_or_init(|| {
            crate::ide::pattern_ops()
                .into_iter()
                .map(|o| o.name)
                .collect()
        });
        let mut d = Diagnostic::new(format!("unknown pattern operation `{name}`"), span);
        if let Some(c) = closest(name, names.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        d
    }

    // ---- arguments -----------------------------------------------------------

    /// The parameter names of each operation, in order. Arguments may be given
    /// by name; the rest fill the remaining parameters in order.
    fn op_params(op: &str) -> &'static [&'static str] {
        match op {
            "fast" | "slow" | "shift" | "transpose" => &["by"],
            "over" => &["length"],
            "repeat_each" => &["times"],
            "thin" => &["amount", "seed"],
            "sometimes" => &["chance", "change", "seed"],
            "every" => &["n", "change"],
            "layer" => &["after", "change"],
            "within" => &["from", "to", "change"],
            "swing" => &["amount", "every"],
            "rotate" | "shuffle" | "pick_random" => &["parts", "seed"],
            "in_turn" => &["parts", "change"],
            "repeat_start" => &["length"],
            "to_steps" => &["steps"],
            "range" => &["from", "to"],
            "where" => &["when", "change"],
            "gain" => &["level"],
            "pan" => &["angle"],
            "begin" => &["share"],
            "left_right" => &["change", "angle"],
            "chop" | "interleave_chops" => &["pieces"],
            "in_scale" => &["scale", "root"],
            "chord" => &["kind"],
            "arpeggiate" => &["order"],
            "invert_melody" => &["around"],
            "markov" => &["seed"],
            "random_walk" => &["step", "seed"],
            "euclid" => &["hits", "steps", "rotation"],
            "rhythm" => &["structure"],
            "mask" => &["mask"],
            _ => &[],
        }
    }

    /// The argument for parameter `param` of `op`, if given.
    fn find<'e>(&self, op: &Ident, args: &'e [Arg], param: &str) -> Option<&'e Expr> {
        let params = Self::op_params(&op.name);
        let mut slots: Vec<Option<&Expr>> = vec![None; params.len()];
        for a in args {
            if let Some(n) = &a.name
                && let Some(i) = params.iter().position(|p| *p == n.name)
            {
                slots[i] = Some(&a.value);
            }
        }
        let mut next = 0;
        for a in args.iter().filter(|a| a.name.is_none()) {
            while next < slots.len() && slots[next].is_some() {
                next += 1;
            }
            if next < slots.len() {
                slots[next] = Some(&a.value);
                next += 1;
            }
        }
        params
            .iter()
            .position(|p| *p == param)
            .and_then(|i| slots[i])
    }

    fn arg<'e>(&self, op: &Ident, args: &'e [Arg], param: &str, span: Span) -> Res<&'e Expr> {
        self.find(op, args, param).ok_or_else(|| {
            Diagnostic::new(format!("`{}` needs an argument `{param}`", op.name), span)
        })
    }

    // ---- clips and scenes ------------------------------------------------------

    fn clip(&self, block: &NamedBlock) -> Res<Clip> {
        let mut length = None;
        let mut keys = None;
        let mut lanes: Vec<(String, Pattern<Step>)> = Vec::new();
        let mut curves: Vec<(String, EnvLane)> = Vec::new();
        for (key, value) in &block.entries {
            if key.name == "keys" {
                if keys.is_some() {
                    return Err(Diagnostic::new("lane `keys` is given twice", key.span));
                }
                keys = Some(self.keys(value)?);
                continue;
            }
            if key.name == "length" {
                let cycles = self.cycles(value)?;
                if cycles <= Rational::ZERO {
                    return Err(Diagnostic::new(
                        "a clip needs a length above zero",
                        value.span,
                    ));
                }
                length = Some(cycles);
                continue;
            }
            if lanes.iter().any(|(n, _)| *n == key.name)
                || curves.iter().any(|(n, _)| *n == key.name)
            {
                return Err(Diagnostic::new(
                    format!("lane `{}` is given twice", key.name),
                    key.span,
                ));
            }
            let pattern = match self.eval(value, None)? {
                PerfValue::Pattern(p) => p,
                PerfValue::Env(curve) => {
                    if key.name == "notes" {
                        return Err(Diagnostic::new(
                            "the `notes` lane holds notes, not an envelope",
                            value.span,
                        )
                        .with_help(
                            "an envelope moves a track's parameter: `cut: env[300hz 1bar 3khz]`",
                        ));
                    }
                    curves.push((key.name.clone(), curve));
                    continue;
                }
                PerfValue::Rest => Pattern::silence(),
                other => {
                    return Err(Diagnostic::new(
                        format!("a lane holds a pattern, but this is {}", other.describe()),
                        value.span,
                    ));
                }
            };
            lanes.push((key.name.clone(), pattern));
        }
        // a lane spans the whole clip, whatever its length
        if let Some(length) = length {
            lanes = lanes
                .into_iter()
                .map(|(name, p)| (name, p.slow(length)))
                .collect();
        }
        Ok(Clip {
            name: block.name.name.clone(),
            length: length.unwrap_or(Rational::ONE),
            endless: length.is_none(),
            lanes,
            keys,
            curves,
            signals: Vec::new(),
        })
    }

    /// `midi.keys()` or `midi.keys(channel: 1)`
    fn keys(&self, e: &Expr) -> Res<Keys> {
        let ExprKind::Call { name, args } = &e.kind else {
            return Err(self.keys_error(e.span));
        };
        let from_midi = matches!(args.first(), Some(Arg { receiver: true, value: Expr { kind: ExprKind::Name(m), .. }, .. }) if m == "midi");
        if name.name != "keys" || !from_midi {
            return Err(self.keys_error(e.span));
        }
        let mut channel = None;
        let mut mpe = false;
        for arg in &args[1..] {
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                Some("mpe") => match &arg.value.kind {
                    ExprKind::Name(n) if n == "true" => mpe = true,
                    ExprKind::Name(n) if n == "false" => mpe = false,
                    _ => {
                        return Err(Diagnostic::new(
                            "`mpe` is `true` or `false`",
                            arg.value.span,
                        ));
                    }
                },
                Some("channel") => {
                    let n = self.plain(&arg.value)?;
                    if !n.is_integer() || n < Rational::ONE || n > Rational::int(16) {
                        return Err(Diagnostic::new(
                            "a MIDI channel is a whole number from 1 to 16",
                            arg.value.span,
                        ));
                    }
                    channel = Some(n.numer() as u8);
                }
                _ => {
                    return Err(Diagnostic::new(
                        "`midi.keys` takes `channel:` or `mpe:`",
                        arg.value.span,
                    ));
                }
            }
        }
        if mpe && channel.is_some() {
            return Err(Diagnostic::new(
                "`mpe: true` plays from every member channel, so it takes no `channel:`",
                e.span,
            ));
        }
        Ok(Keys { channel, mpe })
    }

    fn keys_error(&self, span: Span) -> Diagnostic {
        Diagnostic::new(
            "`keys` plays a clip from a keyboard: write `keys: midi.keys(channel: 1)`",
            span,
        )
    }

    fn scene(&self, block: &NamedBlock) -> Res<Scene> {
        let mut entries: Vec<(usize, Option<Clip>)> = Vec::new();
        for (key, value) in &block.entries {
            let track = self.track(key)?;
            if entries.iter().any(|(t, _)| *t == track) {
                return Err(Diagnostic::new(
                    format!("track `{}` appears twice in this scene", key.name),
                    key.span,
                ));
            }
            let clip = match self.eval(value, None)? {
                PerfValue::Rest => None,
                other => Some(self.to_clip(other, &key.name, value.span)?),
            };
            entries.push((track, clip));
        }
        Ok(Scene {
            name: block.name.name.clone(),
            entries,
        })
    }

    /// Anything playable as a clip: a clip, or a bare pattern of notes.
    fn to_clip(&self, v: PerfValue, hint: &str, span: Span) -> Res<Clip> {
        match v {
            PerfValue::Clip(c) => Ok(c),
            PerfValue::Pattern(p) => Ok(Clip {
                name: hint.to_string(),
                length: Rational::ONE,
                endless: true,
                lanes: vec![("notes".to_string(), p)],
                keys: None,
                curves: Vec::new(),
                signals: Vec::new(),
            }),
            PerfValue::Grid(_) => Err(Diagnostic::new("a grid has no pitches to play", span)
                .with_help("give it notes with `.rhythm(grid[...])` on a pattern instead")),
            PerfValue::Scene(_) => Err(Diagnostic::new("a scene is launched, not played", span)
                .with_help("write `name.start`")),
            PerfValue::Env(_) => Err(Diagnostic::new(
                "an envelope moves a parameter, so it goes in a lane",
                span,
            )
            .with_help("for example `clip c { notes: [c4] cut: env[300hz 1bar 3khz] }`")),
            PerfValue::Rest => Err(Diagnostic::new(
                "`~` stops a track in a scene; to stop one, write `track.stop`",
                span,
            )),
        }
    }

    // ---- commands ----------------------------------------------------------------

    /// A track by name; the implicit track 0 can't be named.
    fn track(&self, id: &Ident) -> Res<usize> {
        match self
            .cx
            .tracks
            .iter()
            .skip(1)
            .position(|t| t.name == id.name)
        {
            Some(i) => Ok(i + 1),
            None => {
                let mut d = Diagnostic::new(format!("no track named `{}`", id.name), id.span);
                if let Some(c) = closest(
                    &id.name,
                    self.cx.tracks.iter().skip(1).map(|t| t.name.as_str()),
                ) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                Err(d)
            }
        }
    }

    fn at_time(&self, at: &Option<AtPos>) -> Res<Time> {
        let bpb = self.cx.tempo.beats_per_bar;
        match at {
            None => Ok(Time::Beats(0.0)),
            Some(AtPos::Time(e) | AtPos::After(e)) => time_of(e, self),
            Some(AtPos::Bar { bar, beat }) => {
                let number = |e: &Expr| self.plain(e).map(|r| r.to_f64());
                let bar_n = number(bar)?;
                let beat_n = beat.as_ref().map(number).transpose()?.unwrap_or(1.0);
                if bar_n < 1.0 || beat_n < 1.0 {
                    return Err(Diagnostic::new("bars and beats count from 1", bar.span));
                }
                Ok(Time::Beats((bar_n - 1.0) * bpb + (beat_n - 1.0)))
            }
        }
    }

    fn command(&self, stmt: &CommandStmt) -> Res<Vec<Scheduled>> {
        let at = self.at_time(&stmt.at)?;
        let quantize = stmt.quantize;
        let schedule = |action| Scheduled {
            at,
            action,
            quantize,
        };

        Ok(match &stmt.command {
            Command::Play { track, what } => {
                if self.cx.failed.contains(&track.name.as_str()) {
                    return Ok(Vec::new());
                }
                let t = self.track(track)?;
                let clip = self.to_clip(self.eval(what, None)?, &track.name, what.span)?;
                let clip = self.on_track(clip, t, track, what.span)?;
                vec![schedule(Action::Play { track: t, clip })]
            }
            Command::Start { name, .. } => {
                if matches!(self.bindings.get(&name.name), Some(PerfValue::Scene(_))) {
                    return self.launch(name, &schedule);
                }
                if self.cx.failed.contains(&name.name.as_str()) {
                    return Ok(Vec::new());
                }
                let Ok(t) = self.track(name) else {
                    let scenes = self
                        .bindings
                        .iter()
                        .filter(|(_, v)| matches!(v, PerfValue::Scene(_)))
                        .map(|(k, _)| k.as_str());
                    let parts = self.cx.tracks.iter().map(|t| t.name.as_str());
                    let mut d = Diagnostic::new(
                        format!("no scene or part named `{}`", name.name),
                        name.span,
                    );
                    if let Some(c) = closest(&name.name, scenes.chain(parts)) {
                        d = d.with_help(format!("did you mean `{c}`?"));
                    }
                    return Err(d);
                };
                if !matches!(
                    self.bindings.get(&name.name),
                    Some(PerfValue::Clip(_) | PerfValue::Pattern(_))
                ) {
                    return Err(Diagnostic::new(
                        format!(
                            "`{0}` is not a scene or a clip, so `{0}.start` has nothing to start",
                            name.name
                        ),
                        name.span,
                    ));
                }
                let what = Expr {
                    kind: ExprKind::Name(name.name.clone()),
                    span: name.span,
                };
                let clip = self.to_clip(self.eval(&what, None)?, &name.name, name.span)?;
                let clip = self.on_track(clip, t, name, name.span)?;
                vec![schedule(Action::Play { track: t, clip })]
            }
            Command::Stop(t) => match self.bindings.get(&t.name) {
                // stopping a scene stops the tracks it plays on
                Some(PerfValue::Scene(scene)) => scene
                    .entries
                    .iter()
                    .map(|(track, _)| schedule(Action::Stop { track: *track }))
                    .collect(),
                _ => vec![schedule(Action::Stop {
                    track: self.track(t)?,
                })],
            },
            Command::Mute(t) => vec![schedule(Action::Mute {
                track: self.track(t)?,
            })],
            Command::Unmute(t) => vec![schedule(Action::Unmute {
                track: self.track(t)?,
            })],
            Command::Solo(t) => vec![schedule(Action::Solo {
                track: self.track(t)?,
            })],
            Command::Unsolo(t) => {
                let track = t.as_ref().map(|t| self.track(t)).transpose()?;
                vec![schedule(Action::Unsolo { track })]
            }
            Command::Hush => vec![schedule(Action::Hush)],
            Command::Panic => vec![schedule(Action::Panic)],
            Command::Reset | Command::New => vec![schedule(Action::Reset)],
            // changing what a track plays is for a live session; in a file it does nothing
            Command::Edit { .. } => Vec::new(),
        })
    }

    /// The commands that launch the scene called `name`: its clips start, and
    /// every other track with an instrument stops.
    fn launch(&self, name: &Ident, schedule: &dyn Fn(Action) -> Scheduled) -> Res<Vec<Scheduled>> {
        let Some(PerfValue::Scene(scene)) = self.bindings.get(&name.name) else {
            let mut d = Diagnostic::new(format!("no scene named `{}`", name.name), name.span);
            let scenes = self
                .bindings
                .iter()
                .filter(|(_, v)| matches!(v, PerfValue::Scene(_)))
                .map(|(k, _)| k.as_str());
            if let Some(c) = closest(&name.name, scenes) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let mut out = Vec::new();
        for (track, clip) in &scene.entries {
            match clip {
                Some(clip) => {
                    let id = Ident {
                        name: self.cx.tracks[*track].name.clone(),
                        span: name.span,
                    };
                    let clip = self.on_track(clip.clone(), *track, &id, name.span)?;
                    out.push(schedule(Action::Play {
                        track: *track,
                        clip,
                    }));
                }
                None => out.push(schedule(Action::Stop { track: *track })),
            }
        }
        for (track, info) in self.cx.tracks.iter().enumerate() {
            if info.instrument.is_some() && !scene.entries.iter().any(|(t, _)| *t == track) {
                out.push(schedule(Action::Stop { track }));
            }
        }
        Ok(out)
    }

    /// Scenes one after another, each for its length, then everything stops. A section may have its own
    /// tempo and meter, from the `tempo` and `meter` lists.
    fn arrangement(&self, decl: &ArrangementDecl) -> Res<Arrangement> {
        for (name, list) in [("tempo", &decl.tempos), ("meter", &decl.meters)] {
            if let Some(list) = list
                && list.len() != decl.sections.len()
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{name}` has {} where there are {} sections",
                        list.len(),
                        decl.sections.len()
                    ),
                    list.first().map_or(decl.name.span, |e| e.span),
                )
                .with_help(format!("give `{name}` one for each section")));
            }
        }
        let ramp = decl.tempo_env.as_ref().map(tempo_curve).transpose()?;
        let own = decl.tempos.is_some() || decl.meters.is_some() || ramp.is_some();
        let mut seconds = 0.0;
        let mut bars_so_far = 0.0;
        let mut actions = Vec::new();
        let mut tempos: Vec<(f64, Tempo)> = Vec::new();
        for (i, section) in decl.sections.iter().enumerate() {
            let mut tempo = self.cx.tempo;
            if let Some(e) = decl.tempos.as_ref().map(|l| &l[i]) {
                tempo.bpm = crate::compile::bpm_of(e)?;
            }
            if let Some(e) = decl.meters.as_ref().map(|l| &l[i]) {
                tempo.beats_per_bar = crate::compile::meter_beats(e)?;
            }
            let length = self.cycles_in(&section.length, tempo.beats_per_bar)?;
            if length <= Rational::ZERO {
                return Err(Diagnostic::new(
                    "a section needs a length above zero",
                    section.length.span,
                ));
            }
            let at = Time::Seconds(seconds);
            actions.extend(self.launch(&section.scene, &|action| Scheduled {
                at,
                action,
                quantize: None,
            })?);
            let bars = length.to_f64();
            match &ramp {
                None => {
                    if own {
                        tempos.push((seconds, tempo));
                    }
                    seconds += bars * tempo.beats_per_bar * 60.0 / tempo.bpm;
                }
                Some(curve) => {
                    // the tempo changes gradually: a run of short steps, each at the tempo of its middle
                    let steps = (bars / RAMP_STEP_BARS).ceil().max(1.0) as usize;
                    for j in 0..steps {
                        let from = j as f64 * RAMP_STEP_BARS;
                        let width = (bars - from).min(RAMP_STEP_BARS);
                        let step = Tempo {
                            bpm: curve.bpm_at(bars_so_far + from + width / 2.0),
                            beats_per_bar: tempo.beats_per_bar,
                        };
                        if tempos.last().is_none_or(|(_, t)| *t != step) {
                            tempos.push((seconds, step));
                        }
                        seconds += width * step.beats_per_bar * 60.0 / step.bpm;
                    }
                }
            }
            bars_so_far += bars;
        }
        actions.push(Scheduled {
            at: Time::Seconds(seconds),
            action: Action::Hush,
            quantize: None,
        });
        let mut lanes = Vec::new();
        for lane in &decl.lanes {
            let (track, param) = (&lane.track, &lane.param);
            if !self.cx.tracks.iter().skip(1).any(|t| t.name == track.name) {
                let mut d = Diagnostic::new(format!("no track named `{}`", track.name), track.span);
                if let Some(c) = closest(
                    &track.name,
                    self.cx.tracks.iter().skip(1).map(|t| t.name.as_str()),
                ) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
            }
            let mut curve = env_lane(&lane.value)?;
            let who = format!(
                "arrangement `{}` moves `{}.{}`, but track `{}`",
                decl.name.name, track.name, param.name, track.name
            );
            self.bind_env(&mut curve, &track.name, &param.name, &who, param.span)?;
            lanes.push((format!("{}::{}", track.name, param.name), curve));
        }
        Ok(Arrangement {
            name: decl.name.name.clone(),
            actions,
            tempos,
            lanes,
        })
    }

    /// The clip as `track` plays it: its `notes` line, if it has one, changes the clip's notes. It also
    /// has to make sense on the track's instrument.
    fn on_track(&self, mut clip: Clip, track: usize, id: &Ident, span: Span) -> Res<Clip> {
        if let Some(effect) = &self.cx.tracks[track].notes
            && let Some(at) = clip.lanes.iter().position(|(lane, _)| lane == "notes")
        {
            let it = PerfValue::Pattern(clip.lanes[at].1.clone());
            match self.eval(effect, Some(&it)).map_err(|d| {
                Diagnostic::new(
                    format!("the `notes` line of track `{}`: {}", id.name, d.message),
                    effect.span,
                )
            })? {
                PerfValue::Pattern(p) => clip.lanes[at].1 = p,
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "the `notes` line of track `{}` makes {}, not a pattern",
                            id.name,
                            other.describe()
                        ),
                        effect.span,
                    )
                    .with_help("it should change `it`, as in `notes = it.thin(10%)`"));
                }
            }
        }
        self.bind_curves(&mut clip, track, id, span)?;
        self.check_clip(&clip, track, id, span)?;
        Ok(clip)
    }

    /// Envelope lanes move a parameter of the track: find it, and check the levels fit its unit.
    fn bind_curves(&self, clip: &mut Clip, track: usize, id: &Ident, span: Span) -> Res<()> {
        let name = &self.cx.tracks[track].name;
        for (lane, curve) in &mut clip.curves {
            let who = format!(
                "clip `{}` has an envelope lane `{lane}`, but track `{}`",
                clip.name, id.name
            );
            self.bind_env(curve, name, lane, &who, span)?;
        }
        // a lane that is a signal and named after a parameter of the track moves it continuously
        clip.signals.clear();
        let probe = niminal_pattern::Span::new(Rational::ZERO, Rational::ZERO);
        for (lane, pattern) in &clip.lanes {
            let is_signal = pattern
                .query(probe)
                .first()
                .is_some_and(|h| h.whole.is_none());
            let control = format!("{name}::{lane}");
            if let Some(def) = self
                .cx
                .names
                .controls
                .iter()
                .find(|c| c.name == control)
                .filter(|_| is_signal)
            {
                clip.signals.push((lane.clone(), def.unit));
            }
        }
        Ok(())
    }

    /// Point `curve` at the track parameter `track::param` and check its levels fit that parameter's unit.
    /// `who` starts the message if there is no such parameter.
    fn bind_env(
        &self,
        curve: &mut EnvLane,
        track: &str,
        param: &str,
        who: &str,
        span: Span,
    ) -> Res<()> {
        let control = format!("{track}::{param}");
        let Some(def) = self.cx.names.controls.iter().find(|c| c.name == control) else {
            let params: Vec<&str> = self
                .cx
                .names
                .controls
                .iter()
                .filter_map(|c| c.name.strip_prefix(&format!("{track}::")))
                .collect();
            let mut d = Diagnostic::new(format!("{who} has no such parameter"), span)
                .with_help("only a track's parameters can be moved by an envelope, as in `track bass(tone: hz = 2khz)`");
            if let Some(c) = closest(param, params) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let tempo = Tempo {
            bpm: 120.0,
            beats_per_bar: 4.0,
        };
        let levels =
            std::iter::once(&curve.start).chain(curve.segments.iter().map(|(_, _, level)| level));
        for level in levels {
            crate::program::engine_value(def.unit, *level, tempo).map_err(|e| {
                Diagnostic::new(
                    format!("envelope lane `{param}` moves {}: {e}", def.name),
                    span,
                )
            })?;
        }
        curve.unit = Some(def.unit);
        Ok(())
    }

    /// Does `clip`'s notes and lanes make sense on `track`'s instrument?
    fn check_clip(&self, clip: &Clip, track: usize, id: &Ident, span: Span) -> Res<()> {
        let info = &self.cx.tracks[track];
        let Some(instrument) = info.instrument.map(|i| &self.cx.instruments[i]) else {
            return Err(Diagnostic::new(
                format!("track `{}` has no instrument to play a clip on", id.name),
                id.span,
            )
            .with_help("add `instrument = ...` to the track"));
        };
        if clip.keys.is_some() {
            if clip.lane("notes").is_some() {
                return Err(Diagnostic::new(
                    format!(
                        "clip `{}` has both `notes` and `keys`: it is played by one or the other",
                        clip.name
                    ),
                    span,
                ));
            }
            if !instrument
                .params
                .iter()
                .any(|p| p.name == PITCH_PARAM && p.unit == Unit::Hz)
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` has no `{PITCH_PARAM}` parameter for the keys to set",
                        instrument.name
                    ),
                    span,
                )
                .with_help(format!(
                    "give the instrument a `{PITCH_PARAM}: hz` parameter"
                )));
            }
        }
        for (lane, _) in &clip.lanes {
            if lane == "notes" {
                let pitched = instrument
                    .params
                    .iter()
                    .any(|p| p.name == PITCH_PARAM && p.unit == Unit::Hz);
                if !pitched {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` has no `{PITCH_PARAM}` parameter for the notes to set",
                            instrument.name
                        ),
                        span,
                    )
                    .with_help(format!(
                        "give the instrument a `{PITCH_PARAM}: hz` parameter"
                    )));
                }
                continue;
            }
            if UNIVERSAL_LANES.contains(&lane.as_str())
                || instrument.params.iter().any(|p| p.name == *lane)
            {
                continue;
            }
            if lane == "to" {
                return Err(Diagnostic::new(
                    format!("the `{lane}` lane isn't supported yet"),
                    span,
                ));
            }
            if self
                .cx
                .names
                .controls
                .iter()
                .any(|c| c.name == format!("{}::{lane}", info.name))
            {
                if clip.lane("notes").is_none()
                    && clip.keys.is_none()
                    && !clip.signals.iter().any(|(l, _)| l == lane)
                {
                    return Err(Diagnostic::new(
                        format!("lane `{lane}` sets a parameter of track `{}` as each note starts, but clip `{}` has no notes", info.name, clip.name),
                        span,
                    )
                    .with_help("add a `notes` lane to step on, or change it as the control instead"));
                }
                continue;
            }
            let mut known: Vec<&str> = instrument.params.iter().map(|p| p.name.as_str()).collect();
            known.extend(UNIVERSAL_LANES);
            let mut d = Diagnostic::new(
                format!(
                    "clip `{}` has a lane `{lane}`, but `{}` has no such parameter",
                    clip.name, instrument.name
                ),
                span,
            );
            if let Some(c) = closest(lane, known) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        }
        Ok(())
    }
}

/// When a kind of change lands, as written in `config` and `with(quantize: ...)`: `1beat`, `4bars`,
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

/// The one option a clip, scene or track takes after its body: `with(quantize: 1bar)`.
pub(crate) fn definition_quantum(options: &[Arg]) -> Res<Option<Quantize>> {
    let mut found = None;
    for arg in options {
        match arg.name.as_ref().map(|n| n.name.as_str()) {
            Some("quantize") => found = Some(quantum_of(&arg.value)?),
            Some(other) => {
                return Err(Diagnostic::new(
                    format!("there is no option `{other}` here"),
                    arg.name.as_ref().map_or(arg.value.span, |n| n.span),
                )
                .with_help("the option is `quantize`, as in `with(quantize: 1bar)`"));
            }
            None => {
                return Err(Diagnostic::new(
                    "an option is a name and a value, as in `quantize: 1bar`",
                    arg.value.span,
                ));
            }
        }
    }
    Ok(found)
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

/// Where in the source a problem inside a pattern's text is.
fn at_offset(message: String, token: Span, offset: usize) -> Diagnostic {
    // the token's text starts after its opening bracket
    let at = token.start + 1 + offset;
    Diagnostic::new(message, Span::new(at, (at + 1).min(token.end)))
}

/// How long, in bars, each step of a gradual tempo change is.
const RAMP_STEP_BARS: f64 = 1.0 / 8.0;

/// A tempo over the piece, in bars of music: `env[84bpm 64bars 84bpm 8bars.log 96bpm]`.
struct TempoCurve {
    start: f64,
    /// For each stretch: how many bars it takes, its shape, and the tempo it reaches.
    segments: Vec<(f64, Curve, f64)>,
}

impl TempoCurve {
    /// The tempo `bar` bars into the piece, holding the last level after the end.
    fn bpm_at(&self, bar: f64) -> f64 {
        let (mut from, mut at) = (self.start, 0.0);
        for &(len, shape, to) in &self.segments {
            if bar < at + len {
                let t = ((bar - at) / len).clamp(0.0, 1.0);
                return f64::from(niminal_engine::ops::shape(
                    from as f32,
                    to as f32,
                    t,
                    shape.bend(),
                ));
            }
            (from, at) = (to, at + len);
        }
        from
    }
}

fn tempo_curve(lit: &EnvLit) -> Res<TempoCurve> {
    let bpm = |v: &EnvValue| {
        let n = lit_step(v)?;
        match (n.unit.as_deref(), n.value) {
            (Some("bpm"), v) if v > 0.0 => Ok(v),
            _ => Err(
                Diagnostic::new("a tempo level is a positive number of bpm", n.span)
                    .with_help("for example `env[84bpm 8bars 96bpm]`"),
            ),
        }
    };
    if lit.sustain_at.is_some() || lit.looped {
        return Err(
            Diagnostic::new("a tempo envelope runs once over the piece", lit.start.span())
                .with_help("take out the `|` or `.loop`"),
        );
    }
    let mut segments = Vec::with_capacity(lit.segments.len());
    for seg in &lit.segments {
        let d = lit_step(&seg.dur)?;
        let len = match (d.unit.as_deref(), d.value) {
            (Some("bar" | "bars"), v) if v > 0.0 => v,
            _ => {
                return Err(Diagnostic::new(
                    "the times in a tempo envelope are lengths in bars",
                    d.span,
                )
                .with_help("for example `8bars`"));
            }
        };
        segments.push((len, curve_of(seg.curve), bpm(&seg.level)?));
    }
    Ok(TempoCurve {
        start: bpm(&lit.start)?,
        segments,
    })
}

pub(crate) fn negate(v: Value) -> Value {
    match v {
        Value::Num(n) => Value::Num(-n),
        Value::Hz(n) => Value::Hz(-n),
        Value::Db(n) => Value::Db(-n),
        Value::Seconds(n) => Value::Seconds(-n),
        Value::Beats(n) => Value::Beats(-n),
        Value::Bars(n) => Value::Bars(-n),
        Value::Semitones(n) => Value::Semitones(-n),
        Value::Degrees(n) => Value::Degrees(-n),
        keep @ (Value::Note(_) | Value::Sample(_)) => keep,
    }
}

/// Move a pitch by `st` semitones. Notes stay notes when the interval is whole.
fn transpose(v: Value, st: f64) -> Value {
    match v {
        Value::Note(m) if st.fract() == 0.0 => Value::Note(m + st as i32),
        Value::Note(_) | Value::Hz(_) => {
            let hz = v.as_hz().expect("notes and frequencies have a frequency");
            Value::Hz(hz * 2f64.powf(st / 12.0))
        }
        other => other,
    }
}
