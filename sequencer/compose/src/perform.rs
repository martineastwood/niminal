//! Evaluating the performance layer: pattern literals and their operations,
//! clips, scenes, and the commands that play them.

use std::collections::{BTreeMap, HashMap};

use niminal_engine::ops::Curve;
use niminal_pattern::{
    Hit, MAX_REPEATS, Param, Pattern, Rational, Strength, Structure, grid, grid_at, parse_with,
    unit_random,
};
use niminal_score::{Tempo, Time, Value};

use crate::ast::*;
use crate::compile::Names;
use crate::performance::*;
use niminal_lang::GAIN_PARAM;
use niminal_lang::Unit;
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
                if let Some(c) = closest(member, sample_names.iter().map(String::as_str)) {
                    message.push_str(&format!(" (did you mean `{c}`?)"));
                }
                Err(message)
            }
            None => Err(e.to_string()),
        }
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
/// Whose timing `add`, `sub`, `mul` and `div` keep.
const TIMINGS: [(&str, &Structure); 3] = [
    ("first", &Structure::Left),
    ("second", &Structure::Right),
    ("both", &Structure::Both),
];

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
        // Score functions are also registered in the project name table.
        if let Some(kind) = self.cx.names.kind_of(&name.name).filter(|k| {
            let own_instrument = matches!(*k, "instrument" | "sample" | "kit")
                && matches!(value, PerfValue::Clip(_))
                && self.cx.nodes.iter().any(|node| {
                    node.name == name.name
                        && node
                            .instrument
                            .is_some_and(|i| self.cx.instruments[i].name == name.name)
                });
            !own_instrument && !(*k == "score function" && matches!(value, PerfValue::Def(_)))
        }) {
            return Err(Diagnostic::new(
                format!("`{}` is already the name of a {kind}", name.name),
                name.span,
            ));
        }
        if matches!(name.name.as_str(), "in" | "it" | "out") {
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
            ExprKind::Steps(raw) => steps(raw)
                .map(PerfValue::Pattern)
                .map_err(|message| Diagnostic::new(message, e.span)),
            ExprKind::FxLanes(_) => Err(Diagnostic::new(
                "a block of lanes belongs in a clip, named after an effect",
                e.span,
            )
            .with_help("for example `clip c { trance_gate { open: x.x. xx.x } }`")),
            ExprKind::Rest => Ok(PerfValue::Rest),
            // inside a change, `same` is the pattern being changed
            ExprKind::Name(name) if name == "same" && it.is_some() => {
                Ok(it.cloned().expect("checked"))
            }
            ExprKind::Name(name) if name == "true" => Ok(PerfValue::Num(Value::Num(1.0))),
            ExprKind::Name(name) if name == "false" => Ok(PerfValue::Num(Value::Num(0.0))),
            ExprKind::Name(name) => match self.bindings.get(name) {
                Some(v) => Ok(v.clone()),
                None if signal(name).is_some() => Ok(PerfValue::Pattern(
                    signal(name)
                        .expect("checked")
                        .map(|x| Step::from(Value::Num(x))),
                )),
                // a lone note or value is a one-step pattern (and a number for arithmetic)
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
            ExprKind::Num { value, unit } => {
                Ok(PerfValue::Num(value_of(*value, unit.as_deref(), e.span)?))
            }
            ExprKind::Neg(inner) => match self.eval(inner, it)? {
                PerfValue::Num(v) => Ok(PerfValue::Num(negate(v))),
                PerfValue::Pattern(p) => Ok(PerfValue::Pattern(p.map(|s| s.with(negate(s.value))))),
                other => Err(Diagnostic::new(
                    format!("can't negate {}", other.describe()),
                    e.span,
                )),
            },
            ExprKind::Call { name, args } if name.name == "load" => self.load(args, e.span),
            ExprKind::Call { name, args }
                if name.name == "if_else" && !args.first().is_some_and(|a| a.receiver) =>
            {
                // `if_else(grid[x.x.], a, b)`: a where the grid hits, b where it rests
                let [cond, yes, no] = args.as_slice() else {
                    return Err(Diagnostic::new(
                        "`if_else` takes a grid and two patterns, as in `if_else(grid[x.x.], a, b)`",
                        e.span,
                    ));
                };
                let PerfValue::Grid(g) = self.eval(&cond.value, it)? else {
                    return Err(Diagnostic::new(
                        "`if_else` starts with a grid, such as `grid[x.x. xxx.]`",
                        cond.value.span,
                    ));
                };
                let pattern = |a: &Arg| match self.eval(&a.value, it)? {
                    PerfValue::Pattern(p) => Ok(p),
                    other => Err(Diagnostic::new(
                        format!("`if_else` needs patterns, found {}", other.describe()),
                        a.value.span,
                    )),
                };
                let (yes, no) = (pattern(yes)?, pattern(no)?);
                Ok(PerfValue::Pattern(Pattern::stack(vec![
                    yes.mask(&g.map(|_| true)),
                    no.mask(&g.invert(Hit::new(Strength::Normal)).map(|_| true)),
                ])))
            }
            ExprKind::Call { name, args }
                if matches!(name.name.as_str(), "count_to" | "random_int")
                    && !args.first().is_some_and(|a| a.receiver) =>
            {
                let [arg] = args.as_slice() else {
                    return Err(Diagnostic::new(
                        format!("`{}` takes one number", name.name),
                        e.span,
                    ));
                };
                let n = self.whole(&arg.value, &name.name)?;
                let number = |x: f64| Step::from(Value::Num(x));
                Ok(PerfValue::Pattern(if name.name == "count_to" {
                    Pattern::fastcat((0..n).map(|i| Pattern::pure(number(i as f64))).collect())
                } else {
                    Pattern::<f64>::rand().map(move |x| number((x * n as f64).floor()))
                }))
            }
            ExprKind::Call { name, args }
                if matches!(name.name.as_str(), "stack" | "cat" | "seq" | "take_turns")
                    && !args.first().is_some_and(|a| a.receiver) =>
            {
                self.combine(name, args, e.span, it)
            }
            ExprKind::Call { name, args } => {
                if let Some(PerfValue::Def(def)) = self.bindings.get(&name.name) {
                    return self.call_def(def.clone(), args, e.span, it);
                }
                self.op(name, args, e.span, it)
            }
            ExprKind::ClipBlock(block) => self.clip(block).map(PerfValue::Clip),
            ExprKind::SceneBlock(block) => self.scene(block).map(PerfValue::Scene),
            ExprKind::Env(lit) => env_lane(lit).map(PerfValue::Env),
            ExprKind::Events(_) => Err(Diagnostic::new(
                "`events { }` belongs inside a clip, as a lane",
                e.span,
            )),
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can only be an argument that asks for it",
                e.span,
            )),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span, it),
            ExprKind::Compare { op, lhs, rhs } => {
                let (l, r) = (self.eval(lhs, it)?, self.eval(rhs, it)?);
                let (lv, rv) = (l.as_num(), r.as_num());
                match (lv, rv) {
                    (Some(a), Some(b)) => {
                        let ok = cmp_values(*op, a, b);
                        Ok(PerfValue::Num(Value::Num(if ok { 1.0 } else { 0.0 })))
                    }
                    _ => Err(Diagnostic::new("comparisons need numbers or notes", e.span)),
                }
            }
            ExprKind::Logic { op, lhs, rhs } => {
                let l = self.eval(lhs, it)?.as_num().map(|v| match v {
                    Value::Num(n) => n != 0.0,
                    _ => true,
                });
                let r = self.eval(rhs, it)?.as_num().map(|v| match v {
                    Value::Num(n) => n != 0.0,
                    _ => true,
                });
                match (l, r, op) {
                    (Some(a), Some(b), LogicOp::And) => {
                        Ok(PerfValue::Num(Value::Num(if a && b { 1.0 } else { 0.0 })))
                    }
                    (Some(a), Some(b), LogicOp::Or) => {
                        Ok(PerfValue::Num(Value::Num(if a || b { 1.0 } else { 0.0 })))
                    }
                    _ => Err(Diagnostic::new("logic needs numbers", e.span)),
                }
            }
            ExprKind::Not(inner) => {
                let v = self.eval(inner, it)?;
                match v.as_num() {
                    Some(Value::Num(n)) => {
                        Ok(PerfValue::Num(Value::Num(if n == 0.0 { 1.0 } else { 0.0 })))
                    }
                    _ => Err(Diagnostic::new("`not` needs a number", e.span)),
                }
            }
            ExprKind::If {
                cond,
                then,
                otherwise,
            } => {
                let c = self.eval(cond, it)?;
                let truthy = match c.as_num() {
                    Some(Value::Num(n)) => n != 0.0,
                    Some(_) => true,
                    None => {
                        return Err(Diagnostic::new(
                            "`if` needs a condition that is a number",
                            cond.span,
                        ));
                    }
                };
                let body = if truthy {
                    then.as_slice()
                } else {
                    otherwise.as_deref().unwrap_or(&[])
                };
                self.block_value(body, it)
            }
            ExprKind::For { var, iter, body } => {
                let items = self.iterate(iter, it)?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let mut child = Perform {
                        cx: self.cx,
                        bindings: self.bindings.clone(),
                        arrangements: HashMap::new(),
                    };
                    child.bindings.insert(var.name.clone(), item);
                    out.push(child.block_value(body, None)?);
                }
                Ok(PerfValue::List(out))
            }
            ExprKind::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.eval(item, it)?);
                }
                Ok(PerfValue::List(out))
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let lo = self
                    .eval(lo, it)?
                    .as_num()
                    .and_then(|v| v.as_number().ok())
                    .ok_or_else(|| Diagnostic::new("range bounds are numbers", e.span))?;
                let hi = self
                    .eval(hi, it)?
                    .as_num()
                    .and_then(|v| v.as_number().ok())
                    .ok_or_else(|| Diagnostic::new("range bounds are numbers", e.span))?;
                let mut out = Vec::new();
                let mut i = lo.floor() as i64;
                let end = hi.floor() as i64;
                while if *inclusive { i <= end } else { i < end } {
                    out.push(PerfValue::Num(Value::Num(i as f64)));
                    i += 1;
                    if out.len() > 10_000 {
                        return Err(Diagnostic::new("range is too long", e.span));
                    }
                }
                Ok(PerfValue::List(out))
            }
            ExprKind::Index { list, index } => {
                let list = self.eval(list, it)?;
                let i = self
                    .eval(index, it)?
                    .as_num()
                    .and_then(|v| v.as_number().ok())
                    .ok_or_else(|| Diagnostic::new("an index is a whole number", index.span))?
                    as i64;
                match list {
                    PerfValue::List(items) => {
                        let i = if i < 0 { items.len() as i64 + i } else { i };
                        items.get(i as usize).cloned().ok_or_else(|| {
                            Diagnostic::new(format!("index {i} is out of range"), index.span)
                        })
                    }
                    _ => Err(Diagnostic::new("only a list can be indexed here", e.span)),
                }
            }
            ExprKind::Lambda { .. } => Err(Diagnostic::new(
                "lambdas belong in instruments and functions, not in clips and scenes",
                e.span,
            )),
        }
    }

    /// A range of the source as the client that sent it knows it: which evaluation, and where in it.
    fn sent(&self, start: usize, end: usize) -> Option<(u64, usize, usize)> {
        let Some(origins) = self.cx.origins else {
            return Some((0, start, end));
        };
        origins
            .iter()
            .find(|(from, to, ..)| start >= *from && end <= *to)
            .map(|&(from, _, eval, offset)| (eval, offset + start - from, offset + end - from))
    }

    fn pattern(&self, raw: &str, span: Span) -> Res<Pattern<Step>> {
        parse_with(raw, |atom, at| {
            // the token's text starts after its opening bracket
            let start = span.start + 1 + at;
            let written = |v: Value| Step {
                src: self.sent(start, start + atom.len()),
                ..Step::from(v)
            };
            if let Ok(v) = atom_value(atom, self.cx.sample_names) {
                return Ok(written(v));
            }
            // Score `def` params and lets: `[root ~]` resolves `root` from bindings.
            // A lone note name is stored as a one-step pattern; take its value.
            match self.bindings.get(atom).and_then(|v| v.as_num()) {
                Some(v) => Ok(written(v)),
                None => match self.bindings.get(atom) {
                    Some(other) => Err(format!(
                        "`{atom}` is {}, not a note or value for a pattern",
                        other.describe()
                    )),
                    None => atom_value(atom, self.cx.sample_names).map(written),
                },
            }
        })
        .map_err(|e| at_offset(e.message, span, e.offset))
    }

    /// A list of notes, rests or patterns as one pattern in sequence.
    fn list_pattern(&self, items: Vec<PerfValue>, span: Span) -> Res<Pattern<Step>> {
        if items.is_empty() {
            return Ok(Pattern::silence());
        }
        let mut parts = Vec::with_capacity(items.len());
        for item in items {
            parts.push(match item {
                PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                PerfValue::Rest => Pattern::silence(),
                PerfValue::Pattern(p) => p,
                PerfValue::List(inner) => self.list_pattern(inner, span)?,
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "a pattern list holds notes, rests or patterns, not {}",
                            other.describe()
                        ),
                        span,
                    ));
                }
            });
        }
        Ok(Pattern::fastcat(parts))
    }

    /// Free functions `stack`, `cat`, `seq` and `take_turns` over patterns.
    fn combine(
        &self,
        name: &Ident,
        args: &[Arg],
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        let mut patterns = Vec::with_capacity(args.len());
        for arg in args {
            if arg.receiver {
                continue;
            }
            let v = self.eval(&arg.value, it)?;
            // `take_turns(for c in 0..<8 { .. })`: each item of a list is one turn
            if name.name == "take_turns"
                && let PerfValue::List(items) = v
            {
                for item in items {
                    patterns.push(self.list_pattern(vec![item], arg.value.span)?);
                }
                continue;
            }
            patterns.push(match v {
                PerfValue::Pattern(p) => p,
                PerfValue::Num(n) => Pattern::pure(Step::from(n)),
                PerfValue::Rest => Pattern::silence(),
                PerfValue::List(items) => self.list_pattern(items, arg.value.span)?,
                PerfValue::Clip(c) => c
                    .lanes
                    .iter()
                    .find(|(n, _)| n == "notes")
                    .map(|(_, p)| p.clone())
                    .ok_or_else(|| {
                        Diagnostic::new(
                            format!("`{}` needs patterns or notes", name.name),
                            arg.value.span,
                        )
                    })?,
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` needs patterns or notes, found {}",
                            name.name,
                            other.describe()
                        ),
                        arg.value.span,
                    ));
                }
            });
        }
        if patterns.is_empty() {
            return Err(Diagnostic::new(
                format!("`{}` needs at least one pattern", name.name),
                span,
            ));
        }
        Ok(PerfValue::Pattern(match name.name.as_str() {
            "stack" => Pattern::stack(patterns),
            "cat" | "seq" => Pattern::fastcat(patterns),
            "take_turns" => Pattern::slowcat(patterns),
            _ => unreachable!(),
        }))
    }

    /// `grid { kick: x... snare: .x.. }`: each row plays the sample it names on its hits,
    /// and the rows play together. A row can name a part to send its hits there, take
    /// parameters (`snare(pan: 10deg): ...`), and use mini-notation (`hat: [x*2 x]`).
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
            let row = parse_grid_row(code).map_err(|(offset, message)| here(offset, message))?;
            let member = row.name.rsplit_once('.').map_or(row.name, |(_, m)| m);
            let node = self
                .cx
                .nodes
                .iter()
                .position(|t| t.name == row.name || t.name == member);
            let id = self
                .cx
                .sample_names
                .iter()
                .position(|n| n == member)
                .or_else(|| {
                    // A row that only names a part takes the node's default sample.
                    node.and_then(|t| {
                        self.cx.nodes[t]
                            .defaults
                            .get("sample")
                            .and_then(|v| match *v {
                                Value::Sample(id) => Some(id as usize),
                                Value::Num(n) if n >= 0.0 => {
                                    let local = n as usize;
                                    self.cx.nodes[t]
                                        .instrument
                                        .and_then(|i| self.cx.instruments[i].members.get(local))
                                        .and_then(|name| {
                                            self.cx.sample_names.iter().position(|n| n == name)
                                        })
                                }
                                _ => None,
                            })
                    })
                });
            let Some(id) = id else {
                let mut message = format!("`{}` isn't a sample in a kit", row.name);
                if node.is_some() {
                    message = format!("`{}` is a node, but it has no kit sample to play", row.name);
                } else if let Some(c) =
                    closest(member, self.cx.sample_names.iter().map(String::as_str))
                {
                    message.push_str(&format!(" (did you mean `{c}`?)"));
                }
                return Err(here(row.name_offset, message));
            };
            let hits = if row.mini {
                parse_with(row.body, |atom, offset| {
                    let strength = match atom {
                        "x" => Strength::Normal,
                        "X" => Strength::Accent,
                        "o" => Strength::Ghost,
                        other => return Err(format!("`{other}` isn't a hit: use x, X, o, or ~")),
                    };
                    let start = at + row.body_offset + offset;
                    Ok(Hit {
                        strength,
                        src: self.sent(start, start + atom.len()),
                    })
                })
                .map_err(|e| here(row.body_offset + e.offset, e.message))?
            } else {
                grid_at(row.body, &|from, to| {
                    let base = at + row.body_offset;
                    self.sent(base + from, base + to)
                })
                .map_err(|e| here(row.body_offset + e.offset, e.message))?
            };
            let id = id as u32;
            let gain = row.gain;
            let pan = row.pan;
            let begin = row.begin;
            let pitch = row.pitch;
            layers.push(hits.map(move |hit| {
                let mut step = Step::from(Value::Sample(id));
                step.node = node;
                step.pan = pan;
                step.begin = begin;
                step.pitch = pitch;
                step.src = hit.src;
                step.gain = match hit.strength {
                    Strength::Normal => gain,
                    Strength::Accent => Some(gain.unwrap_or(0.0) + ACCENT_DB),
                    Strength::Ghost => Some(gain.unwrap_or(0.0) + GHOST_DB),
                };
                step
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
            ExprKind::Name(name) => match self.bindings.get(name).and_then(|v| v.as_num()) {
                Some(Value::Num(n)) => Ok((Rational::from_f64(n), None)),
                Some(Value::Bars(b)) => Ok((Rational::from_f64(b), Some("bars".into()))),
                Some(Value::Beats(b)) => Ok((Rational::from_f64(b), Some("beats".into()))),
                Some(Value::Seconds(s)) => Ok((Rational::from_f64(s), Some("sec".into()))),
                _ => Err(Diagnostic::new("expected a number", e.span)
                    .with_help("for example `2`, `3/2` or `1/8beat`")),
            },
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
                    plays: None,
                    then: Vec::new(),
                    lanes: vec![("notes".to_string(), p)],
                    scored: Vec::new(),
                    keys: None,
                    curves: Vec::new(),
                    signals: Vec::new(),
                    fx_params: Vec::new(),
                };
                self.on_clip(name, rest, clip, span, it)
                    .map(PerfValue::Clip)
            }
            PerfValue::Pattern(p) => self.on_pattern(name, rest, p, span, it),
            PerfValue::Grid(g) if name.name == "invert" => {
                Ok(PerfValue::Grid(g.invert(Hit::new(Strength::Normal))))
            }
            PerfValue::Grid(g) => match self.generic(name, rest, g, span, it)? {
                Some(g) => Ok(PerfValue::Grid(g)),
                None => Err(self.not_for(name, "a grid")),
            },
            PerfValue::Clip(clip) => self
                .on_clip(name, rest, clip, span, it)
                .map(PerfValue::Clip),
            PerfValue::Num(Value::Num(n)) if name.name == "mod" => {
                let by = self.arg(name, rest, "by", span)?;
                match self.eval(by, it)?.as_num() {
                    Some(Value::Num(m)) if m != 0.0 => {
                        Ok(PerfValue::Num(Value::Num(n.rem_euclid(m))))
                    }
                    _ => Err(Diagnostic::new("`mod` takes a number above zero", by.span)),
                }
            }
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
                let by = self.value_param(
                    self.arg(name, rest, "by", span)?,
                    it,
                    |v| match v {
                        Value::Semitones(st) => Some(st),
                        _ => None,
                    },
                    "`transpose` takes an interval in semitones",
                    "for example `transpose(+5st)` or `transpose([<0st +7st>])`",
                )?;
                by.apply(&p, |p, st| p.map(on_value(move |v| transpose(v, st))))
            }
            "add" | "sub" | "mul" | "div" => {
                let op = match name.name.as_str() {
                    "add" => BinOp::Add,
                    "sub" => BinOp::Sub,
                    "mul" => BinOp::Mul,
                    _ => BinOp::Div,
                };
                let structure = match self.find(name, rest, "timing") {
                    Some(e) => *self.word(e, "timing", &TIMINGS)?,
                    None => Structure::Left,
                };
                let other = self.eval(self.arg(name, rest, "other", span)?, it)?;
                return self.arithmetic(op, structure, PerfValue::Pattern(p), other, span);
            }
            "echo" => {
                let times = self.whole(self.arg(name, rest, "times", span)?, "echo")?;
                let gap = self.cycles(self.arg(name, rest, "time", span)?)?;
                let fade = match self.find(name, rest, "fade") {
                    Some(e) => match self.quantity(e)? {
                        (d, Some(u)) if u == "db" => d.to_f64(),
                        _ => {
                            return Err(Diagnostic::new(
                                "`fade` is how much quieter each echo gets, in db",
                                e.span,
                            )
                            .with_help("for example `fade: -6db`"));
                        }
                    },
                    None => -6.0,
                };
                let mut copies = vec![p.clone()];
                for i in 1..=times {
                    copies.push(p.shift(gap * Rational::int(i)).map(move |s| Step {
                        gain: Some(s.gain.unwrap_or(0.0) + fade * i as f64),
                        ..s
                    }));
                }
                Pattern::stack(copies)
            }
            "spread_evenly" => {
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
                        format!("`spread_evenly` takes at most {MAX_REPEATS} steps"),
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
            "with_rhythm_of" => {
                p.rhythm(&self.structure(self.arg(name, rest, "pattern", span)?, it)?)
            }
            "keep_where" => p.mask(&self.structure(self.arg(name, rest, "pattern", span)?, it)?),
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
                // `where(note < c3, change)`: change only the notes that pass the test
                let e = self.arg(name, rest, "when", span)?;
                let bad = || {
                    Diagnostic::new(
                        "`where` takes a test on the note, such as `note < c3`",
                        e.span,
                    )
                    .with_help("write `note` or `note.freq`, a comparison and a note or value")
                };
                let ExprKind::Compare { op, lhs, rhs } = &e.kind else {
                    return Err(bad());
                };
                let subject = match &lhs.kind {
                    ExprKind::Name(n) => n == "note",
                    ExprKind::Call { name, args } => {
                        name.name == PITCH_PARAM
                            && matches!(args.as_slice(), [a] if matches!(&a.value.kind, ExprKind::Name(n) if n == "note"))
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
                let values = match self.eval(e, it)? {
                    PerfValue::Pattern(p) => p,
                    PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                    _ => {
                        return Err(Diagnostic::new(
                            format!("`{}` takes {}", name.name, control.describe()),
                            e.span,
                        ));
                    }
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
            "stereo_copy" => {
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
        if name.name == "times" {
            let n = self.whole(self.arg(name, rest, "count", span)?, "times")?;
            return Ok(Clip {
                plays: Some(n as u32),
                endless: false,
                ..clip
            });
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
                plays: clip.plays,
                then: clip.then,
                lanes,
                scored: clip.scored,
                keys: clip.keys,
                curves: clip.curves,
                signals: Vec::new(),
                fx_params: Vec::new(),
            });
        }
        // Pitch operations touch only the notes; time operations touch every lane.
        let notes_only = matches!(
            name.name.as_str(),
            "transpose"
                | "echo"
                | "strum"
                | "often"
                | "rarely"
                | "add"
                | "sub"
                | "mul"
                | "div"
                | "spread_evenly"
                | "with_rhythm_of"
                | "keep_where"
                | "drop"
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
                | "stereo_copy"
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
            plays: clip.plays,
            then: clip.then,
            lanes,
            scored: clip.scored,
            keys: clip.keys,
            curves: clip.curves,
            signals: Vec::new(),
            fx_params: Vec::new(),
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
                PerfValue::Num(v) => Pattern::pure(Step::from(v)),
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
            "back_and_forth" => p.palindrome(),
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
            "drop" => {
                let amount = self.share_param(name, self.arg(name, rest, "amount", span)?, it)?;
                // The same amount always thins the same events, wherever it is written.
                let default = match &amount {
                    Param::Const(x) => x.to_bits(),
                    Param::Pat(_) => 0x4452_4F50,
                };
                let seed = self.seed(name, rest, default)?;
                amount.apply(&p, move |p, x| p.thin(x.clamp(0.0, 1.0), seed))
            }
            "sometimes" => {
                let chance = self.share_param(name, self.arg(name, rest, "chance", span)?, it)?;
                let default = match &chance {
                    Param::Const(c) => c.to_bits(),
                    Param::Pat(_) => 0x534F_4D45,
                };
                let seed = self.seed(name, rest, default)?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                match self.find(name, rest, "per") {
                    None => p.sometimes_by(&chance, seed, change),
                    Some(e) => match &e.kind {
                        ExprKind::Name(w) if w == "note" => p.sometimes_by(&chance, seed, change),
                        ExprKind::Name(w) if w == "cycle" => p.some_cycles(&chance, seed, change),
                        _ => {
                            return Err(Diagnostic::new(
                                "`per` is `note` or `cycle`, as in `sometimes(25%, reverse, per: cycle)`",
                                e.span,
                            ));
                        }
                    },
                }
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
            "overlay" => {
                let after = match self.find(name, rest, "after") {
                    Some(e) => self.cycles(e)?,
                    None => Rational::ZERO,
                };
                let f = self.arg(name, rest, "change", span)?;
                let change = self.transformer::<T>(f, &p)?;
                p.layer(after, change)
            }
            "only_between" => {
                let from = self.fraction(self.arg(name, rest, "from", span)?)?;
                let to = self.fraction(self.arg(name, rest, "to", span)?)?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.within(from, to, change)
            }
            "swing" => {
                let amount = self.share_param(name, self.arg(name, rest, "amount", span)?, it)?;
                let e = self.arg(name, rest, "every", span)?;
                let every = self.cycles(e)?;
                if every <= Rational::ZERO || every > Rational::ONE {
                    return Err(Diagnostic::new(
                        "`swing` needs `every` to be a share of a bar, such as `1/8beat`",
                        e.span,
                    ));
                }
                let slots = Rational::ONE / every;
                amount.apply(&p, move |p, x| p.swing(Rational::from_f64(x), slots))
            }
            "start_later" => {
                let times = self.num_param(self.arg(name, rest, "parts", span)?, it)?;
                times.apply(&p, |p, n| p.rotate(n.floor().min(MAX_REPEATS)))
            }
            "step_through" => {
                let n = self.whole(self.arg(name, rest, "parts", span)?, "step_through")?;
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
            "loop_start" => {
                let e = self.arg(name, rest, "length", span)?;
                let fraction = self.fraction(e)?;
                if fraction <= Rational::ZERO || fraction > Rational::ONE {
                    return Err(Diagnostic::new(
                        "`loop_start` takes a share of the cycle, from above 0% to 100%",
                        e.span,
                    ));
                }
                p.repeat_start(fraction)
            }
            "take" => {
                let length = self.share_param(name, self.arg(name, rest, "length", span)?, it)?;
                length.apply(&p, |p, x| {
                    if x <= 0.0 {
                        Pattern::silence()
                    } else {
                        p.trunc(Rational::from_f64(x.min(1.0)))
                    }
                })
            }
            "crop" | "squeeze" => {
                let (from_e, to_e) = (
                    self.arg(name, rest, "from", span)?,
                    self.arg(name, rest, "to", span)?,
                );
                let (from, to) = (self.fraction(from_e)?, self.fraction(to_e)?);
                if from < Rational::ZERO || to > Rational::ONE || from >= to {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` takes two points of the cycle, the first before the second, such as `{}(25%, 75%)`",
                            name.name, name.name
                        ),
                        to_e.span,
                    ));
                }
                if name.name == "crop" {
                    p.zoom(from, to)
                } else {
                    p.compress(from, to)
                }
            }
            "strum" => {
                let amount = match self.find(name, rest, "amount") {
                    Some(e) => self.share_param(name, e, it)?,
                    None => Param::Const(0.25),
                };
                amount.apply(&p, |p, x| p.rolled(Rational::from_f64(x.clamp(0.0, 1.0))))
            }
            "cycle_through" => {
                // `riff.cycle_through(same, reverse, fast(2))`: one change per cycle, in turn
                if rest.is_empty() {
                    return Err(Diagnostic::new(
                        "`cycle_through` needs changes to cycle through, as in `cycle_through(same, reverse)`",
                        name.span,
                    ));
                }
                let mut variants = Vec::with_capacity(rest.len());
                for arg in rest {
                    variants.push(self.transformer::<T>(&arg.value, &p)?(p.clone()));
                }
                Pattern::cycle_through(variants)
            }
            "in_cycles" => {
                let (a_e, b_e) = (
                    self.arg(name, rest, "cycles", span)?,
                    self.arg(name, rest, "from", span)?,
                );
                let (a, b) = (self.plain(a_e)?, self.plain(b_e)?);
                if !a.is_integer() || a < Rational::ONE || !b.is_integer() || b < Rational::ZERO {
                    return Err(Diagnostic::new(
                        "`in_cycles` takes a whole number of cycles and where to start in them, as in `in_cycles(4, 3, fast(2))`",
                        name.span,
                    ));
                }
                let (a, b) = (a.numer(), b.numer());
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.when_cycle(move |c| c.rem_euclid(a) >= b, change)
            }
            "often" | "rarely" => {
                let chance: f64 = if name.name == "often" { 0.75 } else { 0.25 };
                let seed = self.seed(name, rest, chance.to_bits())?;
                let change = self.transformer::<T>(self.arg(name, rest, "change", span)?, &p)?;
                p.sometimes(chance, seed, change)
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

    /// An argument with a unit, fixed (`5st`) or a pattern of them (`[<0st 5st>]`). `read` takes the
    /// number out of a value, `None` if it is the wrong kind; `bad` says what the argument should be.
    fn value_param(
        &self,
        e: &Expr,
        it: Option<&PerfValue>,
        read: impl Fn(Value) -> Option<f64> + Send + Sync + 'static,
        bad: &str,
        help: &str,
    ) -> Res<Param<f64>> {
        let wrong = || Diagnostic::new(bad.to_string(), e.span).with_help(help.to_string());
        match self.eval(e, it)? {
            PerfValue::Num(v) => read(v).map(Param::Const).ok_or_else(wrong),
            PerfValue::Pattern(pattern) => {
                // say now, not when the note plays, if a value is of the wrong kind
                let cycle = niminal_pattern::Span::new(Rational::ZERO, Rational::ONE);
                if pattern
                    .query(cycle)
                    .iter()
                    .any(|h| read(h.value.value).is_none())
                {
                    return Err(wrong());
                }
                Ok(Param::Pat(pattern.filter_map(move |s| read(s.value))))
            }
            _ => Err(wrong()),
        }
    }

    /// A share of a cycle that can change from cycle to cycle: `20%`, `1/4`, `2beats`, or a pattern of them.
    fn share_param(&self, name: &Ident, e: &Expr, it: Option<&PerfValue>) -> Res<Param<f64>> {
        let per_bar = self.cx.tempo.beats_per_bar;
        let share = self.value_param(
            e,
            it,
            move |v| match v {
                Value::Num(x) | Value::Bars(x) => Some(x),
                Value::Beats(b) => Some(b / per_bar),
                _ => None,
            },
            &format!("`{}` takes a share of the cycle, such as `25%`", name.name),
            "write a percentage, a fraction or a length in beats or bars",
        )?;
        if let Param::Const(x) = &share
            && !(0.0..=1.0).contains(x)
        {
            return Err(Diagnostic::new(
                format!("`{}` takes an amount from 0% to 100%", name.name),
                e.span,
            ));
        }
        Ok(share)
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
            PerfValue::Num(Value::Num(n)) => Ok(Param::Const(Rational::from_f64(n))),
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
        match self.eval(e, it)? {
            PerfValue::Num(v) => Ok(v),
            PerfValue::Pattern(p) => {
                let at = niminal_pattern::Span::new(Rational::ZERO, Rational::ZERO);
                p.query(at)
                    .into_iter()
                    .next()
                    .map(|h| h.value.value)
                    .ok_or_else(|| Diagnostic::new("expected a single value", e.span))
            }
            _ => Err(Diagnostic::new("expected a single value", e.span)),
        }
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

    /// An argument such as `reverse` or `transpose(+7st)`, as a function from pattern to pattern.
    /// It is tried once on `sample` so that mistakes are reported now.
    fn transformer<'a, T: Kind>(&'a self, e: &'a Expr, sample: &Pattern<T>) -> Res<Change<'a, T>> {
        let written = self.implicit_it(e);
        let apply = move |p: Pattern<T>| -> Res<Pattern<T>> {
            let e = written.as_ref().unwrap_or(e);
            let out = self.eval(e, Some(&T::wrap(p)))?;
            T::unwrap(out).ok_or_else(|| {
                Diagnostic::new("this change must give the same kind of pattern", e.span)
            })
        };
        apply(sample.clone())?;
        Ok(Box::new(move |p: Pattern<T>| apply(p.clone()).unwrap_or(p)))
    }

    /// A change is written as operations on the pattern with the pattern itself left out:
    /// `reverse`, `fast(2).reverse`. This puts `same` in front.
    /// `None` if the change already starts from something else, such as `same` itself.
    fn implicit_it(&self, e: &Expr) -> Option<Expr> {
        let it = |span| Expr {
            kind: ExprKind::Name("same".to_string()),
            span,
        };
        let is_op = |n: &str| {
            !self.bindings.contains_key(n) && crate::ide::pattern_ops().iter().any(|o| o.name == n)
        };
        match &e.kind {
            ExprKind::Name(n) if is_op(n) => Some(Expr {
                kind: ExprKind::Call {
                    name: Ident {
                        name: n.clone(),
                        span: e.span,
                    },
                    args: vec![Arg {
                        name: None,
                        value: it(e.span),
                        receiver: true,
                    }],
                },
                span: e.span,
            }),
            ExprKind::Call { name, args } => match args.first() {
                Some(first) if first.receiver => {
                    let value = self.implicit_it(&first.value)?;
                    let mut args = args.clone();
                    args[0].value = value;
                    Some(Expr {
                        kind: ExprKind::Call {
                            name: name.clone(),
                            args,
                        },
                        span: e.span,
                    })
                }
                _ if is_op(&name.name) => {
                    let mut with_it = vec![Arg {
                        name: None,
                        value: it(name.span),
                        receiver: true,
                    }];
                    with_it.extend(args.iter().cloned());
                    Some(Expr {
                        kind: ExprKind::Call {
                            name: name.clone(),
                            args: with_it,
                        },
                        span: e.span,
                    })
                }
                _ => None,
            },
            _ => None,
        }
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
            "drop" => &["amount", "seed"],
            "sometimes" => &["chance", "change", "per", "seed"],
            "every" => &["n", "change"],
            "overlay" => &["change", "after"],
            "only_between" => &["from", "to", "change"],
            "swing" => &["amount", "every"],
            "start_later" | "shuffle" | "pick_random" => &["parts", "seed"],
            "step_through" => &["parts", "change"],
            "loop_start" => &["length"],
            "to_steps" => &["steps"],
            "range" => &["from", "to"],
            "where" => &["when", "change"],
            "gain" => &["level"],
            "pan" => &["angle"],
            "begin" => &["share"],
            "stereo_copy" => &["change", "angle"],
            "chop" | "interleave_chops" => &["pieces"],
            "in_scale" => &["scale", "root"],
            "chord" => &["kind"],
            "arpeggiate" => &["order"],
            "invert_melody" => &["around"],
            "markov" => &["seed"],
            "random_walk" => &["step", "seed"],
            "spread_evenly" => &["hits", "steps", "rotation"],
            "with_rhythm_of" => &["pattern"],
            "keep_where" => &["pattern"],
            "add" | "sub" | "mul" | "div" => &["other", "timing"],
            "take" => &["length"],
            "mod" => &["by"],
            "times" => &["count"],
            "crop" | "squeeze" => &["from", "to"],
            "strum" => &["amount"],
            "in_cycles" => &["cycles", "from", "change"],
            "often" | "rarely" => &["change", "seed"],
            "echo" => &["times", "time", "fade"],
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

    fn binary(
        &self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        let (l, r) = (self.eval(lhs, it)?, self.eval(rhs, it)?);
        if matches!(l, PerfValue::Pattern(_)) || matches!(r, PerfValue::Pattern(_)) {
            return self.arithmetic(op, Structure::Left, l, r, span);
        }
        match (l.as_num(), r.as_num(), op) {
            (Some(a), Some(b), BinOp::Add) => Ok(PerfValue::Num(add_values(a, b, span)?)),
            (Some(a), Some(b), BinOp::Sub) => Ok(PerfValue::Num(sub_values(a, b, span)?)),
            (Some(a), Some(b), BinOp::Mul) => Ok(PerfValue::Num(mul_values(a, b, span)?)),
            (Some(a), Some(b), BinOp::Div) => Ok(PerfValue::Num(div_values(a, b, span)?)),
            _ => Err(Diagnostic::new(
                format!("can't {} these values in a clip or scene", op_name(op)),
                span,
            )),
        }
    }

    /// `riff + 7st`, `riff * [1 2]`, `a.add(b, structure: both)`: do the sum for every
    /// note. A pattern on both sides is joined by `structure`; a plain number on one
    /// side changes each note of the other.
    fn arithmetic(
        &self,
        op: BinOp,
        structure: Structure,
        l: PerfValue,
        r: PerfValue,
        span: Span,
    ) -> Res<PerfValue> {
        let sum = move |a: Value, b: Value| step_arithmetic(op, a, b);
        let out = match (l, r) {
            (PerfValue::Pattern(a), PerfValue::Pattern(b)) => {
                a.combine(&b, structure, move |x, y| match structure {
                    Structure::Right => y.with(sum(x.value, y.value).unwrap_or(y.value)),
                    _ => x.with(sum(x.value, y.value).unwrap_or(x.value)),
                })
            }
            (PerfValue::Pattern(a), other) if other.as_num().is_some() => {
                let n = other.as_num().expect("checked");
                a.map(move |x| x.with(sum(x.value, n).unwrap_or(x.value)))
            }
            (other, PerfValue::Pattern(b)) if other.as_num().is_some() => {
                let n = other.as_num().expect("checked");
                b.map(move |y| y.with(sum(n, y.value).unwrap_or(y.value)))
            }
            (l, r) => {
                return Err(Diagnostic::new(
                    format!(
                        "can't {} {} and {}",
                        op_name(op),
                        l.describe(),
                        r.describe()
                    ),
                    span,
                )
                .with_help("one side must be a pattern, the other a pattern or a number"));
            }
        };
        Ok(PerfValue::Pattern(out))
    }

    fn block_value(&self, body: &[Stmt], it: Option<&PerfValue>) -> Res<PerfValue> {
        let mut local = Perform {
            cx: self.cx,
            bindings: self.bindings.clone(),
            arrangements: HashMap::new(),
        };
        let mut last = PerfValue::Rest;
        for stmt in body {
            match stmt {
                Stmt::Bind { name, value } => {
                    let v = local.eval(value, it)?;
                    local.bindings.insert(name.name.clone(), v.clone());
                    last = v;
                }
                Stmt::Expr(e) => last = local.eval(e, it)?,
                Stmt::State { name, .. }
                | Stmt::AddAssign { name, .. }
                | Stmt::IndexAssign { name, .. } => {
                    return Err(Diagnostic::new(
                        "only bindings and expressions belong in a score `def`, `for` or `if`",
                        name.span,
                    ));
                }
                Stmt::Destructure { names, .. } => {
                    return Err(Diagnostic::new(
                        "only bindings and expressions belong in a score `def`, `for` or `if`",
                        names.first().map(|n| n.span).unwrap_or_default(),
                    ));
                }
            }
        }
        Ok(last)
    }

    fn iterate(&self, iter: &Expr, it: Option<&PerfValue>) -> Res<Vec<PerfValue>> {
        match self.eval(iter, it)? {
            PerfValue::List(items) => Ok(items),
            other => Err(Diagnostic::new(
                format!("`for` needs a list or range, found {}", other.describe()),
                iter.span,
            )),
        }
    }

    fn call_def(
        &self,
        def: DefFn,
        args: &[Arg],
        span: Span,
        it: Option<&PerfValue>,
    ) -> Res<PerfValue> {
        if self.bindings.values().any(|v| {
            matches!(v, PerfValue::Def(d) if d.name == def.name) && false // recursion recorded below
        }) {
            // fall through
        }
        let mut child = Perform {
            cx: self.cx,
            bindings: HashMap::new(),
            arrangements: HashMap::new(),
        };
        // copy defs and outer bindings except the active call stack marker
        for (k, v) in &self.bindings {
            if !matches!(v, PerfValue::Def(_)) {
                continue;
            }
            child.bindings.insert(k.clone(), v.clone());
        }
        // mark this call to catch recursion
        let marker = format!("__calling:{}", def.name);
        if self.bindings.contains_key(&marker) {
            return Err(Diagnostic::new(
                format!("score function `{}` calls itself", def.name),
                span,
            ));
        }
        child
            .bindings
            .insert(marker.clone(), PerfValue::Num(Value::Num(1.0)));

        let mut positional = args.iter().filter(|a| a.name.is_none() && !a.receiver);
        for param in &def.params {
            let value = if let Some(arg) = args
                .iter()
                .find(|a| a.name.as_ref().is_some_and(|n| n.name == param.name.name))
            {
                self.eval(&arg.value, it)?
            } else if let Some(arg) = positional.next() {
                self.eval(&arg.value, it)?
            } else if let Some(default) = &param.default {
                self.eval(default, it)?
            } else {
                return Err(Diagnostic::new(
                    format!("`{}` needs an argument `{}`", def.name, param.name.name),
                    span,
                ));
            };
            child.bindings.insert(param.name.name.clone(), value);
        }
        child.block_value(&def.body, None)
    }

    /// `load("score/riff.nms")`: a section or single-clip project as a clip.
    fn load(&self, args: &[Arg], span: Span) -> Res<PerfValue> {
        let path = match args {
            [
                Arg {
                    name: None,
                    value:
                        Expr {
                            kind: ExprKind::Str(path),
                            ..
                        },
                    ..
                },
            ] => path.as_str(),
            _ => {
                return Err(Diagnostic::new(
                    "`load` needs a file path, as in `load(\"score/riff.nms\")`",
                    span,
                ));
            }
        };
        let resolved = self.cx.samples.resolve_path(path);
        let text = std::fs::read_to_string(&resolved)
            .map_err(|e| Diagnostic::new(format!("couldn't read `{path}`: {e}"), span))?;
        let file = crate::ScoreFile::from_json(&text)
            .map_err(|e| Diagnostic::new(format!("couldn't load `{path}`: {e}"), span))?;
        let clip = match file {
            crate::ScoreFile::Section(section) => section_to_clip(section, self.cx.tempo)?,
            crate::ScoreFile::Project(project) => {
                if project.clips.len() == 1 && project.scenes.is_empty() {
                    clipdef_to_clip(&project.clips[0], self.cx)?
                } else {
                    return Err(Diagnostic::new(
                        format!(
                            "`{path}` is a project with {} clips and {} scenes; `load` takes a version-1 section or a project with one clip",
                            project.clips.len(),
                            project.scenes.len()
                        ),
                        span,
                    ));
                }
            }
        };
        Ok(PerfValue::Clip(clip))
    }

    // ---- clips and scenes ------------------------------------------------------

    fn clip(&self, block: &NamedBlock) -> Res<Clip> {
        if let Some(base) = &block.base {
            return match self.eval(base, None)? {
                PerfValue::Clip(mut clip) => {
                    clip.name = block.name.name.clone();
                    Ok(clip)
                }
                other => Err(Diagnostic::new(
                    format!(
                        "a clip is made from another clip, but this is {}",
                        other.describe()
                    ),
                    base.span,
                )
                .with_help("for example `clip bass_chorus = bass.transpose(+5st)`")),
            };
        }
        let mut length = None;
        let mut keys = None;
        let mut lanes: Vec<(String, Pattern<Step>)> = Vec::new();
        let mut curves: Vec<(String, EnvLane)> = Vec::new();
        let mut scored = Vec::new();
        let mut plays = None;
        let mut then = Vec::new();
        // `fx_name { open: x.x. }` is the lanes `fx_name.open`, written together
        let mut entries: Vec<(Ident, &Expr, bool)> = Vec::new();
        for (key, value) in &block.entries {
            if let ExprKind::FxLanes(rows) = &value.kind {
                for (row, row_value) in rows {
                    let name = format!("{}::{}", key.name, row.name);
                    entries.push((Ident { name, span: row.span }, row_value, true));
                }
            } else {
                entries.push((key.clone(), value, false));
            }
        }
        for (key, value, in_block) in entries {
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
            if key.name == "times" {
                if plays.is_some() {
                    return Err(Diagnostic::new("`times` is given twice", key.span));
                }
                plays = Some(self.whole(value, "times")? as u32);
                continue;
            }
            if key.name == "then" {
                then.push(self.then_command(value)?);
                continue;
            }
            if key.name == "events" {
                if !scored.is_empty() {
                    return Err(Diagnostic::new("lane `events` is given twice", key.span));
                }
                let ExprKind::Events(raw) = &value.kind else {
                    return Err(
                        Diagnostic::new("an `events` lane is a brace block", value.span).with_help(
                            "for example `events { 0beat c4 for 1/2beat  1/2beat e4 for 1/2beat }`",
                        ),
                    );
                };
                scored = parse_events(raw, value.span, self.cx)?;
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
            // in an effect's block a bracketed row of steps, `[x . <x X .>]`, is read as steps
            if in_block
                && let ExprKind::Pattern(raw) = &value.kind
                && let Ok(pattern) = mini_steps(raw)
            {
                lanes.push((key.name.clone(), pattern));
                continue;
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
                            "an envelope moves a node's parameter: `cut: env[300hz 1bar 3khz]`",
                        ));
                    }
                    curves.push((key.name.clone(), curve));
                    continue;
                }
                PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                PerfValue::Rest => Pattern::silence(),
                PerfValue::List(items) => self.list_pattern(items, value.span)?,
                other => {
                    return Err(Diagnostic::new(
                        format!("a lane holds a pattern, but this is {}", other.describe()),
                        value.span,
                    ));
                }
            };
            lanes.push((key.name.clone(), pattern));
        }
        if !scored.is_empty() && lanes.iter().any(|(n, _)| n == "notes") {
            return Err(Diagnostic::new(
                "a clip can't have both `notes` and `events`",
                block.span,
            ));
        }
        // a clip with rows for an effect and no notes is a gesture: a silent note on every sixteenth gives
        // the rows a step to land on
        let rows_only = crate::dsp::drives_effects(block) && !block.entries.iter().any(|(k, _)| k.name == "instrument");
        if rows_only && keys.is_none() && scored.is_empty() && !lanes.iter().any(|(n, _)| n == "notes") {
            let steps = 16 * length.map_or(1, |l| l.to_f64().ceil().max(1.0) as usize);
            let step = Pattern::pure(Step::from(niminal_score::Value::Note(60)));
            lanes.push(("notes".into(), Pattern::fastcat(vec![step; steps])));
        }
        // a lane spans the whole clip, whatever its length
        if let Some(length) = length {
            lanes = lanes
                .into_iter()
                .map(|(name, p)| (name, p.slow(length)))
                .collect();
        }
        let endless = length.is_none() && plays.is_none();
        Ok(Clip {
            name: block.name.name.clone(),
            length: length.unwrap_or(Rational::ONE),
            endless,
            plays,
            then,
            lanes,
            scored,
            keys,
            curves,
            signals: Vec::new(),
            fx_params: Vec::new(),
        })
    }

    /// `then: chorus.start`: what to do when a clip has played its last time, as the statement to run.
    fn then_command(&self, e: &Expr) -> Res<String> {
        const VERBS: [&str; 6] = ["start", "stop", "mute", "unmute", "solo", "unsolo"];
        match &e.kind {
            ExprKind::Name(n) if n == "hush" => return Ok("hush".to_string()),
            ExprKind::Call { name, args } if VERBS.contains(&name.name.as_str()) => {
                if let [receiver] = args.as_slice()
                    && receiver.receiver
                    && let ExprKind::Name(target) = &receiver.value.kind
                {
                    return Ok(format!("{target}.{}", name.name));
                }
            }
            _ => {}
        }
        Err(
            Diagnostic::new("`then` takes a command on a clip or scene", e.span)
                .with_help("for example `then: chorus.start` or `then: hush`"),
        )
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
        let mut clips: Vec<usize> = Vec::new();
        for (key, value) in &block.entries {
            if !matches!(&value.kind, ExprKind::Name(n) if *n == key.name) {
                return Err(Diagnostic::new(
                    format!("a scene is a list of clips, so `{}` has nothing after it", key.name),
                    value.span,
                )
                .with_help(
                    "write `scene verse { bass, lead }`; for a different bass in this scene, declare another clip",
                ));
            }
            let node = self.node(key)?;
            if clips.contains(&node) {
                return Err(Diagnostic::new(
                    format!("clip `{}` appears twice in this scene", key.name),
                    key.span,
                ));
            }
            clips.push(node);
        }
        Ok(Scene {
            name: block.name.name.clone(),
            clips,
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
                plays: None,
                then: Vec::new(),
                lanes: vec![("notes".to_string(), p)],
                scored: Vec::new(),
                keys: None,
                curves: Vec::new(),
                signals: Vec::new(),
                fx_params: Vec::new(),
            }),
            PerfValue::Num(v) => Ok(Clip {
                name: hint.to_string(),
                length: Rational::ONE,
                endless: true,
                plays: None,
                then: Vec::new(),
                lanes: vec![("notes".to_string(), Pattern::pure(Step::from(v)))],
                scored: Vec::new(),
                keys: None,
                curves: Vec::new(),
                signals: Vec::new(),
                fx_params: Vec::new(),
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
                "`~` stops a clip in a scene; to stop one, write `node.stop`",
                span,
            )),
            PerfValue::List(_) => Err(Diagnostic::new(
                "a list isn't a clip; play one entry, or build a clip from it",
                span,
            )),
            PerfValue::Def(d) => Err(Diagnostic::new(
                format!(
                    "`{}` is a score function; call it, as in `{}(...)`",
                    d.name, d.name
                ),
                span,
            )),
        }
    }

    // ---- commands ----------------------------------------------------------------

    /// A clip by name; the implicit node 0 can't be named.
    fn node(&self, id: &Ident) -> Res<usize> {
        match self.cx.nodes.iter().skip(1).position(|t| t.name == id.name) {
            Some(i) => Ok(i + 1),
            None => {
                let mut d = Diagnostic::new(format!("no clip named `{}`", id.name), id.span);
                if let Some(c) = closest(
                    &id.name,
                    self.cx.nodes.iter().skip(1).map(|t| t.name.as_str()),
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
        // an arrangement is started to run its lines from now, and stopped to drop the ones still to come
        match &stmt.command {
            Command::Start { name, .. } if self.arrangements.contains_key(&name.name) => {
                let arrangement = &self.arrangements[&name.name];
                if !arrangement.tempos.is_empty() || !arrangement.lanes.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "`{0}` sets its own tempo or moves a parameter over the whole piece, which only a render of it can follow",
                            name.name
                        ),
                        name.span,
                    )
                    .with_help(format!("render it with `niminal-live render file.nml {}`", name.name)));
                }
                let tempo = self.cx.tempo;
                let base = at.to_seconds(tempo);
                return Ok(self.arrangements[&name.name]
                    .actions
                    .iter()
                    .filter(|a| !matches!(a.action, Action::Hush) || a.at.to_seconds(tempo) > 0.0)
                    .map(|a| Scheduled {
                        at: Time::Seconds(base + a.at.to_seconds(tempo)),
                        action: a.action.clone(),
                        quantize,
                    })
                    .collect());
            }
            Command::Stop(name) if self.arrangements.contains_key(&name.name) => {
                return Ok(vec![schedule(Action::Cancel {
                    arrangement: name.name.clone(),
                })]);
            }
            _ => {}
        }

        Ok(match &stmt.command {
            Command::Update { node, what } => {
                if self.cx.failed.contains(&node.name.as_str()) {
                    return Ok(Vec::new());
                }
                let t = self.node(node)?;
                let clip = self.to_clip(self.eval(what, None)?, &node.name, what.span)?;
                let clip = self.on_node(clip, t, node, what.span)?;
                vec![schedule(Action::Update { node: t, clip })]
            }
            Command::Start { name, .. } => {
                if self.cx.failed.contains(&name.name.as_str()) {
                    return Ok(Vec::new());
                }
                let mut actions = Vec::new();
                for node in self.targets(name, "start")? {
                    actions.push(schedule(self.start(node, name)?));
                }
                actions
            }
            Command::Stop(t) => self
                .targets(t, "stop")?
                .into_iter()
                .map(|node| schedule(Action::Stop { node }))
                .collect(),
            Command::Mute(t) => self
                .targets(t, "mute")?
                .into_iter()
                .map(|node| schedule(Action::Mute { node }))
                .collect(),
            Command::Unmute(t) => self
                .targets(t, "unmute")?
                .into_iter()
                .map(|node| schedule(Action::Unmute { node }))
                .collect(),
            Command::Solo(t) => self
                .targets(t, "solo")?
                .into_iter()
                .map(|node| schedule(Action::Solo { node }))
                .collect(),
            Command::Unsolo(Some(t)) => self
                .targets(t, "unsolo")?
                .into_iter()
                .map(|node| schedule(Action::Unsolo { node: Some(node) }))
                .collect(),
            Command::Unsolo(None) => vec![schedule(Action::Unsolo { node: None })],
            Command::Hush => vec![schedule(Action::Hush)],
            Command::Panic => vec![schedule(Action::Panic)],
            Command::Reset | Command::New => vec![schedule(Action::Reset)],
            Command::Capture { length, name } => {
                let bars = self.cycles(length)?.to_f64();
                if bars <= 0.0 {
                    return Err(Diagnostic::new(
                        "capture needs a length above zero",
                        length.span,
                    ));
                }
                vec![schedule(Action::Capture {
                    bars,
                    name: name.name.clone(),
                })]
            }
            // changing what a part plays is for a live session; in a file it does nothing
            Command::Edit { .. } => Vec::new(),
        })
    }

    /// The clips a command on `name` is about: the clips of a scene, or the one clip.
    fn targets(&self, name: &Ident, verb: &str) -> Res<Vec<usize>> {
        if let Some(PerfValue::Scene(scene)) = self.bindings.get(&name.name) {
            return Ok(scene.clips.clone());
        }
        if let Ok(node) = self.node(name) {
            return Ok(vec![node]);
        }
        if self.arrangements.contains_key(&name.name) {
            return Err(Diagnostic::new(
                format!(
                    "`{}` is an arrangement, which can only be started and stopped, not `{verb}`",
                    name.name
                ),
                name.span,
            ));
        }
        let mut d = match self.bindings.get(&name.name) {
            Some(PerfValue::Clip(_)) => Diagnostic::new(
                format!(
                    "clip `{0}` has no instrument, so `{0}.{verb}` has nothing to {verb}",
                    name.name
                ),
                name.span,
            )
            .with_help(format!(
                "give it one: `clip {} {{ instrument: ... }}`",
                name.name
            )),
            Some(PerfValue::Pattern(_)) => Diagnostic::new(
                format!(
                    "`{0}` is a pattern, not a clip, so `{0}.{verb}` has nothing to {verb}",
                    name.name
                ),
                name.span,
            )
            .with_help("a clip has an instrument and a `notes:` lane"),
            _ if self.cx.instruments.iter().any(|i| i.name == name.name) => Diagnostic::new(
                format!("`{}` is an instrument, not a clip", name.name),
                name.span,
            )
            .with_help(format!(
                "a clip plays it: `clip riff {{ instrument: {}, notes: [c4 e4] }}`",
                name.name
            )),
            _ => Diagnostic::new(format!("no scene or clip named `{}`", name.name), name.span),
        };
        if d.help.is_none() {
            let scenes = self
                .bindings
                .iter()
                .filter(|(_, v)| matches!(v, PerfValue::Scene(_)))
                .map(|(k, _)| k.as_str());
            let clips = self.cx.nodes.iter().skip(1).map(|t| t.name.as_str());
            if let Some(c) = closest(&name.name, scenes.chain(clips)) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
        }
        Ok(d).and_then(Err)
    }

    /// The action that starts the clip on `node`.
    fn start(&self, node: usize, id: &Ident) -> Res<Action> {
        let name = self.cx.nodes[node].name.clone();
        let at = Ident {
            name: name.clone(),
            span: id.span,
        };
        let value = match self.bindings.get(&name) {
            Some(v) => v.clone(),
            None => return Err(Diagnostic::new(format!("no clip named `{name}`"), id.span)),
        };
        let clip = self.to_clip(value, &name, id.span)?;
        let clip = self.on_node(clip, node, &at, id.span)?;
        Ok(Action::Start { node, clip })
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
        let mut previous: Vec<usize> = Vec::new();
        let mut tempos: Vec<(f64, Tempo)> = Vec::new();
        for (i, section) in decl.sections.iter().enumerate() {
            let mut tempo = self.cx.tempo;
            if let Some(e) = decl.tempos.as_ref().map(|l| &l[i]) {
                tempo.bpm = crate::compile::bpm_of(e)?;
            }
            if let Some(e) = decl.meters.as_ref().map(|l| &l[i]) {
                (tempo.beats_per_bar, tempo.beat_unit) = crate::compile::meter_beats(e)?;
            }
            let length = self.cycles_in(&section.length, tempo.beats_per_bar)?;
            if length <= Rational::ZERO {
                return Err(Diagnostic::new(
                    "a section needs a length above zero",
                    section.length.span,
                ));
            }
            let at = Time::Seconds(seconds);
            let now = self.targets(&section.scene, "start")?;
            // a section stops what the one before it had going and this one does not, and starts its own
            for node in previous.iter().filter(|n| !now.contains(n)) {
                actions.push(Scheduled {
                    at,
                    action: Action::Stop { node: *node },
                    quantize: None,
                });
            }
            for node in &now {
                actions.push(Scheduled {
                    at,
                    action: self.start(*node, &section.scene)?,
                    quantize: None,
                });
            }
            previous = now;
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
                            ..tempo
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
        if !decl.sections.is_empty() {
            actions.push(Scheduled {
                at: Time::Seconds(seconds),
                action: Action::Hush,
                quantize: None,
            });
        }
        // lines placed from the start of the arrangement, such as `at +4bars bass.mute`
        for stmt in &decl.commands {
            actions.extend(self.command(stmt)?);
        }
        let mut lanes = Vec::new();
        for lane in &decl.lanes {
            let (node, param) = (&lane.node, &lane.param);
            if !self.cx.nodes.iter().skip(1).any(|t| t.name == node.name) {
                let mut d = Diagnostic::new(format!("no clip named `{}`", node.name), node.span);
                if let Some(c) = closest(
                    &node.name,
                    self.cx.nodes.iter().skip(1).map(|t| t.name.as_str()),
                ) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
            }
            let mut curve = env_lane(&lane.value)?;
            let who = format!(
                "arrangement `{}` moves `{}.{}`, but node `{}`",
                decl.name.name, node.name, param.name, node.name
            );
            self.bind_env(&mut curve, &node.name, &param.name, &who, param.span)?;
            lanes.push((format!("{}::{}", node.name, param.name), curve));
        }
        Ok(Arrangement {
            name: decl.name.name.clone(),
            actions,
            tempos,
            lanes,
        })
    }

    /// Check the clip against its playback node and instrument.
    fn on_node(&self, mut clip: Clip, node: usize, id: &Ident, span: Span) -> Res<Clip> {
        self.bind_curves(&mut clip, node, id, span)?;
        self.check_clip(&clip, node, id, span)?;
        Ok(clip)
    }

    /// Envelope lanes move a parameter of the node: find it, and check the levels fit its unit.
    fn bind_curves(&self, clip: &mut Clip, node: usize, id: &Ident, span: Span) -> Res<()> {
        let name = &self.cx.nodes[node].name;
        clip.fx_params = clip
            .lanes
            .iter()
            .filter_map(|(lane, _)| {
                lane.contains("::")
                    .then(|| self.cx.names.controls.iter().find(|c| c.name == *lane))
                    .flatten()
                    .map(|c| (lane.clone(), c.unit, c.smooth_seconds, f64::from(c.initial)))
            })
            .collect();
        for (lane, curve) in &mut clip.curves {
            let who = format!(
                "clip `{}` has an envelope lane `{lane}`, but node `{}`",
                clip.name, id.name
            );
            self.bind_env(curve, name, lane, &who, span)?;
        }
        // a lane that is a signal and named after a parameter of the part moves it continuously
        clip.signals.clear();
        let probe = niminal_pattern::Span::new(Rational::ZERO, Rational::ZERO);
        for (lane, pattern) in &clip.lanes {
            let is_signal = pattern
                .query(probe)
                .first()
                .is_some_and(|h| h.whole.is_none());
            let control = if lane.contains("::") {
                lane.clone()
            } else {
                format!("{name}::{lane}")
            };
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

    /// Point `curve` at the part parameter `node::param` and check its levels fit that parameter's unit.
    /// `who` starts the message if there is no such parameter.
    fn bind_env(
        &self,
        curve: &mut EnvLane,
        node: &str,
        param: &str,
        who: &str,
        span: Span,
    ) -> Res<()> {
        let control = if param.contains("::") {
            param.to_string()
        } else {
            format!("{node}::{param}")
        };
        let Some(def) = self.cx.names.controls.iter().find(|c| c.name == control) else {
            let params: Vec<&str> = self
                .cx
                .names
                .controls
                .iter()
                .filter_map(|c| c.name.strip_prefix(&format!("{node}::")))
                .collect();
            let mut d = Diagnostic::new(format!("{who} has no such parameter"), span).with_help(
                "name the fx parameter explicitly, as in `filter.cut: env[300hz 1bar 3khz]`",
            );
            if let Some(c) = closest(param, params) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let tempo = Tempo::new(120.0);
        let levels =
            std::iter::once(&curve.start).chain(curve.segments.iter().map(|(_, _, level)| level));
        for level in levels {
            niminal_lang::engine_value(def.unit, *level, tempo).map_err(|e| {
                Diagnostic::new(
                    format!("envelope lane `{param}` moves {}: {e}", def.name),
                    span,
                )
            })?;
        }
        curve.unit = Some(def.unit);
        Ok(())
    }

    /// Does `clip`'s notes and lanes make sense on `node`'s instrument?
    fn check_clip(&self, clip: &Clip, node: usize, id: &Ident, span: Span) -> Res<()> {
        let info = &self.cx.nodes[node];
        let Some(instrument) = info.instrument.map(|i| &self.cx.instruments[i]) else {
            return Err(Diagnostic::new(
                format!("node `{}` has no instrument to play a clip on", id.name),
                id.span,
            )
            .with_help("add `instrument = ...` to the node"));
        };
        if clip.keys.is_some() {
            if clip.lane("notes").is_some() || !clip.scored.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "clip `{}` has both `keys` and notes: it is played by one or the other",
                        clip.name
                    ),
                    span,
                ));
            }
            if !instrument
                .params
                .iter()
                .any(|p| p.name == instrument.pitch && p.unit == Unit::Hz)
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` has no `{}` parameter for the keys to set",
                        instrument.name, instrument.pitch
                    ),
                    span,
                )
                .with_help(format!(
                    "give the instrument a `{}: hz` parameter",
                    instrument.pitch
                )));
            }
        }
        for (lane, _) in &clip.lanes {
            if lane == "notes" {
                let pitched = instrument
                    .params
                    .iter()
                    .any(|p| p.name == instrument.pitch && p.unit == Unit::Hz);
                if !pitched && instrument.members.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` has no `{}` parameter for the notes to set",
                            instrument.name, instrument.pitch
                        ),
                        span,
                    )
                    .with_help(format!(
                        "give the instrument a `{}: hz` parameter",
                        instrument.pitch
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
            if self.cx.names.controls.iter().any(|c| {
                c.name
                    == if lane.contains("::") {
                        lane.clone()
                    } else {
                        format!("{}::{lane}", info.name)
                    }
            }) {
                if clip.lane("notes").is_none()
                    && clip.scored.is_empty()
                    && clip.keys.is_none()
                    && !clip.signals.iter().any(|(l, _)| l == lane)
                {
                    return Err(Diagnostic::new(
                        format!("lane `{lane}` sets a parameter of node `{}` as each note starts, but clip `{}` has no notes", info.name, clip.name),
                        span,
                    )
                    .with_help("add a `notes` lane to step on, or change it as the control instead"));
                }
                continue;
            }
            if let Some((fx, param)) = lane.split_once("::") {
                let params: Vec<&str> = self
                    .cx
                    .names
                    .controls
                    .iter()
                    .filter_map(|c| c.name.strip_prefix(&format!("{fx}::")))
                    .collect();
                let mut d = Diagnostic::new(
                    format!(
                        "clip `{}` has a lane `{fx}.{param}`, but there is no effect parameter of that name",
                        clip.name
                    ),
                    span,
                );
                if let Some(c) = closest(param, params) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
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

/// Where in the source a problem inside a pattern's text is.
/// What a step character is worth: `.` is 0 and `x` is 1, with `X` an accent (+3db) and `o` a ghost (-12db) as in a
/// drum grid.
fn step_level(c: char) -> f64 {
    match c {
        'x' => 1.0,
        'X' => 10f64.powf(3.0 / 20.0),
        'o' => 10f64.powf(-12.0 / 20.0),
        _ => 0.0,
    }
}

/// A row of steps as a lane. Drawn plain, each character is a step; the steps share one cycle, which the clip
/// stretches over its length like any lane. With mini-notation in it (`...<x X .>`, `x?`, `x(3,8)`) the usual
/// operators shape the steps, and a step that is off or dropped is an explicit 0, so the lane never keeps the last
/// level.
fn steps(raw: &str) -> Result<Pattern<Step>, String> {
    if raw.chars().all(|c| matches!(c, '.' | 'x' | 'X' | 'o' | ' ' | '\t')) {
        return Ok(Pattern::fastcat(
            raw.chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| Pattern::pure(Step::from(Value::Num(step_level(c)))))
                .collect(),
        ));
    }
    mini_steps(raw)
}

/// `raw` as mini-notation whose atoms are the step characters, `.` and `~` being rests.
fn mini_steps(raw: &str) -> Result<Pattern<Step>, String> {
    // each step character is a step of its own, as in a drawn row, so give it room
    let chars: Vec<char> = raw.chars().collect();
    let is_step = |c: char| matches!(c, '.' | 'x' | 'X' | 'o');
    let mut spaced = String::new();
    for (i, &c) in chars.iter().enumerate() {
        spaced.push(if c == '.' { '~' } else { c });
        let ends_a_step = is_step(c) || c.is_ascii_digit() || matches!(c, '>' | ']' | ')' | '?' | '!');
        if ends_a_step && chars.get(i + 1).is_some_and(|&next| is_step(next) || matches!(next, '<' | '[')) {
            spaced.push(' ');
        }
    }
    let on = parse_with(&spaced, |atom, _| match atom {
        "x" | "X" | "o" => Ok(step_level(atom.chars().next().unwrap_or('x'))),
        other => Err(format!("`{other}` isn't a step: use `.` or `~` for off, and `x`, `X` or `o` for on")),
    })
    .map_err(|e| e.message)?;
    // a rest or a dropped step makes no event, so the gaps are filled with an explicit off
    let on = on.map(|level| Step::from(Value::Num(level)));
    let off = on.invert(Step::from(Value::Num(0.0)));
    Ok(Pattern::stack(vec![on, off]))
}

fn at_offset(message: String, token: Span, offset: usize) -> Diagnostic {
    // the token's text starts after its opening bracket
    let at = token.start + 1 + offset;
    Diagnostic::new(message, Span::new(at, (at + 1).min(token.end)))
}

/// One row of `grid { ... }`: a sample or clip name, optional note controls, and a body.
struct GridRow<'a> {
    name: &'a str,
    name_offset: usize,
    gain: Option<f64>,
    pan: Option<f64>,
    begin: Option<f64>,
    pitch: Option<f64>,
    /// True when the body is mini-notation in brackets.
    mini: bool,
    body: &'a str,
    body_offset: usize,
}

/// `kick: x...`, `snare(pan: 10deg): x...`, or `hat: [x*2 x]`.
fn parse_grid_row(code: &str) -> Result<GridRow<'_>, (usize, String)> {
    let trim_start = code.len() - code.trim_start().len();
    let s = code.trim();
    let err = |offset: usize, message: String| (trim_start + offset, message);

    // name: letters, digits, underscores, and at most one dot for `kit.member`
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
    {
        i += 1;
    }
    if i == 0 {
        return Err(err(
            0,
            "a grid row starts with a sample or clip name".into(),
        ));
    }
    let name = &s[..i];
    let name_offset = trim_start;
    if name.starts_with('.') || name.ends_with('.') || name.matches('.').count() > 1 {
        return Err(err(0, format!("`{name}` isn't a sample or clip name")));
    }

    let mut gain = None;
    let mut pan = None;
    let mut begin = None;
    let mut pitch = None;
    if i < s.len() && s.as_bytes()[i] == b'(' {
        let open = i;
        i += 1;
        let mut depth = 1usize;
        let mut close = None;
        while i < s.len() {
            match s.as_bytes()[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let Some(close) = close else {
            return Err(err(open, "this `(` has no matching `)`".into()));
        };
        let params = &s[open + 1..close];
        parse_row_params(
            params,
            open + 1,
            &mut gain,
            &mut pan,
            &mut begin,
            &mut pitch,
        )
        .map_err(|(o, m)| err(o, m))?;
        i = close + 1;
    }

    while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= s.len() || s.as_bytes()[i] != b':' {
        return Err(err(
            0,
            "a grid row is `sample: x...` or `sample: [x*2 x]`".into(),
        ));
    }
    i += 1;
    while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    let body = s[i..].trim_end();
    let body_offset = trim_start + i;
    if body.is_empty() {
        return Err(err(i, "a grid row needs hits after the `:`".into()));
    }
    if body.starts_with('[') {
        if !body.ends_with(']') {
            return Err(err(i, "this `[` has no matching `]`".into()));
        }
        // Strip the outer brackets; parse_with expects the mini-notation text.
        let inner = &body[1..body.len() - 1];
        return Ok(GridRow {
            name,
            name_offset,
            gain,
            pan,
            begin,
            pitch,
            mini: true,
            body: inner,
            body_offset: body_offset + 1,
        });
    }
    Ok(GridRow {
        name,
        name_offset,
        gain,
        pan,
        begin,
        pitch,
        mini: false,
        body,
        body_offset,
    })
}

/// `gain: -6db, pan: 10deg, pitch: -2st, begin: 25%` inside a row's `(...)`.
fn parse_row_params(
    src: &str,
    base: usize,
    gain: &mut Option<f64>,
    pan: &mut Option<f64>,
    begin: &mut Option<f64>,
    pitch: &mut Option<f64>,
) -> Result<(), (usize, String)> {
    let err = |offset: usize, message: String| (base + offset, message);
    for part in src.split(',') {
        let part_start = part.as_ptr() as usize - src.as_ptr() as usize;
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once(':') else {
            return Err(err(
                part_start,
                "a row parameter is `name: value`, such as `pan: 10deg`".into(),
            ));
        };
        let key = key.trim();
        let value = value.trim();
        let value_at = part_start + (value.as_ptr() as usize - part.as_ptr() as usize);
        // `%` is a share, not a Value unit; everything else uses the usual parse.
        let parsed = if let Some(n) = value.strip_suffix('%') {
            let n: f64 = n
                .trim()
                .parse()
                .map_err(|_| err(value_at, format!("`{value}` is not a percentage")))?;
            Value::Num(n / 100.0)
        } else {
            value
                .parse()
                .map_err(|e: niminal_score::ParseError| err(value_at, e.0))?
        };
        match key {
            "gain" => {
                let db = match parsed {
                    Value::Db(db) => db,
                    Value::Num(n) if n > 0.0 => 20.0 * n.log10(),
                    _ => {
                        return Err(err(
                            value_at,
                            "gain is a level in db, such as `-6db`".into(),
                        ));
                    }
                };
                if gain.replace(db).is_some() {
                    return Err(err(part_start, "`gain` is given more than once".into()));
                }
            }
            "pan" => {
                let Value::Degrees(d) = parsed else {
                    return Err(err(
                        value_at,
                        "pan is a direction in degrees, such as `10deg`".into(),
                    ));
                };
                if pan.replace(d).is_some() {
                    return Err(err(part_start, "`pan` is given more than once".into()));
                }
            }
            "begin" => {
                let share = match parsed {
                    Value::Num(n) if (0.0..=1.0).contains(&n) => n,
                    _ => {
                        return Err(err(
                            value_at,
                            "begin is a share of the sample, such as `25%`".into(),
                        ));
                    }
                };
                if begin.replace(share).is_some() {
                    return Err(err(part_start, "`begin` is given more than once".into()));
                }
            }
            "pitch" => {
                let Value::Semitones(st) = parsed else {
                    return Err(err(
                        value_at,
                        "pitch is an interval in semitones, such as `-2st`".into(),
                    ));
                };
                if pitch.replace(st).is_some() {
                    return Err(err(part_start, "`pitch` is given more than once".into()));
                }
            }
            other => {
                return Err(err(
                    part_start,
                    format!("`{other}` isn't a row parameter (gain, pan, begin, or pitch)"),
                ));
            }
        }
    }
    Ok(())
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
        return Err(Diagnostic::new(
            "a tempo envelope runs once over the piece",
            lit.start.span(),
        )
        .with_help("take out the `|` or `.loop`"));
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

fn op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "subtract",
        BinOp::Mul => "multiply",
        BinOp::Div => "divide",
    }
}

fn cmp_values(op: CmpOp, a: Value, b: Value) -> bool {
    let key = |v: Value| match v {
        Value::Note(n) => n as f64,
        Value::Hz(h) => h,
        Value::Num(n) => n,
        Value::Db(d) => d,
        Value::Seconds(s) => s,
        Value::Beats(b) => b,
        Value::Bars(b) => b,
        Value::Semitones(s) => s,
        Value::Degrees(d) => d,
        Value::Sample(s) => s as f64,
    };
    let (a, b) = (key(a), key(b));
    match op {
        CmpOp::Lt => a < b,
        CmpOp::Gt => a > b,
        CmpOp::Le => a <= b,
        CmpOp::Ge => a >= b,
        CmpOp::Eq => (a - b).abs() < 1e-12,
        CmpOp::Ne => (a - b).abs() >= 1e-12,
    }
}

/// One note's value combined with another's. A plain number added to a note moves it
/// by that many semitones, so `[c4 e4] + [0 7]` works. `None` if the units don't mix,
/// which leaves the note as it was.
fn step_arithmetic(op: BinOp, a: Value, b: Value) -> Option<Value> {
    let (a, b) = match (op, a, b) {
        (BinOp::Add | BinOp::Sub, Value::Note(_), Value::Num(n)) => (a, Value::Semitones(n)),
        (BinOp::Add, Value::Num(n), Value::Note(_)) => (Value::Semitones(n), b),
        _ => (a, b),
    };
    let at = Span::default();
    match op {
        BinOp::Add => add_values(a, b, at),
        BinOp::Sub => sub_values(a, b, at),
        BinOp::Mul => mul_values(a, b, at),
        BinOp::Div => div_values(a, b, at),
    }
    .ok()
}

fn add_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) | (Value::Semitones(st), Value::Note(n)) => {
            Ok(transpose(Value::Note(n), st))
        }
        (Value::Hz(h), Value::Semitones(st)) | (Value::Semitones(st), Value::Hz(h)) => {
            Ok(transpose(Value::Hz(h), st))
        }
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a + b)),
        (Value::Db(a), Value::Db(b)) => Ok(Value::Db(a + b)),
        (Value::Beats(a), Value::Beats(b)) => Ok(Value::Beats(a + b)),
        (Value::Bars(a), Value::Bars(b)) => Ok(Value::Bars(a + b)),
        (Value::Seconds(a), Value::Seconds(b)) => Ok(Value::Seconds(a + b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a + b)),
        (Value::Hz(a), Value::Hz(b)) => Ok(Value::Hz(a + b)),
        _ => Err(Diagnostic::new("can't add these units", span)),
    }
}

