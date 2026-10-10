//! Unit checking and lowering, in one pass: each expression is typed and turned
//! into engine graph nodes as it is visited.
//!
//! Signals can have several channels. The engine's nodes are all mono, so a
//! multichannel signal here is a bundle of mono ones, and an opcode applied to
//! it is built once per channel (each with its own state), which is the spec's
//! multichannel expansion. Conversions between layouts are emitted as gain and
//! sum nodes.

use std::collections::HashMap;
use std::sync::Arc;

use niminal_engine::ops::{Add, Curve, Env, Fn1, Func, Mul, PanChannel, SampleData, Sampler, Segment, Sub};
use niminal_engine::{ChainInput, Graph, GraphBuilder, Route, Src};
use niminal_score::{Tempo, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::layout::{Layout, conversion};
use crate::opcodes::{self, Build, BuildArgs, Kind, Registry};
use crate::program::{ControlDef, Instrument, Param};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

const UNIT_NAMES: [&str; 12] = ["hz", "khz", "db", "sec", "ms", "beat", "beats", "bar", "bars", "bpm", "deg", "st"];

/// A literal's unit and value. Decibels become gain factors and beats become
/// seconds at `tempo`.
pub(crate) fn literal_sig(value: f64, unit: Option<&str>, tempo: Tempo, span: Span) -> Res<(Unit, f64)> {
    Ok(match unit {
        None => (Unit::Num, value),
        Some("hz") => (Unit::Hz, value),
        Some("khz") => (Unit::Hz, value * 1000.0),
        Some("db") => (Unit::Db, 10f64.powf(value / 20.0)),
        Some("sec") => (Unit::Time, value),
        Some("ms") => (Unit::Time, value / 1000.0),
        Some("beat" | "beats") => (Unit::Time, value * 60.0 / tempo.bpm),
        Some("bar" | "bars") => (Unit::Time, value * tempo.beats_per_bar * 60.0 / tempo.bpm),
        Some("deg") => (Unit::Angle, value),
        Some("st") => (Unit::Semitones, value),
        Some("bpm") => {
            return Err(Diagnostic::new("`bpm` can only be used with `tempo`", span)
                .with_help("for example `tempo 120bpm`"));
        }
        Some(u) => return Err(unknown_unit(u, span)),
    })
}

pub(crate) fn unknown_name<'a>(name: &str, span: Span, known: impl IntoIterator<Item = &'a str>) -> Diagnostic {
    let mut d = Diagnostic::new(format!("`{name}` is not defined"), span);
    if let Some(c) = closest(name, known) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

pub(crate) fn unknown_unit(unit: &str, span: Span) -> Diagnostic {
    let mut d = Diagnostic::new(format!("unknown unit `{unit}`"), span);
    if let Some(c) = closest(unit, UNIT_NAMES) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

/// The control every instrument has, applied after its output.
pub(crate) const GAIN_PARAM: &str = "gain";

const LAYOUT_HELP: &str = "one of mono, stereo, quad, surround(5.1), surround(7.1), or channels(n)";

/// A layout written in source: `stereo`, `surround(5.1)`, `channels(3)`.
pub(crate) fn layout_from_expr(e: &Expr) -> Res<Layout> {
    let bad = |what: String| Diagnostic::new(what, e.span).with_help(LAYOUT_HELP);
    match &e.kind {
        ExprKind::Name(n) => match n.as_str() {
            "mono" => Ok(Layout::Mono),
            "stereo" => Ok(Layout::Stereo),
            "quad" => Ok(Layout::Quad),
            other => Err(bad(format!("unknown channel layout `{other}`"))),
        },
        ExprKind::Call { name, args } if name.name == "surround" => match args.as_slice() {
            [Arg { name: None, value: Expr { kind: ExprKind::Num { value, unit: None }, .. }, .. }] => {
                if (*value - 5.1).abs() < 1e-9 {
                    Ok(Layout::Surround51)
                } else if (*value - 7.1).abs() < 1e-9 {
                    Ok(Layout::Surround71)
                } else {
                    Err(bad(format!("unsupported surround layout `{value}`")))
                }
            }
            _ => Err(bad("`surround` takes a layout such as `surround(5.1)`".into())),
        },
        ExprKind::Call { name, args } if name.name == "channels" => match args.as_slice() {
            [Arg { name: None, value: Expr { kind: ExprKind::Num { value, unit: None }, .. }, .. }]
                if value.fract() == 0.0 && (1.0..=16.0).contains(value) =>
            {
                Ok(Layout::from_count(*value as usize))
            }
            _ => Err(bad("`channels` takes a count from 1 to 16".into())),
        },
        _ => Err(bad("expected a channel layout".into())),
    }
}

/// Every top-level name, so locals can be kept from shadowing them, plus the
/// layouts that tracks and buses use.
pub(crate) struct Names {
    pub controls: Vec<ControlDef>,
    entries: Vec<(String, &'static str)>,
    pub buses: Vec<String>,
    pub bus_layouts: Vec<Layout>,
    /// The layout of the master, and so of every voice and track.
    pub master: Layout,
}

impl Default for Names {
    fn default() -> Self {
        Names { controls: Vec::new(), entries: Vec::new(), buses: Vec::new(), bus_layouts: Vec::new(), master: Layout::Mono }
    }
}

impl Names {
    pub fn add(&mut self, name: &str, kind: &'static str) {
        if kind == "bus" && !self.buses.iter().any(|b| b == name) {
            self.buses.push(name.to_string());
            self.bus_layouts.push(Layout::Mono);
        }
        self.entries.push((name.to_string(), kind));
    }

    pub fn kind_of(&self, name: &str) -> Option<&'static str> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, k)| *k)
    }

    pub fn bus(&self, name: &str) -> Option<usize> {
        self.buses.iter().position(|b| b == name)
    }

    /// An error if a local may not be called `name`.
    fn check_local(&self, name: &Ident) -> Res<()> {
        if let Some(kind) = self.kind_of(&name.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already the name of a {kind}, so it can't be used for a local", name.name),
                name.span,
            ));
        }
        if matches!(name.name.as_str(), "out" | "it" | "instrument" | "state") {
            return Err(Diagnostic::new(format!("`{}` is a reserved word", name.name), name.span));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Evaluating constants such as parameter defaults.
    Plain,
    Instrument,
    Track,
}

/// A value in the graph: one source per channel.
#[derive(Clone)]
struct Sig {
    srcs: Vec<Src>,
    layout: Layout,
    unit: Unit,
    /// Known at compile time (single channel only). Decibel values are held as
    /// linear gain factors.
    konst: Option<f64>,
}

impl Sig {
    fn constant(unit: Unit, v: f64) -> Sig {
        Sig { srcs: vec![Src::Const(v as f32)], layout: Layout::Mono, unit, konst: Some(v) }
    }

    fn signal(src: Src, unit: Unit) -> Sig {
        Sig { srcs: vec![src], layout: Layout::Mono, unit, konst: None }
    }

    /// A plain signal with one source per channel.
    fn channels(srcs: Vec<Src>, layout: Layout) -> Sig {
        Sig { srcs, layout, unit: Unit::Num, konst: None }
    }

    fn count(&self) -> usize {
        self.srcs.len()
    }

    /// The source for channel `k`; a one-channel signal feeds every channel.
    fn pick(&self, k: usize) -> Src {
        if self.srcs.len() == 1 { self.srcs[0] } else { self.srcs[k] }
    }
}

/// A call's evaluated argument.
enum Val {
    Wave(opcodes::Wave),
    Layout(Layout),
    Sig(Sig),
}

pub(crate) struct Lower<'a> {
    registry: &'a Registry,
    names: &'a Names,
    mode: Mode,
    g: GraphBuilder,
    tempo: Tempo,
    scope: HashMap<String, Sig>,
    /// The value of `out` so far.
    out: Option<Sig>,
    sends: usize,
    /// In a track: what feeds each external input, and the bundles made so far.
    chain_inputs: Vec<ChainInput>,
    input_sigs: HashMap<String, Sig>,
    route: Route,
}

pub(crate) fn compile_control(name: &Ident, value: &Expr, tempo: Tempo, registry: &Registry, names: &Names) -> Res<ControlDef> {
    if matches!(name.name.as_str(), "out" | "it" | "instrument" | "state" | "ctl") {
        return Err(Diagnostic::new(format!("`{}` is a reserved word", name.name), name.span));
    }
    let mut lower = Lower::new(registry, names, tempo);
    let mut initial = value.clone();
    let mut smooth_seconds = 0.005;
    let (smooth_expr, negative) = match &value.kind {
        ExprKind::Neg(inner) => (inner.as_ref(), true),
        _ => (value, false),
    };
    if let ExprKind::Call { name: method, args } = &smooth_expr.kind
        && method.name == "smooth"
    {
        let [receiver, duration] = args.as_slice() else {
            return Err(Diagnostic::new("a control's .smooth takes one duration", value.span));
        };
        if !receiver.receiver || duration.name.is_some() {
            return Err(Diagnostic::new("write ctl name = value.smooth(5ms)", value.span));
        }
        initial = receiver.value.clone();
        if negative { initial = Expr { kind: ExprKind::Neg(Box::new(initial)), span: value.span }; }
        let smooth = lower.expr(&duration.value)?;
        if smooth.unit != Unit::Time {
            return Err(Diagnostic::new("control smoothing needs a duration, such as 5ms", duration.value.span));
        }
        smooth_seconds = smooth.konst.ok_or_else(|| Diagnostic::new("smoothing must be constant", duration.value.span))?;
        if !smooth_seconds.is_finite() || !(0.0..=60.0).contains(&smooth_seconds) {
            return Err(Diagnostic::new("control smoothing must be between 0ms and 60sec", duration.value.span));
        }
    }
    let sig = lower.expr(&initial)?;
    let value = sig.konst.ok_or_else(|| Diagnostic::new("a control's initial value must be constant", initial.span))? as f32;
    if !value.is_finite() || (sig.unit == Unit::Db && value <= 0.0) {
        return Err(Diagnostic::new("control values must be finite and representable", initial.span));
    }
    Ok(ControlDef { name: name.name.clone(), key: format!("ctl:{}:{:?}", name.name, sig.unit),
        unit: sig.unit, initial: value, smooth_seconds })
}

pub(crate) fn compile_instr(def: &InstrDef, tempo: Tempo, registry: &Registry, names: &Names) -> Res<Instrument> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut params: Vec<Param> = Vec::new();

    for p in &def.params {
        if params.iter().any(|q| q.name == p.name.name) {
            return Err(Diagnostic::new(format!("parameter `{}` is declared twice", p.name.name), p.name.span));
        }
        names.check_local(&p.name)?;
        let Some(ty) = &p.ty else {
            return Err(Diagnostic::new(format!("parameter `{}` needs a type", p.name.name), p.name.span)
                .with_help(format!("for example `{}: hz`", p.name.name)));
        };
        let (unit, range) = param_type(ty)?;
        let default = match &p.default {
            None => None,
            Some(e) => Some(lower.param_default(e, unit, range, &p.name.name)?),
        };
        let src = lower
            .g
            .param(&p.name.name, default.unwrap_or(0.0))
            .expect("duplicates checked above");
        lower.scope.insert(p.name.name.clone(), Sig::signal(src, unit));
        params.push(Param { name: p.name.name.clone(), unit, range, default });
    }

    // Every note accepts a `gain` control, applied after the instrument.
    if let Some(p) = def.params.iter().find(|p| p.name.name == GAIN_PARAM) {
        return Err(Diagnostic::new(format!("`{GAIN_PARAM}` is reserved: every note already has a `{GAIN_PARAM}` control"), p.name.span));
    }
    let gain = lower.g.param(GAIN_PARAM, 1.0).expect("name checked above");
    params.push(Param { name: GAIN_PARAM.to_string(), unit: Unit::Db, range: None, default: Some(1.0) });

    if def.body.is_empty() {
        return Err(Diagnostic::new(format!("instrument `{}` is empty", def.name.name), def.name.span)
            .with_help("the last line of an instrument is the signal it outputs"));
    }
    let body: Vec<&Stmt> = def.body.iter().collect();
    let tail = lower.run_body(&body)?;

    let (result, span) = match (lower.out.take(), tail) {
        (Some(_), Some((_, span))) => {
            return Err(Diagnostic::new("this instrument sets `out`, so its last line can't also be a value", span)
                .with_help("assign it to `out`, or remove the `out` lines"));
        }
        (Some(out), None) => (out, def.name.span),
        (None, Some((sig, span))) => {
            if sig.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!("an instrument outputs a plain signal, but this is {}", sig.unit.describe()),
                    span,
                ));
            }
            (sig, span)
        }
        // Only sends: the instrument makes no sound of its own.
        (None, None) if lower.sends > 0 => (Sig::constant(Unit::Num, 0.0), def.name.span),
        (None, None) => {
            let name = match body.last() {
                Some(Stmt::Bind { name, .. }) => name,
                _ => &def.name,
            };
            return Err(Diagnostic::new("an instrument must end with the signal it outputs", name.span)
                .with_help(format!("add a last line such as `{}`", name.name)));
        }
    };

    // Every voice leaves the instrument in the master's layout.
    let result = lower.convert(&result, names.master, span)?;
    let result = lower.mul(&result, &Sig::signal(gain, Unit::Num), Unit::Num, span)?;
    Ok(Instrument {
        name: def.name.name.clone(),
        graph: Arc::new(lower.g.build_channels(&result.srcs)),
        params,
        members: Vec::new(),
        chokes: Vec::new(),
    })
}

