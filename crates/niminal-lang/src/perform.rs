//! Evaluating the performance layer: pattern literals and their operations,
//! clips, scenes, and the commands that play them.

use std::collections::HashMap;

use niminal_pattern::{Hit, Pattern, Rational, grid, parse_with};
use niminal_score::{Tempo, Time, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::lower::{GAIN_PARAM, Names};
use crate::performance::*;
use crate::program::{Instrument, TrackInfo};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

/// A change to a pattern, such as the `it.reverse` in `every(2, it.reverse)`.
type Change<'a, T> = Box<dyn Fn(Pattern<T>) -> Pattern<T> + 'a>;

/// The parameter that notes in a clip's `notes` lane set.
pub const PITCH_PARAM: &str = "freq";
/// Lanes every note understands, whatever the instrument.
const UNIVERSAL_LANES: [&str; 3] = ["dur", "nudge", GAIN_PARAM];

#[derive(Clone)]
enum PerfValue {
    Pattern(Pattern<Value>),
    Grid(Pattern<Hit>),
    Clip(Clip),
    Scene(Scene),
    Rest,
}

impl PerfValue {
    fn describe(&self) -> &'static str {
        match self {
            PerfValue::Pattern(_) => "a pattern",
            PerfValue::Grid(_) => "a grid",
            PerfValue::Clip(_) => "a clip",
            PerfValue::Scene(_) => "a scene",
            PerfValue::Rest => "`~`",
        }
    }
}

/// The kinds of pattern the generic operations (`fast`, `every`...) work on.
trait Kind: Clone + Send + Sync + 'static {
    fn wrap(p: Pattern<Self>) -> PerfValue;
    fn unwrap(v: PerfValue) -> Option<Pattern<Self>>;
}

impl Kind for Value {
    fn wrap(p: Pattern<Value>) -> PerfValue {
        PerfValue::Pattern(p)
    }
    fn unwrap(v: PerfValue) -> Option<Pattern<Value>> {
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
}

pub(crate) struct Context<'a> {
    pub tempo: Tempo,
    pub names: &'a Names,
    pub instruments: &'a [Instrument],
    pub tracks: &'a [TrackInfo],
    /// Tracks whose definition failed to compile; mentions of them are not
    /// reported again.
    pub failed: &'a [&'a str],
}

struct Perform<'a> {
    cx: &'a Context<'a>,
    bindings: HashMap<String, PerfValue>,
}

pub(crate) fn compile_performance(items: &[Item], cx: &Context) -> Result<Compiled, Vec<Diagnostic>> {
    let mut p = Perform { cx, bindings: HashMap::new() };
    let mut errors = Vec::new();

    // Definitions first, in order, so commands may come before what they play.
    for item in items {
        let result = match item {
            Item::Bind { name, value } => p.eval(value, None).and_then(|v| p.define(name, v)),
            Item::Clip(block) => p.clip(block).and_then(|c| p.define(&block.name, PerfValue::Clip(c))),
            Item::Scene(block) => p.scene(block).and_then(|s| p.define(&block.name, PerfValue::Scene(s))),
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

    if errors.is_empty() { Ok(Compiled { actions }) } else { Err(errors) }
}

fn value_of(value: f64, unit: Option<&str>, span: Span) -> Res<Value> {
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
        Some(u) => return Err(Diagnostic::new(format!("unknown unit `{u}`"), span)),
    })
}

/// A pattern atom as a value: a note name, or a number with a unit.
fn atom_value(atom: &str) -> Result<Value, String> {
    atom.parse::<Value>().map_err(|e| {
        if atom.chars().all(|c| c.is_ascii_alphabetic()) {
            format!("`{atom}` isn't a note or a value (samples aren't supported yet)")
        } else {
            e.to_string()
        }
    })
}