fn sub_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) => Ok(transpose(Value::Note(n), -st)),
        (Value::Hz(h), Value::Semitones(st)) => Ok(transpose(Value::Hz(h), -st)),
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a - b)),
        (Value::Db(a), Value::Db(b)) => Ok(Value::Db(a - b)),
        (Value::Beats(a), Value::Beats(b)) => Ok(Value::Beats(a - b)),
        (Value::Bars(a), Value::Bars(b)) => Ok(Value::Bars(a - b)),
        (Value::Seconds(a), Value::Seconds(b)) => Ok(Value::Seconds(a - b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a - b)),
        (Value::Hz(a), Value::Hz(b)) => Ok(Value::Hz(a - b)),
        _ => Err(Diagnostic::new("can't subtract these units", span)),
    }
}

fn mul_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a * b)),
        (v, Value::Num(n)) | (Value::Num(n), v) => match v {
            Value::Beats(x) => Ok(Value::Beats(x * n)),
            Value::Bars(x) => Ok(Value::Bars(x * n)),
            Value::Seconds(x) => Ok(Value::Seconds(x * n)),
            Value::Hz(x) => Ok(Value::Hz(x * n)),
            Value::Db(x) => Ok(Value::Db(x * n)),
            Value::Semitones(x) => Ok(Value::Semitones(x * n)),
            Value::Num(x) => Ok(Value::Num(x * n)),
            _ => Err(Diagnostic::new("can't multiply these units", span)),
        },
        _ => Err(Diagnostic::new("can't multiply these units", span)),
    }
}