/// One sample of a sampler instrument.
pub(crate) struct Member {
    pub name: String,
    pub data: Arc<SampleData>,
    /// The frames of `data` it plays, if not all of them.
    pub slice: Option<(usize, usize)>,
    /// Playback speed that fits a loop to the tempo; 1 for none.
    pub speed: f64,
}

/// An instrument that plays recorded audio. A kit has one player per sample,
/// and its `sample` parameter picks which one sounds.
pub(crate) fn compile_sampler(
    name: &Ident,
    members: &[Member],
    is_kit: bool,
    root_hz: f32,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Instrument> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut params: Vec<Param> = Vec::new();
    let mut declare = |lower: &mut Lower, pname: &str, unit: Unit, default: f32| {
        params.push(Param { name: pname.to_string(), unit, range: None, default: Some(default) });
        lower.g.param(pname, default).expect("sampler parameter names are distinct")
    };
    let freq = declare(&mut lower, "freq", Unit::Hz, root_hz);
    let pitch = declare(&mut lower, "pitch", Unit::Semitones, 0.0);
    let rate = declare(&mut lower, "rate", Unit::Num, 1.0);
    let start = declare(&mut lower, "start", Unit::Time, 0.0);
    let pick = is_kit.then(|| declare(&mut lower, "sample", Unit::Num, 0.0));
    let gain = declare(&mut lower, GAIN_PARAM, Unit::Db, 1.0);

    let too_many = |n: usize| {
        Diagnostic::new(format!("a sample with {n} channels is more than the {MAX_SAMPLE_CHANNELS} niminal supports"), name.span)
    };
    let mut mix: Option<Sig> = None;
    for (i, member) in members.iter().enumerate() {
        let data = &member.data;
        let channels = data.channels.len();
        if channels > MAX_SAMPLE_CHANNELS {
            return Err(too_many(channels));
        }
        let mut srcs = Vec::with_capacity(channels);
        for c in 0..channels {
            let mut wires = vec![("freq", freq), ("pitch", pitch), ("rate", rate), ("start", start)];
            wires.extend(pick.map(|p| ("sample", p)));
            let mut player = Sampler::new(data.clone(), c, root_hz, is_kit.then_some(i)).with_speed(member.speed);
            if let Some((start, end)) = member.slice {
                player = player.slice(start, end);
            }
            srcs.push(lower.g.add(player, &wires).map_err(|e| Diagnostic::new(e.to_string(), name.span))?);
        }
        let sig = lower.convert(&Sig::channels(srcs, Layout::from_count(channels)), names.master, name.span)?;
        mix = Some(match mix {
            None => sig,
            Some(so_far) => lower.binary(BinOp::Add, so_far, sig, name.span)?,
        });
    }
    let result = mix.expect("a sample or kit has at least one sample");
    let result = lower.mul(&result, &Sig::signal(gain, Unit::Num), Unit::Num, name.span)?;
    Ok(Instrument {
        name: name.name.clone(),
        graph: Arc::new(lower.g.build_channels(&result.srcs)),
        params,
        members: if is_kit { members.iter().map(|m| m.name.clone()).collect() } else { Vec::new() },
        chokes: Vec::new(),
    })
}

