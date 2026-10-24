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

use niminal_engine::ops::{
    Add, ArrayRead, Atan2, BlockHold, BufferRead, BufferRecord, BufferWrite, Cmp, Compare, Context,
    Curve, Custom, Delay1, Div, Env, EnvField, Fn1, Func, Hold, Interp, KitSampler, LoopMode, Max,
    Min, Mul, NoteStart, NoteTime, PanChannel, Pow, ReadContext, Released, Rem, SampleRate,
    Sampler, Segment, Select, Sub, TableRead,
};
use niminal_engine::{Array, Buffer, ChainInput, Edge, Graph, GraphBuilder, Route, Src, Table};
use niminal_score::{Tempo, Value};

use crate::ast::*;
use crate::call::{self, ArgVal};
use crate::diag::{Diagnostic, Span, closest};
use crate::funcs::{self, Kind as PKind, Param as PParam};
use crate::kernel::{
    Body, KArg, KernelResult, cmp_op, edge_of, ends_with_value, interp_of, lambda_of, range_numbers,
};
use crate::layout::{Layout, conversion};
use crate::opcodes::{self, Build, Kind, Registry};
use crate::program::MidiTarget;
use crate::program::{ControlDef, Instrument, Param};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;
pub(crate) type BufferChoices = Arc<Choices>;
#[derive(Debug)]
pub(crate) struct Choices {
    pub key: u64,
    values: Vec<(String, Arc<Buffer>)>,
}
impl Choices {
    pub(crate) fn named(values: Vec<(String, Arc<Buffer>)>, name: &str) -> Arc<Self> {
        Arc::new(Self {
            key: niminal_engine::instance_id(name),
            values,
        })
    }
    pub(crate) fn new(values: Vec<(String, Arc<Buffer>)>) -> Arc<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Arc::new(Self {
            key: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            values,
        })
    }
}
impl std::ops::Deref for Choices {
    type Target = [(String, Arc<Buffer>)];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
type BoundFnArgs = Vec<Option<(Datum, Expr)>>;

enum DspScope {
    Spawn {
        max: usize,
        trigger: Sig,
        duration: Sig,
    },
    Oversample(usize),
    /// `control { ... }`: runs once per block.
    Control,
}

const UNIT_NAMES: [&str; 13] = [
    "hz", "khz", "db", "sec", "ms", "beat", "beats", "bar", "bars", "bpm", "deg", "st", "ct",
];

/// `note.foo` or `clock.foo`, where `owner` has no `foo`.
fn no_field(owner: &str, field: &Ident, fields: &[&str]) -> Diagnostic {
    let list = fields
        .iter()
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut d = Diagnostic::new(format!("`{owner}` has no `{}`", field.name), field.span)
        .with_help(format!("it has {list}"));
    if let Some(c) = closest(&field.name, fields.iter().copied()) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

/// A literal's unit and value. Decibels become gain factors and beats become
/// seconds at `tempo`.
pub(crate) fn literal_sig(
    value: f64,
    unit: Option<&str>,
    tempo: Tempo,
    span: Span,
) -> Res<(Unit, f64)> {
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
        Some("ct") => (Unit::Semitones, value / 100.0),
        Some("bpm") => {
            return Err(Diagnostic::new("`bpm` can only be used with `tempo`", span)
                .with_help("for example `tempo 120bpm`"));
        }
        Some(u) => return Err(unknown_unit(u, span)),
    })
}

pub(crate) fn unknown_name<'a>(
    name: &str,
    span: Span,
    known: impl IntoIterator<Item = &'a str>,
) -> Diagnostic {
    let mut d = Diagnostic::new(format!("`{name}` is not defined"), span);
    if let Some(c) = closest(name, known) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

pub fn unknown_unit(unit: &str, span: Span) -> Diagnostic {
    let mut d = Diagnostic::new(format!("unknown unit `{unit}`"), span);
    if let Some(c) = closest(unit, UNIT_NAMES) {
        d = d.with_help(format!("did you mean `{c}`?"));
    }
    d
}

/// The control every instrument has, applied after its output.
pub const GAIN_PARAM: &str = "gain";

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
            [
                Arg {
                    name: None,
                    value:
                        Expr {
                            kind: ExprKind::Num { value, unit: None },
                            ..
                        },
                    ..
                },
            ] => {
                if (*value - 5.1).abs() < 1e-9 {
                    Ok(Layout::Surround51)
                } else if (*value - 7.1).abs() < 1e-9 {
                    Ok(Layout::Surround71)
                } else {
                    Err(bad(format!("unsupported surround layout `{value}`")))
                }
            }
            _ => Err(bad(
                "`surround` takes a layout such as `surround(5.1)`".into()
            )),
        },
        ExprKind::Call { name, args } if name.name == "channels" => match args.as_slice() {
            [
                Arg {
                    name: None,
                    value:
                        Expr {
                            kind: ExprKind::Num { value, unit: None },
                            ..
                        },
                    ..
                },
            ] if value.fract() == 0.0 && (1.0..=16.0).contains(value) => {
                Ok(Layout::from_count(*value as usize))
            }
            _ => Err(bad("`channels` takes a count from 1 to 16".into())),
        },
        _ => Err(bad("expected a channel layout".into())),
    }
}

/// Every top-level name, so locals can be kept from shadowing them, plus the
/// layouts that nodes and buses use.
pub(crate) struct Names {
    pub controls: Vec<ControlDef>,
    /// Functions, by name.
    pub fns: HashMap<String, FnDef>,
    /// Instruments, by name, so one can be played inside another.
    pub instrs: HashMap<String, InstrDef>,
    /// `const` definitions, worked out.
    pub consts: HashMap<String, Datum>,
    /// Functions whose definitions have mistakes, already reported: calls to them stay quiet.
    pub failed_fns: Vec<String>,
    entries: Vec<(String, &'static str)>,
    pub buses: Vec<String>,
    pub bus_layouts: Vec<Layout>,
    /// The layout of the master, and so of every voice and node.
    pub master: Layout,
    pub node_outputs: HashMap<String, Layout>,
    /// Where `file()` and sample paths are resolved from.
    pub samples: crate::Samples,
    /// Sample instruments' underlying tables, for `.data`.
    pub sample_data: HashMap<String, Vec<Arc<Table>>>,
    pub samplers: HashMap<String, SamplerDef>,
    /// Shared writable buffers, keyed by name with a stable index for graph deps.
    pub buffers: HashMap<String, (usize, Arc<niminal_engine::Buffer>)>,
    pub buffer_controls: HashMap<String, BufferChoices>,
}

impl Default for Names {
    fn default() -> Self {
        Names {
            controls: Vec::new(),
            fns: HashMap::new(),
            instrs: HashMap::new(),
            consts: HashMap::new(),
            failed_fns: Vec::new(),
            entries: Vec::new(),
            buses: Vec::new(),
            bus_layouts: Vec::new(),
            master: Layout::Mono,
            node_outputs: HashMap::new(),
            samples: crate::Samples::default(),
            sample_data: HashMap::new(),
            samplers: HashMap::new(),
            buffers: HashMap::new(),
            buffer_controls: HashMap::new(),
        }
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
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, k)| *k)
    }

    pub fn bus(&self, name: &str) -> Option<usize> {
        self.buses.iter().position(|b| b == name)
    }

    /// An error if a local may not be called `name`.
    fn check_local(&self, name: &Ident) -> Res<()> {
        self.check_local_in(name, false)
    }

    /// Like [`Names::check_local`]. Inside a function only the names its body could read are taken:
    /// buses, controls, functions and constants.
    pub(crate) fn check_local_in(&self, name: &Ident, in_fn: bool) -> Res<()> {
        if let Some(kind) = self.kind_of(&name.name)
            && (!in_fn || matches!(kind, "bus" | "control" | "function" | "constant"))
        {
            let article = if kind.starts_with(['a', 'e', 'i', 'o', 'u']) {
                "an"
            } else {
                "a"
            };
            return Err(Diagnostic::new(
                format!(
                    "`{}` is already the name of {article} {kind}, so it can't be used for a local",
                    name.name
                ),
                name.span,
            ));
        }
        if matches!(
            name.name.as_str(),
            "out"
                | "in"
                | "it"
                | "instrument"
                | "state"
                | "note"
                | "clock"
                | "true"
                | "false"
                | "pi"
                | "sample_rate"
                | "reset_on"
                | "spawn"
                | "oversample"
                | "control"
        ) {
            return Err(Diagnostic::new(
                format!("`{}` is a reserved word", name.name),
                name.span,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Evaluating constants such as parameter defaults.
    Plain,
    Instrument,
    Node,
}

/// A value in the graph: one source per channel.
#[derive(Clone)]
pub(crate) struct Sig {
    srcs: Array<Src>,
    layout: Layout,
    unit: Unit,
    /// Known at compile time (single channel only). Decibel values are held as
    /// linear gain factors.
    konst: Option<f64>,
}

impl Datum {
    fn array(items: Vec<Datum>) -> Self {
        Self::Array(items.into())
    }
}

impl Sig {
    fn constant(unit: Unit, v: f64) -> Sig {
        Sig {
            srcs: vec![Src::Const(v as f32)].into(),
            layout: Layout::Mono,
            unit,
            konst: Some(v),
        }
    }

    fn signal(src: Src, unit: Unit) -> Sig {
        Sig {
            srcs: vec![src].into(),
            layout: Layout::Mono,
            unit,
            konst: None,
        }
    }

    /// A plain signal with one source per channel.
    fn channels(srcs: impl Into<Array<Src>>, layout: Layout) -> Sig {
        Sig {
            srcs: srcs.into(),
            layout,
            unit: Unit::Num,
            konst: None,
        }
    }

    fn count(&self) -> usize {
        self.srcs.len()
    }

    /// The value, if it is known while compiling.
    pub(crate) fn konst(&self) -> Option<f64> {
        self.konst
    }

    pub(crate) fn unit(&self) -> Unit {
        self.unit
    }

    /// The source for channel `k`; a one-channel signal feeds every channel.
    fn pick(&self, k: usize) -> Src {
        if self.srcs.len() == 1 {
            self.srcs[0]
        } else {
            self.srcs[k]
        }
    }
}

/// Arrays are general values. Signals specialize arrays with a rate, unit and channel layout;
/// conversion to an audio layout happens where audio is consumed.
#[derive(Clone)]
pub(crate) enum Datum {
    Function {
        def: FnDef,
        captures: HashMap<String, Datum>,
    },
    Sig(Sig),
    Array(Array<Datum>),
    /// A compile-time numeric table (shared storage).
    Table(Arc<Table>),
    /// A shared writable buffer declared at the top level (`id` indexes Program.buffers).
    Buffer {
        id: usize,
        data: Arc<Buffer>,
    },
    BufferRef {
        selector: Sig,
        choices: BufferChoices,
    },
    Record(Vec<(String, Datum)>),
    /// `lo..hi`, or `lo..<hi`.
    Range {
        lo: Sig,
        hi: Sig,
        inclusive: bool,
    },
}

/// A call's evaluated argument.
type Val = ArgVal<Sig>;

pub(crate) struct Lower<'a> {
    registry: &'a Registry,
    names: &'a Names,
    mode: Mode,
    g: GraphBuilder,
    tempo: Tempo,
    scope: HashMap<String, Datum>,
    /// The value of `out` so far.
    out: Option<Sig>,
    sends: usize,
    kit: Option<(Vec<String>, Vec<Option<u64>>)>,
    /// Set while lowering a bus send. `Some(true)` means `prev` marked it one block late.
    in_send: Option<bool>,
    /// In a node: what feeds each external input, and the bundles made so far.
    chain_inputs: Vec<ChainInput>,
    input_sigs: HashMap<String, Sig>,
    route: Route,
    /// In a node: the signals sent over OSC.
    taps: Vec<crate::program::OscTap>,
    /// The rate of each value bound in the instrument's own body.
    rate_hints: Vec<crate::program::RateHint>,
    /// The rate of each node result that is slower than audio.
    rates: HashMap<usize, Rate>,
    /// The rate of each graph parameter, by index.
    param_rates: Vec<Rate>,
    /// Functions being lowered, outermost first, to catch recursion.
    fn_stack: Vec<String>,
    /// Set while lowering a function's body, which can't send to a bus or read `note`.
    in_fn: bool,
    /// While inside an `if` that changes while playing: 1 where the branch is taken. Writes to shared
    /// buffers only happen then.
    write_gate: Option<Src>,
}

pub(crate) fn compile_control(
    name: &Ident,
    value: &Expr,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<ControlDef> {
    if matches!(
        name.name.as_str(),
        "out" | "in" | "it" | "instrument" | "state" | "ctl"
    ) {
        return Err(Diagnostic::new(
            format!("`{}` is a reserved word", name.name),
            name.span,
        ));
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
            return Err(Diagnostic::new(
                "a control's .smooth takes one duration",
                value.span,
            ));
        };
        if !receiver.receiver || duration.name.is_some() {
            return Err(Diagnostic::new(
                "write ctl name = value.smooth(5ms)",
                value.span,
            ));
        }
        initial = receiver.value.clone();
        if negative {
            initial = Expr {
                kind: ExprKind::Neg(Box::new(initial)),
                span: value.span,
            };
        }
        let smooth = lower.expr(&duration.value)?;
        if smooth.unit != Unit::Time {
            return Err(Diagnostic::new(
                "control smoothing needs a duration, such as 5ms",
                duration.value.span,
            ));
        }
        smooth_seconds = smooth
            .konst
            .ok_or_else(|| Diagnostic::new("smoothing must be constant", duration.value.span))?;
        if !smooth_seconds.is_finite() || !(0.0..=60.0).contains(&smooth_seconds) {
            return Err(Diagnostic::new(
                "control smoothing must be between 0ms and 60sec",
                duration.value.span,
            ));
        }
    }
    let sig = lower.expr(&initial)?;
    let value = sig.konst.ok_or_else(|| {
        Diagnostic::new("a control's initial value must be constant", initial.span)
    })? as f32;
    if !value.is_finite() || (sig.unit == Unit::Db && value <= 0.0) {
        return Err(Diagnostic::new(
            "control values must be finite and representable",
            initial.span,
        ));
    }
    Ok(ControlDef {
        range: None,
        name: name.name.clone(),
        key: format!("ctl:{}:{:?}", name.name, sig.unit),
        unit: sig.unit,
        initial: value,
        smooth_seconds,
    })
}

pub(crate) fn compile_instr(
    def: &InstrDef,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Instrument> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut params: Vec<Param> = Vec::new();
    let mut arrays = std::collections::BTreeMap::new();
    let mut resources = std::collections::BTreeMap::new();
    let mut resource_keys = std::collections::BTreeMap::new();

    let signature = funcs::signature(&def.params, tempo)?;
    for p in &signature {
        names.check_local(&p.name)?;
        if p.kind == PKind::Buffer && p.rate != Some(Rate::Const) {
            let default = p.default.as_ref().ok_or_else(|| {
                Diagnostic::new("a buffer parameter needs a default resource", p.name.span)
            })?;
            let Datum::Buffer { data, .. } = lower.value(default)? else {
                return Err(Diagnostic::new(
                    "buffer default is a named shared buffer",
                    default.span,
                ));
            };
            let resource = names
                .buffers
                .iter()
                .find(|(_, (_, b))| Arc::ptr_eq(b, &data))
                .map(|(n, _)| n.as_str())
                .unwrap();
            let owner = resource.split_once("::").map(|(o, _)| o).unwrap_or("");
            let mut choices: Vec<_> = names
                .buffers
                .iter()
                .filter(|(n, _)| n.split_once("::").map(|(o, _)| o).unwrap_or("") == owner)
                .map(|(n, (id, b))| (*id, n.clone(), b.clone()))
                .collect();
            choices.sort_by_key(|v| v.0);
            let choices: BufferChoices = Choices::named(
                choices.into_iter().map(|(_, n, b)| (n, b)).collect(),
                &format!("{}::{}", def.name.name, p.name.name),
            );
            let index = choices
                .iter()
                .position(|(_, b)| Arc::ptr_eq(b, &data))
                .unwrap();
            let src = lower
                .g
                .param(&p.name.name, index as f32)
                .expect("unique parameter");
            lower.param_rates.push(Rate::Audio);
            params.push(Param {
                name: p.name.name.clone(),
                unit: Unit::Num,
                range: Some((0.0, (choices.len() - 1) as f64)),
                default: Some(index as f32),
            });
            resource_keys.insert(p.name.name.clone(), choices.key);
            resources.insert(
                p.name.name.clone(),
                choices.iter().map(|(n, _)| n.clone()).collect(),
            );
            let selector = lower.at_rate(Sig::signal(src, Unit::Num), p.rate);
            lower
                .scope
                .insert(p.name.name.clone(), Datum::BufferRef { selector, choices });
            continue;
        }
        if matches!(p.kind, PKind::Array(_)) && p.rate != Some(Rate::Const) {
            let default = p.default.as_ref().ok_or_else(|| {
                Diagnostic::new(
                    "a live array parameter needs a default to fix its shape",
                    p.name.span,
                )
            })?;
            let value = lower.value(default)?;
            let value = lower.check_param(&def.name.name, p, value, default)?;
            let start = params.len();
            let value = lower.live_array(
                value,
                &p.name.name,
                &p.kind,
                p.rate,
                &mut params,
                default.span,
            )?;
            arrays.insert(
                p.name.name.clone(),
                params[start..].iter().map(|p| p.name.clone()).collect(),
            );
            lower.scope.insert(p.name.name.clone(), value);
            continue;
        }
        // Structural arguments specialize the graph. Notes only see the remaining scalar ports.
        if p.rate == Some(Rate::Const)
            || matches!(p.kind, PKind::Choice(_) | PKind::Function | PKind::Array(_))
        {
            let default = p.default.as_ref().ok_or_else(|| {
                Diagnostic::new(
                    format!("structural parameter `{}` needs a default", p.name.name),
                    p.name.span,
                )
                .with_help("override it by calling this instrument inside another instrument")
            })?;
            let value = if let PKind::Choice(options) = &p.kind {
                let ExprKind::Name(n) = &default.kind else {
                    unreachable!("signature checked")
                };
                for (i, option) in options.iter().enumerate() {
                    lower.scope.insert(
                        option.clone(),
                        Datum::Sig(Sig::constant(Unit::Num, i as f64)),
                    );
                }
                Datum::Sig(Sig::constant(
                    Unit::Num,
                    options.iter().position(|o| o == n).unwrap() as f64,
                ))
            } else {
                lower.value(default)?
            };
            let mut structural = p.clone();
            structural.rate = Some(Rate::Const);
            let value = lower.check_param(&def.name.name, &structural, value, default)?;
            lower.scope.insert(p.name.name.clone(), value);
            continue;
        }
        let (unit, range) = match &p.kind {
            PKind::Unit { unit, .. } => (*unit, None),
            PKind::Range { unit, lo, hi } => (*unit, Some((*lo, *hi))),
            PKind::Any => match &p.default {
                Some(e) => (lower.expr(e)?.unit, None),
                None if p.name.name == "sample" => (Unit::Num, None),
                None => {
                    return Err(Diagnostic::new(
                        format!("parameter `{}` needs a type", p.name.name),
                        p.name.span,
                    )
                    .with_help(format!("for example `{}: hz`", p.name.name)));
                }
            },
            PKind::Buffer | PKind::Choice(_) | PKind::Array(_) | PKind::Function => {
                unreachable!("structural arguments handled above")
            }
        };
        let default = match &p.default {
            None => None,
            Some(e) => Some(lower.param_default(e, unit, range, &p.name.name)?),
        };
        let src = lower
            .g
            .param(&p.name.name, default.unwrap_or(0.0))
            .expect("duplicates checked above");
        lower
            .g
            .set_param_range(&p.name.name, range.map(|(lo, hi)| (lo as f32, hi as f32)));
        lower.param_rates.push(Rate::Audio);
        let sig = lower.at_rate(Sig::signal(src, unit), p.rate);
        lower.scope.insert(p.name.name.clone(), Datum::Sig(sig));
        params.push(Param {
            name: p.name.name.clone(),
            unit,
            range,
            default,
        });
    }
    if let Some(tail) = def
        .options
        .iter()
        .find(|o| o.name.as_ref().is_some_and(|n| n.name == "tail"))
    {
        let t = lower.expr(&tail.value)?;
        match (t.unit, t.konst) {
            (Unit::Time, Some(seconds)) if (0.0..=600.0).contains(&seconds) => {
                lower.g.set_tail(seconds as f32)
            }
            _ => {
                return Err(Diagnostic::new(
                    "`tail` is a time from 0 to 600sec, such as `2sec`",
                    tail.value.span,
                ));
            }
        }
    }

    // Every note accepts a `gain` control, applied after the instrument, unless the instrument takes it
    // over by declaring its own.
    let own_gain = params.iter().find(|p| p.name == GAIN_PARAM);
    if let Some(p) = own_gain
        && p.unit != Unit::Db
    {
        let span = def
            .params
            .iter()
            .find(|q| q.name.name == GAIN_PARAM)
            .map_or(def.name.span, |q| q.name.span);
        return Err(Diagnostic::new(
            format!("an instrument's own `{GAIN_PARAM}` is in db, like the one every note has"),
            span,
        )
        .with_help(format!("`{GAIN_PARAM}: db = 0db`")));
    }
    let gain = if own_gain.is_none() {
        let gain = lower.g.param(GAIN_PARAM, 1.0).expect("name checked above");
        lower.param_rates.push(Rate::Audio);
        params.push(Param {
            name: GAIN_PARAM.to_string(),
            unit: Unit::Db,
            range: None,
            default: Some(1.0),
        });
        Some(gain)
    } else {
        None
    };

    if def.body.is_empty() {
        return Err(Diagnostic::new(
            format!("instrument `{}` is empty", def.name.name),
            def.name.span,
        )
        .with_help("the last line of an instrument is the signal it outputs"));
    }
    let body: Vec<&Stmt> = def.body.iter().collect();
    let tail = lower.run_body(&body)?;

    let (result, span) = match (lower.out.take(), tail) {
        (Some(_), Some((_, span))) => {
            return Err(Diagnostic::new(
                "this instrument sets `out`, so its last line can't also be a value",
                span,
            )
            .with_help("assign it to `out`, or remove the `out` lines"));
        }
        (Some(out), None) => (out, def.name.span),
        (None, Some((sig, span))) => {
            if sig.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "an instrument outputs a plain signal, but this is {}",
                        sig.unit.describe()
                    ),
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
            return Err(Diagnostic::new(
                "an instrument must end with the signal it outputs",
                name.span,
            )
            .with_help(format!("add a last line such as `{}`", name.name)));
        }
    };

    // Every voice leaves the instrument in the master's layout.
    let result = lower.convert(&result, names.master, span)?;
    let result = match gain {
        Some(gain) => lower.mul(&result, &Sig::signal(gain, Unit::Num), Unit::Num, span)?,
        None => result,
    };
    let mut instrument = Instrument::new(
        def.name.name.clone(),
        Arc::new(lower.g.build_channels(&result.srcs)),
        params,
    );
    if let Some((members, chokes)) = lower.kit {
        instrument.members = members;
        instrument.chokes = chokes;
    }
    instrument.taps = lower.taps;
    instrument.rate_hints = lower.rate_hints;
    instrument.arrays = arrays;
    instrument.resources = resources;
    instrument.resource_keys = resource_keys;
    Ok(instrument)
}

/// One sample of a sampler instrument.
pub(crate) struct Member {
    pub name: String,
    /// One table per channel.
    pub channels: Vec<Arc<Table>>,
    /// The frames of the tables it plays, if not all of them.
    pub slice: Option<(usize, usize)>,
    /// Playback speed that fits a loop to the tempo; 1 for none.
    pub speed: f64,
    pub loop_mode: LoopMode,
    pub interp: niminal_engine::ops::SampleInterp,
}

/// An instrument that plays recorded audio, selecting a kit member once per note.
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
        params.push(Param {
            name: pname.to_string(),
            unit,
            range: None,
            default: Some(default),
        });
        lower
            .g
            .param(pname, default)
            .expect("sampler parameter names are distinct")
    };
    let freq = declare(&mut lower, "freq", Unit::Hz, root_hz);
    let pitch = declare(&mut lower, "pitch", Unit::Semitones, 0.0);
    let rate = declare(&mut lower, "rate", Unit::Num, 1.0);
    let stretch = declare(&mut lower, "stretch", Unit::Num, 1.0);
    let stretch_mode = declare(&mut lower, "stretch_mode", Unit::Num, 0.0);
    let loop_start = declare(&mut lower, "loop_start", Unit::Num, 0.0);
    let loop_end = declare(&mut lower, "loop_end", Unit::Num, 1.0);
    let start = declare(&mut lower, "start", Unit::Time, 0.0);
    // where in the sample to start, as a share of its length (a note's `begin`, set by `chop`)
    let begin = declare(&mut lower, "begin", Unit::Num, 0.0);
    let pick = is_kit.then(|| declare(&mut lower, "sample", Unit::Num, 0.0));
    let gain = declare(&mut lower, GAIN_PARAM, Unit::Db, 1.0);

    let mut wires = vec![
        ("freq", freq),
        ("pitch", pitch),
        ("rate", rate),
        ("stretch", stretch),
        ("stretch_mode", stretch_mode),
        ("loop_start", loop_start),
        ("loop_end", loop_end),
        ("start", start),
        ("begin", begin),
    ];
    wires.extend(pick.map(|p| ("sample", p)));
    let result = lower.sampler_signal(name, members, root_hz, &wires)?;
    let result = lower.mul(&result, &Sig::signal(gain, Unit::Num), Unit::Num, name.span)?;
    let mut instrument = Instrument::new(
        name.name.clone(),
        Arc::new(lower.g.build_channels(&result.srcs)),
        params,
    );
    if is_kit {
        instrument.members = members.iter().map(|m| m.name.clone()).collect();
    }
    Ok(instrument)
}