fn div_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) if b != 0.0 => Ok(Value::Num(a / b)),
        (v, Value::Num(n)) if n != 0.0 => match v {
            Value::Beats(x) => Ok(Value::Beats(x / n)),
            Value::Bars(x) => Ok(Value::Bars(x / n)),
            Value::Seconds(x) => Ok(Value::Seconds(x / n)),
            Value::Hz(x) => Ok(Value::Hz(x / n)),
            Value::Num(x) => Ok(Value::Num(x / n)),
            _ => Err(Diagnostic::new("can't divide these units", span)),
        },
        _ => Err(Diagnostic::new("can't divide these values", span)),
    }
}

/// Move a pitch by `st` semitones. Notes stay notes when the interval is whole.
pub(crate) fn transpose_public(v: Value, st: f64) -> Value {
    transpose(v, st)
}

fn transpose(v: Value, st: f64) -> Value {
    match v {
        Value::Note(m) if st.fract() == 0.0 => Value::Note(m.saturating_add(st as i32)),
        Value::Note(_) | Value::Hz(_) => {
            let hz = v.as_hz().expect("notes and frequencies have a frequency");
            Value::Hz(hz * 2f64.powf(st / 12.0))
        }
        other => other,
    }
}

/// Time as a number of cycles (bars) at `tempo`.
fn time_cycles(t: Time, tempo: Tempo) -> Rational {
    let beats = match t {
        Time::Bars(b) => b * tempo.beats_per_bar,
        Time::Beats(b) => b,
        Time::Seconds(s) => s * tempo.bpm / 60.0,
    };
    Rational::from_f64(beats / tempo.beats_per_bar)
}