const MAX_SAMPLE_CHANNELS: usize = 16;

/// A compiled track: its processing, what feeds that processing, and where
/// the result goes.
pub(crate) struct Chain {
    pub graph: Option<Arc<Graph>>,
    pub inputs: Vec<ChainInput>,
    pub route: Route,
}

/// Compile a track's statements (everything except its `instrument` line).
pub(crate) fn compile_chain(
    track: &Ident,
    body: &[&Stmt],
    has_instrument: bool,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Chain> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Track;

    if body.is_empty() {
        return if has_instrument {
            Ok(Chain { graph: None, inputs: Vec::new(), route: Route::Master })
        } else {
            Err(Diagnostic::new(format!("track `{}` does nothing", track.name), track.span)
                .with_help("give it an `instrument = ...`, or an `out = ...` that reads a bus"))
        };
    }
    lower.run_body(body)?;

    let out = match lower.out.take() {
        Some(out) => out,
        None if has_instrument => lower.it(),
        None if lower.sends > 0 => Sig::constant(Unit::Num, 0.0),
        None => {
            return Err(Diagnostic::new(format!("track `{}` never sets `out`", track.name), track.span)
                .with_help("add `out = it` to pass its sound through, or send to a bus with `bus += ...`"));
        }
    };
    // The chain delivers in its destination's layout.
    let destination = match lower.route {
        Route::Master => names.master,
        Route::Bus(b) => names.bus_layouts[b],
    };
    let out = lower.convert(&out, destination, track.span)?;
    Ok(Chain {
        graph: Some(Arc::new(lower.g.build_channels(&out.srcs))),
        inputs: lower.chain_inputs,
        route: lower.route,
    })
}