impl Perform<'_> {
    fn define(&mut self, name: &Ident, value: PerfValue) -> Res<()> {
        if let Some(kind) = self.cx.names.kind_of(&name.name) {
            return Err(Diagnostic::new(format!("`{}` is already the name of a {kind}", name.name), name.span));
        }
        if matches!(name.name.as_str(), "it" | "out") {
            return Err(Diagnostic::new(format!("`{}` is a reserved word", name.name), name.span));
        }
        if self.bindings.contains_key(&name.name) {
            return Err(Diagnostic::new(format!("`{}` is defined twice", name.name), name.span));
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
            ExprKind::Rest => Ok(PerfValue::Rest),
            ExprKind::Name(name) if name == "it" => it.cloned().ok_or_else(|| {
                Diagnostic::new("`it` is only available inside an argument, as in `every(4, it.reverse)`", e.span)
            }),
            ExprKind::Name(name) => match self.bindings.get(name) {
                Some(v) => Ok(v.clone()),
                // a lone note or value is a one-step pattern
                None => match atom_value(name) {
                    Ok(v) => Ok(PerfValue::Pattern(Pattern::pure(v))),
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
                Ok(PerfValue::Pattern(Pattern::pure(value_of(*value, unit.as_deref(), e.span)?)))
            }
            ExprKind::Neg(inner) => match self.eval(inner, it)? {
                PerfValue::Pattern(p) => Ok(PerfValue::Pattern(p.map(negate))),
                other => Err(Diagnostic::new(format!("can't negate {}", other.describe()), e.span)),
            },
            ExprKind::Call { name, args } => self.op(name, args, e.span, it),
            ExprKind::Env(_) => Err(Diagnostic::new("envelope lanes aren't supported yet", e.span)),
            ExprKind::Channels(_) | ExprKind::Binary { .. } => {
                Err(Diagnostic::new("expected a pattern, clip or scene here", e.span)
                    .with_help("write steps in brackets, as in `[c4 e4 g4]`"))
            }
        }
    }

    fn pattern(&self, raw: &str, span: Span) -> Res<Pattern<Value>> {
        parse_with(raw, atom_value).map_err(|e| at_offset(e.message, span, e.offset))
    }

    // ---- numbers -----------------------------------------------------------

    /// A constant number with an optional unit, evaluated exactly: `1/8 beat`, `-5st`.
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
            _ => Err(Diagnostic::new("expected a number", e.span).with_help("for example `2`, `3/2` or `1/8 beat`")),
        }
    }

    /// A plain number (no unit).
    fn plain(&self, e: &Expr) -> Res<Rational> {
        match self.quantity(e)? {
            (r, None) => Ok(r),
            (_, Some(u)) => Err(Diagnostic::new(format!("expected a plain number, found a value in `{u}`"), e.span)),
        }
    }

    /// A length of time as a number of cycles (bars). Beats are a share of a
    /// bar; seconds would depend on the tempo, so they are not accepted.
    fn cycles(&self, e: &Expr) -> Res<Rational> {
        let (r, unit) = self.quantity(e)?;
        match unit.as_deref() {
            None | Some("bar" | "bars") => Ok(r),
            Some("beat" | "beats") => Ok(r / Rational::from_f64(self.cx.tempo.beats_per_bar)),
            Some(u) => Err(Diagnostic::new(format!("expected a length in beats or bars, found `{u}`"), e.span)),
        }
    }

    // ---- operations ----------------------------------------------------------

    fn op(&self, name: &Ident, args: &[Arg], span: Span, it: Option<&PerfValue>) -> Res<PerfValue> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                format!("`{}` works on a pattern: write it after one, as in `riff.{}`", name.name, name.name),
                name.span,
            ));
        };
        let rest = &args[1..];
        match self.eval(&receiver.value, it)? {
            PerfValue::Pattern(p) => self.on_pattern(name, rest, p, span, it),
            PerfValue::Grid(g) => match self.generic(name, rest, g, span, it)? {
                Some(g) => Ok(PerfValue::Grid(g)),
                None => Err(self.not_for(name, "a grid")),
            },
            PerfValue::Clip(clip) => self.on_clip(name, rest, clip, span, it).map(PerfValue::Clip),
            other => Err(self.not_for(name, other.describe())),
        }
    }

    fn not_for(&self, name: &Ident, what: &str) -> Diagnostic {
        Diagnostic::new(format!("`{}` can't be used on {what}", name.name), name.span)
    }

    fn on_pattern(
        &self,
        name: &Ident,
        rest: &[Arg],
        p: Pattern<Value>,
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
                    return Err(Diagnostic::new("`transpose` takes an interval in semitones", arg.span)
                        .with_help("for example `transpose(+5st)`"));
                }
                let st = st.to_f64();
                p.map(move |v| transpose(v, st))
            }
            "euclid" => {
                let hits = self.plain(self.arg(name, rest, "hits", span)?)?;
                let steps = self.plain(self.arg(name, rest, "steps", span)?)?;
                let rotation = match self.find(name, rest, "rotation") {
                    Some(e) => self.plain(e)?,
                    None => Rational::ZERO,
                };
                let count = |r: Rational, what: &str| -> Res<usize> {
                    if r.is_integer() && r >= Rational::ZERO {
                        Ok(r.numer() as usize)
                    } else {
                        Err(Diagnostic::new(format!("`{what}` must be a whole number"), name.span))
                    }
                };
                let structure = Pattern::<bool>::euclid(count(hits, "hits")?, count(steps, "steps")?, count(rotation, "rotation")?);
                p.rhythm(&structure)
            }
            "rhythm" => p.rhythm(&self.structure(self.arg(name, rest, "structure", span)?, it)?),
            "mask" => p.mask(&self.structure(self.arg(name, rest, "mask", span)?, it)?),
            other => return Err(self.unknown_op(other, name.span)),
        };
        Ok(PerfValue::Pattern(out))
    }

    /// A grid (or pattern) read as on/off.
    fn structure(&self, e: &Expr, it: Option<&PerfValue>) -> Res<Pattern<bool>> {
        match self.eval(e, it)? {
            PerfValue::Grid(g) => Ok(g.map(|_| true)),
            PerfValue::Pattern(p) => Ok(p.map(|_| true)),
            other => Err(Diagnostic::new(format!("expected a grid, found {}", other.describe()), e.span)),
        }
    }

    fn on_clip(&self, name: &Ident, rest: &[Arg], clip: Clip, span: Span, it: Option<&PerfValue>) -> Res<Clip> {
        // Pitch operations touch only the notes; time operations touch every lane.
        let notes_only = matches!(name.name.as_str(), "transpose" | "euclid" | "rhythm" | "mask");
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
            "fast" => clip.length / self.plain(self.arg(name, rest, "by", span)?)?,
            "slow" => clip.length * self.plain(self.arg(name, rest, "by", span)?)?,
            "over" => self.cycles(self.arg(name, rest, "length", span)?)?.max(Rational::new(1, 1000)),
            _ => clip.length,
        };
        Ok(Clip { name: clip.name, length, lanes })
    }

    /// Operations that work on any kind of pattern. `None` if `name` isn't one.
    fn generic<'a, T: Kind>(
        &'a self,
        name: &Ident,
        rest: &'a [Arg],
        p: Pattern<T>,
        span: Span,
        _it: Option<&PerfValue>,
    ) -> Res<Option<Pattern<T>>> {
        let positive = |r: Rational, what: &str, at: Span| {
            if r > Rational::ZERO { Ok(r) } else { Err(Diagnostic::new(format!("`{what}` must be above zero"), at)) }
        };
        Ok(Some(match name.name.as_str() {
            "fast" => {
                let e = self.arg(name, rest, "by", span)?;
                p.fast(positive(self.plain(e)?, "fast", e.span)?)
            }
            "slow" => {
                let e = self.arg(name, rest, "by", span)?;
                p.slow(positive(self.plain(e)?, "slow", e.span)?)
            }
            "over" => {
                let e = self.arg(name, rest, "length", span)?;
                p.slow(positive(self.cycles(e)?, "over", e.span)?)
            }
            "reverse" => p.reverse(),
            "shift" => p.shift(self.cycles(self.arg(name, rest, "by", span)?)?),
            "repeat_each" => {
                let n = self.plain(self.arg(name, rest, "times", span)?)?;
                if !n.is_integer() || n < Rational::ONE {
                    return Err(Diagnostic::new("`repeat_each` takes a whole number of at least 1", name.span));
                }
                p.repeat_each(n.numer())
            }
            "thin" => {
                let e = self.arg(name, rest, "amount", span)?;
                let (amount, unit) = self.quantity(e)?;
                let drop = match unit.as_deref() {
                    Some("%") => amount.to_f64() / 100.0,
                    None => amount.to_f64(),
                    Some(u) => return Err(Diagnostic::new(format!("`thin` takes a percentage, not `{u}`"), e.span)),
                };
                if !(0.0..=1.0).contains(&drop) {
                    return Err(Diagnostic::new("`thin` takes an amount from 0% to 100%", e.span));
                }
                // The same source always thins the same events.
                p.thin(drop, span.start as u64)
            }
            "every" => {
                let n = self.plain(self.arg(name, rest, "n", span)?)?;
                if !n.is_integer() || n < Rational::ONE {
                    return Err(Diagnostic::new("`every` takes a whole number of at least 1", name.span));
                }
                let f = self.arg(name, rest, "change", span)?;
                let change = self.transformer::<T>(f, &p)?;
                p.every(n.numer(), change)
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
            _ => return Ok(None),
        }))
    }

    /// An argument such as `it.reverse`, as a function from pattern to pattern.
    /// It is tried once on `sample` so that mistakes are reported now.
    fn transformer<'a, T: Kind>(
        &'a self,
        e: &'a Expr,
        sample: &Pattern<T>,
    ) -> Res<Change<'a, T>> {
        let apply = move |p: Pattern<T>| -> Res<Pattern<T>> {
            let out = self.eval(e, Some(&T::wrap(p)))?;
            T::unwrap(out).ok_or_else(|| Diagnostic::new("this change must give the same kind of pattern", e.span))
        };
        apply(sample.clone())?;
        Ok(Box::new(move |p: Pattern<T>| apply(p.clone()).unwrap_or(p)))
    }

    fn unknown_op(&self, name: &str, span: Span) -> Diagnostic {
        const OPS: [&str; 15] = [
            "fast", "slow", "over", "reverse", "shift", "repeat_each", "thin", "every", "layer", "transpose",
            "euclid", "rhythm", "mask", "play", "launch",
        ];
        let mut d = Diagnostic::new(format!("unknown pattern operation `{name}`"), span);
        if let Some(c) = closest(name, OPS) {
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
            "thin" => &["amount"],
            "every" => &["n", "change"],
            "layer" => &["after", "change"],
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
        params.iter().position(|p| *p == param).and_then(|i| slots[i])
    }

    fn arg<'e>(&self, op: &Ident, args: &'e [Arg], param: &str, span: Span) -> Res<&'e Expr> {
        self.find(op, args, param)
            .ok_or_else(|| Diagnostic::new(format!("`{}` needs an argument `{param}`", op.name), span))
    }

    // ---- clips and scenes ------------------------------------------------------

    fn clip(&self, block: &NamedBlock) -> Res<Clip> {
        let mut length = Rational::ONE;
        let mut lanes: Vec<(String, Pattern<Value>)> = Vec::new();
        for (key, value) in &block.entries {
            if key.name == "length" {
                length = self.cycles(value)?;
                if length <= Rational::ZERO {
                    return Err(Diagnostic::new("a clip needs a length above zero", value.span));
                }
                continue;
            }
            if lanes.iter().any(|(n, _)| *n == key.name) {
                return Err(Diagnostic::new(format!("lane `{}` is given twice", key.name), key.span));
            }
            let pattern = match self.eval(value, None)? {
                PerfValue::Pattern(p) => p,
                PerfValue::Rest => Pattern::silence(),
                other => {
                    return Err(Diagnostic::new(format!("a lane holds a pattern, but this is {}", other.describe()), value.span));
                }
            };
            lanes.push((key.name.clone(), pattern));
        }
        Ok(Clip { name: block.name.name.clone(), length, lanes })
    }

    fn scene(&self, block: &NamedBlock) -> Res<Scene> {
        let mut entries: Vec<(usize, Option<Clip>)> = Vec::new();
        for (key, value) in &block.entries {
            let track = self.track(key)?;
            if entries.iter().any(|(t, _)| *t == track) {
                return Err(Diagnostic::new(format!("track `{}` appears twice in this scene", key.name), key.span));
            }
            let clip = match self.eval(value, None)? {
                PerfValue::Rest => None,
                other => Some(self.to_clip(other, &key.name, value.span)?),
            };
            entries.push((track, clip));
        }
        Ok(Scene { name: block.name.name.clone(), entries })
    }

    /// Anything playable as a clip: a clip, or a bare pattern of notes.
    fn to_clip(&self, v: PerfValue, hint: &str, span: Span) -> Res<Clip> {
        match v {
            PerfValue::Clip(c) => Ok(c),
            PerfValue::Pattern(p) => Ok(Clip {
                name: hint.to_string(),
                length: Rational::ONE,
                lanes: vec![("notes".to_string(), p)],
            }),
            PerfValue::Grid(_) => Err(Diagnostic::new("a grid has no pitches to play", span)
                .with_help("give it notes with `.rhythm(grid[...])` on a pattern instead")),
            PerfValue::Scene(_) => Err(Diagnostic::new("a scene is launched, not played", span)
                .with_help("write `launch name`")),
            PerfValue::Rest => Err(Diagnostic::new("`~` stops a track in a scene; to stop one, write `stop track`", span)),
        }
    }

    // ---- commands ----------------------------------------------------------------

    /// A track by name; the implicit track 0 can't be named.
    fn track(&self, id: &Ident) -> Res<usize> {
        match self.cx.tracks.iter().skip(1).position(|t| t.name == id.name) {
            Some(i) => Ok(i + 1),
            None => {
                let mut d = Diagnostic::new(format!("no track named `{}`", id.name), id.span);
                if let Some(c) = closest(&id.name, self.cx.tracks.iter().skip(1).map(|t| t.name.as_str())) {
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
            Some(AtPos::Time(e)) => time_of(e, self),
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
        let quantize = stmt.quantize.map(convert_quantize);
        let schedule = |action| Scheduled { at, action, quantize };

        Ok(match &stmt.command {
            Command::Play { track, what } => {
                if self.cx.failed.contains(&track.name.as_str()) {
                    return Ok(Vec::new());
                }
                let t = self.track(track)?;
                let clip = self.to_clip(self.eval(what, None)?, &track.name, what.span)?;
                self.check_clip(&clip, t, track, what.span)?;
                vec![schedule(Action::Play { track: t, clip })]
            }
            Command::Stop(t) => vec![schedule(Action::Stop { track: self.track(t)? })],
            Command::Mute(t) => vec![schedule(Action::Mute { track: self.track(t)? })],
            Command::Unmute(t) => vec![schedule(Action::Unmute { track: self.track(t)? })],
            Command::Solo(t) => vec![schedule(Action::Solo { track: self.track(t)? })],
            Command::Unsolo(t) => {
                let track = t.as_ref().map(|t| self.track(t)).transpose()?;
                vec![schedule(Action::Unsolo { track })]
            }
            Command::Hush => vec![schedule(Action::Hush)],
            Command::Panic => vec![schedule(Action::Panic)],
            Command::Launch(name) => {
                let Some(PerfValue::Scene(scene)) = self.bindings.get(&name.name) else {
                    let mut d = Diagnostic::new(format!("no scene named `{}`", name.name), name.span);
                    let scenes = self.bindings.iter().filter(|(_, v)| matches!(v, PerfValue::Scene(_))).map(|(k, _)| k.as_str());
                    if let Some(c) = closest(&name.name, scenes) {
                        d = d.with_help(format!("did you mean `{c}`?"));
                    }
                    return Err(d);
                };
                let mut out = Vec::new();
                for (track, clip) in &scene.entries {
                    match clip {
                        Some(clip) => {
                            let id = Ident { name: self.cx.tracks[*track].name.clone(), span: name.span };
                            self.check_clip(clip, *track, &id, name.span)?;
                            out.push(schedule(Action::Play { track: *track, clip: clip.clone() }));
                        }
                        None => out.push(schedule(Action::Stop { track: *track })),
                    }
                }
                out
            }
        })
    }

    /// Does `clip`'s notes and lanes make sense on `track`'s instrument?
    fn check_clip(&self, clip: &Clip, track: usize, id: &Ident, span: Span) -> Res<()> {
        let info = &self.cx.tracks[track];
        let Some(instrument) = info.instrument.map(|i| &self.cx.instruments[i]) else {
            return Err(Diagnostic::new(format!("track `{}` has no instrument to play a clip on", id.name), id.span)
                .with_help("add `instrument = ...` to the track"));
        };
        for (lane, _) in &clip.lanes {
            if lane == "notes" {
                let pitched = instrument.params.iter().any(|p| p.name == PITCH_PARAM && p.unit == Unit::Hz);
                if !pitched {
                    return Err(Diagnostic::new(
                        format!("`{}` has no `{PITCH_PARAM}` parameter for the notes to set", instrument.name),
                        span,
                    )
                    .with_help(format!("give the instrument a `{PITCH_PARAM}: hz` parameter")));
                }
                continue;
            }
            if UNIVERSAL_LANES.contains(&lane.as_str()) || instrument.params.iter().any(|p| p.name == *lane) {
                continue;
            }
            if matches!(lane.as_str(), "pan" | "to") {
                return Err(Diagnostic::new(format!("the `{lane}` lane isn't supported yet"), span));
            }
            let mut known: Vec<&str> = instrument.params.iter().map(|p| p.name.as_str()).collect();
            known.extend(UNIVERSAL_LANES);
            let mut d = Diagnostic::new(
                format!("clip `{}` has a lane `{lane}`, but `{}` has no such parameter", clip.name, instrument.name),
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

fn convert_quantize(q: QuantizeSpec) -> Quantize {
    Quantize {
        relation: match q.relation {
            Relation::OnGrid => QuantizeRelation::OnGrid,
            Relation::Next => QuantizeRelation::Next,
            Relation::In => QuantizeRelation::In,
        },
        count: q.count,
        unit: match q.unit {
            QuantUnit::Now => QuantizeUnit::Now,
            QuantUnit::Beat => QuantizeUnit::Beat,
            QuantUnit::Bar => QuantizeUnit::Bar,
            QuantUnit::Cycle => QuantizeUnit::Cycle,
        },
    }
}

/// A time written in source: `2beats`, `1/2 beat`, `4 bars`, `250ms`.
fn time_of(e: &Expr, p: &Perform) -> Res<Time> {
    let (r, unit) = p.quantity(e)?;
    let v = r.to_f64();
    match unit.as_deref() {
        Some("sec") => Ok(Time::Seconds(v)),
        Some("ms") => Ok(Time::Seconds(v / 1000.0)),
        Some("beat" | "beats") => Ok(Time::Beats(v)),
        Some("bar" | "bars") => Ok(Time::Bars(v)),
        _ => Err(Diagnostic::new("expected a time", e.span).with_help("for example `2beats`, `4 bars` or `250ms`, or `bar 5 beat 3`")),
    }
}

/// Where in the source a problem inside a pattern's text is.
fn at_offset(message: String, token: Span, offset: usize) -> Diagnostic {
    // the token's text starts after its opening bracket
    let at = token.start + 1 + offset;
    Diagnostic::new(message, Span::new(at, (at + 1).min(token.end)))
}

fn negate(v: Value) -> Value {
    match v {
        Value::Num(n) => Value::Num(-n),
        Value::Hz(n) => Value::Hz(-n),
        Value::Db(n) => Value::Db(-n),
        Value::Seconds(n) => Value::Seconds(-n),
        Value::Beats(n) => Value::Beats(-n),
        Value::Bars(n) => Value::Bars(-n),
        Value::Semitones(n) => Value::Semitones(-n),
        Value::Degrees(n) => Value::Degrees(-n),
        note @ Value::Note(_) => note,
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