/// `events { 0beat c4 for 1/2beat gain: -6db }` inside a clip.
fn parse_events(raw: &str, span: Span, cx: &Context) -> Res<Vec<ScoredNote>> {
    let mut out = Vec::new();
    for (line_i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let err = |message: String| {
            Diagnostic::new(message, span).with_help(format!("on line {}: `{line}`", line_i + 1))
        };
        // `<at> <value> for <dur> [name: value …]`
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 4 || tokens[2] != "for" {
            return Err(err(
                "an event is `at value for dur`, as in `0beat c4 for 1/2beat`".into(),
            ));
        }
        let at: Time = tokens[0]
            .parse()
            .map_err(|e: niminal_score::ParseError| err(e.to_string()))?;
        let value = atom_value(tokens[1], cx.sample_names).map_err(err)?;
        let dur: Time = tokens[3]
            .parse()
            .map_err(|e: niminal_score::ParseError| err(e.to_string()))?;
        let mut step = Step::from(value);
        let mut args = Vec::new();
        let mut i = 4;
        while i < tokens.len() {
            let part = tokens[i];
            let (name, val_tok) = if let Some((n, v)) = part.split_once(':') {
                if v.is_empty() {
                    i += 1;
                    if i >= tokens.len() {
                        return Err(err(format!("`{n}:` needs a value")));
                    }
                    (n, tokens[i])
                } else {
                    (n, v)
                }
            } else if let Some(n) = part.strip_suffix(':') {
                i += 1;
                if i >= tokens.len() {
                    return Err(err(format!("`{n}:` needs a value")));
                }
                (n, tokens[i])
            } else {
                return Err(err(format!(
                    "expected `name: value` after the duration, found `{part}`"
                )));
            };
            let v = atom_value(val_tok, cx.sample_names).map_err(err)?;
            match name {
                "gain" => {
                    step.gain = Some(match v {
                        Value::Db(db) => db,
                        other => other.as_gain().map_err(|e| err(e.to_string()))?.log10() * 20.0,
                    });
                }
                "pan" => {
                    step.pan = Some(v.as_angle().map_err(|e| err(e.to_string()))?);
                }
                "begin" => {
                    step.begin = Some(v.as_number().map_err(|e| err(e.to_string()))?);
                }
                "pitch" => {
                    step.pitch = Some(v.as_semitones().map_err(|e| err(e.to_string()))?);
                }
                other => args.push((other.to_string(), v)),
            }
            i += 1;
        }
        out.push(ScoredNote {
            at: time_cycles(at, cx.tempo),
            dur: time_cycles(dur, cx.tempo),
            step,
            args,
        });
    }
    Ok(out)
}