fn state_outside_opcode(name: &Ident) -> Diagnostic {
    Diagnostic::new("`state` can only be used inside an `opcode`", name.span)
        .with_help("an instrument's filters and delays keep their own state")
}

pub(crate) fn param_type(ty: &TypeSpec) -> Res<(Unit, Option<(f64, f64)>)> {
    match ty {
        TypeSpec::Range { lo, hi, span } => {
            if lo >= hi {
                return Err(Diagnostic::new("the lower bound of a range must be below the upper bound", *span));
            }
            Ok((Unit::Num, Some((*lo, *hi))))
        }
        TypeSpec::Unit(id) => match id.name.as_str() {
            "hz" => Ok((Unit::Hz, None)),
            "db" => Ok((Unit::Db, None)),
            "sec" => Ok((Unit::Time, None)),
            "deg" => Ok((Unit::Angle, None)),
            "st" => Ok((Unit::Semitones, None)),
            other => Err(Diagnostic::new(format!("unknown parameter type `{other}`"), id.span)
                .with_help("use a unit (`hz`, `db`, `sec`, `deg`, `st`) or a range such as `0..1`")),
        },
    }
}

impl<'a> Lower<'a> {
    pub(crate) fn new(registry: &'a Registry, names: &'a Names, tempo: Tempo) -> Self {
        Lower {
            registry,
            names,
            mode: Mode::Plain,
            g: GraphBuilder::new(),
            tempo,
            scope: HashMap::new(),
            out: None,
            sends: 0,
            chain_inputs: Vec::new(),
            input_sigs: HashMap::new(),
            route: Route::Master,
        }
    }

    /// Run a body's statements, returning its trailing bare expression, if any.
    fn run_body(&mut self, body: &[&Stmt]) -> Res<Option<(Sig, Span)>> {
        let mut tail = None;
        for (i, stmt) in body.iter().enumerate() {
            match stmt {
                Stmt::State { name, .. } => return Err(state_outside_opcode(name)),
                Stmt::Bind { name, value } => self.assign(name, value)?,
                Stmt::Destructure { names, value } => self.destructure(names, value)?,
                Stmt::AddAssign { name, value } => self.add_assign(name, value)?,
                Stmt::Expr(e) if self.mode == Mode::Track => {
                    return Err(Diagnostic::new("a track makes sound through `out`", e.span)
                        .with_help("write `out = ...`"));
                }
                Stmt::Expr(e) if i + 1 == body.len() => tail = Some((self.expr(e)?, e.span)),
                Stmt::Expr(e) => {
                    return Err(Diagnostic::new("this value is never used", e.span)
                        .with_help("bind it with `name = ...`, or make it the last line"));
                }
            }
        }
        Ok(tail)
    }

    fn assign(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        match name.name.as_str() {
            "out" => {
                let sig = self.out_value(value)?;
                self.set_out(sig, value.span)
            }
            "it" => Err(Diagnostic::new("`it` is an input and can't be assigned", name.span)),
            "instrument" => Err(Diagnostic::new("`instrument` is only available in a track", name.span)),
            _ => {
                self.names.check_local(name)?;
                let sig = self.expr(value)?;
                self.scope.insert(name.name.clone(), sig);
                Ok(())
            }
        }
    }

    /// `[l, r] = src`: one name per channel.
    fn destructure(&mut self, names: &[Ident], value: &Expr) -> Res<()> {
        let sig = self.expr(value)?;
        self.require_signal("a channel", &sig, value.span)?;
        if sig.count() != names.len() {
            return Err(Diagnostic::new(
                format!("this signal has {} channel(s), but {} name(s) are given", sig.count(), names.len()),
                value.span,
            ));
        }
        for (name, src) in names.iter().zip(&sig.srcs) {
            self.names.check_local(name)?;
            self.scope.insert(name.name.clone(), Sig::signal(*src, Unit::Num));
        }
        Ok(())
    }

    fn add_assign(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        if name.name == "out" {
            let sig = self.expr(value)?;
            self.require_signal("out", &sig, value.span)?;
            let sum = match self.out.take() {
                None => sig,
                Some(prev) => self.binary(BinOp::Add, prev, sig, value.span)?,
            };
            self.out = Some(sum);
            return Ok(());
        }
        if let Some(bus) = self.names.bus(&name.name) {
            let sig = self.expr(value)?;
            self.require_signal(&name.name, &sig, value.span)?;
            let sig = self.convert(&sig, self.names.bus_layouts[bus], value.span)?;
            for (channel, src) in sig.srcs.iter().enumerate() {
                self.g.send_channel(bus, channel, *src);
            }
            self.sends += 1;
            return Ok(());
        }
        let mut d = Diagnostic::new(format!("`+=` can only add to `out` or a bus, not `{}`", name.name), name.span);
        if self.scope.contains_key(&name.name) {
            d = d.with_help(format!("to change a local, write `{0} = {0} + ...`", name.name));
        } else if let Some(c) = closest(&name.name, self.names.buses.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean the bus `{c}`?"));
        }
        Err(d)
    }

    fn require_signal(&self, what: &str, sig: &Sig, span: Span) -> Res<()> {
        if sig.unit == Unit::Num {
            Ok(())
        } else {
            Err(Diagnostic::new(format!("`{what}` carries a plain signal, but this is {}", sig.unit.describe()), span))
        }
    }

