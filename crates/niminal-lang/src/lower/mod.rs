//! Unit checking and lowering, in one pass: each expression is typed and turned
//! into engine graph nodes as it is visited.
//!
//! Signals can have several channels. The engine's nodes are all mono, so a
//! multichannel signal here is a bundle of mono ones, and an opcode applied to
//! it is built once per channel (each with its own state), which is the spec's
//! multichannel expansion. Conversions between layouts are emitted as gain and
//! sum nodes.

mod arith;
mod calls;
mod chain;
mod check;
mod entry;
mod expr;
mod io;
mod lists;
mod sampler;
mod signals;
mod stmt;
mod tables;
mod user_fn;

pub(crate) use chain::*;
pub(crate) use check::*;
pub(crate) use entry::*;
pub(crate) use sampler::*;

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
    /// buses, controls and functions. Instruments, nodes, effects and constants are never taken: a local
    /// of those names is its own, and hides a constant of the same name.
    pub(crate) fn check_local_in(&self, name: &Ident, in_fn: bool) -> Res<()> {
        // an instrument, node or effect is only ever reached by calling it, so a local of that name is its own
        // and clashes with nothing; the other names can be read as plain names, so a local may not hide them
        if let Some(kind) = self.kind_of(&name.name)
            && !matches!(kind, "instrument" | "node" | "fx" | "constant")
            && (!in_fn || matches!(kind, "bus" | "control" | "function"))
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
}