fn section_to_clip(section: crate::Section, tempo: Tempo) -> Res<Clip> {
    let length = time_cycles(section.length, tempo);
    let mut scored = Vec::with_capacity(section.events.len());
    for e in section.events {
        let mut step = Step::from(Value::Num(0.0));
        let mut args = Vec::new();
        for (k, v) in e.args {
            match k.as_str() {
                "freq" | "note" => step = Step::from(v),
                "gain" | "amp" => {
                    step.gain = match v {
                        Value::Db(db) => Some(db),
                        other => Some(
                            other
                                .as_gain()
                                .map_err(|err| Diagnostic::new(err.to_string(), Span::new(0, 0)))?
                                .log10()
                                * 20.0,
                        ),
                    };
                }
                "pan" => {
                    step.pan = Some(
                        v.as_angle()
                            .map_err(|err| Diagnostic::new(err.to_string(), Span::new(0, 0)))?,
                    );
                }
                other => args.push((other.to_string(), v)),
            }
        }
        if matches!(step.value, Value::Num(0.0)) && args.is_empty() && step.gain.is_none() {
            // no pitch: skip empty shell events that only name the target
            continue;
        }
        scored.push(ScoredNote {
            at: time_cycles(e.at, tempo),
            dur: time_cycles(e.dur, tempo),
            step,
            args,
        });
    }
    Ok(Clip {
        name: section.name,
        length: if length > Rational::ZERO {
            length
        } else {
            Rational::ONE
        },
        endless: false,
        plays: None,
        then: Vec::new(),
        lanes: Vec::new(),
        scored,
        keys: None,
        curves: Vec::new(),
        signals: Vec::new(),
        fx_params: Vec::new(),
    })
}

fn clipdef_to_clip(def: &crate::ClipDef, cx: &Context) -> Res<Clip> {
    // Re-parse the canonical source so lanes go through the same path as handwritten clips.
    let src = def.to_source();
    let items = crate::parser::parse(&src)?;
    let Item::Clip(block) = items
        .into_iter()
        .find(|i| matches!(i, Item::Clip(_)))
        .ok_or_else(|| {
            Diagnostic::new(
                format!("clip `{}` produced no clip source", def.name),
                Span::new(0, 0),
            )
        })?
    else {
        unreachable!()
    };
    let p = Perform {
        cx,
        bindings: HashMap::new(),
        arrangements: HashMap::new(),
    };
    p.clip(&block)
}