    fn set_out(&mut self, sig: Sig, span: Span) -> Res<()> {
        self.require_signal("out", &sig, span)?;
        self.out = Some(sig);
        Ok(())
    }

    /// The value assigned to `out`. In a track, `x.to(bus)` routes the track's
    /// output to a bus instead of the master.
    fn out_value(&mut self, value: &Expr) -> Res<Sig> {
        let ExprKind::Call { name, args } = &value.kind else { return self.expr(value) };
        if name.name != "to" || self.mode != Mode::Track {
            return self.expr(value);
        }
        let [source, target] = args.as_slice() else {
            return Err(Diagnostic::new("`to` takes a bus", name.span).with_help("for example `out = it.to(space)`"));
        };
        let bus = match (&target.name, &target.value.kind) {
            (None, ExprKind::Name(b)) => self.names.bus(b).ok_or_else(|| {
                unknown_name(b, target.value.span, self.names.buses.iter().map(String::as_str))
            })?,
            _ => return Err(Diagnostic::new("`to` takes the name of a bus", target.value.span)),
        };
        self.route = Route::Bus(bus);
        self.expr(&source.value)
    }

    /// A bundle of a track chain's external inputs, made on first use. Each
    /// channel is its own mono input.
    fn input_bundle(&mut self, key: &str, layout: Layout, source: impl Fn(usize) -> ChainInput) -> Sig {
        if let Some(sig) = self.input_sigs.get(key) {
            return sig.clone();
        }
        let srcs = (0..layout.channels())
            .map(|c| {
                self.chain_inputs.push(source(c));
                self.g.input()
            })
            .collect();
        let sig = Sig::channels(srcs, layout);
        self.input_sigs.insert(key.to_string(), sig.clone());
        sig
    }

    /// `it`: what the track's voices produced, in the master's layout.
    fn it(&mut self) -> Sig {
        self.input_bundle("it", self.names.master, |channel| ChainInput::It { channel })
    }

    /// Turn a signal into another layout using the automatic rules, as gain and
    /// sum nodes. An unnamed `channels(n)` signal may be taken as any layout
    /// with `n` channels.
    fn convert(&mut self, sig: &Sig, to: Layout, span: Span) -> Res<Sig> {
        if sig.layout == to {
            return Ok(sig.clone());
        }
        if matches!(sig.layout, Layout::Channels(_)) && sig.count() == to.channels() {
            return Ok(Sig { layout: to, ..sig.clone() });
        }
        let Some(matrix) = conversion(sig.layout, to) else {
            return Err(Diagnostic::new(
                format!("can't convert {} to {} automatically", sig.layout, to),
                span,
            )
            .with_help("pan the signal, or take its channels apart with `[a, b] = ...` and rebuild them"));
        };

        let mut srcs = Vec::with_capacity(to.channels());
        for row in &matrix {
            let mut sum: Option<Src> = None;
            for (input, &coeff) in row.iter().enumerate() {
                if coeff == 0.0 {
                    continue;
                }
                let term = if coeff == 1.0 {
                    sig.srcs[input]
                } else {
                    self.g
                        .add(Mul, &[("a", sig.srcs[input]), ("b", Src::Const(coeff as f32))])
                        .expect("mul ports")
                };
                sum = Some(match sum {
                    None => term,
                    Some(prev) => self.g.add(Add, &[("a", prev), ("b", term)]).expect("add ports"),
                });
            }
            srcs.push(sum.unwrap_or(Src::Const(0.0)));
        }
        Ok(Sig::channels(srcs, to))
    }

    pub(crate) fn param_default(&mut self, e: &Expr, unit: Unit, range: Option<(f64, f64)>, name: &str) -> Res<f32> {
        let sig = self.expr(e)?;
        let Some(v) = sig.konst else {
            return Err(Diagnostic::new(format!("the default for `{name}` must be a constant"), e.span));
        };
        // A gain parameter may default to a plain linear factor.
        let ok = sig.unit == unit || (unit == Unit::Db && sig.unit == Unit::Num);
        if !ok {
            return Err(self.unit_mismatch(name, unit, sig.unit, e));
        }
        if let Some((lo, hi)) = range
            && !(lo..=hi).contains(&v)
        {
            return Err(Diagnostic::new(format!("the default for `{name}` is outside {lo}..{hi}"), e.span));
        }
        Ok(v as f32)
    }

    fn literal(&self, value: f64, unit: Option<&str>, span: Span) -> Res<Sig> {
        literal_sig(value, unit, self.tempo, span).map(|(unit, v)| Sig::constant(unit, v))
    }