/// Loaded sampler data reused when a sampler is called inside an instrument.
pub(crate) struct SamplerDef {
    pub members: Vec<Member>,
    pub root_hz: f32,
    pub instrument: Instrument,
}

impl Lower<'_> {
    fn sampler_signal(
        &mut self,
        name: &Ident,
        members: &[Member],
        root_hz: f32,
        wires: &[(&str, Src)],
    ) -> Res<Sig> {
        let too_many = |n: usize| {
            Diagnostic::new(
                format!(
                    "a sample with {n} channels is more than the {MAX_SAMPLE_CHANNELS} niminal supports"
                ),
                name.span,
            )
        };
        // Members with the same channel layout share one selectable player per channel. A note
        // chooses its member once; it does not run and sum every other member's silent graph.
        let mut layouts = std::collections::BTreeMap::<usize, Vec<usize>>::new();
        for (i, member) in members.iter().enumerate() {
            let channels = member.channels.len();
            if channels > MAX_SAMPLE_CHANNELS {
                return Err(too_many(channels));
            }
            layouts.entry(channels).or_default().push(i);
        }
        // One reference per member, shared by all its playback channels. Pick the
        // loudest channel rather than mixing: anti-phase stereo must not cancel.
        let references: Vec<_> = members
            .iter()
            .map(|member| {
                member
                    .channels
                    .iter()
                    .map(|channel| {
                        let energy = channel
                            .data()
                            .iter()
                            .map(|x| f64::from(*x).powi(2))
                            .sum::<f64>();
                        (energy, channel)
                    })
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, channel)| channel.clone())
            })
            .collect();
        let transients: Vec<Arc<[usize]>> = references
            .iter()
            .map(|reference| {
                reference.as_ref().map_or_else(
                    || Arc::from([]),
                    |table| {
                        niminal_engine::ops::detect_transients(
                            table.data(),
                            table.sample_rate_or(48_000.0),
                            usize::MAX,
                        )
                        .into()
                    },
                )
            })
            .collect();
        let mut mix: Option<Sig> = None;
        for (channels, indices) in layouts {
            let mut srcs = Vec::with_capacity(channels);
            for channel in 0..channels {
                let mut players = vec![None; members.len()];
                for &i in &indices {
                    let member = &members[i];
                    let mut player = Sampler::new(member.channels[channel].clone(), root_hz, None)
                        .with_stretch_reference(
                            references[i].as_ref().expect("sample has channels").clone(),
                        )
                        .with_transients(transients[i].clone())
                        .with_speed(member.speed)
                        .with_loop(member.loop_mode)
                        .with_interp(member.interp);
                    if let Some((start, end)) = member.slice {
                        player = player.slice(start, end);
                    }
                    players[i] = Some(player);
                }
                srcs.push(
                    self.g
                        .add(KitSampler::new(players), wires)
                        .map_err(|e| Diagnostic::new(e.to_string(), name.span))?,
                );
            }
            let sig = self.convert(
                &Sig::channels(srcs, Layout::from_count(channels)),
                self.names.master,
                name.span,
            )?;
            mix = Some(match mix {
                None => sig,
                Some(so_far) => self.binary(BinOp::Add, so_far, sig, name.span)?,
            });
        }
        Ok(mix.expect("a sample or kit has at least one sample"))
    }

    fn call_sampler(&mut self, sampler: &SamplerDef, name: &Ident, args: &[Arg]) -> Res<Datum> {
        if self.mode != Mode::Instrument || self.in_fn {
            return Err(Diagnostic::new(
                "a sampler can only be played inside an instrument",
                name.span,
            ));
        }
        let mut values = HashMap::new();
        for arg in args {
            let Some(key) = &arg.name else {
                return Err(Diagnostic::new(
                    "name the sampler arguments, as in `freq: freq`",
                    arg.value.span,
                ));
            };
            let Some(param) = sampler
                .instrument
                .params
                .iter()
                .find(|p| p.name == key.name)
            else {
                return Err(Diagnostic::new(
                    format!("`{}` has no parameter `{}`", name.name, key.name),
                    key.span,
                ));
            };
            let value = if key.name == "sample"
                && let ExprKind::Name(member) = &arg.value.kind
                && !self.scope.contains_key(member)
                && let Some(index) = sampler.instrument.members.iter().position(|m| m == member)
            {
                Sig::constant(Unit::Num, index as f64)
            } else {
                self.expr(&arg.value)?
            };
            if value.count() != 1 {
                return Err(Diagnostic::new(
                    format!("`{}` expects one value", key.name),
                    arg.value.span,
                ));
            }
            if value.unit != param.unit && !(key.name == GAIN_PARAM && value.unit == Unit::Num) {
                return Err(self.unit_mismatch(&key.name, param.unit, value.unit, &arg.value));
            }
            if key.name == "sample"
                && let Some(Datum::Sig(pick)) = self.scope.get("sample")
                && value.srcs == pick.srcs
            {
                let kit = (
                    sampler.instrument.members.clone(),
                    sampler.instrument.chokes.clone(),
                );
                if self.kit.as_ref().is_some_and(|previous| previous != &kit) {
                    return Err(Diagnostic::new(
                        "a forwarded `sample` parameter must select from the same kit",
                        arg.value.span,
                    ));
                }
                self.kit = Some(kit);
            }
            if values.insert(key.name.as_str(), value).is_some() {
                return Err(Diagnostic::new(
                    format!("`{}` is given twice", key.name),
                    key.span,
                ));
            }
        }
        let mut wires = Vec::new();
        let mut gain = Sig::constant(Unit::Num, 1.0);
        for param in &sampler.instrument.params {
            let value = values.remove(param.name.as_str()).unwrap_or_else(|| {
                Sig::constant(param.unit, f64::from(param.default.unwrap_or(0.0)))
            });
            if param.name == GAIN_PARAM {
                gain = value;
            } else {
                wires.push((param.name.as_str(), value.srcs[0]));
            }
        }
        let signal = self.sampler_signal(name, &sampler.members, sampler.root_hz, &wires)?;
        self.mul(&signal, &gain, Unit::Num, name.span)
            .map(Datum::Sig)
    }
}

/// `midi_out drums(port: "IAC Driver", channel: 10)`: an instrument whose notes go to external gear.
/// It is silent here; a note's `freq` is the key that is pressed and its `gain` the velocity.
pub(crate) fn compile_midi_out(decl: &MidiOutDecl, names: &Names) -> Res<Instrument> {
    let (mut port, mut channel) = (None, 1u8);
    for arg in &decl.args {
        let Some(option) = &arg.name else {
            return Err(Diagnostic::new(
                "an argument is a name and a value, as in `port: \"IAC Driver\"`",
                arg.value.span,
            ));
        };
        match option.name.as_str() {
            "port" => match &arg.value.kind {
                ExprKind::Str(text) if !text.trim().is_empty() => port = Some(text.clone()),
                _ => {
                    return Err(Diagnostic::new(
                        "the port is the name of a MIDI output, in quotes",
                        arg.value.span,
                    ));
                }
            },
            "channel" => match &arg.value.kind {
                ExprKind::Num { value, unit: None }
                    if value.fract() == 0.0 && (1.0..=16.0).contains(value) =>
                {
                    channel = *value as u8
                }
                _ => {
                    return Err(Diagnostic::new(
                        "a MIDI channel is a whole number from 1 to 16",
                        arg.value.span,
                    ));
                }
            },
            other => {
                let mut d =
                    Diagnostic::new(format!("`midi_out` has no argument `{other}`"), option.span);
                if let Some(c) = closest(other, ["port", "channel"]) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
            }
        }
    }
    let Some(port) = port else {
        return Err(Diagnostic::new(
            format!("`{}` needs a `port:`", decl.name.name),
            decl.name.span,
        )
        .with_help("for example `midi_out drums(port: \"IAC Driver\", channel: 10)`"));
    };
    let mut g = GraphBuilder::new();
    g.param("freq", 440.0).expect("a fresh graph");
    g.param(GAIN_PARAM, 1.0).expect("a fresh graph");
    let silence = vec![Src::Const(0.0); names.master.channels()];
    let mut instrument = Instrument::new(
        decl.name.name.clone(),
        Arc::new(g.build_channels(&silence)),
        vec![
            Param {
                name: "freq".into(),
                unit: Unit::Hz,
                range: None,
                default: None,
            },
            Param {
                name: GAIN_PARAM.into(),
                unit: Unit::Db,
                range: None,
                default: Some(1.0),
            },
        ],
    );
    instrument.midi = Some(MidiTarget {
        port: port.into(),
        channel,
    });
    Ok(instrument)
}

const MAX_SAMPLE_CHANNELS: usize = 16;

/// Envelope curve written in source → engine curve.
pub(crate) fn curve_of(kind: Option<CurveKind>) -> Curve {
    match kind {
        None => Curve::Linear,
        Some(CurveKind::Exp) => Curve::Exp,
        Some(CurveKind::Log) => Curve::Log,
        Some(CurveKind::Custom(k)) => Curve::Custom(k as f32),
    }
}

/// A compiled node: its processing, what feeds that processing, and where
/// the result goes.
pub(crate) struct Chain {
    pub graph: Option<Arc<Graph>>,
    pub inputs: Vec<ChainInput>,
    pub route: Route,
    pub taps: Vec<crate::program::OscTap>,
}

/// A node's mixer lines: `gain = -3db` and `pan = 20deg`, applied to what the node puts out.
#[derive(Default)]
pub(crate) struct Mixer<'a> {
    pub gain: Option<&'a Expr>,
    pub pan: Option<&'a Expr>,
}

/// Compile a node's statements (everything except its `instrument` line).
pub(crate) fn compile_chain(
    node: &Ident,
    body: &[&Stmt],
    has_instrument: bool,
    mixer: &Mixer,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Chain> {
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Node;

    let mixed = mixer.gain.is_some() || mixer.pan.is_some();
    if body.is_empty() && !mixed {
        return if has_instrument {
            Ok(Chain {
                graph: None,
                inputs: Vec::new(),
                route: Route::Master,
                taps: Vec::new(),
            })
        } else {
            Err(
                Diagnostic::new(format!("node `{}` does nothing", node.name), node.span)
                    .with_help("give it an `instrument = ...`, or an `out = ...` that reads a bus"),
            )
        };
    }
    lower.run_body(body)?;

    let out = match lower.out.take() {
        Some(out) => out,
        None if has_instrument => lower.node_input(),
        None if lower.sends > 0 => Sig::constant(Unit::Num, 0.0),
        None => {
            return Err(Diagnostic::new(
                format!("node `{}` never sets `out`", node.name),
                node.span,
            )
            .with_help(
                "add `out = it` to pass its sound through, or send to a bus with `bus += ...`",
            ));
        }
    };
    // The mixer's lines come last: the fader, then the placement.
    let mut out = out;
    if mixed {
        lower.out = Some(out);
        let at = |kind: ExprKind, span: Span| Expr { kind, span };
        let call = |name: &str, args: Vec<Arg>, span: Span| {
            at(
                ExprKind::Call {
                    name: Ident {
                        name: name.into(),
                        span,
                    },
                    args,
                },
                span,
            )
        };
        let positional = |value: Expr| Arg {
            name: None,
            value,
            receiver: false,
        };
        let receiver = |value: Expr| Arg {
            name: None,
            value,
            receiver: true,
        };
        let mut signal = at(ExprKind::Name("out".into()), node.span);
        if let Some(g) = mixer.gain {
            signal = call(
                "gain",
                vec![receiver(signal), positional(g.clone())],
                g.span,
            );
        }
        if let Some(p) = mixer.pan {
            // a pan places one signal, so a wider node is mixed down first, as loud as one of its channels
            let mono = if names.master.channels() > 1 {
                let to_mono = call(
                    "to_layout",
                    vec![
                        receiver(signal),
                        positional(at(ExprKind::Name("mono".into()), p.span)),
                    ],
                    p.span,
                );
                let level = at(
                    ExprKind::Num {
                        value: (names.master.channels() as f64).sqrt(),
                        unit: None,
                    },
                    p.span,
                );
                call("gain", vec![receiver(to_mono), positional(level)], p.span)
            } else {
                signal
            };
            let azimuth = Arg {
                name: Some(Ident {
                    name: "azimuth".into(),
                    span: p.span,
                }),
                value: p.clone(),
                receiver: false,
            };
            signal = call("pan", vec![receiver(mono), azimuth], p.span);
        }
        out = lower.expr(&signal)?;
    }
    // The chain delivers in its destination's layout.
    let destination = match lower.route {
        Route::Master => names
            .node_outputs
            .get(&node.name)
            .copied()
            .unwrap_or(names.master),
        Route::Bus(b) => names.bus_layouts[b],
    };
    let out = lower.convert(&out, destination, node.span)?;
    Ok(Chain {
        graph: Some(Arc::new(lower.g.build_channels(&out.srcs))),
        inputs: lower.chain_inputs,
        route: lower.route,
        taps: lower.taps,
    })
}

/// The span of the first argument that is not the receiver, for a problem with its value.
/// Entries of one channel each, joined into a list. They share a unit, so `[800hz, 1200hz]` is a two-channel frequency.
fn join_channels(entries: &[(Sig, Span)], span: Span) -> Res<Sig> {
    if entries.is_empty() {
        return Err(Diagnostic::new(
            "audio output needs at least one channel",
            span,
        ));
    }
    let mut srcs = Vec::with_capacity(entries.len());
    let mut unit = None;
    for (sig, at) in entries {
        if sig.count() != 1 {
            return Err(Diagnostic::new(
                format!(
                    "a channel array takes one channel per entry, but this has {}",
                    sig.count()
                ),
                *at,
            ));
        }
        match unit {
            None => unit = Some(sig.unit),
            Some(u) if u != sig.unit => {
                return Err(Diagnostic::new(
                    format!(
                        "the entries of a channel array must have the same unit: this is {}, the first was {}",
                        sig.unit.describe(),
                        u.describe()
                    ),
                    *at,
                ));
            }
            Some(_) => {}
        }
        srcs.push(sig.srcs[0]);
    }
    if srcs.len() > niminal_engine::MAX_CHANNELS {
        return Err(Diagnostic::new(
            format!(
                "audio output supports at most {} channels",
                niminal_engine::MAX_CHANNELS
            ),
            span,
        ));
    }
    let layout = Layout::from_count(srcs.len());
    Ok(Sig {
        srcs: srcs.into(),
        layout,
        unit: unit.unwrap_or(Unit::Num),
        konst: None,
    })
}

fn address_span(args: &[Arg]) -> Span {
    args.iter()
        .find(|a| !a.receiver && a.name.is_none())
        .map_or(Span::new(0, 0), |a| a.value.span)
}

fn state_outside_fn(name: &Ident) -> Diagnostic {
    Diagnostic::new("`state` can only be used inside a `fn`", name.span)
        .with_help("an instrument's filters and delays keep their own state; write a `fn` for state of your own")
}

/// Whether `e` reads `prev(name)`.
fn reads_prev_of(e: &Expr, name: &str) -> bool {
    if let ExprKind::Call { name: callee, args } = &e.kind
        && callee.name == "prev"
        && matches!(args.as_slice(), [a] if matches!(&a.value.kind, ExprKind::Name(n) if n == name))
    {
        return true;
    }
    let mut found = false;
    e.for_each_child(&mut |c| found = found || reads_prev_of(c, name));
    found
}

/// The names `e` reads, in order, once each.
fn names_read(e: &Expr, out: &mut Vec<String>) {
    if let ExprKind::Name(n) = &e.kind
        && !out.contains(n)
    {
        out.push(n.clone());
    }
    e.for_each_child(&mut |c| names_read(c, out));
}

/// Whether two values are the same, for names a branch didn't change.
fn same_datum(a: &Datum, b: &Datum) -> bool {
    match (a, b) {
        (Datum::Sig(x), Datum::Sig(y)) => x.srcs == y.srcs && x.unit == y.unit,
        (Datum::Record(x), Datum::Record(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((xn, xv), (yn, yv))| xn == yn && same_datum(xv, yv))
        }
        (Datum::Array(x), Datum::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_datum(p, q))
        }
        (Datum::Table(a), Datum::Table(b)) => Arc::ptr_eq(a, b) || a == b,
        (Datum::Buffer { data: a, .. }, Datum::Buffer { data: b, .. }) => Arc::ptr_eq(a, b),
        (Datum::Range { lo: a, hi: b, .. }, Datum::Range { lo: c, hi: d, .. }) => {
            a.srcs == c.srcs && b.srcs == d.srcs
        }
        _ => false,
    }
}