    fn expr(&mut self, e: &Expr) -> Res<Sig> {
        match &e.kind {
            ExprKind::Num { value, unit } => self.literal(*value, unit.as_deref(), e.span),
            ExprKind::Name(name) => self.name(name, e.span),
            ExprKind::Neg(inner) => {
                let s = self.expr(inner)?;
                self.negate(s, e.span)
            }
            ExprKind::Channels(items) => self.channel_list(items, e.span),
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                self.binary(*op, l, r, e.span)
            }
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Env(env) => self.env(env),
            ExprKind::Pattern(_) | ExprKind::Rest => Err(Diagnostic::new(
                "a pattern can't be used inside an instrument, opcode or track",
                e.span,
            )
            .with_help("patterns go in `clip`, `scene` and `play`; use `[a, b]` here for a channel list")),
            ExprKind::Grid(_) => Err(Diagnostic::new("grids as trigger signals aren't supported yet", e.span)),
        }
    }

    /// `[a, b, c]`: one single-channel entry per channel. Entries share a unit,
    /// so `[800hz, 1200hz]` is a two-channel frequency.
    fn channel_list(&mut self, items: &[Expr], span: Span) -> Res<Sig> {
        let mut srcs = Vec::with_capacity(items.len());
        let mut unit = None;
        for item in items {
            let sig = self.expr(item)?;
            if sig.count() != 1 {
                return Err(Diagnostic::new(
                    format!("a channel list takes one channel per entry, but this has {}", sig.count()),
                    item.span,
                ));
            }
            match unit {
                None => unit = Some(sig.unit),
                Some(u) if u != sig.unit => {
                    return Err(Diagnostic::new(
                        format!(
                            "the entries of a channel list must have the same unit: this is {}, the first was {}",
                            sig.unit.describe(),
                            u.describe()
                        ),
                        item.span,
                    ));
                }
                Some(_) => {}
            }
            srcs.push(sig.srcs[0]);
        }
        if srcs.len() > niminal_engine::MAX_CHANNELS {
            return Err(Diagnostic::new("too many channels", span));
        }
        let layout = Layout::from_count(srcs.len());
        Ok(Sig { srcs, layout, unit: unit.unwrap_or(Unit::Num), konst: None })
    }

    fn name(&mut self, name: &str, span: Span) -> Res<Sig> {
        if let Some(s) = self.scope.get(name) {
            return Ok(s.clone());
        }
        match name {
            "out" => {
                return self.out.clone().ok_or_else(|| {
                    Diagnostic::new("`out` has no value yet", span).with_help("set it first, with `out = ...`")
                });
            }
            "it" => {
                return match self.mode {
                    Mode::Track => Ok(self.it()),
                    _ => Err(Diagnostic::new("`it` is only available inside a track", span)),
                };
            }
            _ => {}
        }
        if let Some(control) = self.names.controls.iter().find(|c| c.name == name) {
            if self.mode == Mode::Plain {
                return Err(Diagnostic::new("a live control cannot be used as a constant", span));
            }
            let src = self.g.add(niminal_engine::ops::Control::new(control.key.clone(), control.initial), &[])
                .map_err(|e| Diagnostic::new(e.to_string(), span))?;
            return Ok(Sig { srcs: vec![src], layout: Layout::Mono, unit: control.unit, konst: None });
        }
        if let Some(bus) = self.names.bus(name) {
            return match self.mode {
                Mode::Track => {
                    let layout = self.names.bus_layouts[bus];
                    Ok(self.input_bundle(name, layout, |channel| ChainInput::Bus { bus, channel }))
                }
                _ => Err(Diagnostic::new(format!("`{name}` is a bus, and only a track can read a bus"), span)
                    .with_help(format!("an instrument can send to it with `{name} += ...`"))),
            };
        }
        if let Ok(note @ Value::Note(_)) = name.parse::<Value>() {
            return Ok(Sig::constant(Unit::Hz, note.as_hz().expect("a note is a frequency")));
        }
        if opcodes::wave(name).is_some() {
            return Err(Diagnostic::new(format!("`{name}` is a waveform, not a value"), span)
                .with_help("waveforms go in the first slot of `osc`, as in `osc(saw, freq)`"));
        }
        Err(unknown_name(name, span, self.scope.keys().map(String::as_str)))
    }

    // ---- arithmetic ----------------------------------------------------

    /// Combine two signals channel by channel. A one-channel signal is applied
    /// to every channel of the other.
    fn elementwise(
        &mut self,
        a: &Sig,
        b: &Sig,
        unit: Unit,
        span: Span,
        make: fn(&mut GraphBuilder, Src, Src) -> Src,
    ) -> Res<Sig> {
        let n = match (a.count(), b.count()) {
            (x, y) if x == y => x,
            (1, y) => y,
            (x, 1) => x,
            (x, y) => {
                return Err(Diagnostic::new(
                    format!("can't combine a {x}-channel signal with a {y}-channel one"),
                    span,
                )
                .with_help("a one-channel signal can be combined with any; otherwise the counts must match"));
            }
        };
        let layout = if a.count() >= b.count() { a.layout } else { b.layout };
        let srcs = (0..n).map(|k| make(&mut self.g, a.pick(k), b.pick(k))).collect();
        Ok(Sig { srcs, layout, unit, konst: None })
    }

    fn mul(&mut self, a: &Sig, b: &Sig, unit: Unit, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(unit, x * y));
        }
        self.elementwise(a, b, unit, span, |g, x, y| g.add(Mul, &[("a", x), ("b", y)]).expect("mul ports"))
    }

    fn negate(&mut self, s: Sig, span: Span) -> Res<Sig> {
        match (s.unit, s.konst) {
            // A decibel value is held as a gain factor, so negating it inverts it.
            (Unit::Db, Some(k)) => Ok(Sig::constant(Unit::Db, 1.0 / k)),
            (Unit::Db, None) => Err(Diagnostic::new("negating a changing db value isn't supported yet", span)),
            (unit, _) => self.mul(&s, &Sig::constant(Unit::Num, -1.0), unit, span),
        }
    }

    fn binary(&mut self, op: BinOp, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            BinOp::Mul => "multiply",
            BinOp::Div => "divide",
        };
        let (lu, ru) = (l.unit, r.unit);
        let mismatch = || Diagnostic::new(format!("can't {verb} {} and {}", lu.describe(), ru.describe()), span);

        match op {
            BinOp::Add | BinOp::Sub => {
                // A frequency moved by an interval.
                if lu == Unit::Hz && ru == Unit::Semitones {
                    return self.transpose(&l, &r, op == BinOp::Sub, span);
                }
                if lu == Unit::Semitones && ru == Unit::Hz && op == BinOp::Add {
                    return self.transpose(&r, &l, false, span);
                }
                if lu != ru {
                    return Err(mismatch());
                }
                if lu == Unit::Db {
                    // Adding decibels multiplies gain factors.
                    let r = if op == BinOp::Sub { self.invert(&r, span)? } else { r };
                    return self.mul(&l, &r, Unit::Db, span);
                }
                if let (Some(x), Some(y)) = (l.konst, r.konst) {
                    let v = if op == BinOp::Add { x + y } else { x - y };
                    return Ok(Sig::constant(lu, v));
                }
                if op == BinOp::Add {
                    self.elementwise(&l, &r, lu, span, |g, x, y| g.add(Add, &[("a", x), ("b", y)]).expect("add ports"))
                } else {
                    self.elementwise(&l, &r, lu, span, |g, x, y| g.add(Sub, &[("a", x), ("b", y)]).expect("sub ports"))
                }
            }
            BinOp::Mul => match (lu, ru) {
                // Applying a gain to a plain value.
                (Unit::Num, Unit::Db) | (Unit::Db, Unit::Num) => self.mul(&l, &r, Unit::Num, span),
                (Unit::Db, _) | (_, Unit::Db) => Err(mismatch()),
                (Unit::Num, u) | (u, Unit::Num) => self.mul(&l, &r, u, span),
                _ => Err(mismatch()),
            },
            BinOp::Div => {
                let Some(k) = r.konst else {
                    return Err(Diagnostic::new("dividing by a changing signal isn't supported yet", span));
                };
                if k == 0.0 {
                    return Err(Diagnostic::new("division by zero", span));
                }
                let unit = match (lu, ru) {
                    (Unit::Db, _) | (_, Unit::Db) => return Err(mismatch()),
                    (u, Unit::Num) => u,
                    (a, b) if a == b => Unit::Num,
                    _ => return Err(mismatch()),
                };
                self.mul(&l, &Sig::constant(Unit::Num, 1.0 / k), unit, span)
            }
        }
    }

    /// `freq + 7st`: scale a frequency by 2^(st/12).
    fn transpose(&mut self, freq: &Sig, interval: &Sig, down: bool, span: Span) -> Res<Sig> {
        let direction = if down { -1.0 } else { 1.0 };
        if let Some(st) = interval.konst {
            let ratio = Sig::constant(Unit::Num, 2f64.powf(direction * st / 12.0));
            return self.mul(freq, &ratio, Unit::Hz, span);
        }
        // Changing intervals go through exp: 2^x = e^(x ln 2).
        let scale = Sig::constant(Unit::Num, direction * std::f64::consts::LN_2 / 12.0);
        let exponent = self.mul(interval, &scale, Unit::Num, span)?;
        let srcs = exponent
            .srcs
            .iter()
            .map(|&s| self.g.add(Func(Fn1::Exp), &[("x", s)]).expect("exp has one port"))
            .collect();
        let ratio = Sig { srcs, layout: exponent.layout, unit: Unit::Num, konst: None };
        self.mul(freq, &ratio, Unit::Hz, span)
    }

    fn invert(&mut self, s: &Sig, span: Span) -> Res<Sig> {
        match s.konst {
            Some(k) => Ok(Sig::constant(s.unit, 1.0 / k)),
            None => Err(Diagnostic::new("subtracting a changing db value isn't supported yet", span)),
        }
    }

    // ---- calls ----------------------------------------------------------

    fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Sig> {
        if name.name == "to" {
            return Err(Diagnostic::new("`to` can only end the line that sets a track's `out`", name.span)
                .with_help("for example `out = it.lpf(cutoff: 1khz).to(space)`"));
        }
        let registry = self.registry;
        let Some(spec) = registry.find(&name.name) else {
            let mut d = Diagnostic::new(format!("unknown opcode `{}`", name.name), name.span);
            if let Some(c) = closest(&name.name, registry.names()) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let op = spec.name.as_str();

        // Match arguments to parameters.
        let mut bound: Vec<Option<&Arg>> = vec![None; spec.params.len()];
        let mut next_positional = 0;
        for arg in args {
            let index = match &arg.name {
                None => {
                    let i = next_positional;
                    next_positional += 1;
                    if i >= spec.positional || i >= spec.params.len() {
                        let mut d = Diagnostic::new(format!("`{op}` takes its other arguments by name"), arg.value.span);
                        if let Some(p) = spec.params.get(i) {
                            d = d.with_help(format!("write `{}: ...`", p.name));
                        }
                        return Err(d);
                    }
                    i
                }
                Some(n) => match spec.params.iter().position(|p| p.name == n.name) {
                    Some(i) => i,
                    None => {
                        let mut d = Diagnostic::new(format!("`{op}` has no argument named `{}`", n.name), n.span);
                        if let Some(c) = closest(&n.name, spec.params.iter().map(|p| p.name.as_str())) {
                            d = d.with_help(format!("did you mean `{c}`?"));
                        }
                        return Err(d);
                    }
                },
            };
            if bound[index].is_some() {
                return Err(Diagnostic::new(
                    format!("`{op}` argument `{}` given more than once", spec.params[index].name),
                    arg.value.span,
                ));
            }
            bound[index] = Some(arg);
        }

        // Evaluate each argument according to what the parameter wants.
        let mut vals: Vec<Option<Val>> = Vec::with_capacity(spec.params.len());
        for (param, arg) in spec.params.iter().zip(bound) {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(format!("`{op}` needs an argument `{}`", param.name), span));
                }
                vals.push(None);
                continue;
            };
            vals.push(Some(match param.kind {
                Kind::Wave => {
                    let found = match &arg.value.kind {
                        ExprKind::Name(n) => opcodes::wave(n),
                        _ => None,
                    };
                    let Some(w) = found else {
                        let names: Vec<_> = opcodes::WAVES.iter().map(|(n, _)| *n).collect();
                        return Err(Diagnostic::new("expected a waveform", arg.value.span)
                            .with_help(format!("one of {}", names.join(", "))));
                    };
                    Val::Wave(w)
                }
                Kind::Layout => Val::Layout(layout_from_expr(&arg.value)?),
                Kind::Signal(expected) => {
                    let sig = self.expr(&arg.value)?;
                    if sig.unit != expected {
                        return Err(self.unit_mismatch(&param.name, expected, sig.unit, &arg.value));
                    }
                    Val::Sig(sig)
                }
                Kind::Gain => {
                    let sig = self.expr(&arg.value)?;
                    if !matches!(sig.unit, Unit::Db | Unit::Num) {
                        return Err(Diagnostic::new(
                            format!("`{}` expects db or a plain factor, found {}", param.name, sig.unit.describe()),
                            arg.value.span,
                        ));
                    }
                    Val::Sig(sig)
                }
            }));
        }

        match spec.build {
            Build::Invalid => Ok(Sig::constant(Unit::Num, 0.0)), // already reported
            Build::Pan => self.pan(spec, &vals, span),
            Build::ToLayout => {
                let (Some(Val::Sig(x)), Some(Val::Layout(layout))) = (&vals[0], &vals[1]) else {
                    unreachable!("to_layout has a signal and a layout")
                };
                self.convert(x, *layout, span)
            }
            _ => self.expand(spec, &vals, span),
        }
    }

    /// Build an opcode once per channel of its widest argument.
    fn expand(&mut self, spec: &opcodes::OpSpec, vals: &[Option<Val>], span: Span) -> Res<Sig> {
        let mut build = BuildArgs::default();
        let mut width = 1;
        let mut layout = Layout::Mono;
        for (param, val) in spec.params.iter().zip(vals) {
            match val {
                Some(Val::Wave(w)) => build.wave = Some(*w),
                Some(Val::Layout(_)) | None => {}
                Some(Val::Sig(sig)) => {
                    if let Some(k) = sig.konst {
                        build.consts.insert(param.name.clone(), k);
                    }
                    if param.wired && sig.count() > 1 {
                        if width > 1 && sig.count() != width {
                            return Err(Diagnostic::new(
                                format!(
                                    "`{}`'s arguments have {width} and {} channels, which can't be combined",
                                    spec.name,
                                    sig.count()
                                ),
                                span,
                            ));
                        }
                        width = sig.count();
                        layout = sig.layout;
                    }
                }
            }
        }

        let mut srcs = Vec::with_capacity(width);
        for channel in 0..width {
            build.channel = channel;
            let Some(opcode) = spec.instantiate(&build).map_err(|m| Diagnostic::new(m, span))? else {
                return Ok(Sig::constant(Unit::Num, 0.0));
            };
            let wired: Vec<(&str, Src)> = spec
                .params
                .iter()
                .zip(vals)
                .filter(|(p, _)| p.wired)
                .filter_map(|(p, v)| match v {
                    Some(Val::Sig(sig)) => Some((p.name.as_str(), sig.pick(channel))),
                    _ => None,
                })
                .collect();
            srcs.push(self.g.add(opcode, &wired).expect("ports match the opcode table"));
        }
        Ok(if width == 1 { Sig::signal(srcs[0], Unit::Num) } else { Sig::channels(srcs, layout) })
    }

    /// `pan`: place a mono signal in the master's layout.
    fn pan(&mut self, spec: &opcodes::OpSpec, vals: &[Option<Val>], span: Span) -> Res<Sig> {
        let mut wired: Vec<(&str, Src)> = Vec::new();
        for (param, val) in spec.params.iter().zip(vals) {
            let Some(Val::Sig(sig)) = val else { continue };
            if sig.count() != 1 {
                return Err(Diagnostic::new(
                    format!("`pan` takes a one-channel signal, but `{}` has {}", param.name, sig.count()),
                    span,
                )
                .with_help("pan each channel separately, or convert to mono first"));
            }
            wired.push((param.name.as_str(), sig.srcs[0]));
        }

        let master = self.names.master;
        let angles: Arc<[Option<f32>]> = master.speaker_angles().into();
        let srcs = (0..master.channels())
            .map(|k| {
                self.g
                    .add(PanChannel::new(angles.clone(), k), &wired)
                    .expect("ports match the opcode table")
            })
            .collect::<Vec<_>>();
        Ok(if srcs.len() == 1 { Sig::signal(srcs[0], Unit::Num) } else { Sig::channels(srcs, master) })
    }

    fn unit_mismatch(&self, param: &str, expected: Unit, found: Unit, arg: &Expr) -> Diagnostic {
        let mut d = Diagnostic::new(
            format!("`{param}` expects {}, found {}", expected.describe(), found.describe()),
            arg.span,
        );
        if found == Unit::Num {
            let literal = match &arg.kind {
                ExprKind::Num { value, .. } => Some(*value),
                ExprKind::Neg(inner) => match inner.kind {
                    ExprKind::Num { value, .. } => Some(-value),
                    _ => None,
                },
                _ => None,
            };
            d = match (literal, expected.suffix()) {
                (Some(v), Some(suffix)) => d.with_help(format!("did you mean `{v}{suffix}`?")),
                (None, Some(suffix)) => d.with_help(format!("multiply by a value with a unit, such as `* 1{suffix}`")),
                _ => d,
            };
        }
        d
    }

    // ---- envelopes -------------------------------------------------------

    fn env(&mut self, env: &EnvLit) -> Res<Sig> {
        let start = self.env_level(&env.start)?;
        let unit = start.unit;

        let mut segments = Vec::with_capacity(env.segments.len());
        for seg in &env.segments {
            let level = self.env_level(&seg.level)?;
            if level.unit != unit {
                return Err(Diagnostic::new(
                    format!(
                        "envelope levels must all have the same unit: this is {}, the first was {}",
                        level.unit.describe(),
                        unit.describe()
                    ),
                    seg.level.span,
                ));
            }
            let dur = self.literal(seg.dur.value, seg.dur.unit.as_deref(), seg.dur.span)?;
            let curve = match seg.curve {
                None => Curve::Linear,
                Some(CurveKind::Exp) => Curve::Exp,
                Some(CurveKind::Log) => Curve::Log,
                Some(CurveKind::Custom(k)) => Curve::Custom(k as f32),
            };
            segments.push(Segment::new(dur.konst.expect("a literal") as f32, level.konst.unwrap() as f32, curve));
        }

        let env = Env::new(start.konst.unwrap() as f32, segments, env.sustain_at);
        let src = self.g.add(env, &[]).expect("env has no ports");
        Ok(Sig::signal(src, unit))
    }

    fn env_level(&self, n: &EnvNum) -> Res<Sig> {
        let sig = self.literal(n.value, n.unit.as_deref(), n.span)?;
        if sig.unit == Unit::Db {
            return Err(Diagnostic::new("db levels in envelopes aren't supported yet", n.span));
        }
        Ok(sig)
    }
}