fn table_entries(table: &Table) -> Vec<Datum> {
    table
        .data()
        .iter()
        .map(|&v| Datum::Sig(Sig::constant(Unit::Num, f64::from(v))))
        .collect()
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
            kit: None,
            in_send: None,
            chain_inputs: Vec::new(),
            input_sigs: HashMap::new(),
            route: Route::Master,
            taps: Vec::new(),
            rate_hints: Vec::new(),
            rates: HashMap::new(),
            param_rates: Vec::new(),
            fn_stack: Vec::new(),
            in_fn: false,
            write_gate: None,
        }
    }

    /// Run a body's statements, returning its trailing value as a signal, if there is one.
    fn run_body(&mut self, body: &[&Stmt]) -> Res<Option<(Sig, Span)>> {
        match self.run_body_value(body)? {
            Some((d, span)) => Ok(Some((self.signal_of(d, span)?, span))),
            None => Ok(None),
        }
    }

    /// Run a body's statements, returning its trailing value, if there is one.
    fn run_body_value(&mut self, body: &[&Stmt]) -> Res<Option<(Datum, Span)>> {
        let n = body.len();
        let mut tail = None;
        for (i, stmt) in body.iter().enumerate() {
            if let Some(v) = self.stmt(stmt, i + 1 == n)? {
                tail = Some(v);
            }
        }
        Ok(tail)
    }

    /// Run one statement. The value of a trailing expression, if `last`.
    fn stmt(&mut self, stmt: &Stmt, last: bool) -> Res<Option<(Datum, Span)>> {
        match stmt {
            Stmt::State { name, .. } => Err(state_outside_fn(name)),
            Stmt::IndexAssign { name, index, value } => {
                if let Some((id, buffer)) = self.names.buffers.get(&name.name).cloned() {
                    if self.in_fn {
                        return Err(Diagnostic::new(
                            "a shared buffer can't be written inside a function",
                            name.span,
                        )
                        .with_help("write it from an instrument or node"));
                    }
                    let idx = self.expr(index)?;
                    let val = self.expr(value)?;
                    if idx.unit != Unit::Num || val.unit != Unit::Num {
                        return Err(Diagnostic::new(
                            "a buffer write needs plain numbers for the index and value",
                            name.span,
                        ));
                    }
                    self.g.touch_buffer_write(id);
                    for (i, v) in idx.srcs.iter().zip(&val.srcs) {
                        let mut ports = vec![("idx", *i), ("value", *v)];
                        if let Some(gate) = self.write_gate {
                            ports.push(("gate", gate));
                        }
                        let _ = self
                            .g
                            .add(BufferWrite::new(buffer.clone()), &ports)
                            .map_err(|e| Diagnostic::new(e.to_string(), name.span))?;
                    }
                    return Ok(None);
                }
                Err(Diagnostic::new(
                    format!(
                        "only a state array or shared buffer can be written, and `{}` isn't one",
                        name.name
                    ),
                    name.span,
                )
                .with_help(
                    "state arrays belong to a `fn`: `state buf = zeros(64)`, then `buf[i] = x`",
                ))
            }
            Stmt::Bind { name, value } => self.assign(name, value).map(|()| None),
            Stmt::Destructure { names, value } => self.destructure(names, value).map(|()| None),
            Stmt::AddAssign { name, value } => self.add_assign(name, value).map(|()| None),
            Stmt::Expr(e) => match &e.kind {
                ExprKind::If {
                    then,
                    otherwise: None,
                    ..
                } if last && ends_with_value(then) => Err(Diagnostic::new(
                    "an `if` used as a value needs an `else`",
                    e.span,
                )
                .with_help("it makes a value, so both ways need one: `if x > 0 { a } else { b }`")),
                ExprKind::If {
                    then, otherwise, ..
                } if !(last && otherwise.is_some() && ends_with_value(then)) => {
                    self.if_stmt(e).map(|()| None)
                }
                ExprKind::For { body, .. } if !(last && ends_with_value(body)) => {
                    self.for_stmt(e).map(|()| None)
                }
                _ if self.mode == Mode::Node && !self.in_fn => {
                    Err(Diagnostic::new("a node makes sound through `out`", e.span)
                        .with_help("write `out = ...`"))
                }
                _ if last => Ok(Some((self.value(e)?, e.span))),
                _ => Err(Diagnostic::new("this value is never used", e.span)
                    .with_help("bind it with `name = ...`, or make it the last line")),
            },
        }
    }

    /// Run a block's statements. Names first bound inside it stay inside it. The trailing value,
    /// if `value` is asked for.
    fn block(&mut self, stmts: &[Stmt], value: bool, span: Span) -> Res<Option<Datum>> {
        let outer: Vec<String> = self.scope.keys().cloned().collect();
        let mut tail = None;
        let n = stmts.len();
        for (i, s) in stmts.iter().enumerate() {
            if let Some((v, _)) = self.stmt(s, value && i + 1 == n)? {
                tail = Some(v);
            }
        }
        self.scope.retain(|k, _| outer.contains(k));
        if value && tail.is_none() {
            return Err(Diagnostic::new("this block must end with a value", span));
        }
        Ok(tail)
    }

    /// The condition of an `if`, as a plain signal.
    fn condition(&mut self, cond: &Expr) -> Res<Sig> {
        let c = self.expr(cond)?;
        if c.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "an `if` needs a plain number to decide on, but this is {}",
                    c.unit.describe()
                ),
                cond.span,
            )
            .with_help("a comparison gives 1 or 0: `if freq < 2khz`"));
        }
        Ok(c)
    }

    /// `if cond { ... } else { ... }` as a statement: each name a branch changes takes the value of the
    /// branch that is taken.
    fn if_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::If {
            cond,
            then,
            otherwise,
        } = &e.kind
        else {
            unreachable!()
        };
        let c = self.condition(cond)?;
        if let Some(k) = c.konst {
            let taken = if k != 0.0 {
                Some(then)
            } else {
                otherwise.as_ref()
            };
            if let Some(stmts) = taken {
                self.block(stmts, false, e.span)?;
            }
            return Ok(());
        }
        let sends = self.sends;
        let saved_scope = self.scope.clone();
        let saved_out = self.out.clone();
        // a write to a shared buffer happens only in the branch that is taken
        let outer_gate = self.write_gate;
        let taken = self.truthy(c.clone(), e.span)?;
        let (then_gate, else_gate) = self.branch_gates(outer_gate, &taken, e.span)?;
        self.write_gate = Some(then_gate);
        let then_run = self.block(then, false, e.span);
        self.write_gate = outer_gate;
        then_run?;
        let then_scope = std::mem::replace(&mut self.scope, saved_scope.clone());
        let then_out = std::mem::replace(&mut self.out, saved_out.clone());
        if let Some(stmts) = otherwise {
            self.write_gate = Some(else_gate);
            let else_run = self.block(stmts, false, e.span);
            self.write_gate = outer_gate;
            else_run?;
        }
        let else_scope = std::mem::replace(&mut self.scope, saved_scope);
        let else_out = std::mem::replace(&mut self.out, saved_out);
        if self.sends != sends {
            return Err(Diagnostic::new(
                "a send inside an `if` that changes while playing would always be made",
                e.span,
            )
            .with_help("send the `if` instead: `space += if c { a } else { b }`"));
        }
        let names: Vec<String> = self.scope.keys().cloned().collect();
        for name in names {
            let (a, b) = (then_scope[&name].clone(), else_scope[&name].clone());
            if same_datum(&a, &b) {
                continue;
            }
            let merged = self.select_value(&c, a, b, e.span)?;
            self.scope.insert(name, merged);
        }
        self.out = match (then_out, else_out) {
            (Some(a), Some(b)) => Some(self.select_sig(&c, a, b, e.span)?),
            (Some(a), None) => {
                let zero = Sig::constant(a.unit, 0.0);
                Some(self.select_sig(&c, a, zero, e.span)?)
            }
            (None, Some(b)) => {
                let zero = Sig::constant(b.unit, 0.0);
                Some(self.select_sig(&c, zero, b, e.span)?)
            }
            (None, None) => None,
        };
        Ok(())
    }

    /// Where each branch of an `if` is taken, inside whatever `outer` already says.
    fn branch_gates(&mut self, outer: Option<Src>, taken: &Sig, span: Span) -> Res<(Src, Src)> {
        let zero = Sig::constant(Unit::Num, 0.0);
        let not_taken = self.compare(CmpOp::Eq, taken.clone(), zero, span)?;
        let (mut then_gate, mut else_gate) = (
            Sig::signal(taken.pick(0), Unit::Num),
            Sig::signal(not_taken.pick(0), Unit::Num),
        );
        if let Some(outer) = outer {
            let outer = Sig::signal(outer, Unit::Num);
            then_gate = self.mul(&outer, &then_gate, Unit::Num, span)?;
            else_gate = self.mul(&outer, &else_gate, Unit::Num, span)?;
        }
        Ok((then_gate.pick(0), else_gate.pick(0)))
    }

    /// The entries of a list or range, for `for`, `map` and the like.
    fn entries(&mut self, d: Datum, span: Span) -> Res<Vec<Datum>> {
        match d {
            Datum::BufferRef { .. } => Err(Diagnostic::new(
                "a buffer reference is read with indexing or .at",
                span,
            )),
            Datum::Function { .. } => Err(Diagnostic::new("a function is not an array", span)),
            Datum::Record(fields) => self.entries(fields[0].1.clone(), span),
            Datum::Array(items) => Ok(items.into_vec()),
            Datum::Table(table) => Ok(table_entries(&table)),
            Datum::Buffer { data: buf, .. } => Ok((0..buf.len())
                .map(|i| Datum::Sig(Sig::constant(Unit::Num, f64::from(buf.read(i)))))
                .collect()),
            Datum::Range { lo, hi, inclusive } => {
                let (Some(a), Some(b)) = (lo.konst, hi.konst) else {
                    return Err(Diagnostic::new(
                        "the ends of a range must be known while compiling",
                        span,
                    ));
                };
                Ok(range_numbers(a, b, inclusive, span)?
                    .into_iter()
                    .map(|k| Datum::Sig(Sig::constant(lo.unit, k)))
                    .collect())
            }
            Datum::Sig(sig) => Ok((0..sig.count())
                .map(|k| Datum::Sig(Self::element(&sig, k)))
                .collect()),
        }
    }

    /// A compile-time numeric table, or a constant numeric array coerced into one.
    fn as_table(&mut self, value: Datum, span: Span) -> Res<Arc<Table>> {
        match value {
            Datum::Table(t) => Ok(t),
            Datum::Buffer { data: b, .. } => Ok(Arc::new(b.snapshot())),
            Datum::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items.iter() {
                    match item {
                        Datum::Sig(Sig {
                            konst: Some(k),
                            unit: Unit::Num,
                            ..
                        }) => values.push(*k),
                        Datum::Sig(Sig { konst: None, .. }) => {
                            return Err(Diagnostic::new(
                                "the table must be known while compiling",
                                span,
                            ));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                "a table is a one-dimensional array of plain numbers",
                                span,
                            ));
                        }
                    }
                }
                if values.is_empty() {
                    return Err(Diagnostic::new("a table can't be empty", span));
                }
                Ok(Arc::new(Table::from_f64(values, None)))
            }
            _ => Err(Diagnostic::new(
                "expected a table of plain numbers known while compiling",
                span,
            )),
        }
    }

    fn for_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::For { var, iter, body } = &e.kind else {
            unreachable!()
        };
        self.check_loop_name(var)?;
        let d = self.value(iter)?;
        for entry in self.entries(d, iter.span)? {
            self.scope.insert(var.name.clone(), entry);
            let r = self.block(body, false, e.span);
            self.scope.remove(&var.name);
            r?;
        }
        Ok(())
    }

    fn check_loop_name(&self, var: &Ident) -> Res<()> {
        self.names.check_local_in(var, self.in_fn)?;
        if self.scope.contains_key(&var.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already in use", var.name),
                var.span,
            ));
        }
        Ok(())
    }

    fn assign(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        match name.name.as_str() {
            "out" => {
                let sig = self.out_value(value)?;
                self.set_out(sig, value.span)
            }
            "in" => Err(Diagnostic::new(
                "`in` is an input and can't be assigned",
                name.span,
            )),
            "instrument" => Err(Diagnostic::new(
                "`instrument` is only available in a node",
                name.span,
            )),
            _ => {
                self.names.check_local_in(name, self.in_fn)?;
                let v = if reads_prev_of(value, &name.name) && self.in_send.is_none() {
                    self.feedback(name, value)?
                } else {
                    self.value(value)?
                };
                if !self.in_fn
                    && let Datum::Sig(sig) = &v
                {
                    let rate = self.sig_rate(sig);
                    if rate > Rate::Const {
                        self.rate_hints.push(crate::program::RateHint {
                            span: name.span,
                            rate: rate.keyword(),
                        });
                    }
                }
                self.scope.insert(name.name.clone(), v);
                Ok(())
            }
        }
    }

    /// `y = x + prev(y) * 0.5`: a loop one sample long, compiled as a function with state whose
    /// inputs are the names the line reads.
    fn feedback(&mut self, name: &Ident, value: &Expr) -> Res<Datum> {
        let mut read = Vec::new();
        names_read(value, &mut read);
        let inputs: Vec<String> = read
            .into_iter()
            .filter(|n| *n != name.name && self.scope.contains_key(n))
            .collect();
        let span = value.span;
        let def = FnDef {
            name: name.clone(),
            params: inputs
                .iter()
                .map(|n| ParamDef {
                    name: Ident {
                        name: n.clone(),
                        span,
                    },
                    rate: None,
                    ty: None,
                    default: None,
                })
                .collect(),
            result: FnResult::Inferred,
            body: vec![
                Stmt::Bind {
                    name: name.clone(),
                    value: value.clone(),
                },
                Stmt::Expr(Expr {
                    kind: ExprKind::Name(name.name.clone()),
                    span,
                }),
            ],
            span,
        };
        let params = funcs::signature(&def.params, self.tempo)?;
        let given = inputs
            .iter()
            .map(|n| {
                Some((
                    self.scope[n].clone(),
                    Expr {
                        kind: ExprKind::Name(n.clone()),
                        span,
                    },
                ))
            })
            .collect();
        self.call_kernel(&def, name, &params, given, span)
    }

    /// `[l, r] = src`: one name per channel, or per entry of a list.
    fn destructure(&mut self, names: &[Ident], value: &Expr) -> Res<()> {
        let entries = match self.value(value)? {
            Datum::Function { .. } => {
                return Err(Diagnostic::new(
                    "a function cannot be destructured",
                    value.span,
                ));
            }
            Datum::Record(fields) => fields.into_iter().map(|(_, v)| v).collect(),
            Datum::Range { .. } => {
                return Err(Diagnostic::new(
                    "a range can't be taken apart with `[a, b] = ...`",
                    value.span,
                ));
            }
            Datum::Table(table) => {
                let items = table_entries(&table);
                if items.len() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this array has {} entries, but {} name(s) are given",
                            items.len(),
                            names.len()
                        ),
                        value.span,
                    ));
                }
                items
            }
            Datum::Buffer { .. } | Datum::BufferRef { .. } => {
                return Err(Diagnostic::new(
                    "a shared buffer can't be taken apart with `[a, b] = ...`",
                    value.span,
                )
                .with_help("read it with `buf[i]` or `buf.at(pos)`"));
            }
            Datum::Array(items) => {
                if items.len() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this array has {} entries, but {} name(s) are given",
                            items.len(),
                            names.len()
                        ),
                        value.span,
                    ));
                }
                items.into_vec()
            }
            Datum::Sig(sig) => {
                self.require_signal("a channel", &sig, value.span)?;
                if sig.count() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this signal has {} channel(s), but {} name(s) are given",
                            sig.count(),
                            names.len()
                        ),
                        value.span,
                    ));
                }
                (0..sig.count())
                    .map(|k| Datum::Sig(Self::element(&sig, k)))
                    .collect()
            }
        };
        if entries.len() != names.len() {
            return Err(Diagnostic::new(
                format!(
                    "this array has {} entries, but {} name(s) are given",
                    entries.len(),
                    names.len()
                ),
                value.span,
            ));
        }
        for (name, entry) in names.iter().zip(entries) {
            self.names.check_local_in(name, self.in_fn)?;
            self.scope.insert(name.name.clone(), entry);
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
            if self.in_fn {
                return Err(Diagnostic::new(
                    format!("a function can't send to the bus `{}`", name.name),
                    name.span,
                )
                .with_help("send from the instrument that calls it"));
            }
            self.in_send = Some(false);
            let sig = self.expr(value);
            let delayed = self.in_send.take() == Some(true);
            let sig = sig?;
            self.require_signal(&name.name, &sig, value.span)?;
            let sig = self.convert(&sig, self.names.bus_layouts[bus], value.span)?;
            for (channel, src) in sig.srcs.iter().enumerate() {
                if delayed {
                    self.g.send_channel_delayed(bus, channel, *src);
                } else {
                    self.g.send_channel(bus, channel, *src);
                }
            }
            self.sends += 1;
            return Ok(());
        }
        let mut d = Diagnostic::new(
            format!("`+=` can only add to `out` or a bus, not `{}`", name.name),
            name.span,
        );
        if self.scope.contains_key(&name.name) {
            d = d.with_help(format!(
                "to change a local, write `{0} = {0} + ...`",
                name.name
            ));
        } else if let Some(c) = closest(&name.name, self.names.buses.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean the bus `{c}`?"));
        }
        Err(d)
    }

    fn require_signal(&self, what: &str, sig: &Sig, span: Span) -> Res<()> {
        if sig.unit == Unit::Num {
            Ok(())
        } else {
            Err(Diagnostic::new(
                format!(
                    "`{what}` carries a plain signal, but this is {}",
                    sig.unit.describe()
                ),
                span,
            ))
        }
    }

    fn set_out(&mut self, sig: Sig, span: Span) -> Res<()> {
        self.require_signal("out", &sig, span)?;
        self.out = Some(sig);
        Ok(())
    }

    /// The value assigned to `out`. In a node, `x.to(bus)` routes the node's
    /// output to a bus instead of the master.
    fn out_value(&mut self, value: &Expr) -> Res<Sig> {
        let ExprKind::Call { name, args } = &value.kind else {
            return self.expr(value);
        };
        if name.name != "to" || self.mode != Mode::Node {
            return self.expr(value);
        }
        let [source, target] = args.as_slice() else {
            return Err(Diagnostic::new("`to` takes a bus", name.span)
                .with_help("for example `out = in.to(space)`"));
        };
        let bus = match (&target.name, &target.value.kind) {
            (None, ExprKind::Name(b)) => self.names.bus(b).ok_or_else(|| {
                unknown_name(
                    b,
                    target.value.span,
                    self.names.buses.iter().map(String::as_str),
                )
            })?,
            _ => {
                return Err(Diagnostic::new(
                    "`to` takes the name of a bus",
                    target.value.span,
                ));
            }
        };
        self.route = Route::Bus(bus);
        self.expr(&source.value)
    }

    /// A bundle of a node chain's external inputs, made on first use. Each
    /// channel is its own mono input.
    fn input_bundle(
        &mut self,
        key: &str,
        layout: Layout,
        source: impl Fn(usize) -> ChainInput,
    ) -> Sig {
        if let Some(sig) = self.input_sigs.get(key) {
            return sig.clone();
        }
        let srcs: Array<Src> = (0..layout.channels())
            .map(|c| {
                self.chain_inputs.push(source(c));
                self.g.input()
            })
            .collect();
        let sig = Sig::channels(srcs, layout);
        self.input_sigs.insert(key.to_string(), sig.clone());
        sig
    }

    /// `in`: what the node's voices produced, in the master's layout.
    fn node_input(&mut self) -> Sig {
        self.input_bundle("in", self.names.master, |channel| ChainInput::It {
            channel,
        })
    }

    /// Turn a signal into another layout using the automatic rules, as gain and
    /// sum nodes. An unnamed `channels(n)` signal may be taken as any layout
    /// with `n` channels.
    fn convert(&mut self, sig: &Sig, to: Layout, span: Span) -> Res<Sig> {
        if sig.layout == to {
            return Ok(sig.clone());
        }
        if matches!(sig.layout, Layout::Channels(_)) && sig.count() == to.channels() {
            return Ok(Sig {
                layout: to,
                ..sig.clone()
            });
        }
        let Some(matrix) = conversion(sig.layout, to) else {
            return Err(Diagnostic::new(
                format!("can't convert {} to {} automatically", sig.layout, to),
                span,
            )
            .with_help(
                "pan the signal, or take its channels apart with `[a, b] = ...` and rebuild them",
            ));
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
                        .add(
                            Mul,
                            &[("a", sig.srcs[input]), ("b", Src::Const(coeff as f32))],
                        )
                        .expect("mul ports")
                };
                sum = Some(match sum {
                    None => term,
                    Some(prev) => self
                        .g
                        .add(Add, &[("a", prev), ("b", term)])
                        .expect("add ports"),
                });
            }
            srcs.push(sum.unwrap_or(Src::Const(0.0)));
        }
        Ok(Sig::channels(srcs, to))
    }

    pub(crate) fn param_default(
        &mut self,
        e: &Expr,
        unit: Unit,
        range: Option<(f64, f64)>,
        name: &str,
    ) -> Res<f32> {
        let sig = self.expr(e)?;
        let Some(v) = sig.konst else {
            return Err(Diagnostic::new(
                format!("the default for `{name}` must be a constant"),
                e.span,
            ));
        };
        // A gain parameter may default to a plain linear factor.
        let ok = sig.unit == unit || (unit == Unit::Db && sig.unit == Unit::Num);
        if !ok {
            return Err(self.unit_mismatch(name, unit, sig.unit, e));
        }
        if let Some((lo, hi)) = range
            && !(lo..=hi).contains(&v)
        {
            return Err(Diagnostic::new(
                format!("the default for `{name}` is outside {lo}..{hi}"),
                e.span,
            ));
        }
        Ok(v as f32)
    }

    fn literal(&self, value: f64, unit: Option<&str>, span: Span) -> Res<Sig> {
        literal_sig(value, unit, self.tempo, span).map(|(unit, v)| Sig::constant(unit, v))
    }

    /// The signal an expression makes. A list becomes a multichannel signal, one channel per entry.
    fn expr(&mut self, e: &Expr) -> Res<Sig> {
        let v = self.value(e)?;
        self.signal_of(v, e.span)
    }

    /// A signal, a list of values, or a range.
    fn value(&mut self, e: &Expr) -> Res<Datum> {
        match &e.kind {
            ExprKind::Name(name) if self.names.buffer_controls.contains_key(name) => {
                let choices = self.names.buffer_controls[name].clone();
                let selector = self.name(name, e.span)?;
                Ok(Datum::BufferRef { selector, choices })
            }
            ExprKind::Name(name) => match self.scope.get(name) {
                Some(v) => Ok(v.clone()),
                None => match self.names.consts.get(name) {
                    Some(v) => Ok(v.clone()),
                    None => match self.names.buffers.get(name) {
                        Some(&(id, ref b)) => Ok(Datum::Buffer {
                            id,
                            data: b.clone(),
                        }),
                        None => match self.names.fns.get(name) {
                            Some(def) => Ok(Datum::Function {
                                def: def.clone(),
                                captures: HashMap::new(),
                            }),
                            None => self.name(name, e.span).map(Datum::Sig),
                        },
                    },
                },
            },
            ExprKind::Array(items) => items
                .iter()
                .map(|i| self.value(i))
                .collect::<Res<_>>()
                .map(|items: Vec<_>| Datum::array(items)),
            ExprKind::Range { lo, hi, inclusive } => {
                let (l, h) = (self.expr(lo)?, self.expr(hi)?);
                if l.unit != h.unit {
                    return Err(Diagnostic::new(
                        format!(
                            "the ends of a range need the same unit: {} and {}",
                            l.unit.describe(),
                            h.unit.describe()
                        ),
                        e.span,
                    ));
                }
                Ok(Datum::Range {
                    lo: l,
                    hi: h,
                    inclusive: *inclusive,
                })
            }
            ExprKind::Index { list, index } => self.index(list, index, e.span),
            ExprKind::Neg(inner) => {
                let v = self.value(inner)?;
                self.negate_value(v, e.span)
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let (a, b) = (self.value(lhs)?, self.value(rhs)?);
                self.zip_values(a, b, e.span, |this, a, b| this.compare(*op, a, b, e.span))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                self.binary_value(*op, l, r, e.span)
            }
            ExprKind::Logic { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                let (l, r) = (self.truthy(l, lhs.span)?, self.truthy(r, rhs.span)?);
                Ok(Datum::Sig(match op {
                    LogicOp::And => self.mul(&l, &r, Unit::Num, e.span)?,
                    LogicOp::Or => self.max_sig(&l, &r, e.span)?,
                }))
            }
            ExprKind::Not(inner) => {
                let v = self.expr(inner)?;
                let zero = Sig::constant(v.unit, 0.0);
                self.compare(CmpOp::Eq, v, zero, e.span).map(Datum::Sig)
            }
            ExprKind::If { .. } => self.if_value(e),
            ExprKind::For { var, iter, body } => {
                self.check_loop_name(var)?;
                let d = self.value(iter)?;
                let mut results = Vec::new();
                for entry in self.entries(d, iter.span)? {
                    self.scope.insert(var.name.clone(), entry);
                    let v = self.block(body, true, e.span);
                    self.scope.remove(&var.name);
                    results.push(v?.expect("a value"));
                }
                Ok(Datum::array(results))
            }
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Lambda { params, body } => {
                let def = funcs::lambda(params, body, e.span);
                let captures = self
                    .scope
                    .iter()
                    .filter(|(n, _)| funcs::reads(&def, n))
                    .map(|(n, v)| (n.clone(), v.clone()))
                    .collect();
                Ok(Datum::Function { def, captures })
            }
            _ => self.signal(e).map(Datum::Sig),
        }
    }

    /// 1 where `s` is not 0, 0 where it is.
    fn truthy(&mut self, s: Sig, span: Span) -> Res<Sig> {
        let zero = Sig::constant(s.unit, 0.0);
        self.compare(CmpOp::Ne, s, zero, span)
    }

    fn max_sig(&mut self, a: &Sig, b: &Sig, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(a.unit, x.max(y)));
        }
        self.elementwise(a, b, a.unit, span, |g, x, y| {
            g.add(Max, &[("a", x), ("b", y)]).expect("max ports")
        })
    }

    fn min_sig(&mut self, a: &Sig, b: &Sig, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(a.unit, x.min(y)));
        }
        self.elementwise(a, b, a.unit, span, |g, x, y| {
            g.add(Min, &[("a", x), ("b", y)]).expect("min ports")
        })
    }

    /// An expression that makes a signal directly: not a list, and not arithmetic on one.
    fn signal(&mut self, e: &Expr) -> Res<Sig> {
        match &e.kind {
            ExprKind::Num { value, unit } => self.literal(*value, unit.as_deref(), e.span),
            ExprKind::Call { .. } => unreachable!("`value` handles calls"),
            ExprKind::Env(env) => self.env(env),
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can only be an argument that asks for it",
                e.span,
            )),
            ExprKind::Compare { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                self.compare(*op, l, r, e.span)
            }
            ExprKind::Name(_)
            | ExprKind::Neg(_)
            | ExprKind::Binary { .. }
            | ExprKind::Logic { .. }
            | ExprKind::Not(_)
            | ExprKind::If { .. }
            | ExprKind::For { .. }
            | ExprKind::Array(_)
            | ExprKind::Range { .. }
            | ExprKind::Index { .. }
            | ExprKind::Lambda { .. } => unreachable!("`value` handles these"),
        }
    }

    /// A list as a signal, one channel per entry. The entries share a unit, so `[800hz, 1200hz]` is a
    /// two-channel frequency.
    fn signal_of(&self, v: Datum, span: Span) -> Res<Sig> {
        match v {
            Datum::Function { .. } => Err(Diagnostic::new("a function is not a signal", span)),
            Datum::Sig(sig) => Ok(sig),
            Datum::Record(fields) => self.signal_of(fields[0].1.clone(), span),
            Datum::Table(_) => Err(Diagnostic::new(
                "a table isn't a signal; play it with `osc(table, freq)` or read it with `.at`",
                span,
            )),
            Datum::Buffer { .. } | Datum::BufferRef { .. } => Err(Diagnostic::new(
                "a buffer isn't a signal; read it with `buf[i]` or `buf.at(pos)`",
                span,
            )),
            Datum::Array(items) => {
                let mut entries = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Datum::Function { .. } => return Err(Diagnostic::new("a function is not a signal", span)),
                        Datum::Record(fields) => entries.push((self.signal_of(fields[0].1.clone(), span)?, span)),
                        Datum::Sig(sig) => entries.push((sig, span)),
                        Datum::Table(_) | Datum::Buffer { .. } | Datum::BufferRef { .. } => {
                            return Err(Diagnostic::new(
                                "an array of tables can't be used as a signal",
                                span,
                            ));
                        }
                        Datum::Array(_) => {
                            return Err(Diagnostic::new("an array of arrays can't be used as a signal", span)
                                .with_help("sum the inner arrays, or index into them"));
                        }
                        Datum::Range { .. } => {
                            return Err(Diagnostic::new("an array of ranges can't be used as a signal", span));
                        }
                    }
                }
                join_channels(&entries, span)
            }
            Datum::Range { .. } => Err(Diagnostic::new("a range isn't a signal", span).with_help(
                "use it in `.range(...)`, `.clamp(...)`, a `for` loop or a slice such as `xs[0..<4]`",
            )),
        }
    }

    /// `list[i]`, counting from 0 and wrapping at both ends, or `list[a..<b]`.
    fn index(&mut self, list: &Expr, index: &Expr, span: Span) -> Res<Datum> {
        let l = self.value(list)?;
        if let Datum::BufferRef { selector, choices } = &l {
            let pos = self.expr(index)?;
            return self
                .read_buffer_ref(selector, choices, &pos, Interp::None, span)
                .map(Datum::Sig);
        }
        if let Datum::Buffer { id, data: buffer } = &l {
            if let ExprKind::Range { .. } = index.kind {
                return Err(Diagnostic::new(
                    "slicing a shared buffer isn't supported; read with `buf[i]` or `buf.at(pos)`",
                    span,
                ));
            }
            let i = self.expr(index)?;
            if i.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!("an index is a plain number, found {}", i.unit.describe()),
                    index.span,
                ));
            }
            self.g.touch_buffer_read(*id);
            let rate = self.sig_rate(&i);
            let srcs: Array<Src> = i
                .srcs
                .iter()
                .map(|&p| {
                    let src = self
                        .g
                        .add(BufferRead::new(buffer.clone(), Interp::None), &[("pos", p)])
                        .expect("buffer ports");
                    self.note_rate(src, rate);
                    src
                })
                .collect();
            return Ok(Datum::Sig(Sig {
                srcs,
                layout: i.layout,
                unit: Unit::Num,
                konst: None,
            }));
        }
        let items = self.entries(l, list.span)?;
        let n = items.len();
        if let ExprKind::Range { .. } = index.kind {
            let picks = self.value(index)?;
            let picks = self.entries(picks, index.span)?;
            if n == 0 && !picks.is_empty() {
                return Err(Diagnostic::new("this array is empty", span));
            }
            let mut out = Vec::with_capacity(picks.len());
            for p in picks {
                let Datum::Sig(Sig { konst: Some(k), .. }) = p else {
                    unreachable!("a range gives numbers")
                };
                out.push(items[niminal_engine::wrapped_index(k, n)].clone());
            }
            return Ok(Datum::array(out));
        }
        if n == 0 {
            return Err(Diagnostic::new("this array is empty", span));
        }
        let i = self.expr(index)?;
        if i.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("an index is a plain number, found {}", i.unit.describe()),
                index.span,
            ));
        }
        if let Some(k) = i.konst {
            return Ok(items[niminal_engine::wrapped_index(k, n)].clone());
        }
        self.index_values(items, &i, span)
    }

    /// Transpose nested values into scalar reads. Each read shares the same
    /// wrapping rules and takes constant time, regardless of array length.
    fn index_values(&mut self, items: Vec<Datum>, index: &Sig, span: Span) -> Res<Datum> {
        let Some(first) = items.first() else {
            return Err(Diagnostic::new("this array is empty", span));
        };
        match first {
            Datum::Sig(first) => {
                let width = first.count();
                let layout = first.layout;
                let unit = first.unit;
                let signals = items
                    .into_iter()
                    .map(|v| match v {
                        Datum::Sig(s) if s.count() == width && s.unit == unit => Ok(s),
                        _ => Err(Diagnostic::new(
                            "an index that changes needs matching element shapes and units",
                            span,
                        )),
                    })
                    .collect::<Res<Vec<_>>>()?;
                if width == 1 {
                    return self
                        .read_values(signals, index, Interp::None, span)
                        .map(Datum::Sig);
                }
                if index.count() != 1 {
                    return Err(Diagnostic::new(
                        "indexing channel arrays needs one index",
                        span,
                    ));
                }
                let srcs = (0..width)
                    .map(|channel| {
                        let elements = signals.iter().map(|s| Self::element(s, channel)).collect();
                        self.read_values(elements, index, Interp::None, span)
                            .map(|s| s.srcs[0])
                    })
                    .collect::<Res<_>>()?;
                Ok(Datum::Sig(Sig {
                    srcs,
                    layout,
                    unit,
                    konst: None,
                }))
            }
            Datum::Array(first) => {
                let width = first.len();
                let rows = items
                    .into_iter()
                    .map(|v| match v {
                        Datum::Array(row) if row.len() == width => Ok(row),
                        _ => Err(Diagnostic::new(
                            "an index that changes needs matching element shapes",
                            span,
                        )),
                    })
                    .collect::<Res<Vec<_>>>()?;
                Ok(Datum::Array(
                    (0..width)
                        .map(|column| {
                            let elements = rows.iter().map(|row| row[column].clone()).collect();
                            self.index_values(elements, index, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "an index that changes needs numeric elements or nested arrays",
                span,
            )),
        }
    }

    fn read_values(&mut self, sigs: Vec<Sig>, pos: &Sig, interp: Interp, span: Span) -> Res<Sig> {
        let Some(first) = sigs.first() else {
            return Err(Diagnostic::new("this array is empty", span));
        };
        let unit = first.unit;
        if sigs.iter().any(|s| s.count() != 1 || s.unit != unit) {
            return Err(Diagnostic::new(
                "this operation needs a one-dimensional array with one unit",
                span,
            ));
        }
        if sigs.iter().all(|s| s.konst.is_some()) {
            let data = Arc::new(Table::from_f64(sigs.iter().map(|s| s.konst.unwrap()), None));
            return self.table_read(data, unit, pos, interp, Edge::Wrap, span);
        }
        let op = ArrayRead::new(sigs.len(), interp);
        let names: Vec<_> = (0..sigs.len()).map(|i| format!("e{i}")).collect();
        let rate = sigs
            .iter()
            .fold(self.sig_rate(pos), |rate, sig| rate.max(self.sig_rate(sig)));
        let srcs = pos
            .srcs
            .iter()
            .map(|&pos| {
                let mut wires = vec![("pos", pos)];
                wires.extend(
                    names
                        .iter()
                        .zip(&sigs)
                        .map(|(name, sig)| (name.as_str(), sig.srcs[0])),
                );
                let src = self
                    .g
                    .add(op.clone(), &wires)
                    .expect("one port per array element");
                self.note_rate(src, rate);
                src
            })
            .collect();
        Ok(Sig {
            srcs,
            unit,
            layout: pos.layout,
            konst: None,
        })
    }

    /// A table read at `pos`, wrapping, one read per channel of `pos`.
    fn table_read(
        &mut self,
        data: Arc<Table>,
        unit: Unit,
        pos: &Sig,
        interp: Interp,
        edge: Edge,
        _span: Span,
    ) -> Res<Sig> {
        if let Some(position) = pos.konst {
            return Ok(Sig::constant(
                unit,
                data.read_at_edge(position, interp, edge),
            ));
        }
        let rate = self.sig_rate(pos);
        let srcs: Array<Src> = pos
            .srcs
            .iter()
            .map(|&p| {
                let src = self
                    .g
                    .add(
                        TableRead::new(data.clone(), interp).with_edge(edge),
                        &[("pos", p)],
                    )
                    .expect("table ports");
                self.note_rate(src, rate);
                src
            })
            .collect();
        Ok(Sig {
            srcs,
            layout: pos.layout,
            unit,
            konst: None,
        })
    }

    /// `f(x)` on every channel, worked out while compiling where `x` is known.
    fn func(&mut self, f: Fn1, x: &Sig) -> Sig {
        if let Some(k) = x.konst {
            return Sig::constant(Unit::Num, f.apply(k));
        }
        let rate = self.sig_rate(x);
        let srcs: Array<Src> = x
            .srcs
            .iter()
            .map(|&s| {
                let out = self.g.add(Func(f), &[("x", s)]).expect("func ports");
                self.note_rate(out, rate);
                out
            })
            .collect();
        Sig {
            srcs,
            layout: x.layout,
            unit: Unit::Num,
            konst: None,
        }
    }

    /// How often a source can change.
    fn src_rate(&self, src: Src) -> Rate {
        match src {
            Src::Const(_) => Rate::Const,
            Src::Param(i) => self.param_rates.get(i).copied().unwrap_or(Rate::Audio),
            Src::Node(i) => self.rates.get(&i).copied().unwrap_or(Rate::Audio),
            Src::Input(_) => Rate::Audio,
        }
    }

    fn sig_rate(&self, s: &Sig) -> Rate {
        if s.konst.is_some() {
            return Rate::Const;
        }
        s.srcs
            .iter()
            .map(|&x| self.src_rate(x))
            .max()
            .unwrap_or(Rate::Const)
    }

    fn note_rate(&mut self, src: Src, rate: Rate) {
        if let Src::Node(i) = src
            && rate < Rate::Audio
        {
            self.rates.insert(i, rate);
        }
    }

    /// A signal seen at a slower rate: `init` holds its value from note start, `control` reads it
    /// once per block.
    fn at_rate(&mut self, s: Sig, rate: Option<Rate>) -> Sig {
        let Some(rate) = rate else { return s };
        if self.sig_rate(&s) <= rate || rate == Rate::Const {
            return s;
        }
        let srcs: Array<Src> = s
            .srcs
            .iter()
            .map(|&x| {
                let out = match rate {
                    Rate::Init => self.g.add(Hold::default(), &[("x", x)]),
                    _ => self.g.add(BlockHold, &[("x", x)]),
                }
                .expect("hold ports");
                self.note_rate(out, rate);
                out
            })
            .collect();
        Sig {
            srcs,
            konst: None,
            ..s
        }
    }

    /// Channel `k` of a signal, with its value known if it was.
    fn element(sig: &Sig, k: usize) -> Sig {
        match sig.srcs[k] {
            Src::Const(v) => Sig::constant(sig.unit, f64::from(v)),
            src => Sig::signal(src, sig.unit),
        }
    }

    fn extreme_value(&mut self, a: Datum, b: Datum, minimum: bool, span: Span) -> Res<Datum> {
        self.zip_values(a, b, span, |this, a, b| {
            this.same_unit(&a, &b, if minimum { "min" } else { "max" }, span)?;
            if minimum {
                this.min_sig(&a, &b, span)
            } else {
                this.max_sig(&a, &b, span)
            }
        })
    }

    fn math_value(&mut self, f: Fn1, value: Datum, span: Span) -> Res<Datum> {
        self.map_value(value, span, |this, value| {
            if value.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` expects a plain number, found {}",
                        f.name(),
                        value.unit.describe()
                    ),
                    span,
                ));
            }
            Ok(this.func(f, &value))
        })
    }

    fn negate_value(&mut self, value: Datum, span: Span) -> Res<Datum> {
        self.map_value(value, span, |this, value| this.negate(value, span))
    }

    /// Arithmetic on lists goes entry by entry. A single value applies to every entry, and two lists must be the same length.
    fn binary_value(&mut self, op: BinOp, l: Datum, r: Datum, span: Span) -> Res<Datum> {
        self.zip_values(l, r, span, |this, a, b| this.binary(op, a, b, span))
    }

    fn zip_values(
        &mut self,
        a: Datum,
        b: Datum,
        span: Span,
        operation: impl Fn(&mut Self, Sig, Sig) -> Res<Sig> + Copy,
    ) -> Res<Datum> {
        match (a, b) {
            (Datum::Record(fields), b) => self.zip_values(fields[0].1.clone(), b, span, operation),
            (a, Datum::Record(fields)) => self.zip_values(a, fields[0].1.clone(), span, operation),
            (Datum::Sig(a), Datum::Sig(b)) => operation(self, a, b).map(Datum::Sig),
            (Datum::Array(a), Datum::Array(b)) => {
                if a.len() != b.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "can't combine an array of {} with an array of {}",
                            a.len(),
                            b.len()
                        ),
                        span,
                    ));
                }
                Ok(Datum::Array(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| self.zip_values(a, b, span, operation))
                        .collect::<Res<_>>()?,
                ))
            }
            (Datum::Array(a), Datum::Sig(b)) if b.count() > 1 => {
                let layout = b.layout;
                let b = (0..b.count())
                    .map(|i| Datum::Sig(Self::element(&b, i)))
                    .collect();
                let result = self.zip_values(Datum::Array(a), Datum::Array(b), span, operation)?;
                Self::signal_array(result, layout, span)
            }
            (Datum::Sig(a), Datum::Array(b)) if a.count() > 1 => {
                let layout = a.layout;
                let a = (0..a.count())
                    .map(|i| Datum::Sig(Self::element(&a, i)))
                    .collect();
                let result = self.zip_values(Datum::Array(a), Datum::Array(b), span, operation)?;
                Self::signal_array(result, layout, span)
            }
            (Datum::Array(a), b @ Datum::Sig(_)) => Ok(Datum::Array(
                a.into_iter()
                    .map(|a| self.zip_values(a, b.clone(), span, operation))
                    .collect::<Res<_>>()?,
            )),
            (a @ Datum::Sig(_), Datum::Array(b)) => Ok(Datum::Array(
                b.into_iter()
                    .map(|b| self.zip_values(a.clone(), b, span, operation))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new("arithmetic doesn't work on ranges", span)),
        }
    }

    fn signal_array(value: Datum, layout: Layout, span: Span) -> Res<Datum> {
        if let Datum::Array(items) = &value
            && items.len() == layout.channels()
            && items
                .iter()
                .all(|v| matches!(v, Datum::Sig(sig) if sig.count() == 1))
        {
            let entries = items
                .iter()
                .map(|v| match v {
                    Datum::Sig(sig) => (sig.clone(), span),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            let mut signal = join_channels(&entries, span)?;
            signal.layout = layout;
            return Ok(Datum::Sig(signal));
        }
        Ok(value)
    }

    fn map_value(
        &mut self,
        value: Datum,
        span: Span,
        operation: impl Fn(&mut Self, Sig) -> Res<Sig> + Copy,
    ) -> Res<Datum> {
        match value {
            Datum::Sig(value) => operation(self, value).map(Datum::Sig),
            Datum::Array(items) => Ok(Datum::Array(
                items
                    .into_iter()
                    .map(|v| self.map_value(v, span, operation))
                    .collect::<Res<_>>()?,
            )),
            Datum::Record(fields) => self.map_value(fields[0].1.clone(), span, operation),
            _ => Err(Diagnostic::new(
                "this operation needs a number or an array",
                span,
            )),
        }
    }

    /// `list.map(f)`, `list.sum`, `list.len`, `list.indices`, `list.min`, `list.max`, `list.at(pos)`
    /// and `signal.channels`.
    fn array_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                format!("`{}` is called on an array", name.name),
                name.span,
            )
            .with_help(format!("for example `[1, 2, 3].{}`", name.name)));
        };
        let value = self.value(&receiver.value)?;
        let layout = match &value {
            Datum::Sig(signal) => Some(signal.layout),
            _ => None,
        };
        let no_args = |what: &str| -> Res<()> {
            if args.len() > 1 {
                return Err(Diagnostic::new(
                    format!("`{what}` takes no arguments"),
                    name.span,
                ));
            }
            Ok(())
        };
        if name.name == "channels" {
            no_args("channels")?;
            return match value {
                Datum::Function { .. } => Err(Diagnostic::new(
                    "a function has no channels",
                    receiver.value.span,
                )),
                list @ (Datum::Array(_) | Datum::Table(_) | Datum::Buffer { .. }) => Ok(list),
                Datum::Sig(sig) => Ok(Datum::Array(
                    (0..sig.count())
                        .map(|k| Datum::Sig(Self::element(&sig, k)))
                        .collect(),
                )),
                Datum::Record(fields) => Ok(Datum::array(self.entries(fields[0].1.clone(), span)?)),
                Datum::BufferRef { .. } => Err(Diagnostic::new(
                    "a buffer reference has no channel array",
                    receiver.value.span,
                )),
                Datum::Range { .. } => Err(Diagnostic::new(
                    "a range has no channels",
                    receiver.value.span,
                )),
            };
        }
        if name.name == "at" {
            return self
                .at(name, args, value, receiver.value.span)
                .map(Datum::Sig);
        }
        if name.name == "phase" {
            return self
                .phase(name, args, value, receiver.value.span)
                .map(Datum::Sig);
        }
        if let Some(result) = self.table_transform(name, args, &value, span)? {
            return Ok(result);
        }
        if name.name == "step" {
            return self.step(name, args, value, receiver.value.span);
        }
        let items = self.entries(value, receiver.value.span)?;
        if name.name == "active" {
            return self.active(name, args, items, span);
        }
        match name.name.as_str() {
            "len" => {
                no_args("len")?;
                Ok(Datum::Sig(Sig::constant(Unit::Num, items.len() as f64)))
            }
            "indices" => {
                no_args("indices")?;
                Ok(Datum::Range {
                    lo: Sig::constant(Unit::Num, 0.0),
                    hi: Sig::constant(Unit::Num, items.len() as f64),
                    inclusive: false,
                })
            }
            "sum" | "min" | "max" => {
                no_args(&name.name)?;
                let mut total: Option<Datum> = None;
                for item in items {
                    total = Some(match total {
                        None => item,
                        Some(t) if name.name == "sum" => {
                            self.binary_value(BinOp::Add, t, item, span)?
                        }
                        Some(t) => self.extreme_value(t, item, name.name == "min", span)?,
                    });
                }
                total.ok_or_else(|| Diagnostic::new("this array is empty", span))
            }
            _ => {
                let result = self.map(name, args, items)?;
                match layout {
                    Some(layout) => Self::signal_array(result, layout, span),
                    None => Ok(result),
                }
            }
        }
    }

    /// `(0..1).step(0.25)`: the values of a range in steps.
    fn step(&mut self, name: &Ident, args: &[Arg], range: Datum, span: Span) -> Res<Datum> {
        let Datum::Range { lo, hi, inclusive } = range else {
            return Err(Diagnostic::new("`step` is called on a range", span)
                .with_help("for example `(0..1).step(0.25)`"));
        };
        let [_, size] = args else {
            return Err(Diagnostic::new(
                "`step` takes the size of a step",
                name.span,
            ));
        };
        let size_sig = self.expr(&size.value)?;
        let (Some(a), Some(b), Some(size_k)) = (lo.konst, hi.konst, size_sig.konst) else {
            return Err(Diagnostic::new(
                "the ends of a range and its step must be known while compiling",
                span,
            ));
        };
        if size_sig.unit != lo.unit || lo.unit == Unit::Db {
            return Err(Diagnostic::new(
                "a step has the same unit as the range, and a db range can't be stepped",
                size.value.span,
            ));
        }
        let values = crate::kernel::step_numbers(a, b, inclusive, size_k, span)?;
        Ok(Datum::array(
            values
                .into_iter()
                .map(|v| Datum::Sig(Sig::constant(lo.unit, v)))
                .collect(),
        ))
    }

    /// `xs.active(count)`: the first `count` entries sound and the rest are silent. `count` can change
    /// while playing, which a loop's length can't, so build the most there can be and switch the rest off.
    fn active(&mut self, name: &Ident, args: &[Arg], items: Vec<Datum>, span: Span) -> Res<Datum> {
        let [_, count] = args else {
            return Err(Diagnostic::new(
                "`active` takes how many entries are on",
                name.span,
            ));
        };
        let count = self.expr(&count.value)?;
        if count.unit != Unit::Num {
            return Err(Diagnostic::new(
                "`active` takes a plain number of entries",
                span,
            ));
        }
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let on = self.compare(
                CmpOp::Gt,
                count.clone(),
                Sig::constant(Unit::Num, i as f64),
                span,
            )?;
            out.push(self.silence_unless(&on, item, span)?);
        }
        Ok(Datum::array(out))
    }

    /// `item` where `on` is not 0, and zero (of the same unit) elsewhere.
    fn silence_unless(&mut self, on: &Sig, item: Datum, span: Span) -> Res<Datum> {
        match item {
            Datum::Sig(sig) => {
                let off = Sig::constant(sig.unit, 0.0);
                self.select_sig(on, sig, off, span).map(Datum::Sig)
            }
            Datum::Array(inner) => Ok(Datum::Array(
                inner
                    .into_iter()
                    .map(|d| self.silence_unless(on, d, span))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new(
                "`active` works on arrays of numbers and signals",
                span,
            )),
        }
    }

    /// `t.phase(p)`: read at normalized position 0..1.
    fn phase(&mut self, name: &Ident, args: &[Arg], list: Datum, span: Span) -> Res<Sig> {
        let mut pos = None;
        let mut interp = Interp::Linear;
        for a in &args[1..] {
            match a.name.as_ref().map(|n| n.name.as_str()) {
                None if pos.is_none() => pos = Some(self.expr(&a.value)?),
                Some("interp") => interp = interp_of(&a.value)?,
                _ => {
                    return Err(Diagnostic::new(
                        "`phase` takes a position in 0..1 and `interp:`",
                        a.value.span,
                    ));
                }
            }
        }
        let Some(pos) = pos else {
            return Err(Diagnostic::new(
                "`phase` takes a position in 0..1",
                name.span,
            ));
        };
        if let Datum::Buffer { id, data: buffer } = &list {
            self.g.touch_buffer_read(*id);
            let len = Sig::constant(Unit::Num, buffer.len() as f64);
            let index = self.mul(&pos, &len, Unit::Num, span)?;
            let rate = self.sig_rate(&index);
            let srcs: Array<Src> = index
                .srcs
                .iter()
                .map(|&p| {
                    let src = self
                        .g
                        .add(BufferRead::new(buffer.clone(), interp), &[("pos", p)])
                        .expect("buffer ports");
                    self.note_rate(src, rate);
                    src
                })
                .collect();
            return Ok(Sig {
                srcs,
                layout: index.layout,
                unit: Unit::Num,
                konst: None,
            });
        }
        let table = self.as_table(list, span)?;
        if table.is_empty() {
            return Err(Diagnostic::new("this array is empty", span));
        }
        let len = Sig::constant(Unit::Num, table.len() as f64);
        let index = self.mul(&pos, &len, Unit::Num, span)?;
        self.table_read(table, Unit::Num, &index, interp, Edge::Wrap, span)
    }

    fn table_transform(
        &mut self,
        name: &Ident,
        args: &[Arg],
        value: &Datum,
        span: Span,
    ) -> Res<Option<Datum>> {
        let table = match value {
            Datum::Table(t) => t.clone(),
            Datum::Array(_)
                if matches!(
                    name.name.as_str(),
                    "normalize"
                        | "reverse"
                        | "rotate"
                        | "resample"
                        | "concat"
                        | "integrate"
                        | "mix"
                ) =>
            {
                self.as_table(value.clone(), span)?
            }
            _ => return Ok(None),
        };
        Ok(Some(match name.name.as_str() {
            "normalize" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`normalize` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.normalize()))
            }
            "reverse" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`reverse` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.reverse()))
            }
            "integrate" => {
                if args.len() > 1 {
                    return Err(Diagnostic::new("`integrate` takes no arguments", name.span));
                }
                Datum::Table(Arc::new(table.integrate()))
            }
            "rotate" => {
                let amount = match args.get(1) {
                    Some(a) => self.expr(&a.value)?.konst.unwrap_or(0.0) as isize,
                    None => {
                        return Err(Diagnostic::new("`rotate` takes an amount", name.span));
                    }
                };
                Datum::Table(Arc::new(table.rotate(amount)))
            }
            "resample" => {
                let size = match args.get(1) {
                    Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
                    None => {
                        return Err(Diagnostic::new("`resample` takes a size", name.span));
                    }
                };
                Datum::Table(Arc::new(table.resample(size)))
            }
            "concat" => {
                let other = match args.get(1) {
                    Some(a) => {
                        let v = self.value(&a.value)?;
                        self.as_table(v, a.value.span)?
                    }
                    None => {
                        return Err(Diagnostic::new("`concat` takes another table", name.span));
                    }
                };
                Datum::Table(Arc::new(table.concat(&other)))
            }
            "mix" => {
                let names = ["other", "amount"];
                let bound = funcs::bind("mix", name.span, &names, 1, &args[1..])?;
                let Some(other_arg) = bound[0] else {
                    return Err(Diagnostic::new("`mix` takes another table", name.span));
                };
                let other_v = self.value(&other_arg.value)?;
                let other = self.as_table(other_v, other_arg.value.span)?;
                let amount = match bound[1] {
                    Some(a) => self.expr(&a.value)?.konst.unwrap_or(0.5),
                    None => 0.5,
                };
                Datum::Table(Arc::new(table.mix(&other, amount)))
            }
            _ => return Ok(None),
        }))
    }

    /// `take.record(sig, loop: true)`: write consecutive samples into a shared buffer.
    fn buffer_record(&mut self, name: &Ident, args: &[Arg], _span: Span) -> Res<Datum> {
        let Some(receiver) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new("`record` is called on a buffer", name.span));
        };
        let Datum::Buffer { id, data: buffer } = self.value(&receiver.value)? else {
            return Err(Diagnostic::new(
                "`record` needs a shared buffer",
                receiver.value.span,
            ));
        };
        if self.in_fn {
            return Err(Diagnostic::new(
                "a shared buffer can't be written inside a function",
                name.span,
            ));
        }
        let names = ["x", "loop", "channel"];
        let bound = funcs::bind("record", name.span, &names, 1, &args[1..])?;
        let Some(x_arg) = bound[0] else {
            return Err(Diagnostic::new(
                "`record` takes a signal to write",
                name.span,
            ));
        };
        let looped = match bound[1] {
            Some(a) => match &a.value.kind {
                ExprKind::Name(n) if n == "true" || n == "forever" => true,
                ExprKind::Name(n) if n == "false" || n == "once" => false,
                _ => {
                    return Err(Diagnostic::new(
                        "`loop` is `true`/`forever` or `false`/`once`",
                        a.value.span,
                    ));
                }
            },
            None => true,
        };
        self.g.touch_buffer_write(id);
        let x = self.expr(&x_arg.value)?;
        let channel = if let Some(arg) = bound[2] {
            let value = self.value(&arg.value)?;
            let value = self.signal_of(value, arg.value.span)?;
            let Some(n) = value.konst else {
                return Err(Diagnostic::new(
                    "record channel is a constant 1-based integer",
                    arg.value.span,
                ));
            };
            if n < 1. || n.fract() != 0. {
                return Err(Diagnostic::new(
                    "record channel is a constant 1-based integer",
                    arg.value.span,
                ));
            }
            n as usize - 1
        } else {
            0
        };
        if channel + x.srcs.len() > buffer.channels() {
            return Err(Diagnostic::new(
                "record signal exceeds the buffer channel layout",
                x_arg.value.span,
            ));
        }
        let rate = self.sig_rate(&x);
        let srcs: Array<Src> = x
            .srcs
            .iter()
            .enumerate()
            .map(|(c, &s)| {
                let mut ports = vec![("x", s)];
                if let Some(gate) = self.write_gate {
                    ports.push(("gate", gate));
                }
                let src = self
                    .g
                    .add(
                        BufferRecord::new(buffer.clone(), looped).with_channel(channel + c),
                        &ports,
                    )
                    .expect("record ports");
                self.note_rate(src, rate);
                src
            })
            .collect();
        Ok(Datum::Sig(Sig {
            srcs,
            layout: x.layout,
            unit: Unit::Num,
            konst: None,
        }))
    }

    /// `kick.data`: the sample's underlying table(s).
    fn sample_data(&mut self, name: &Ident, receiver: &Expr) -> Res<Datum> {
        let ExprKind::Name(sample) = &receiver.kind else {
            return Err(Diagnostic::new(
                "`.data` is used on a sample name",
                receiver.span,
            ));
        };
        let Some(channels) = self.names.sample_data.get(sample) else {
            let mut d = Diagnostic::new(format!("no sample named `{sample}`"), receiver.span);
            if let Some(c) = closest(sample, self.names.sample_data.keys().map(String::as_str)) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        match channels.as_slice() {
            [] => Err(Diagnostic::new("this sample has no audio", name.span)),
            [one] => Ok(Datum::Table(one.clone())),
            many => Ok(Datum::Array(
                many.iter().cloned().map(Datum::Table).collect(),
            )),
        }
    }

    fn same_unit(&self, a: &Sig, b: &Sig, what: &str, span: Span) -> Res<()> {
        if a.unit != b.unit {
            return Err(Diagnostic::new(
                format!(
                    "`{what}` needs values with the same unit: {} and {}",
                    a.unit.describe(),
                    b.unit.describe()
                ),
                span,
            ));
        }
        Ok(())
    }

    fn read_buffer_ref(
        &mut self,
        selector: &Sig,
        choices: &BufferChoices,
        pos: &Sig,
        interp: Interp,
        span: Span,
    ) -> Res<Sig> {
        self.read_buffer_channel(selector, choices, pos, interp, 0, span)
    }
    fn read_buffer_channel(
        &mut self,
        selector: &Sig,
        choices: &BufferChoices,
        pos: &Sig,
        interp: Interp,
        channel: usize,
        span: Span,
    ) -> Res<Sig> {
        if pos.unit != Unit::Num || pos.count() != 1 {
            return Err(Diagnostic::new(
                "a buffer position is one plain number",
                span,
            ));
        }
        for (name, _) in choices.iter() {
            self.g.touch_buffer_read(self.names.buffers[name].0);
        }
        let buffers = Arc::new(choices.iter().map(|(_, b)| b.clone()).collect());
        let src = self
            .g
            .add(
                niminal_engine::ops::BufferReadBank::new(buffers, interp, choices.key)
                    .channel(channel),
                &[("source", selector.srcs[0]), ("pos", pos.srcs[0])],
            )
            .expect("buffer reader ports");
        Ok(Sig::signal(src, Unit::Num))
    }

    /// `table.at(pos, interp: linear, edge: wrap)`: a list of numbers read at a fractional position.
    fn at(&mut self, name: &Ident, args: &[Arg], list: Datum, span: Span) -> Res<Sig> {
        let mut pos = None;
        let mut interp = Interp::Linear;
        let mut edge = Edge::Wrap;
        let mut channel = 0usize;
        for a in &args[1..] {
            match a.name.as_ref().map(|n| n.name.as_str()) {
                None if pos.is_none() => pos = Some(self.expr(&a.value)?),
                Some("interp") => interp = interp_of(&a.value)?,
                Some("edge") => edge = edge_of(&a.value)?,
                Some("channel") => {
                    let value = self
                        .expr(&a.value)?
                        .konst()
                        .ok_or_else(|| Diagnostic::new("channel is constant", a.value.span))?;
                    if value.fract() != 0. || !(1.0..=16.).contains(&value) {
                        return Err(Diagnostic::new("channel is 1–16", a.value.span));
                    }
                    channel = value as usize - 1;
                }
                _ => {
                    return Err(Diagnostic::new(
                        "`at` takes a position, `interp:` and `edge:`",
                        a.value.span,
                    ));
                }
            }
        }
        let Some(pos) = pos else {
            return Err(Diagnostic::new("`at` takes a position", name.span));
        };
        if pos.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "a position is a plain number, found {}",
                    pos.unit.describe()
                ),
                name.span,
            ));
        }
        if let Datum::Table(table) = list {
            if table.is_empty() {
                return Err(Diagnostic::new("this array is empty", span));
            }
            return self.table_read(table, Unit::Num, &pos, interp, edge, span);
        }
        if let Datum::BufferRef { selector, choices } = &list {
            return self.read_buffer_channel(selector, choices, &pos, interp, channel, span);
        }
        if let Datum::Buffer { id, data: buffer } = list {
            self.g.touch_buffer_read(id);
            let rate = self.sig_rate(&pos);
            let srcs: Array<Src> = pos
                .srcs
                .iter()
                .map(|&p| {
                    let src = self
                        .g
                        .add(
                            BufferRead::new(buffer.clone(), interp).channel(channel),
                            &[("pos", p)],
                        )
                        .expect("buffer ports");
                    self.note_rate(src, rate);
                    src
                })
                .collect();
            return Ok(Sig {
                srcs,
                layout: pos.layout,
                unit: Unit::Num,
                konst: None,
            });
        }
        let items = self.entries(list, span)?;
        let sigs = items
            .into_iter()
            .map(|item| match item {
                Datum::Sig(sig) if sig.count() == 1 => Ok(sig),
                _ => Err(Diagnostic::new(
                    "`at` reads a one-dimensional numeric array",
                    span,
                )),
            })
            .collect::<Res<Vec<_>>>()?;
        if edge != Edge::Wrap {
            let data = Arc::new(Table::from_f64(
                sigs.iter()
                    .map(|s| {
                        s.konst.ok_or_else(|| {
                            Diagnostic::new("`edge:` needs a table known while compiling", span)
                        })
                    })
                    .collect::<Res<Vec<_>>>()?,
                None,
            ));
            return self.table_read(data, sigs[0].unit, &pos, interp, edge, span);
        }
        self.read_values(sigs, &pos, interp, span)
    }

    /// The function runs once for each entry, while compiling, and the results are the new list.
    fn map(&mut self, name: &Ident, args: &[Arg], items: Vec<Datum>) -> Res<Datum> {
        let [_, f] = args else {
            return Err(Diagnostic::new("`map` takes a function", name.span)
                .with_help("for example `[1, 2, 3].map(fn(x) { x * 2 })` or `.map(it * 2)`"));
        };
        let mapped = match &f.value.kind {
            ExprKind::Name(n)
                if self.names.fns.contains_key(n)
                    || matches!(self.scope.get(n), Some(Datum::Function { .. })) =>
            {
                Some(funcs::mapped_call(n, f.value.span))
            }
            _ => None,
        };
        let (param, body) = lambda_of(mapped.as_ref().unwrap_or(&f.value))?;
        if param != "it" {
            self.names.check_local_in(
                &Ident {
                    name: param.clone(),
                    span: f.value.span,
                },
                self.in_fn,
            )?;
        }
        if self.scope.contains_key(&param) {
            return Err(Diagnostic::new(
                format!("`{param}` is already in use, so it can't be this function's parameter"),
                f.value.span,
            ));
        }
        let mut results = Vec::with_capacity(items.len());
        for item in items {
            self.scope.insert(param.clone(), item);
            let v = match &body {
                Body::Block(stmts) => self
                    .block(stmts, true, f.value.span)
                    .map(|v| v.expect("a value")),
                Body::Expr(e) => self.value(e),
            };
            self.scope.remove(&param);
            results.push(v?);
        }
        Ok(Datum::array(results))
    }

    fn name(&mut self, name: &str, span: Span) -> Res<Sig> {
        if let Some(v) = self.scope.get(name) {
            return self.signal_of(v.clone(), span);
        }
        match name {
            "out" => {
                return self.out.clone().ok_or_else(|| {
                    Diagnostic::new("`out` has no value yet", span)
                        .with_help("set it first, with `out = ...`")
                });
            }
            "in" => {
                return match self.mode {
                    Mode::Node => Ok(self.node_input()),
                    _ => Err(Diagnostic::new(
                        "`in` is only available inside an fx, where it is the audio coming in",
                        span,
                    )),
                };
            }
            "it" => {
                return Err(Diagnostic::new(
                    "`it` is only available inside a function given to `map`",
                    span,
                ));
            }
            "pi" => return Ok(Sig::constant(Unit::Num, std::f64::consts::PI)),
            "true" => return Ok(Sig::constant(Unit::Num, 1.0)),
            "false" => return Ok(Sig::constant(Unit::Num, 0.0)),
            "sample_rate" => {
                if self.mode == Mode::Plain {
                    return Err(Diagnostic::new(
                        "the sample rate isn't known while compiling",
                        span,
                    ));
                }
                let src = self.g.add(SampleRate, &[]).expect("no ports");
                self.note_rate(src, Rate::Init);
                return Ok(Sig::signal(src, Unit::Hz));
            }
            "note" => {
                return Err(Diagnostic::new(
                    "`note` describes the running voice: `note.time`, `note.released`, `note.start`, `note.dur` or `note.index`",
                    span,
                ));
            }
            "clock" => {
                return Err(Diagnostic::new(
                    "`clock` describes the transport: `clock.time`",
                    span,
                ));
            }
            _ => {}
        }
        if let Some(control) = self.names.controls.iter().find(|c| c.name == name) {
            if self.mode == Mode::Plain {
                return Err(Diagnostic::new(
                    "a live control cannot be used as a constant",
                    span,
                ));
            }
            let src = self
                .g
                .add(
                    niminal_engine::ops::Control::new(control.key.clone(), control.initial),
                    &[],
                )
                .map_err(|e| Diagnostic::new(e.to_string(), span))?;
            self.note_rate(src, Rate::Control);
            return Ok(Sig {
                srcs: vec![src].into(),
                layout: Layout::Mono,
                unit: control.unit,
                konst: None,
            });
        }
        if let Some(bus) = self.names.bus(name) {
            return match self.mode {
                Mode::Node => {
                    let layout = self.names.bus_layouts[bus];
                    Ok(self.input_bundle(name, layout, |channel| ChainInput::Bus { bus, channel }))
                }
                _ => Err(Diagnostic::new(
                    format!("`{name}` is a bus, and only an fx can read a bus"),
                    span,
                )
                .with_help(format!("an instrument can send to it with `{name} += ...`"))),
            };
        }
        if let Ok(note @ Value::Note(_)) = name.parse::<Value>() {
            return Ok(Sig::constant(
                Unit::Hz,
                note.as_hz_in(&self.tempo.tuning)
                    .expect("a note is a frequency"),
            ));
        }
        if self.names.fns.contains_key(name) {
            return Err(Diagnostic::new(
                format!("`{name}` is a function: call it with `{name}(...)`"),
                span,
            ));
        }
        if opcodes::wave(name).is_some() {
            return Err(
                Diagnostic::new(format!("`{name}` is a waveform, not a value"), span)
                    .with_help("waveforms go in the first slot of `osc`, as in `osc(saw, freq)`"),
            );
        }
        Err(unknown_name(
            name,
            span,
            self.scope.keys().map(String::as_str),
        ))
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
        make: impl Fn(&mut GraphBuilder, Src, Src) -> Src,
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
        let layout = if a.count() >= b.count() {
            a.layout
        } else {
            b.layout
        };
        let rate = self.sig_rate(a).max(self.sig_rate(b));
        let srcs: Vec<Src> = (0..n)
            .map(|k| make(&mut self.g, a.pick(k), b.pick(k)))
            .collect();
        for &src in &srcs {
            self.note_rate(src, rate);
        }
        Ok(Sig {
            srcs: srcs.into(),
            layout,
            unit,
            konst: None,
        })
    }

    fn mul(&mut self, a: &Sig, b: &Sig, unit: Unit, span: Span) -> Res<Sig> {
        if let (Some(x), Some(y)) = (a.konst, b.konst) {
            return Ok(Sig::constant(unit, x * y));
        }
        self.elementwise(a, b, unit, span, |g, x, y| {
            g.add(Mul, &[("a", x), ("b", y)]).expect("mul ports")
        })
    }

    fn negate(&mut self, s: Sig, span: Span) -> Res<Sig> {
        match (s.unit, s.konst) {
            // A decibel value is held as a gain factor, so negating it inverts it.
            (Unit::Db, Some(k)) => Ok(Sig::constant(Unit::Db, 1.0 / k)),
            (Unit::Db, None) => Err(Diagnostic::new(
                "negating a changing db value isn't supported yet",
                span,
            )),
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
        let mismatch = || {
            Diagnostic::new(
                format!("can't {verb} {} and {}", lu.describe(), ru.describe()),
                span,
            )
        };

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
                    let r = if op == BinOp::Sub {
                        self.invert(&r, span)?
                    } else {
                        r
                    };
                    return self.mul(&l, &r, Unit::Db, span);
                }
                if let (Some(x), Some(y)) = (l.konst, r.konst) {
                    let v = if op == BinOp::Add { x + y } else { x - y };
                    return Ok(Sig::constant(lu, v));
                }
                if op == BinOp::Add {
                    self.elementwise(&l, &r, lu, span, |g, x, y| {
                        g.add(Add, &[("a", x), ("b", y)]).expect("add ports")
                    })
                } else {
                    self.elementwise(&l, &r, lu, span, |g, x, y| {
                        g.add(Sub, &[("a", x), ("b", y)]).expect("sub ports")
                    })
                }
            }
            BinOp::Mul => {
                let unit = crate::unit::mul_unit(lu, ru).ok_or_else(mismatch)?;
                self.mul(&l, &r, unit, span)
            }
            BinOp::Div => {
                let unit = crate::unit::div_unit(lu, ru).ok_or_else(mismatch)?;
                let Some(k) = r.konst else {
                    // a changing divisor: worked out sample by sample (0 where it is 0)
                    return self.elementwise(&l, &r, unit, span, |g, x, y| {
                        g.add(Div, &[("a", x), ("b", y)]).expect("div ports")
                    });
                };
                if k == 0.0 {
                    return Ok(Sig::constant(unit, 0.0));
                }
                self.mul(&l, &Sig::constant(Unit::Num, 1.0 / k), unit, span)
            }
        }
    }

    /// `a.mod(b)`: the remainder, with the sign of `b`.
    fn rem(&mut self, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        let unit = crate::unit::rem_unit(l.unit, r.unit).ok_or_else(|| {
            Diagnostic::new(
                format!(
                    "can't take the remainder of {} and {}",
                    l.unit.describe(),
                    r.unit.describe()
                ),
                span,
            )
        })?;
        if let (Some(x), Some(y)) = (l.konst, r.konst) {
            return Ok(Sig::constant(unit, niminal_engine::ops::rem(x, y)));
        }
        self.elementwise(&l, &r, unit, span, |g, x, y| {
            g.add(Rem, &[("a", x), ("b", y)]).expect("rem ports")
        })
    }

    /// `a < b`: 1 where it holds, 0 where it doesn't.
    fn compare(&mut self, op: CmpOp, l: Sig, r: Sig, span: Span) -> Res<Sig> {
        if l.unit != r.unit {
            return Err(Diagnostic::new(
                format!(
                    "can't compare {} and {}",
                    l.unit.describe(),
                    r.unit.describe()
                ),
                span,
            ));
        }
        let cmp: Cmp = cmp_op(op);
        if let (Some(x), Some(y)) = (l.konst, r.konst) {
            return Ok(Sig::constant(
                Unit::Num,
                if cmp.holds(x, y) { 1.0 } else { 0.0 },
            ));
        }
        self.elementwise(&l, &r, Unit::Num, span, move |g, x, y| {
            g.add(Compare(cmp), &[("a", x), ("b", y)])
                .expect("compare ports")
        })
    }

    /// `if cond { a } else { b }` as a value. A condition known while compiling picks a side then and
    /// there; otherwise both are worked out and one is kept, sample by sample.
    fn if_value(&mut self, e: &Expr) -> Res<Datum> {
        let ExprKind::If {
            cond,
            then,
            otherwise,
        } = &e.kind
        else {
            unreachable!()
        };
        let Some(otherwise) = otherwise else {
            return Err(
                Diagnostic::new("an `if` used as a value needs an `else`", e.span).with_help(
                    "it makes a value, so both ways need one: `if x > 0 { a } else { b }`",
                ),
            );
        };
        let c = self.condition(cond)?;
        if let Some(k) = c.konst {
            let taken = if k != 0.0 { then } else { otherwise };
            return Ok(self.block(taken, true, e.span)?.expect("a value"));
        }
        let sends = self.sends;
        let a = self.block(then, true, e.span)?.expect("a value");
        let b = self.block(otherwise, true, e.span)?.expect("a value");
        if self.sends != sends {
            return Err(Diagnostic::new(
                "a send inside an `if` that changes while playing would always be made",
                e.span,
            ));
        }
        self.select_value(&c, a, b, e.span)
    }

    fn select_value(&mut self, condition: &Sig, a: Datum, b: Datum, span: Span) -> Res<Datum> {
        match (a, b) {
            (Datum::Sig(a), Datum::Sig(b)) => {
                self.select_sig(condition, a, b, span).map(Datum::Sig)
            }
            (Datum::Array(a), Datum::Array(b)) => {
                if a.len() != b.len() {
                    return Err(Diagnostic::new(
                        "both branches need the same array shape",
                        span,
                    ));
                }
                Ok(Datum::Array(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| self.select_value(condition, a, b, span))
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "both branches need matching numbers or array shapes",
                span,
            )),
        }
    }

    /// `a` where `c` is not 0, `b` elsewhere, channel by channel.
    fn select_sig(&mut self, c: &Sig, a: Sig, b: Sig, span: Span) -> Res<Sig> {
        if a.unit != b.unit {
            return Err(Diagnostic::new(
                format!(
                    "the two ways of an `if` must have the same unit: one is {}, the other {}",
                    a.unit.describe(),
                    b.unit.describe()
                ),
                span,
            ));
        }
        if let Some(k) = c.konst {
            return Ok(if k != 0.0 { a } else { b });
        }
        let n = [c.count(), a.count(), b.count()]
            .into_iter()
            .max()
            .unwrap_or(1);
        for s in [c, &a, &b] {
            if s.count() != 1 && s.count() != n {
                return Err(Diagnostic::new(
                    format!("can't combine a {n}-channel signal with a {}-channel one", s.count()),
                    span,
                )
                .with_help("a one-channel signal can be combined with any; otherwise the counts must match"));
            }
        }
        let layout = [c, &a, &b]
            .into_iter()
            .find(|s| s.count() == n)
            .map_or(Layout::Mono, |s| s.layout);
        let rate = self
            .sig_rate(c)
            .max(self.sig_rate(&a))
            .max(self.sig_rate(&b));
        let srcs: Vec<Src> = (0..n)
            .map(|k| {
                self.g
                    .add(
                        Select,
                        &[("cond", c.pick(k)), ("a", a.pick(k)), ("b", b.pick(k))],
                    )
                    .expect("select ports")
            })
            .collect();
        for &src in &srcs {
            self.note_rate(src, rate);
        }
        Ok(Sig {
            srcs: srcs.into(),
            layout,
            unit: a.unit,
            konst: None,
        })
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
        let srcs: Array<Src> = exponent
            .srcs
            .iter()
            .map(|&s| {
                self.g
                    .add(Func(Fn1::Exp), &[("x", s)])
                    .expect("exp has one port")
            })
            .collect();
        let ratio = Sig {
            srcs,
            layout: exponent.layout,
            unit: Unit::Num,
            konst: None,
        };
        self.mul(freq, &ratio, Unit::Hz, span)
    }

    fn invert(&mut self, s: &Sig, span: Span) -> Res<Sig> {
        match s.konst {
            Some(k) => Ok(Sig::constant(s.unit, 1.0 / k)),
            None => Err(Diagnostic::new(
                "subtracting a changing db value isn't supported yet",
                span,
            )),
        }
    }

    // ---- calls ----------------------------------------------------------

    /// The value of `e` if it is an opcode's several results: a name bound to them, or a call that makes them.
    fn record(&mut self, e: &Expr) -> Res<Option<Vec<(String, Datum)>>> {
        if let ExprKind::Name(name) = &e.kind {
            return Ok(match self.scope.get(name) {
                Some(Datum::Record(fields)) => Some(fields.clone()),
                _ => None,
            });
        }
        if let ExprKind::Call { name, .. } = &e.kind
            && (self
                .names
                .fns
                .get(&name.name)
                .is_some_and(|def| matches!(def.result, FnResult::Named(_)))
                || matches!(self.scope.get(&name.name), Some(Datum::Function { def, .. }) if matches!(def.result, FnResult::Named(_))))
        {
            return Ok(match self.value(e)? {
                Datum::Record(fields) => Some(fields),
                _ => None,
            });
        }
        Ok(None)
    }

    fn audio_input(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Sig> {
        if self.in_fn {
            return Err(
                Diagnostic::new("`input` can't be used inside a function", name.span)
                    .with_help("read it in the node, and pass it in as an argument"),
            );
        }
        if self.mode != Mode::Node {
            return Err(Diagnostic::new("`input` is read in a node", name.span)
                .with_help("for example `fx voice { out = input(1).lpf(cutoff: 2khz) }`"));
        }
        let channel = |arg: &Arg| -> Res<usize> {
            match &arg.value.kind {
                ExprKind::Num { value, unit: None }
                    if arg.name.is_none()
                        && value.fract() == 0.0
                        && (1.0..=16.0).contains(value) =>
                {
                    Ok(*value as usize)
                }
                _ => Err(Diagnostic::new(
                    "an input channel is a whole number from 1 to 16",
                    arg.value.span,
                )),
            }
        };
        let ends = |arg: &Arg| -> Option<(Arg, Arg)> {
            let ExprKind::Range {
                lo,
                hi,
                inclusive: true,
            } = &arg.value.kind
            else {
                return None;
            };
            let wrap = |e: &Expr| Arg {
                name: None,
                value: e.clone(),
                receiver: false,
            };
            Some((wrap(lo), wrap(hi)))
        };
        let (first, last) = match args {
            [a] if ends(a).is_some() => {
                let (lo, hi) = ends(a).unwrap();
                (channel(&lo)?, channel(&hi)?)
            }
            [a] => (channel(a)?, channel(a)?),
            _ => {
                return Err(Diagnostic::new(
                    "`input` takes a channel, or a range of them",
                    name.span,
                )
                .with_help("for example `input(1)` or `input(1..2)`"));
            }
        };
        if last < first {
            return Err(
                Diagnostic::new("the first channel of a range comes first", span)
                    .with_help("for example `input(1..2)`"),
            );
        }
        let layout = Layout::from_count(last - first + 1);
        Ok(
            self.input_bundle(&format!("input:{first}:{last}"), layout, move |c| {
                ChainInput::Audio {
                    channel: first - 1 + c,
                }
            }),
        )
    }

    fn report(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        if self.in_fn {
            return Err(Diagnostic::new(
                "report belongs in an instrument or fx",
                name.span,
            ));
        }
        let receiver = args
            .first()
            .filter(|a| a.receiver)
            .ok_or_else(|| Diagnostic::new("use signal.report(\"name\")", name.span))?;
        let value = self.value(&receiver.value)?;
        let mut entries = self
            .entries(value.clone(), receiver.value.span)
            .or_else(|_| Ok::<_, Diagnostic>(vec![value.clone()]))?;
        if entries.is_empty() || entries.len() > 64 {
            return Err(Diagnostic::new("report needs 1–64 values", name.span));
        }
        let mut sources = Vec::new();
        for entry in entries.drain(..) {
            let sig = self.signal_of(entry, receiver.value.span)?;
            sources.extend(sig.srcs.iter().copied());
        }
        if sources.is_empty() || sources.len() > 64 {
            return Err(Diagnostic::new(
                "report needs 1–64 scalar values",
                name.span,
            ));
        }
        let mut label = None;
        let mut rate = 30.;
        let mut trigger = None;
        for arg in &args[1..] {
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                None if label.is_none() => {
                    if let ExprKind::Str(s) = &arg.value.kind {
                        label = Some(s.clone());
                    } else {
                        return Err(Diagnostic::new("report name is text", arg.value.span));
                    }
                }
                Some("rate") => {
                    if let ExprKind::Num { value, unit } = &arg.value.kind {
                        if unit.as_deref() != Some("hz") || !(0.5..=200.).contains(value) {
                            return Err(Diagnostic::new(
                                "report rate is 0.5hz–200hz",
                                arg.value.span,
                            ));
                        }
                        rate = *value;
                    } else {
                        return Err(Diagnostic::new(
                            "report rate is a constant in hz",
                            arg.value.span,
                        ));
                    }
                }
                Some("on") => {
                    let sig = self.expr(&arg.value)?;
                    if sig.count() != 1 || sig.unit != Unit::Num {
                        return Err(Diagnostic::new(
                            "report trigger is one plain signal",
                            arg.value.span,
                        ));
                    }
                    trigger = Some(sig.srcs[0]);
                }
                _ => {
                    return Err(Diagnostic::new(
                        "unknown or repeated report argument",
                        arg.value.span,
                    ));
                }
            }
        }
        let label = label
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or_else(|| Diagnostic::new("report needs a name of 1–128 characters", name.span))?;
        let report = niminal_engine::ops::Report::new(label, sources.len());
        let mut ports: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("x{i}"), *s))
            .collect();
        if let Some(trigger) = trigger {
            ports.push(("on".into(), trigger));
        }
        let ports: Vec<_> = ports.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        self.g
            .add(
                niminal_engine::ops::ReportOut::new(report, trigger.is_some(), rate),
                &ports,
            )
            .expect("report ports");
        Ok(value)
    }

    /// `x.osc_out("/vj/level", port: 9000)`: passes `x` through and sends its value as an OSC message,
    /// 30 times a second unless `rate:` says otherwise, to `host:` (this computer if none).
    fn osc_out(&mut self, name: &Ident, args: &[Arg]) -> Res<Sig> {
        use niminal_engine::ops::{Tap, TapOut};
        if self.in_fn {
            return Err(
                Diagnostic::new("`osc_out` can't be used inside a function", name.span)
                    .with_help("send from the node that calls it"),
            );
        }
        let Some(signal) = args.first().filter(|a| a.receiver) else {
            return Err(Diagnostic::new(
                "`osc_out` works on a signal: write it after one, as in `out.osc_out(...)`",
                name.span,
            ));
        };
        let mut address = None;
        let (mut host, mut port, mut rate, mut trigger) = (None, None, None, None);
        for arg in &args[1..] {
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                None => {
                    if address.is_some() {
                        return Err(Diagnostic::new(
                            "`osc_out` takes its other arguments by name",
                            arg.value.span,
                        ));
                    }
                    address = Some(&arg.value);
                }
                Some("host") => host = Some(&arg.value),
                Some("port") => port = Some(&arg.value),
                Some("rate") => rate = Some(&arg.value),
                Some("on") => trigger = Some(&arg.value),
                Some(other) => {
                    let mut d = Diagnostic::new(
                        format!("`osc_out` has no argument named `{other}`"),
                        arg.name.as_ref().map_or(arg.value.span, |n| n.span),
                    );
                    if let Some(c) = closest(other, ["host", "port", "rate", "on"]) {
                        d = d.with_help(format!("did you mean `{c}`?"));
                    }
                    return Err(d);
                }
            }
        }
        let text = |e: &Expr, what: &str| match &e.kind {
            ExprKind::Str(t) => Ok(t.clone()),
            _ => Err(Diagnostic::new(format!("{what} is text in quotes"), e.span)),
        };
        let address = match address {
            Some(e) => text(e, "an OSC address")?,
            None => {
                return Err(Diagnostic::new("`osc_out` needs an address", name.span)
                    .with_help("for example `osc_out(\"/vj/level\", port: 9000)`"));
            }
        };
        if !address.starts_with('/') {
            return Err(Diagnostic::new(
                "an OSC address starts with `/`",
                address_span(args),
            ));
        }
        let host = match host {
            Some(e) => text(e, "a host")?,
            None => "127.0.0.1".to_string(),
        };
        let number = |e: &Expr, what: &str, unit: Option<&str>| match &e.kind {
            ExprKind::Num { value, unit: u } if u.as_deref() == unit => Ok(*value),
            _ => Err(Diagnostic::new(
                format!(
                    "{what} is a number{}",
                    unit.map_or(String::new(), |u| format!(" in {u}"))
                ),
                e.span,
            )),
        };
        let port = match port {
            Some(e) => {
                let p = number(e, "a port", None)?;
                if p.fract() != 0.0 || !(1.0..=65535.0).contains(&p) {
                    return Err(Diagnostic::new(
                        "a port is a whole number from 1 to 65535",
                        e.span,
                    ));
                }
                p as u16
            }
            None => {
                return Err(Diagnostic::new("`osc_out` needs a `port:`", name.span)
                    .with_help("for example `osc_out(\"/vj/level\", port: 9000)`"));
            }
        };
        let rate = match rate {
            Some(e) => {
                let r = number(e, "a rate", Some("hz"))?;
                if !(0.5..=200.0).contains(&r) {
                    return Err(Diagnostic::new("`rate` is from 0.5hz to 200hz", e.span));
                }
                r
            }
            None => 30.0,
        };
        let sig = self.expr(&signal.value)?;
        self.require_signal("osc_out", &sig, signal.value.span)?;
        if sig.count() != 1 {
            return Err(Diagnostic::new(
                "`osc_out` sends one value, but this signal has several channels",
                signal.value.span,
            )
            .with_help("send a channel at a time: `[l, r] = ...` then `l.osc_out(...)`"));
        }
        let mut ports = vec![("x", sig.srcs[0])];
        let tap = if let Some(trigger) = trigger {
            let value = self.expr(trigger)?;
            if value.count() != 1 || value.unit != Unit::Num {
                return Err(Diagnostic::new(
                    "OSC on is one plain trigger signal",
                    trigger.span,
                ));
            }
            ports.push(("on", value.srcs[0]));
            Tap::triggered()
        } else {
            Tap::new()
        };
        let src = self
            .g
            .add(TapOut::new(tap.clone()), &ports)
            .expect("tap ports");
        self.taps.push(crate::program::OscTap {
            address,
            host,
            port,
            rate,
            value: tap,
        });
        Ok(Sig {
            srcs: vec![src].into(),
            konst: None,
            ..sig
        })
    }

    /// `x.range(200hz..4khz)`: a plain signal from 0 to 1 (a control, a knob) mapped to a range. Decibels
    /// move evenly in db, everything else evenly in its own unit.
    fn range(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`range` takes a range", name.span));
        };
        if range.name.is_some() {
            return Err(Diagnostic::new(
                "`range` takes its range without a name",
                range.value.span,
            ));
        }
        let value = self.value(&x.value)?;
        let Datum::Range { lo, hi, .. } = self.value(&range.value)? else {
            return Err(Diagnostic::new(
                "`range` takes a range, such as `200hz..4khz`",
                range.value.span,
            ));
        };
        self.map_value(value, span, |this, x| {
            if x.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "`range` maps a plain signal from 0 to 1, but this is {}",
                        x.unit.describe()
                    ),
                    span,
                ));
            }
            if lo.unit == Unit::Db {
                let (Some(a), Some(b)) = (lo.konst, hi.konst) else {
                    return Err(Diagnostic::new(
                        "the ends of a db range must be constants",
                        span,
                    ));
                };
                let exponent =
                    this.mul(&x, &Sig::constant(Unit::Num, (b / a).ln()), Unit::Num, span)?;
                let grown = this.func(Fn1::Exp, &exponent);
                return this.mul(&grown, &Sig::constant(Unit::Num, a), Unit::Db, span);
            }
            let width = this.binary(BinOp::Sub, hi.clone(), lo.clone(), span)?;
            let scaled = this.binary(BinOp::Mul, x, width, span)?;
            this.binary(BinOp::Add, scaled, lo.clone(), span)
        })
    }

    fn spawn_scope(&mut self, args: &[Arg], span: Span) -> Res<Datum> {
        let usage = || Diagnostic::new("write `spawn(trigger, dur: 100ms, max: 8) { ... }`", span);
        let Some((body, options)) = args.split_last() else {
            return Err(usage());
        };
        let ExprKind::Lambda { params, body } = &body.value.kind else {
            return Err(usage());
        };
        if !params.is_empty() {
            return Err(usage());
        }
        let Some((trigger, options)) = options.split_first() else {
            return Err(usage());
        };
        if trigger.name.is_some() {
            return Err(usage());
        }
        let trigger = self.expr(&trigger.value)?;
        if trigger.unit != Unit::Num || trigger.count() != 1 {
            return Err(usage());
        }
        let mut duration = None;
        let mut max = None;
        for option in options {
            match option.name.as_ref().map(|n| n.name.as_str()) {
                Some("dur") if duration.is_none() => {
                    let value = self.expr(&option.value)?;
                    if value.unit != Unit::Time || value.count() != 1 {
                        return Err(usage());
                    }
                    if value
                        .konst
                        .is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 600.0)
                    {
                        return Err(Diagnostic::new(
                            "spawn duration is above 0 and at most 600sec",
                            option.value.span,
                        ));
                    }
                    duration = Some(value);
                }
                Some("max") if max.is_none() => {
                    let value = self.expr(&option.value)?;
                    let Some(v) = value.konst.filter(|v| {
                        value.unit == Unit::Num && v.fract() == 0.0 && (1.0..=64.0).contains(v)
                    }) else {
                        return Err(Diagnostic::new(
                            "spawn max is a constant whole number from 1 to 64",
                            option.value.span,
                        ));
                    };
                    max = Some(v as usize);
                }
                _ => return Err(usage()),
            }
        }
        let duration = duration.ok_or_else(usage)?;
        let (def, params, given) = self.scope_kernel(body, span);
        self.call_scoped_kernel(
            &def,
            &def.name,
            &params,
            given,
            span,
            Some(DspScope::Spawn {
                max: max.unwrap_or(8),
                trigger,
                duration,
            }),
        )
    }

    /// Make free variables explicit ports; each fx decides when to read them.
    fn scope_kernel(&mut self, body: &[Stmt], span: Span) -> (FnDef, Vec<PParam>, BoundFnArgs) {
        let mut def = funcs::lambda(&[], body, span);
        let capture_in =
            self.mode == Mode::Node && !self.scope.contains_key("in") && funcs::reads(&def, "in");
        if capture_in {
            let input = self.node_input();
            self.scope.insert("in".into(), Datum::Sig(input));
        }
        let mut captures: Vec<_> = self
            .scope
            .iter()
            .filter(|(n, _)| funcs::reads(&def, n))
            .collect();
        captures.sort_by(|a, b| a.0.cmp(b.0));
        let mut params = Vec::new();
        let mut given = Vec::new();
        for (name, value) in captures {
            let id = Ident {
                name: name.clone(),
                span,
            };
            def.params.push(ParamDef {
                name: id.clone(),
                rate: None,
                ty: None,
                default: None,
            });
            params.push(PParam {
                name: id,
                rate: None,
                kind: PKind::Any,
                default: None,
            });
            given.push(Some((
                value.clone(),
                Expr {
                    kind: ExprKind::Name(name.clone()),
                    span,
                },
            )));
        }
        if capture_in {
            self.scope.remove("in");
        }
        (def, params, given)
    }

    fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        let n = name.name.as_str();
        if let Some(receiver) = args.first().filter(|a| a.receiver)
            && let ExprKind::Name(kit) = &receiver.value.kind
            && let Some(sampler) = self.names.samplers.get(kit)
            && sampler.instrument.members.iter().any(|member| member == n)
        {
            let mut selected = args[1..].to_vec();
            selected.push(Arg {
                name: Some(Ident {
                    name: "sample".into(),
                    span: name.span,
                }),
                value: Expr {
                    kind: ExprKind::Name(n.into()),
                    span: name.span,
                },
                receiver: false,
            });
            return self.call_sampler(sampler, name, &selected);
        }
        if let Some(sampler) = self.names.samplers.get(n) {
            return self.call_sampler(sampler, name, args);
        }
        if n == "control"
            && let [body] = args
            && !body.receiver
            && let ExprKind::Lambda { params, body } = &body.value.kind
        {
            if !params.is_empty() {
                return Err(Diagnostic::new("a control body takes no arguments", span));
            }
            let (def, params, given) = self.scope_kernel(body, span);
            return self.call_scoped_kernel(
                &def,
                &def.name,
                &params,
                given,
                span,
                Some(DspScope::Control),
            );
        }
        if n == "oversample" {
            let [factor, body] = args else {
                return Err(Diagnostic::new("write `oversample(4) { ... }`", span));
            };
            let value = self.expr(&factor.value)?;
            let Some(factor) = value
                .konst
                .filter(|f| value.unit == Unit::Num && [2.0, 4.0, 8.0].contains(f))
            else {
                return Err(Diagnostic::new(
                    "oversample factor is 2, 4 or 8",
                    factor.value.span,
                ));
            };
            let ExprKind::Lambda { params, body } = &body.value.kind else {
                return Err(Diagnostic::new("oversample needs a body", span));
            };
            if !params.is_empty() {
                return Err(Diagnostic::new("oversample body takes no arguments", span));
            }
            let (def, params, given) = self.scope_kernel(body, span);
            return self.call_scoped_kernel(
                &def,
                &def.name,
                &params,
                given,
                span,
                Some(DspScope::Oversample(factor as usize)),
            );
        }
        if n == "spawn" {
            return self.spawn_scope(args, span);
        }
        if n == "reset_on" {
            let (trigger, body) = funcs::reset_args(args, span)?;
            if self.mode == Mode::Plain {
                return Err(Diagnostic::new("reset scopes run while playing", span));
            }
            let trig = self.expr(trigger)?;
            if trig.unit != Unit::Num || trig.count() != 1 {
                return Err(Diagnostic::new(
                    "a reset trigger is one plain signal",
                    trigger.span,
                ));
            }
            let first = self.g.node_count();
            let def = funcs::lambda(&[], body, span);
            let id = def.name.clone();
            if !funcs::has_state(&def, &self.names.fns) {
                let saved_scope = self.scope.clone();
                let out = self.out.take();
                let result = self.fn_body(&def, &id, &[], Vec::new());
                self.scope = saved_scope;
                self.out = out;
                self.g.reset_since(first, trig.srcs[0]);
                return result;
            }
            let captures = self
                .scope
                .iter()
                .filter(|(n, _)| funcs::reads(&def, n))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            self.scope
                .insert(id.name.clone(), Datum::Function { def, captures });
            let result = self.call(&id, &[], span);
            self.scope.remove(&id.name);
            let result = result?;
            self.g.reset_since(first, trig.srcs[0]);
            return Ok(result);
        }
        // `f.band`: one result of a function that has several
        if let [receiver] = args
            && receiver.receiver
            && !self.names.fns.contains_key(n)
            && let Some(record) = self.record(&receiver.value)?
        {
            return record
                .iter()
                .find(|(o, _)| o == n)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| {
                    let names: Vec<&str> = record.iter().map(|(o, _)| o.as_str()).collect();
                    Diagnostic::new(format!("this has no result `{n}`"), name.span)
                        .with_help(format!("its results are {}", names.join(", ")))
                });
        }
        // `note.time`: the running voice
        if let [receiver] = args
            && receiver.receiver
            && matches!(&receiver.value.kind, ExprKind::Name(r) if r == "note" && !self.scope.contains_key("note"))
        {
            return self.note_context(name).map(Datum::Sig);
        }
        if let [receiver] = args
            && receiver.receiver
            && matches!(&receiver.value.kind, ExprKind::Name(r) if r == "clock" && !self.scope.contains_key("clock"))
        {
            return self.clock_context(name).map(Datum::Sig);
        }
        if let Some(Datum::Function { mut def, captures }) = self.scope.get(n).cloned() {
            let params = funcs::signature(&def.params, self.tempo)?;
            let mut given = self.fn_args(name, &params, args)?;
            let mut params = params;
            let mut captures: Vec<_> = captures.into_iter().collect();
            captures.sort_by(|a, b| a.0.cmp(&b.0));
            for (n, v) in captures {
                let id = Ident { name: n, span };
                def.params.push(ParamDef {
                    name: id.clone(),
                    rate: None,
                    ty: None,
                    default: None,
                });
                params.push(PParam {
                    name: id.clone(),
                    rate: None,
                    kind: PKind::Any,
                    default: None,
                });
                given.push(Some((
                    v,
                    Expr {
                        kind: ExprKind::Name(id.name),
                        span,
                    },
                )));
            }
            if self.fn_stack.contains(&def.name.name) {
                return Err(Diagnostic::new(
                    "functions can't call themselves",
                    name.span,
                ));
            }
            if funcs::has_state(&def, &self.names.fns) {
                return self.call_kernel(&def, name, &params, given, span);
            }
            let saved = std::mem::take(&mut self.scope);
            let out = self.out.take();
            let in_fn = std::mem::replace(&mut self.in_fn, true);
            self.fn_stack.push(def.name.name.clone());
            let result = self.fn_body(&def, name, &params, given);
            self.fn_stack.pop();
            self.scope = saved;
            self.out = out;
            self.in_fn = in_fn;
            return result;
        }
        if self.names.failed_fns.iter().any(|f| f == n) {
            // already reported where it is defined; this stops the caller without a second report
            return Err(Diagnostic::quiet(name.span));
        }
        if let Some(def) = self.names.fns.get(n) {
            let def = def.clone();
            return self.call_fn(&def, name, args, span, false);
        }
        if let Some(def) = self.names.instrs.get(n) {
            let def = def.clone();
            return self.call_instr(&def, name, args, span);
        }
        if let Some(f) = opcodes::math_fn(n) {
            let [arg] = args else {
                return Err(Diagnostic::new(format!("`{n}` takes one value"), name.span));
            };
            if arg.name.is_some() {
                return Err(Diagnostic::new(
                    format!("`{n}` takes its value without a name"),
                    arg.value.span,
                ));
            }
            let value = self.value(&arg.value)?;
            return self.math_value(f, value, arg.value.span);
        }
        match n {
            "map" | "sum" | "len" | "channels" | "indices" | "at" | "phase" | "step" | "active"
            | "normalize" | "reverse" | "rotate" | "resample" | "concat" | "integrate" | "mix" => {
                return self.array_call(name, args, span);
            }
            "record" => return self.buffer_record(name, args, span),
            "data" if args.len() == 1 && args[0].receiver => {
                return self.sample_data(name, &args[0].value);
            }
            "min" | "max" if args.len() == 1 && args[0].receiver => {
                return self.array_call(name, args, span);
            }
            "split" if args.len() == 1 && args[0].receiver => {
                return Err(Diagnostic::new(
                    "`split` without arguments is now `channels`",
                    name.span,
                )
                .with_help("write `signal.channels` for an array of its channels"));
            }
            "min" | "max" | "pow" | "mod" | "atan2" => return self.combine(name, args),
            "clamp" => return self.clamp(name, args),
            "hold" | "smooth" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                let smooth = n == "smooth";
                return self.map_value(value, span, move |this, v| {
                    if this.sig_rate(&v) > Rate::Control {
                        return Err(Diagnostic::new(
                            format!("`{n}` is for values that change at most once per block"),
                            span,
                        )
                        .with_help(
                            "compute it in a `control { ... }` scope, or read it with `.control`",
                        ));
                    }
                    if !smooth || v.konst.is_some() {
                        return Ok(v);
                    }
                    let srcs: Array<Src> = v
                        .srcs
                        .iter()
                        .map(|&x| {
                            this.g
                                .add(niminal_engine::ops::Ramp::default(), &[("x", x)])
                                .expect("ramp ports")
                        })
                        .collect();
                    Ok(Sig { srcs, ..v })
                });
            }
            "smooth" if args.len() == 2 && args[0].receiver => {
                // `x.smooth(5ms)` is a one-pole lag
                let mut lag = args.to_vec();
                if lag[1].name.is_none() {
                    lag[1].name = Some(Ident {
                        name: "time".into(),
                        span: name.span,
                    });
                }
                let lag_name = Ident {
                    name: "lag".into(),
                    span: name.span,
                };
                return self.call(&lag_name, &lag, span);
            }
            "init" | "control" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                let rate = if n == "init" {
                    Rate::Init
                } else {
                    Rate::Control
                };
                return self.map_value(value, span, |this, v| Ok(this.at_rate(v, Some(rate))));
            }
            "zeros" => return self.zeros(name, args),
            "harmonics" => return self.harmonics(name, args),
            "partials" => return self.partials(name, args),
            "cosines" => return self.cosines(name, args),
            "curve" => return self.curve(name, args),
            "steps" => return self.steps(name, args),
            "poly" => return self.poly(name, args),
            "ramp" => return self.ramp(name, args),
            "cheby" => return self.cheby(name, args),
            "window" => return self.window(name, args),
            "random" => return self.random_table(name, args),
            "file" => return self.file_table(name, args),
            "bank" => return self.bank(name, args),
            _ => {}
        }
        if let Some(v) = self.unit_method(name, args)? {
            return Ok(v);
        }
        self.call_builtin(name, args, span)
    }

    fn call_builtin(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        if name.name == "prev" {
            return self.prev(name, args).map(Datum::Sig);
        }
        if name.name == "late" {
            return self.late(name, args).map(Datum::Sig);
        }
        if name.name == "range" && self.registry.find("range").is_none() {
            return self.range(name, args, span);
        }
        if name.name == "input" && self.registry.find("input").is_none() {
            return self.audio_input(name, args, span).map(Datum::Sig);
        }
        if name.name == "report" {
            return self.report(name, args);
        }
        if name.name == "osc_out" && self.registry.find("osc_out").is_none() {
            return self.osc_out(name, args).map(Datum::Sig);
        }
        if name.name == "to" {
            return Err(Diagnostic::new(
                "`to` can only end the line that sets a node's `out`",
                name.span,
            )
            .with_help("for example `out = in.lpf(cutoff: 1khz).to(space)`"));
        }
        let registry = self.registry;
        let Some(spec) = registry.find(&name.name) else {
            let known: Vec<&str> = registry
                .names()
                .chain(self.names.fns.keys().map(String::as_str))
                .chain(self.names.instrs.keys().map(String::as_str))
                .chain([
                    "min",
                    "max",
                    "pow",
                    "atan2",
                    "mod",
                    "clamp",
                    "prev",
                    "late",
                    "range",
                    "zeros",
                    "harmonics",
                    "partials",
                    "cosines",
                    "curve",
                    "steps",
                    "poly",
                    "ramp",
                    "cheby",
                    "window",
                    "random",
                    "file",
                    "bank",
                ])
                .collect();
            let mut d = Diagnostic::new(format!("unknown function `{}`", name.name), name.span);
            if let Some(c) = closest(&name.name, known) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let op = spec.name.as_str();

        // Match arguments to parameters.
        let names: Vec<&str> = spec.params.iter().map(|p| p.name.as_str()).collect();
        let bound = funcs::bind(op, name.span, &names, spec.positional, args)?;

        // Evaluate each argument according to what the parameter wants.
        let mut vals: Vec<Option<Val>> = Vec::with_capacity(spec.params.len());
        for (param, arg) in spec.params.iter().zip(bound) {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(
                        format!("`{op}` needs an argument `{}`", param.name),
                        span,
                    ));
                }
                vals.push(None);
                continue;
            };
            vals.push(Some(match param.kind {
                Kind::Wave => Val::Wave(self.wave_arg(&arg.value)?),
                Kind::Layout => Val::Layout(layout_from_expr(&arg.value)?),
                Kind::Table => Val::Tables(self.table_arg(&arg.value)?),
                Kind::Signal(_) | Kind::Gain => {
                    let value = self.value(&arg.value)?;
                    self.builtin_value(value, param, &arg.value)?
                }
            }));
        }

        self.dispatch_arrays(spec, &vals, span)
    }

    /// One table, or an array of them (one per channel), known while compiling.
    fn table_arg(&mut self, e: &Expr) -> Res<Vec<Arc<Table>>> {
        let tables = match self.value(e)? {
            Datum::Array(items)
                if !items.is_empty() && items.iter().all(|i| matches!(i, Datum::Table(_))) =>
            {
                items
                    .into_iter()
                    .map(|item| self.as_table(item, e.span))
                    .collect::<Res<Vec<_>>>()?
            }
            other => vec![self.as_table(other, e.span)?],
        };
        if tables.iter().any(|t| t.is_empty()) {
            return Err(Diagnostic::new("a table can't be empty", e.span));
        }
        Ok(tables)
    }

    /// A waveform name, or a table of plain numbers known while compiling that holds one cycle.
    fn wave_arg(&mut self, e: &Expr) -> Res<opcodes::WaveArg> {
        if let ExprKind::Name(n) = &e.kind
            && let Some(w) = opcodes::wave(n)
        {
            return Ok(opcodes::WaveArg::Builtin(w));
        }
        let not_a_wave = || {
            let names: Vec<_> = opcodes::WAVES.iter().map(|(n, _)| *n).collect();
            Diagnostic::new("expected a waveform or a table", e.span).with_help(format!(
                "one of {}, or an array of numbers",
                names.join(", ")
            ))
        };
        let tables = match self.table_arg(e) {
            Err(_) if matches!(e.kind, ExprKind::Name(_)) => return Err(not_a_wave()),
            tables => tables?,
        };
        opcodes::WaveArg::from_tables(&tables).map_err(|m| Diagnostic::new(m, e.span))
    }

    fn builtin_value(&self, value: Datum, param: &opcodes::ParamSpec, arg: &Expr) -> Res<Val> {
        match value {
            Datum::Function { .. } => Err(Diagnostic::new(
                "this argument needs a numeric value",
                arg.span,
            )),
            Datum::Table(_) | Datum::Buffer { .. } | Datum::BufferRef { .. } => {
                Err(Diagnostic::new(
                    "pass a table as the wave of `osc`, not as a signal argument",
                    arg.span,
                ))
            }
            Datum::Array(items) => Ok(Val::Array(
                items
                    .into_iter()
                    .map(|v| self.builtin_value(v, param, arg))
                    .collect::<Res<_>>()?,
            )),
            Datum::Record(fields) => self.builtin_value(fields[0].1.clone(), param, arg),
            Datum::Sig(sig) => {
                if let Some(expected) = call::unit_expected(param.kind, sig.unit) {
                    if param.kind == Kind::Gain {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` expects {}, found {}",
                                param.name,
                                call::expectation(param.kind, expected),
                                sig.unit.describe()
                            ),
                            arg.span,
                        ));
                    }
                    return Err(self.unit_mismatch(&param.name, expected, sig.unit, arg));
                }
                Ok(Val::Sig(sig))
            }
            Datum::Range { .. } => Err(Diagnostic::new("a range isn't a numeric array", arg.span)),
        }
    }

    fn dispatch_arrays(
        &mut self,
        spec: &opcodes::OpSpec,
        vals: &[Option<Val>],
        span: Span,
    ) -> Res<Datum> {
        if !vals.iter().any(|v| matches!(v, Some(Val::Array(_)))) {
            return self.dispatch(spec, vals, span).map(Datum::Sig);
        }
        // These operations consume audio channel layouts, rather than mapping
        // a scalar DSP operation over the array.
        if matches!(spec.build, Build::Pan | Build::ToLayout) {
            let vals = vals
                .iter()
                .map(|v| match v {
                    Some(Val::Array(items)) => {
                        let entries = items
                            .iter()
                            .map(|item| match item {
                                Val::Sig(sig) => Ok((sig.clone(), span)),
                                _ => Err(Diagnostic::new(
                                    "audio channels must be a one-dimensional numeric array",
                                    span,
                                )),
                            })
                            .collect::<Res<Vec<_>>>()?;
                        Ok(Some(Val::Sig(join_channels(&entries, span)?)))
                    }
                    other => Ok(other.clone()),
                })
                .collect::<Res<Vec<_>>>()?;
            return self.dispatch(spec, &vals, span).map(Datum::Sig);
        }
        let layout = vals.iter().find_map(|v| match v {
            Some(Val::Sig(signal)) if signal.count() > 1 => Some(signal.layout),
            _ => None,
        });
        let vals: Vec<_> = vals
            .iter()
            .map(|v| match v {
                Some(Val::Sig(sig)) if sig.count() > 1 => Some(Val::Array(
                    (0..sig.count())
                        .map(|i| Val::Sig(Self::element(sig, i)))
                        .collect(),
                )),
                other => other.clone(),
            })
            .collect();
        let rows = call::rows(&vals)
            .map_err(|m| Diagnostic::new(m, span))?
            .expect("a call with an array argument has rows");
        let items = rows
            .iter()
            .map(|row| self.dispatch_arrays(spec, row, span))
            .collect::<Res<_>>()?;
        let result = Datum::Array(items);
        match layout {
            Some(layout) => Self::signal_array(result, layout, span),
            None => Ok(result),
        }
    }

    fn dispatch(&mut self, spec: &opcodes::OpSpec, vals: &[Option<Val>], span: Span) -> Res<Sig> {
        match spec.build {
            Build::Pan => self.pan(spec, vals, span),
            Build::ToLayout => {
                let (Some(Val::Sig(x)), Some(Val::Layout(layout))) = (&vals[0], &vals[1]) else {
                    unreachable!("to_layout has a signal and a layout")
                };
                self.convert(x, *layout, span)
            }
            _ => self.expand(spec, vals, span),
        }
    }

    /// Build an opcode once per channel of its widest argument.
    fn expand(&mut self, spec: &opcodes::OpSpec, vals: &[Option<Val>], span: Span) -> Res<Sig> {
        let mut width = 1;
        let mut layout = Layout::Mono;
        for (param, val) in spec.params.iter().zip(vals) {
            let Some(Val::Sig(sig)) = val else { continue };
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

        let mut srcs = Vec::with_capacity(width);
        for channel in 0..width {
            let salt = (self.g.node_count() + channel) as u32;
            let build = call::build_args(spec, vals, |s| s.konst, channel, salt);
            let opcode = spec
                .instantiate(&build)
                .map_err(|m| Diagnostic::new(m, span))?
                .expect("only built-in opcodes are expanded");
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
            srcs.push(
                self.g
                    .add(opcode, &wired)
                    .expect("ports match the opcode table"),
            );
        }
        Ok(if width == 1 {
            Sig::signal(srcs[0], Unit::Num)
        } else {
            Sig::channels(srcs, layout)
        })
    }

    /// `pan`: place a mono signal in the master's layout.
    fn pan(&mut self, spec: &opcodes::OpSpec, vals: &[Option<Val>], span: Span) -> Res<Sig> {
        let mut wired: Vec<(&str, Src)> = Vec::new();
        for (param, val) in spec.params.iter().zip(vals) {
            let Some(Val::Sig(sig)) = val else { continue };
            if sig.count() != 1 {
                return Err(Diagnostic::new(
                    format!(
                        "`pan` takes a one-channel signal, but `{}` has {}",
                        param.name,
                        sig.count()
                    ),
                    span,
                )
                .with_help("pan each channel separately, or convert to mono first"));
            }
            wired.push((param.name.as_str(), sig.srcs[0]));
        }

        let master = self.names.master;
        let angles: Arc<[Option<f32>]> = master.speaker_angles().into();
        let srcs: Vec<Src> = (0..master.channels())
            .map(|k| {
                self.g
                    .add(PanChannel::new(angles.clone(), k), &wired)
                    .expect("ports match the opcode table")
            })
            .collect::<Vec<_>>();
        Ok(if srcs.len() == 1 {
            Sig::signal(srcs[0], Unit::Num)
        } else {
            Sig::channels(srcs, master)
        })
    }

    fn unit_mismatch(&self, param: &str, expected: Unit, found: Unit, arg: &Expr) -> Diagnostic {
        let mut d = Diagnostic::new(
            format!(
                "`{param}` expects {}, found {}",
                expected.describe(),
                found.describe()
            ),
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
                (None, Some(suffix)) => d.with_help(format!(
                    "multiply by a value with a unit, such as `* 1{suffix}`"
                )),
                _ => d,
            };
        }
        d
    }

    // ---- envelopes -------------------------------------------------------

    fn env(&mut self, env: &EnvLit) -> Res<Sig> {
        // each step: its value now, and the signal it is read from at note start if it isn't known yet
        let mut inputs: Vec<(EnvField, Src)> = Vec::new();
        let start = self.env_step(&env.start, false)?;
        let unit = start.unit;
        if start.konst.is_none() {
            inputs.push((EnvField::Start, start.srcs[0]));
        }

        let mut segments = Vec::with_capacity(env.segments.len());
        for (k, seg) in env.segments.iter().enumerate() {
            let level = self.env_step(&seg.level, false)?;
            if level.unit != unit {
                return Err(Diagnostic::new(
                    format!(
                        "envelope levels must all have the same unit: this is {}, the first was {}",
                        level.unit.describe(),
                        unit.describe()
                    ),
                    seg.level.span(),
                ));
            }
            let dur = self.env_step(&seg.dur, true)?;
            if dur.konst.is_none() {
                inputs.push((EnvField::Dur(k), dur.srcs[0]));
            }
            if level.konst.is_none() {
                inputs.push((EnvField::Level(k), level.srcs[0]));
            }
            segments.push(Segment::new(
                dur.konst.unwrap_or(0.0) as f32,
                level.konst.unwrap_or(0.0) as f32,
                curve_of(seg.curve),
            ));
        }
        if inputs.len() > niminal_engine::MAX_INPUTS {
            return Err(Diagnostic::new(
                format!(
                    "an envelope can take at most {} of its steps from names; write the others as numbers",
                    niminal_engine::MAX_INPUTS
                ),
                env.start.span(),
            ));
        }

        let looped = env.looped;
        let mut op = Env::new(start.konst.unwrap_or(0.0) as f32, segments, env.sustain_at);
        if looped {
            op = op.looped();
        }
        let (fields, srcs): (Vec<EnvField>, Vec<Src>) = inputs.into_iter().unzip();
        let ports: Vec<String> = (0..srcs.len()).map(|i| format!("v{i}")).collect();
        let wired: Vec<(&str, Src)> = ports.iter().map(String::as_str).zip(srcs).collect();
        let src = self
            .g
            .add(op.with_inputs(fields), &wired)
            .expect("one port per step read at note start");
        Ok(Sig::signal(src, unit))
    }

    /// One step of an envelope: a duration in seconds, or a level.
    fn env_step(&mut self, v: &EnvValue, duration: bool) -> Res<Sig> {
        let sig = match v {
            EnvValue::Lit(n) => self.literal(n.value, n.unit.as_deref(), n.span)?,
            EnvValue::Expr(e) => self.expr(e)?,
        };
        let span = v.span();
        if sig.count() != 1 {
            return Err(Diagnostic::new(
                "an envelope step is one value, not several channels",
                span,
            ));
        }
        if duration && sig.unit != Unit::Time {
            return Err(Diagnostic::new(
                format!("expected a duration here, found {}", sig.unit.describe()),
                span,
            )
            .with_help("levels and durations alternate; durations use `ms`, `sec` or `beats`"));
        }
        if !duration && sig.unit == Unit::Time {
            return Err(
                Diagnostic::new("expected a level here, found a duration", span).with_help(
                    "levels and durations alternate: two durations in a row is an error",
                ),
            );
        }
        if !duration && sig.unit == Unit::Db {
            return Err(Diagnostic::new(
                "db levels in envelopes aren't supported yet",
                span,
            ));
        }
        Ok(sig)
    }
}

impl Lower<'_> {
    // ---- functions -----------------------------------------------------

    fn live_array(
        &mut self,
        value: Datum,
        path: &str,
        kind: &PKind,
        rate: Option<Rate>,
        params: &mut Vec<Param>,
        span: Span,
    ) -> Res<Datum> {
        if *kind == PKind::Buffer {
            return Err(Diagnostic::new(
                "use individual buffer parameters instead of a live array of buffers",
                span,
            ));
        }
        match value {
            Datum::Sig(sig) if sig.count() == 1 => {
                let default = sig
                    .konst
                    .ok_or_else(|| Diagnostic::new("array defaults must be constant", span))?
                    as f32;
                let src = self
                    .g
                    .param(path, default)
                    .map_err(|e| Diagnostic::new(e.to_string(), span))?;
                self.param_rates.push(Rate::Audio);
                let range = if let PKind::Range { lo, hi, .. } = kind {
                    Some((*lo, *hi))
                } else {
                    None
                };
                params.push(Param {
                    name: path.into(),
                    unit: sig.unit,
                    default: Some(default),
                    range,
                });
                Ok(Datum::Sig(self.at_rate(Sig::signal(src, sig.unit), rate)))
            }
            value => {
                let entries = self.entries(value, span)?;
                let kind = if let PKind::Array(inner) = kind {
                    inner
                } else {
                    kind
                };
                Ok(Datum::Array(
                    entries
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            self.live_array(v, &format!("{path}[{i}]"), kind, rate, params, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
        }
    }

    /// Check a value against a parameter, and fit it to the parameter's range.
    fn check_param(&mut self, callee: &str, p: &PParam, v: Datum, arg: &Expr) -> Res<Datum> {
        let name = &p.name.name;
        let span = arg.span;
        let mismatch = |found: &str| {
            Diagnostic::new(
                format!("`{name}` expects {}, found {found}", p.kind.describe()),
                span,
            )
            .with_help(format!("in the call of `{callee}`"))
        };
        let sig = match (&p.kind, v) {
            (PKind::Buffer | PKind::Any, d @ (Datum::Buffer { .. } | Datum::BufferRef { .. })) => {
                return Ok(d);
            }
            (PKind::Buffer, _) => return Err(mismatch("a value")),
            (_, Datum::BufferRef { .. }) => return Err(mismatch("a buffer reference")),
            (PKind::Function | PKind::Any, d @ Datum::Function { .. }) => return Ok(d),
            (PKind::Function, _) => return Err(mismatch("a value")),
            (_, Datum::Function { .. }) => return Err(mismatch("a function")),
            (_, Datum::Record(fields)) => {
                return self.check_param(callee, p, fields[0].1.clone(), arg);
            }
            (PKind::Array(inner), value) => {
                let elements = self.entries(value, span)?;
                let element_param = PParam {
                    kind: *inner.clone(),
                    ..p.clone()
                };
                return Ok(Datum::Array(
                    elements
                        .into_iter()
                        .map(|v| self.check_param(callee, &element_param, v, arg))
                        .collect::<Res<_>>()?,
                ));
            }
            (PKind::Any, d @ (Datum::Array(_) | Datum::Table(_) | Datum::Range { .. })) => {
                return Ok(d);
            }
            (_, Datum::Range { .. }) => return Err(mismatch("a range")),
            (_, Datum::Table(table)) => {
                // Tables are arrays of plain numbers for typed `[num]` parameters.
                if matches!(
                    p.kind,
                    PKind::Unit {
                        unit: Unit::Num,
                        ..
                    }
                ) {
                    return Ok(Datum::Table(table));
                }
                return Err(mismatch("a table"));
            }
            (_, Datum::Buffer { .. }) => return Err(mismatch("a shared buffer")),
            (_, Datum::Array(items)) => {
                // a list where one value is expected: each entry is checked
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.check_param(callee, p, item, arg)?);
                }
                return Ok(Datum::array(out));
            }
            (_, Datum::Sig(s)) => s,
        };
        if let Some(unit) = p.kind.unit() {
            let fits = sig.unit == unit || (unit == Unit::Db && sig.unit == Unit::Num);
            if !fits {
                return Err(self.unit_mismatch(name, unit, sig.unit, arg));
            }
        }
        if p.rate() == Some(Rate::Const) && sig.konst.is_none() {
            return Err(Diagnostic::new(
                format!("`{name}` must be known while compiling"),
                span,
            ));
        }
        if let (PKind::Unit { whole: true, .. }, Some(k)) = (&p.kind, sig.konst)
            && k.fract() != 0.0
        {
            return Err(mismatch(&format!("{k}")));
        }
        let mut sig = sig;
        if let PKind::Range { lo, hi, .. } = p.kind {
            match sig.konst {
                Some(k) if !(lo..=hi).contains(&k) => {
                    return Err(Diagnostic::new(
                        format!("`{name}` is between {lo} and {hi}, but this is {k}"),
                        span,
                    ));
                }
                Some(_) => {}
                None => {
                    let low = Sig::constant(sig.unit, lo);
                    let high = Sig::constant(sig.unit, hi);
                    sig = self.max_sig(&sig, &low, span)?;
                    sig = self.min_sig(&sig, &high, span)?;
                }
            }
        }
        Ok(Datum::Sig(self.at_rate(sig, p.rate())))
    }

    /// The arguments of a call to a user function, worked out in the caller's scope. A choice's name
    /// is its index.
    fn fn_args(&mut self, callee: &Ident, params: &[PParam], args: &[Arg]) -> Res<BoundFnArgs> {
        let names: Vec<&str> = params.iter().map(|p| p.name.name.as_str()).collect();
        let positional = funcs::positional(params.iter().map(PParam::required));
        let bound = funcs::bind(&callee.name, callee.span, &names, positional, args)?;
        let mut given = Vec::with_capacity(params.len());
        for (p, arg) in params.iter().zip(bound) {
            let Some(arg) = arg else {
                given.push(None);
                continue;
            };
            let v = if let PKind::Choice(options) = &p.kind {
                match &arg.value.kind {
                    ExprKind::Name(n) if options.contains(n) => {
                        let i = options.iter().position(|o| o == n).unwrap();
                        Datum::Sig(Sig::constant(Unit::Num, i as f64))
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            format!("`{}` is one of {}", p.name.name, options.join(", ")),
                            arg.value.span,
                        ));
                    }
                }
            } else {
                self.value(&arg.value)?
            };
            given.push(Some((v, arg.value.clone())));
        }
        Ok(given)
    }

    /// A call of a user function. One without state is lowered in place, its parameters standing for
    /// the arguments; one with state becomes a kernel, compiled for this call.
    fn call_fn(
        &mut self,
        def: &FnDef,
        name: &Ident,
        args: &[Arg],
        span: Span,
        instrument: bool,
    ) -> Res<Datum> {
        if self.fn_stack.contains(&def.name.name) {
            let mut path = self.fn_stack.clone();
            path.push(def.name.name.clone());
            return Err(Diagnostic::new(
                format!("functions can't call themselves: {}", path.join(" -> ")),
                name.span,
            ));
        }
        let params = funcs::signature(&def.params, self.tempo)?;
        let given = self.fn_args(name, &params, args)?;
        if !instrument && funcs::has_state(def, &self.names.fns) {
            return self.call_kernel(def, name, &params, given, span);
        }
        let saved_scope = std::mem::take(&mut self.scope);
        let saved_out = self.out.take();
        let saved_in_fn = std::mem::replace(&mut self.in_fn, !instrument);
        self.fn_stack.push(def.name.name.clone());
        let result = self.fn_body(def, name, &params, given);
        self.fn_stack.pop();
        self.scope = saved_scope;
        self.out = saved_out;
        self.in_fn = saved_in_fn;
        result
    }

    fn fn_body(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
    ) -> Res<Datum> {
        for (p, g) in params.iter().zip(given) {
            let (v, arg) = match g {
                Some(g) => g,
                None => match &p.default {
                    Some(d) => {
                        let v = if let PKind::Choice(options) = &p.kind {
                            let ExprKind::Name(n) = &d.kind else {
                                unreachable!("checked")
                            };
                            let i = options.iter().position(|o| o == n).unwrap();
                            Datum::Sig(Sig::constant(Unit::Num, i as f64))
                        } else {
                            self.value(d)?
                        };
                        (v, d.clone())
                    }
                    None => {
                        return Err(Diagnostic::new(
                            format!("`{}` needs an argument `{}`", name.name, p.name.name),
                            name.span,
                        ));
                    }
                },
            };
            let v = self.check_param(&name.name, p, v, &arg)?;
            if let PKind::Choice(options) = &p.kind {
                for (i, o) in options.iter().enumerate() {
                    self.scope
                        .insert(o.clone(), Datum::Sig(Sig::constant(Unit::Num, i as f64)));
                }
            }
            self.scope.insert(p.name.name.clone(), v);
        }
        let body: Vec<&Stmt> = def.body.iter().collect();
        let named = matches!(def.result, FnResult::Named(_));
        let tail = if named {
            for stmt in &body {
                self.stmt(stmt, false)?;
            }
            None
        } else {
            self.run_body_value(&body)?
        };
        if let FnResult::Named(outputs) = &def.result {
            let mut results = Vec::with_capacity(outputs.len());
            for out in outputs {
                let Some(v) = self.scope.get(&out.name).cloned() else {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` is a result of `{}`, but the body never defines it",
                            out.name, def.name.name
                        ),
                        out.span,
                    )
                    .with_help(format!("add a line `{} = ...`", out.name)));
                };
                results.push((out.name.clone(), v));
            }
            return Ok(Datum::Record(results));
        }
        let (value, span) = match (tail, self.out.take()) {
            (Some(_), Some(_)) => {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` sets `out`, so its last line can't also be a value",
                        def.name.name
                    ),
                    def.name.span,
                )
                .with_help("assign it to `out`, or remove the `out` lines"));
            }
            (Some(t), None) => t,
            (None, Some(out)) => (Datum::Sig(out), def.name.span),
            (None, None) if self.sends > 0 && !self.in_fn => {
                (Datum::Sig(Sig::constant(Unit::Num, 0.0)), def.name.span)
            }
            (None, None) => {
                return Err(Diagnostic::new(
                    format!("`{}` must end with the value it gives", def.name.name),
                    def.name.span,
                )
                .with_help("the last line of a function is its result"));
            }
        };
        if let Some(unit) = funcs::result_unit(&def.result)? {
            let found = self.signal_of(value.clone(), span)?.unit;
            if found != unit {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` gives {}, but its result is declared as {}",
                        def.name.name,
                        found.describe(),
                        unit.describe()
                    ),
                    span,
                ));
            }
        }
        Ok(value)
    }

    /// A function with state, compiled to a kernel for this call: one copy per channel of its widest
    /// signal argument, each delivering all of the function's results.
    fn call_kernel(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
        span: Span,
    ) -> Res<Datum> {
        self.call_scoped_kernel(def, name, params, given, span, None)
    }

    fn call_scoped_kernel(
        &mut self,
        def: &FnDef,
        name: &Ident,
        params: &[PParam],
        given: BoundFnArgs,
        span: Span,
        dsp_scope: Option<DspScope>,
    ) -> Res<Datum> {
        if self.mode == Mode::Plain {
            return Err(Diagnostic::new(
                format!(
                    "`{}` has state, so it runs while playing, not while compiling",
                    name.name
                ),
                name.span,
            ));
        }
        let mut kargs = Vec::with_capacity(params.len());
        let mut wires: Vec<(String, Sig)> = Vec::new();
        let mut width = 1;
        let mut layout = Layout::Mono;
        for (p, g) in params.iter().zip(given) {
            let Some((v, arg)) = g else {
                kargs.push(KArg::Missing);
                continue;
            };
            let at = arg.span;
            let v = self.check_param(&name.name, p, v, &arg)?;
            kargs.push(self.kernel_arg(
                v,
                &p.name.name,
                &mut wires,
                &mut width,
                &mut layout,
                at,
            )?);
        }
        if dsp_scope.is_some() {
            // Shared reads hidden in a subvoice still participate in cross-node ordering.
            let mut pending = vec![def.clone()];
            let mut visited = std::collections::HashSet::new();
            while let Some(body) = pending.pop() {
                if !visited.insert(body.name.name.clone()) {
                    continue;
                }
                for (name, (id, _)) in &self.names.buffers {
                    if funcs::reads(&body, name) {
                        self.g.touch_buffer_read(*id);
                    }
                }
                for (name, nested) in &self.names.fns {
                    if funcs::reads(&body, name) {
                        pending.push(nested.clone());
                    }
                }
                for (name, nested) in &self.names.instrs {
                    if funcs::reads(&body, name) {
                        pending.push(FnDef {
                            name: nested.name.clone(),
                            params: nested.params.clone(),
                            body: nested.body.clone(),
                            result: FnResult::Inferred,
                            span: nested.span,
                        });
                    }
                }
            }
        }
        let (kernel, shape) = if dsp_scope.is_some() {
            crate::kernel::compile_scoped_kernel(
                def,
                &kargs,
                self.registry,
                self.names,
                self.tempo,
                true,
                matches!(dsp_scope, Some(DspScope::Control)),
            )?
        } else {
            crate::kernel::compile_kernel(def, &kargs, self.registry, self.names, self.tempo)?
        };
        let kernel = Arc::new(kernel);
        let outputs = kernel.outputs.len();
        let mut per_output: Vec<Vec<Src>> = vec![Vec::with_capacity(width); outputs];
        for channel in 0..width {
            let wired: Vec<(&str, Src)> = wires
                .iter()
                .map(|(n, s)| (n.as_str(), s.pick(channel)))
                .collect();
            let mut wired = wired;
            let op: Box<dyn niminal_engine::Opcode> = match &dsp_scope {
                Some(DspScope::Spawn {
                    max,
                    trigger,
                    duration,
                }) => {
                    wired.push(("__trigger", trigger.srcs[0]));
                    wired.push(("__duration", duration.srcs[0]));
                    Box::new(niminal_engine::ops::Spawn::new(
                        Box::new(Custom::new(kernel.clone())),
                        *max,
                    ))
                }
                Some(DspScope::Oversample(factor)) => {
                    Box::new(niminal_engine::ops::Oversample::new(
                        Box::new(Custom::new(kernel.clone())),
                        *factor,
                    ))
                }
                Some(DspScope::Control) => Box::new(niminal_engine::ops::ControlRate::new(
                    Box::new(Custom::new(kernel.clone())),
                )),
                None => Box::new(Custom::new(kernel.clone())),
            };
            let results = self
                .g
                .add_outputs(op, &wired)
                .map_err(|e| Diagnostic::new(e.to_string(), span))?;
            for (k, src) in results.into_iter().take(outputs).enumerate() {
                if matches!(dsp_scope, Some(DspScope::Control)) {
                    self.note_rate(src, Rate::Control);
                }
                per_output[k].push(src);
            }
        }
        Ok(Self::kernel_result(shape, &per_output, layout))
    }

    fn kernel_arg(
        &mut self,
        value: Datum,
        path: &str,
        wires: &mut Vec<(String, Sig)>,
        width: &mut usize,
        layout: &mut Layout,
        span: Span,
    ) -> Res<KArg> {
        match value {
            Datum::BufferRef { selector, choices } => {
                let selector =
                    self.kernel_arg(Datum::Sig(selector), path, wires, width, layout, span)?;
                for (name, _) in choices.iter() {
                    self.g.touch_buffer_read(self.names.buffers[name].0);
                }
                Ok(KArg::Buffer {
                    selector: Box::new(selector),
                    choices,
                })
            }
            Datum::Function { def, captures } => {
                let mut entries: Vec<_> = captures.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                let captures = entries
                    .into_iter()
                    .map(|(n, v)| {
                        let arg = self.kernel_arg(
                            v,
                            &format!("{path}::{n}"),
                            wires,
                            width,
                            layout,
                            span,
                        )?;
                        Ok((n, arg))
                    })
                    .collect::<Res<_>>()?;
                Ok(KArg::Function { def, captures })
            }
            Datum::Sig(s) => {
                if let Some(value) = s.konst {
                    return Ok(KArg::Const {
                        unit: s.unit,
                        value,
                    });
                }
                if s.count() > 1 {
                    if *width > 1 && s.count() != *width {
                        return Err(Diagnostic::new(
                            "function arguments have different channel counts",
                            span,
                        ));
                    }
                    *width = s.count();
                    *layout = s.layout;
                }
                let arg = KArg::Port {
                    unit: s.unit,
                    rate: self.sig_rate(&s),
                };
                wires.push((path.to_string(), s));
                Ok(arg)
            }
            Datum::Table(table) => Ok(KArg::Table {
                unit: Unit::Num,
                values: table,
            }),
            Datum::Buffer { id, data } => {
                self.g.touch_buffer_read(id);
                Ok(KArg::Buffer {
                    selector: Box::new(KArg::Const {
                        unit: Unit::Num,
                        value: 0.0,
                    }),
                    choices: Choices::new(vec![(path.to_string(), data)]),
                })
            }
            Datum::Array(items) => {
                // Constant numeric arrays retain shared table storage.
                let numbers: Option<Vec<_>> = items
                    .iter()
                    .map(|v| match v {
                        Datum::Sig(s) => s.konst.map(|k| (s.unit, k)),
                        _ => None,
                    })
                    .collect();
                if let Some(values) = numbers
                    && !values.is_empty()
                    && values.iter().all(|(u, _)| *u == values[0].0)
                {
                    return Ok(KArg::Table {
                        unit: values[0].0,
                        values: Arc::new(Table::from_f64(values.into_iter().map(|(_, v)| v), None)),
                    });
                }
                Ok(KArg::Array(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            self.kernel_arg(v, &format!("{path}[{i}]"), wires, width, layout, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
            Datum::Range { lo, hi, inclusive } => match (lo.konst, hi.konst) {
                (Some(lo_value), Some(hi_value)) => Ok(KArg::Range {
                    unit: lo.unit,
                    lo: lo_value,
                    hi: hi_value,
                    inclusive,
                }),
                _ => Err(Diagnostic::new(
                    "a range given to a function must be known while compiling",
                    span,
                )),
            },
            Datum::Record(fields) => {
                self.kernel_arg(fields[0].1.clone(), path, wires, width, layout, span)
            }
        }
    }

    fn kernel_result(shape: KernelResult, sources: &[Vec<Src>], layout: Layout) -> Datum {
        match shape {
            KernelResult::Scalar { index, unit } => Datum::Sig(Sig {
                srcs: sources[index].clone().into(),
                layout: if sources[index].len() == 1 {
                    Layout::Mono
                } else {
                    layout
                },
                unit,
                konst: None,
            }),
            KernelResult::Array(items) => Datum::Array(
                items
                    .into_iter()
                    .map(|v| Self::kernel_result(v, sources, layout))
                    .collect(),
            ),
            KernelResult::Record(fields) => Datum::Record(
                fields
                    .into_iter()
                    .map(|(name, v)| (name, Self::kernel_result(v, sources, layout)))
                    .collect(),
            ),
        }
    }

    /// An instrument played inside another: its body runs as part of this voice. `gain:` is the
    /// note's gain, applied after it, as for any note.
    fn call_instr(&mut self, def: &InstrDef, name: &Ident, args: &[Arg], span: Span) -> Res<Datum> {
        if self.mode != Mode::Instrument || self.in_fn {
            return Err(Diagnostic::new(
                format!(
                    "`{}` is an instrument, so it can only be played inside another instrument",
                    name.name
                ),
                name.span,
            ));
        }
        let declares_gain = def.params.iter().any(|p| p.name.name == GAIN_PARAM);
        let (gain, rest): (Vec<&Arg>, Vec<&Arg>) = args
            .iter()
            .partition(|a| !declares_gain && a.name.as_ref().is_some_and(|n| n.name == GAIN_PARAM));
        let rest: Vec<Arg> = rest.into_iter().cloned().collect();
        let as_fn = FnDef {
            name: def.name.clone(),
            params: def.params.clone(),
            result: FnResult::Inferred,
            body: def.body.clone(),
            span: def.span,
        };
        let result = self.call_fn(&as_fn, name, &rest, span, true)?;
        let sig = self.signal_of(result, span)?;
        match gain.first() {
            Some(g) => {
                let gain = self.expr(&g.value)?;
                if !matches!(gain.unit, Unit::Db | Unit::Num) {
                    return Err(Diagnostic::new(
                        format!(
                            "`gain` expects db or a plain factor, found {}",
                            gain.unit.describe()
                        ),
                        g.value.span,
                    ));
                }
                self.mul(&sig, &gain, Unit::Num, span).map(Datum::Sig)
            }
            None => Ok(Datum::Sig(sig)),
        }
    }

    // ---- built-in functions ----------------------------------------------

    /// The values of a call such as `min(a, b)` or `a.mod(b)`, without names.
    fn plain_args(&mut self, name: &Ident, args: &[Arg], count: usize) -> Res<Vec<(Datum, Span)>> {
        if args.len() != count {
            return Err(Diagnostic::new(
                format!("`{}` takes {count} values", name.name),
                name.span,
            ));
        }
        if let Some(n) = args.iter().find_map(|a| a.name.as_ref()) {
            return Err(Diagnostic::new(
                format!("`{}` takes its values without names", name.name),
                n.span,
            ));
        }
        let mut values = Vec::with_capacity(count);
        for a in args {
            values.push((self.value(&a.value)?, a.value.span));
        }
        Ok(values)
    }

    /// `min(a, b)`, `max(a, b)`, `pow(x, y)` and `a.mod(b)`.
    fn combine(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let mut values = self.plain_args(name, args, 2)?;
        let (b, b_span) = values.pop().unwrap();
        let (a, _) = values.pop().unwrap();
        self.zip_values(a, b, b_span, |this, a, b| {
            this.combine_scalar(name, a, b, b_span)
        })
    }

    fn combine_scalar(&mut self, name: &Ident, a: Sig, b: Sig, b_span: Span) -> Res<Sig> {
        match name.name.as_str() {
            "mod" => self.rem(a, b, b_span),
            "atan2" => {
                self.same_unit(&a, &b, "atan2", b_span)?;
                if let (Some(x), Some(y)) = (a.konst, b.konst) {
                    return Ok(Sig::constant(Unit::Num, x.atan2(y)));
                }
                self.elementwise(&a, &b, Unit::Num, b_span, |g, x, y| {
                    g.add(Atan2, &[("a", x), ("b", y)]).expect("atan2 ports")
                })
            }
            "pow" => {
                if a.unit != Unit::Num || b.unit != Unit::Num {
                    return Err(Diagnostic::new("`pow` expects plain numbers", b_span));
                }
                if let (Some(x), Some(y)) = (a.konst, b.konst) {
                    return Ok(Sig::constant(Unit::Num, x.powf(y)));
                }
                self.elementwise(&a, &b, Unit::Num, b_span, |g, x, y| {
                    g.add(Pow, &[("a", x), ("b", y)]).expect("pow ports")
                })
            }
            m => {
                self.same_unit(&a, &b, m, b_span)?;
                if m == "min" {
                    self.min_sig(&a, &b, b_span)
                } else {
                    self.max_sig(&a, &b, b_span)
                }
            }
        }
    }

    /// `x.clamp(lo..hi)`
    fn clamp(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`clamp` takes a range", name.span)
                .with_help("for example `cutoff.clamp(20hz..18khz)`"));
        };
        let v = self.value(&x.value)?;
        let Datum::Range { lo, hi, .. } = self.value(&range.value)? else {
            return Err(Diagnostic::new(
                "`clamp` takes a range, such as `20hz..18khz`",
                range.value.span,
            ));
        };
        let up = self.extreme_value(v, Datum::Sig(lo), false, name.span)?;
        self.extreme_value(up, Datum::Sig(hi), true, name.span)
    }

    /// `zeros(n)`: a list of `n` zeros.
    fn zeros(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let [count] = args else {
            return Err(Diagnostic::new("`zeros` takes a count", name.span));
        };
        let n = self.whole_number(&count.value, 0, 1 << 22)?;
        Ok(Datum::array(vec![
            Datum::Sig(Sig::constant(Unit::Num, 0.0));
            n
        ]))
    }

    /// A whole number from 1 to `max`, known while compiling.
    fn whole_number(&mut self, e: &Expr, min: usize, max: usize) -> Res<usize> {
        match self.expr(e)? {
            Sig {
                konst: Some(k),
                unit: Unit::Num,
                ..
            } if k.fract() == 0.0 && k >= min as f64 && k <= max as f64 => Ok(k as usize),
            _ => Err(Diagnostic::new(
                format!("expected a whole number from {min} to {max}, known while compiling"),
                e.span,
            )),
        }
    }

    fn const_numbers(&mut self, value: Datum, span: Span, what: &str) -> Res<Vec<f64>> {
        let mut out = Vec::new();
        for item in self.entries(value, span)? {
            match item {
                Datum::Sig(Sig {
                    konst: Some(k),
                    unit: Unit::Num,
                    ..
                }) => out.push(k),
                _ => {
                    return Err(Diagnostic::new(
                        format!("{what} are plain numbers known while compiling"),
                        span,
                    ));
                }
            }
        }
        Ok(out)
    }

    /// `harmonics([1, -1/2, 1/3], size: 4096, phases: [...])`
    fn harmonics(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["amps", "size", "phases"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(amps) = bound[0] else {
            return Err(Diagnostic::new(
                "`harmonics` takes an array of amplitudes",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 2048,
        };
        let weights = {
            let __v = self.value(&amps.value)?;
            self.const_numbers(__v, amps.value.span, "the amplitudes")
        }?;
        let phases = match bound[2] {
            Some(a) => Some({
                let __v = self.value(&a.value)?;
                self.const_numbers(__v, a.value.span, "the phases")
            }?),
            None => None,
        };
        Ok(Datum::Table(Arc::new(crate::tables::harmonics(
            &weights,
            phases.as_deref(),
            size,
        ))))
    }

    fn partials(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["partials", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(list) = bound[0] else {
            return Err(Diagnostic::new(
                "`partials` takes an array of `[ratio, amp]` or `[ratio, amp, phase]`",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 2048,
        };
        let mut rows = Vec::new();
        for item in {
            let __v = self.value(&list.value)?;
            self.entries(__v, list.value.span)
        }? {
            let nums = self.const_numbers(item, list.value.span, "each partial")?;
            let row = match nums.as_slice() {
                [ratio, amp] => [*ratio, *amp, 0.0],
                [ratio, amp, phase] => [*ratio, *amp, *phase],
                _ => {
                    return Err(Diagnostic::new(
                        "each partial is `[ratio, amp]` or `[ratio, amp, phase]`",
                        list.value.span,
                    ));
                }
            };
            rows.push(row);
        }
        Ok(Datum::Table(Arc::new(crate::tables::partials(&rows, size))))
    }

    fn cosines(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["amps", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(amps) = bound[0] else {
            return Err(Diagnostic::new(
                "`cosines` takes an array of amplitudes",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 2048,
        };
        let weights = {
            let __v = self.value(&amps.value)?;
            self.const_numbers(__v, amps.value.span, "the amplitudes")
        }?;
        Ok(Datum::Table(Arc::new(crate::tables::cosines(
            &weights, size,
        ))))
    }

    fn curve(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["points", "size", "shape"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(points) = bound[0] else {
            return Err(Diagnostic::new(
                "`curve` takes an array of `[x, y]` points",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let shape = match bound[2] {
            Some(a) => match &a.value.kind {
                ExprKind::Name(n) if n == "linear" => crate::tables::CurveShape::Linear,
                ExprKind::Name(n) if n == "exp" => crate::tables::CurveShape::Exp,
                ExprKind::Name(n) if n == "cubic" => crate::tables::CurveShape::Cubic,
                ExprKind::Name(n) if n == "spline" => crate::tables::CurveShape::Spline,
                _ => {
                    return Err(Diagnostic::new(
                        "`shape` is `linear`, `exp`, `cubic` or `spline`",
                        a.value.span,
                    ));
                }
            },
            None => crate::tables::CurveShape::Linear,
        };
        let mut rows = Vec::new();
        for item in {
            let __v = self.value(&points.value)?;
            self.entries(__v, points.value.span)
        }? {
            let nums = self.const_numbers(item, points.value.span, "each point")?;
            let [x, y] = nums.as_slice() else {
                return Err(Diagnostic::new("each point is `[x, y]`", points.value.span));
            };
            rows.push([*x, *y]);
        }
        crate::tables::curve(&rows, size, shape)
            .map(|t| Datum::Table(Arc::new(t)))
            .map_err(|e| Diagnostic::new(e, name.span))
    }

    fn steps(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["xs", "ys", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 2, args)?;
        let (Some(xs), Some(ys)) = (bound[0], bound[1]) else {
            return Err(Diagnostic::new(
                "`steps` takes `xs` and `ys` arrays",
                name.span,
            ));
        };
        let size = match bound[2] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let x = {
            let __v = self.value(&xs.value)?;
            self.const_numbers(__v, xs.value.span, "the x positions")
        }?;
        let y = {
            let __v = self.value(&ys.value)?;
            self.const_numbers(__v, ys.value.span, "the y values")
        }?;
        crate::tables::steps(&x, &y, size)
            .map(|t| Datum::Table(Arc::new(t)))
            .map_err(|e| Diagnostic::new(e, name.span))
    }

    fn poly(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["coeffs", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(coeffs) = bound[0] else {
            return Err(Diagnostic::new(
                "`poly` takes an array of coefficients",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let c = {
            let __v = self.value(&coeffs.value)?;
            self.const_numbers(__v, coeffs.value.span, "the coefficients")
        }?;
        Ok(Datum::Table(Arc::new(crate::tables::poly(&c, size))))
    }

    fn ramp(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["start", "end", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 2, args)?;
        let (Some(start), Some(end)) = (bound[0], bound[1]) else {
            return Err(Diagnostic::new("`ramp` takes `start` and `end`", name.span));
        };
        let size = match bound[2] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let s = self.expr(&start.value)?;
        let e = self.expr(&end.value)?;
        let (Some(start_v), Some(end_v)) = (s.konst, e.konst) else {
            return Err(Diagnostic::new(
                "`ramp` ends must be known while compiling",
                name.span,
            ));
        };
        Ok(Datum::Table(Arc::new(crate::tables::ramp(
            start_v, end_v, size,
        ))))
    }

    fn cheby(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["amps", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(amps) = bound[0] else {
            return Err(Diagnostic::new(
                "`cheby` takes an array of amplitudes",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let weights = {
            let __v = self.value(&amps.value)?;
            self.const_numbers(__v, amps.value.span, "the amplitudes")
        }?;
        Ok(Datum::Table(Arc::new(crate::tables::cheby(&weights, size))))
    }

    fn window(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["kind", "size"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(kind_arg) = bound[0] else {
            return Err(Diagnostic::new(
                "`window` takes a kind such as `hann`",
                name.span,
            ));
        };
        let size = match bound[1] {
            Some(a) => self.whole_number(&a.value, 1, 1 << 20)?,
            None => 1024,
        };
        let kind = match &kind_arg.value.kind {
            ExprKind::Name(n) if n == "hann" => crate::tables::WindowKind::Hann,
            ExprKind::Name(n) if n == "hamming" => crate::tables::WindowKind::Hamming,
            ExprKind::Name(n) if n == "blackman" => crate::tables::WindowKind::Blackman,
            ExprKind::Name(n) if n == "sinc" => crate::tables::WindowKind::Sinc,
            ExprKind::Name(n) if n == "tri" => crate::tables::WindowKind::Tri,
            ExprKind::Call { name: n, args } if n.name == "kaiser" => {
                let beta = match args.as_slice() {
                    [a] => self.expr(&a.value)?.konst.unwrap_or(8.0),
                    _ => 8.0,
                };
                crate::tables::WindowKind::Kaiser { beta }
            }
            ExprKind::Call { name: n, args } if n.name == "gauss" => {
                let alpha = match args.as_slice() {
                    [a] => self.expr(&a.value)?.konst.unwrap_or(2.5),
                    _ => 2.5,
                };
                crate::tables::WindowKind::Gauss { alpha }
            }
            ExprKind::Call { name: n, args } if n.name == "tukey" => {
                let alpha = match args.as_slice() {
                    [a] => self.expr(&a.value)?.konst.unwrap_or(0.5),
                    _ => 0.5,
                };
                crate::tables::WindowKind::Tukey { alpha }
            }
            _ => {
                return Err(Diagnostic::new(
                    "`window` kind is hann, hamming, blackman, kaiser(beta), gauss, tukey, sinc or tri",
                    kind_arg.value.span,
                ));
            }
        };
        Ok(Datum::Table(Arc::new(crate::tables::window(kind, size))))
    }

    fn random_table(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["size", "dist", "seed"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(size_arg) = bound[0] else {
            return Err(Diagnostic::new("`random` takes a size", name.span));
        };
        let size = self.whole_number(&size_arg.value, 1, 1 << 20)?;
        let dist = match bound[1] {
            Some(a) => match &a.value.kind {
                ExprKind::Name(n) if n == "uniform" => crate::tables::Dist::Uniform,
                ExprKind::Name(n) if n == "linear" => crate::tables::Dist::Linear,
                ExprKind::Name(n) if n == "triangle" => crate::tables::Dist::Triangle,
                ExprKind::Name(n) if n == "exponential" => crate::tables::Dist::Exponential,
                ExprKind::Name(n) if n == "bilateral" => crate::tables::Dist::Bilateral,
                ExprKind::Name(n) if n == "cauchy" => crate::tables::Dist::Cauchy,
                ExprKind::Name(n) if n == "poisson" => crate::tables::Dist::Poisson,
                _ => {
                    return Err(Diagnostic::new(
                        "`dist` is uniform, linear, triangle, exponential, bilateral, cauchy or poisson",
                        a.value.span,
                    ));
                }
            },
            None => crate::tables::Dist::Uniform,
        };
        let Some(seed_arg) = bound[2] else {
            return Err(Diagnostic::new(
                "`random` needs `seed:` so renders stay identical",
                name.span,
            ));
        };
        let seed = self.whole_number(&seed_arg.value, 0, usize::MAX)? as u64;
        Ok(Datum::Table(Arc::new(crate::tables::random(
            size, dist, seed,
        ))))
    }

    fn file_table(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let names = ["path", "channel", "start", "end"];
        let bound = funcs::bind(&name.name, name.span, &names, 1, args)?;
        let Some(path_arg) = bound[0] else {
            return Err(Diagnostic::new("`file` takes a path string", name.span));
        };
        let path = self.path_text(&path_arg.value)?;
        let path = path.as_str();
        let channel = match bound[1] {
            Some(a) => self.whole_number(&a.value, 0, 64)?,
            None => 0,
        };
        let start = match bound[2] {
            Some(a) => Some(self.whole_number(&a.value, 0, 1 << 30)?),
            None => None,
        };
        let end = match bound[3] {
            Some(a) => Some(self.whole_number(&a.value, 0, 1 << 30)?),
            None => None,
        };
        self.names
            .samples
            .load_channel(path, channel, start, end)
            .map(Datum::Table)
            .map_err(|e| Diagnostic::new(e, path_arg.value.span))
    }

    /// A path in quotes. A `{name}` in it is the whole number the constant `name` holds, and
    /// `{name:3}` pads it with zeros to 3 digits: `"kick{i:2}.wav"` is `kick07.wav`.
    fn path_text(&mut self, e: &Expr) -> Res<String> {
        let ExprKind::Str(text) = &e.kind else {
            return Err(Diagnostic::new("expected a path in quotes", e.span));
        };
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('}') else {
                return Err(Diagnostic::new(
                    "a `{` in a path needs a closing `}`",
                    e.span,
                ));
            };
            let inner = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            let (name, width) = match inner.split_once(':') {
                Some((n, w)) => (n.trim(), w.trim().parse::<usize>().ok()),
                None => (inner.trim(), Some(0)),
            };
            let (Some(width), true) = (
                width,
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'),
            ) else {
                return Err(Diagnostic::new(
                    format!(
                        "`{{{inner}}}` in a path is a constant's name, as `{{i}}` or `{{i:2}}`"
                    ),
                    e.span,
                ));
            };
            let value = self.expr(&Expr {
                kind: ExprKind::Name(name.to_string()),
                span: e.span,
            })?;
            match value {
                Sig {
                    konst: Some(k),
                    unit: Unit::Num,
                    ..
                } if k.fract() == 0.0 && k >= 0.0 => {
                    out.push_str(&format!("{:0width$}", k as u64));
                }
                _ => {
                    return Err(Diagnostic::new(
                        format!("`{name}` in a path is a whole number known while compiling"),
                        e.span,
                    ));
                }
            }
        }
        out.push_str(rest);
        Ok(out)
    }

    fn bank(&mut self, name: &Ident, args: &[Arg]) -> Res<Datum> {
        let [tables] = args else {
            return Err(Diagnostic::new(
                "`bank` takes an array of tables",
                name.span,
            ));
        };
        let list = self.value(&tables.value)?;
        let mut out = Vec::new();
        for item in self.entries(list, tables.value.span)? {
            out.push(Datum::Table(self.as_table(item, tables.value.span)?));
        }
        if out.len() < 2 {
            return Err(Diagnostic::new(
                "a wavetable bank needs at least two tables",
                name.span,
            ));
        }
        Ok(Datum::Array(out.into()))
    }

    /// `x.hz`, `n.ms`, `(-6db).num`...: give a plain value a unit, or take it away.
    fn unit_method(&mut self, name: &Ident, args: &[Arg]) -> Res<Option<Datum>> {
        if !funcs::UNIT_WORDS.contains(&name.name.as_str())
            || !(args.len() == 1 && args[0].receiver)
        {
            return Ok(None);
        }
        let value = self.value(&args[0].value)?;
        let span = args[0].value.span;
        self.map_value(value, span, |this, value| {
            this.unit_value(name, value, span)
        })
        .map(Some)
    }

    fn unit_value(&mut self, name: &Ident, v: Sig, span: Span) -> Res<Sig> {
        if name.name == "num" {
            if v.unit == Unit::Db {
                // a db value is held as a gain factor
                let ln = self.func(
                    Fn1::Log,
                    &Sig {
                        unit: Unit::Num,
                        ..v
                    },
                );
                let k = Sig::constant(Unit::Num, 20.0 / std::f64::consts::LN_10);
                return self.mul(&ln, &k, Unit::Num, span);
            }
            return Ok(Sig {
                unit: Unit::Num,
                ..v
            });
        }
        if v.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!(
                    "`.{}` gives a plain number a unit, but this is {}",
                    name.name,
                    v.unit.describe()
                ),
                span,
            )
            .with_help("take the unit away first with `.num`"));
        }
        let (unit, factor) = literal_sig(1.0, Some(&name.name), self.tempo, name.span)?;
        if unit == Unit::Db {
            let k = Sig::constant(Unit::Num, std::f64::consts::LN_10 / 20.0);
            let e = self.mul(&v, &k, Unit::Num, span)?;
            let g = self.func(Fn1::Exp, &e);
            return Ok(Sig {
                unit: Unit::Db,
                ..g
            });
        }
        self.mul(&v, &Sig::constant(Unit::Num, factor), unit, span)
    }

    /// `note.time`, `note.released`, `note.start`, `note.dur` and `note.index`: the running voice.
    fn note_context(&mut self, name: &Ident) -> Res<Sig> {
        if self.mode != Mode::Instrument || self.in_fn {
            return Err(
                Diagnostic::new("`note` is only available in an instrument", name.span)
                    .with_help("pass `note.time` or `note.released` to a function as an argument"),
            );
        }
        match name.name.as_str() {
            "time" => Ok(Sig::signal(
                self.g.add(NoteTime::default(), &[]).expect("no ports"),
                Unit::Time,
            )),
            "released" => Ok(Sig::signal(
                self.g.add(Released, &[]).expect("no ports"),
                Unit::Num,
            )),
            "start" => Ok(Sig::signal(
                self.g.add(NoteStart::default(), &[]).expect("no ports"),
                Unit::Num,
            )),
            "dur" => Ok(self.read_context(Context::NoteDur, Unit::Time, Rate::Init)),
            "index" => Ok(self.read_context(Context::NoteIndex, Unit::Num, Rate::Init)),
            _ => Err(no_field(
                "note",
                name,
                &["time", "released", "start", "dur", "index"],
            )),
        }
    }

    fn read_context(&mut self, what: Context, unit: Unit, rate: Rate) -> Sig {
        let src = self.g.add(ReadContext::new(what), &[]).expect("no ports");
        self.note_rate(src, rate);
        Sig::signal(src, unit)
    }

    /// `clock.time`: where the session's transport is, in seconds.
    fn clock_context(&mut self, name: &Ident) -> Res<Sig> {
        if self.mode == Mode::Plain || self.in_fn {
            return Err(Diagnostic::new(
                "`clock` is only available in an instrument, node or bus",
                name.span,
            )
            .with_help("pass `clock.time` to a function as an argument"));
        }
        match name.name.as_str() {
            "time" => Ok(self.read_context(Context::ClockTime, Unit::Time, Rate::Audio)),
            _ => Err(no_field("clock", name, &["time"])),
        }
    }

    /// `late(x)`, in a bus send: `x` as it was a block ago, which breaks a loop through buses.
    fn late(&mut self, name: &Ident, args: &[Arg]) -> Res<Sig> {
        let [arg] = args else {
            return Err(Diagnostic::new("`late` takes one value", name.span));
        };
        let Some(ctx) = &mut self.in_send else {
            return Err(Diagnostic::new("`late` is only for a send to a bus", name.span)
                .with_help("for a loop through buses, write `echo += late(out)`; for one sample ago, use `prev`"));
        };
        *ctx = true;
        self.expr(&arg.value)
    }

    /// `prev(x)`: what `x` was one sample ago.
    fn prev(&mut self, name: &Ident, args: &[Arg]) -> Res<Sig> {
        let [arg] = args else {
            return Err(Diagnostic::new("`prev` takes one value", name.span));
        };
        if self.in_send.is_some() {
            return Err(Diagnostic::new(
                "a bus runs in blocks, so a send can't read one sample ago",
                name.span,
            )
            .with_help("use `late(x)` for a send that arrives a block late: `echo += late(out)`"));
        }
        if let ExprKind::Name(n) = &arg.value.kind
            && !self.scope.contains_key(n)
            && self.names.controls.iter().all(|c| c.name != *n)
            && self.names.bus(n).is_none()
            && !self.names.consts.contains_key(n)
        {
            return Err(Diagnostic::new(
                format!("`prev({n})` reads `{n}`, which isn't defined here"),
                arg.value.span,
            )
            .with_help(format!(
                "a loop is written on one line: `{n} = x + prev({n}) * 0.5`"
            )));
        }
        let x = self.expr(&arg.value)?;
        let srcs: Array<Src> = x
            .srcs
            .iter()
            .map(|&s| {
                self.g
                    .add(Delay1::default(), &[("x", s)])
                    .expect("prev ports")
            })
            .collect();
        Ok(Sig {
            srcs,
            konst: None,
            ..x
        })
    }
}

/// Work out a `const` definition while compiling.
pub(crate) fn compile_const(
    value: &Expr,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Datum> {
    let mut lower = Lower::new(registry, names, tempo);
    let v = lower.value(value)?;
    fn known(d: &Datum) -> bool {
        match d {
            Datum::Function { .. } => false,
            Datum::Sig(s) => s.konst.is_some(),
            Datum::Table(_) => true,
            Datum::Buffer { .. } | Datum::BufferRef { .. } => false,
            Datum::Array(items) => items.iter().all(known),
            Datum::Record(fields) => fields.iter().all(|(_, v)| known(v)),
            Datum::Range { lo, hi, .. } => lo.konst.is_some() && hi.konst.is_some(),
        }
    }
    if !known(&v) {
        return Err(
            Diagnostic::new("a `const` must be known while compiling", value.span)
                .with_help("use numbers, ranges, arrays and functions without state"),
        );
    }
    Ok(v)
}

/// Check a function where it is defined, so mistakes in its body are found even if nothing calls it.
/// Untyped parameters take their units from each call, so with one of those only mistakes in names are
/// reported here; the rest are found where it is called.
pub(crate) fn check_fn(def: &FnDef, tempo: Tempo, registry: &Registry, names: &Names) -> Res<()> {
    let params = funcs::signature(&def.params, tempo)?;
    funcs::result_unit(&def.result)?;
    if params
        .iter()
        .any(|p| matches!(p.kind, PKind::Function | PKind::Buffer))
    {
        return Ok(());
    }
    // an untyped parameter takes its unit from each call, and a list parameter its length: the body is
    // checked with two guesses, and a mistake that could come from the guess only counts if both make it
    match check_fn_with(def, tempo, registry, names, Unit::Num, 2) {
        Ok(()) => Ok(()),
        Err((d, true)) if from_a_guess(&d) && reads_untyped(def, d.span) => {
            match check_fn_with(def, tempo, registry, names, Unit::Hz, 3) {
                Err((again, _)) if again.message == d.message && again.span == d.span => Err(d),
                _ => Ok(()),
            }
        }
        Err((d, _)) => Err(d),
    }
}

/// Whether a mistake could come from guessing a unit or a list's length: one about units, or about how
/// many entries a list has.
fn from_a_guess(d: &Diagnostic) -> bool {
    let m = d.message.as_str();
    m.starts_with("can't ")
        || m.contains(" expects ")
        || m.contains("same unit")
        || m.contains(" holds ")
        || m.contains(" gives ")
        || m.contains("plain signal")
        || m.contains(" entries")
        || m.contains("an array of ")
}

/// Whether the body reads an untyped parameter inside `span`.
fn reads_untyped(def: &FnDef, span: Span) -> bool {
    let untyped: Vec<&str> = def
        .params
        .iter()
        .filter(|p| p.default.is_none() && matches!(p.ty, None | Some(TypeSpec::Array(..))))
        .map(|p| p.name.name.as_str())
        .collect();
    fn visit(e: &Expr, names: &[&str], span: Span, found: &mut bool) {
        if let ExprKind::Name(n) = &e.kind
            && names.contains(&n.as_str())
            && e.span.start >= span.start
            && e.span.end <= span.end
        {
            *found = true;
        }
        e.for_each_child(&mut |c| visit(c, names, span, found));
    }
    let mut found = false;
    for s in &def.body {
        for e in s.exprs() {
            visit(e, &untyped, span, &mut found);
        }
    }
    found
}

/// The error, and whether an untyped parameter was given a signal of unit `guess`, or a list parameter
/// a list of `length` entries, to check the body with.
fn check_fn_with(
    def: &FnDef,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
    guess: Unit,
    length: usize,
) -> Result<(), (Diagnostic, bool)> {
    let params = funcs::signature(&def.params, tempo).map_err(|d| (d, false))?;
    funcs::result_unit(&def.result).map_err(|d| (d, false))?;
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut given = Vec::with_capacity(params.len());
    let mut guessed = false;
    for p in &params {
        if p.default.is_some() {
            given.push(None);
            continue;
        }
        let kind = match &p.kind {
            PKind::Any => {
                guessed = true;
                PKind::Unit {
                    unit: guess,
                    whole: false,
                }
            }
            k => k.clone(),
        };
        if p.rate() == Some(Rate::Const)
            && let Some(unit) = kind.unit()
        {
            let v = Datum::Sig(Sig::constant(unit, 1.0));
            given.push(Some((
                v,
                Expr {
                    kind: ExprKind::Name(p.name.name.clone()),
                    span: p.name.span,
                },
            )));
            continue;
        }
        let v = match &kind {
            PKind::Unit { unit, .. } | PKind::Range { unit, .. } => {
                let src = lower
                    .g
                    .param(&p.name.name, 0.0)
                    .map_err(|e| (Diagnostic::new(e.to_string(), p.name.span), false))?;
                lower.param_rates.push(Rate::Audio);
                Some(Datum::Sig(Sig::signal(src, *unit)))
            }
            PKind::Choice(_) => Some(Datum::Sig(Sig::constant(Unit::Num, 0.0))),
            PKind::Array(inner) => {
                guessed = true;
                let unit = inner.unit().unwrap_or(guess);
                Some(Datum::array(vec![
                    Datum::Sig(Sig::constant(unit, 0.0));
                    length
                ]))
            }
            PKind::Any | PKind::Buffer | PKind::Function => None,
        };
        given.push(v.map(|v| {
            (
                v,
                Expr {
                    kind: ExprKind::Name(p.name.name.clone()),
                    span: p.name.span,
                },
            )
        }));
    }
    let name = def.name.clone();
    let result = if funcs::has_state(def, &names.fns) {
        lower
            .call_kernel(def, &name, &params, given, def.span)
            .map(|_| ())
    } else {
        lower.in_fn = true;
        lower.fn_stack.push(def.name.name.clone());
        lower.fn_body(def, &name, &params, given).map(|_| ())
    };
    result.map_err(|d| (d, guessed))
}
