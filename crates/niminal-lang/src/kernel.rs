//! Compiles a function with state to an engine [`Kernel`]: straight-line
//! register code run once per sample. A kernel is compiled for each call, so a
//! function's untyped parameters take whatever units the call gives them.
//!
//! Every value has a rate, and the code that makes it runs at that rate: once
//! at note start, once per block, or every sample. Functions the body calls are
//! copied in, with their own state.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use niminal_engine::ops::{
    ArraySource, BinOp as Op, Cmp, Fn1, Instr, Interp, Kernel, LaneSrc, Native, NativeInput, Reduce,
};
use niminal_engine::{Array, Port, Table};
use niminal_score::Tempo;

use crate::ast::*;
use crate::call::{self, ArgVal};
use crate::diag::{Diagnostic, Span, closest};
use crate::funcs::{self, Kind, Param};
use crate::lower::{Datum, Names, literal_sig, unknown_name};
use crate::opcodes::{self, Build, Registry, math_fn};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

/// What a call gives one parameter of a function compiled to a kernel.
#[derive(Clone, Debug)]
pub(crate) enum KArg {
    Buffer {
        selector: Box<KArg>,
        choices: crate::lower::BufferChoices,
    },
    Function {
        def: FnDef,
        captures: Vec<(String, KArg)>,
    },
    /// A signal wired into one of the kernel's ports.
    Port {
        unit: Unit,
        rate: Rate,
    },
    /// A value known while compiling.
    Const {
        unit: Unit,
        value: f64,
    },
    /// A list of numbers known while compiling.
    Table {
        unit: Unit,
        values: Arc<Table>,
    },
    /// A range known while compiling.
    Range {
        unit: Unit,
        lo: f64,
        hi: f64,
        inclusive: bool,
    },
    /// Not given: the parameter's default.
    Missing,
    Array(Array<KArg>),
}

#[derive(Clone, Copy, Debug)]
struct Value {
    reg: usize,
    unit: Unit,
    rate: Rate,
    /// The port this is, if it is a parameter wired to one.
    port: Option<usize>,
}

#[derive(Clone, Debug)]
enum KVal {
    Buffer {
        selector: Value,
        choices: crate::lower::BufferChoices,
    },
    Function {
        def: FnDef,
        captures: HashMap<String, KVal>,
    },
    Num(Value),
    /// A list known while compiling.
    Array(Array<KVal>),
    /// A list of numbers shared by every voice.
    Table {
        id: usize,
        unit: Unit,
        values: Arc<Table>,
    },
    /// A list this kernel writes.
    State {
        id: usize,
        unit: Unit,
        len: usize,
    },
    /// A list a vector instruction computed, held in the kernel.
    Lane {
        id: usize,
        unit: Unit,
        len: usize,
    },
    Range {
        unit: Unit,
        lo: f64,
        hi: f64,
        inclusive: bool,
    },
    /// The results of a function with several, by name.
    Record(Vec<(String, KVal)>),
}

/// The shape and units of a kernel's result; leaves refer to engine output slots.
pub(crate) enum KernelResult {
    Scalar { index: usize, unit: Unit },
    Array(Array<KernelResult>),
    Record(Vec<(String, KernelResult)>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Init,
    Block,
    Sample,
}

fn section_of(rate: Rate) -> Section {
    match rate {
        Rate::Const | Rate::Init => Section::Init,
        Rate::Control => Section::Block,
        Rate::Audio => Section::Sample,
    }
}

fn rate_of(section: Section) -> Rate {
    match section {
        Section::Init => Rate::Init,
        Section::Block => Rate::Control,
        Section::Sample => Rate::Audio,
    }
}

/// A state variable of the function being compiled.
struct StateVar {
    name: String,
    slot: usize,
    /// The register holding its value at the start of the sample.
    loaded: usize,
    unit: Unit,
}

/// What one function call being compiled keeps to itself.
#[derive(Default)]
struct Frame {
    scope: HashMap<String, KVal>,
    states: Vec<StateVar>,
    /// Only these names own writable array storage.
    buffers: HashSet<String>,
    /// `prev(y)` written before `y` is: the state it reads.
    forward: HashMap<String, (usize, Span)>,
    /// `prev(expr)`: the state to store the value in at the end of the sample.
    deferred: Vec<(usize, usize)>,
    /// The value of `out`, if the body builds it.
    out: Option<Value>,
}

struct Kb<'a> {
    instrument_scope: bool,
    scope_depth: usize,
    registry: &'a Registry,
    names: &'a Names,
    tempo: Tempo,
    init: Vec<Instr>,
    block: Vec<Instr>,
    code: Vec<Instr>,
    registers: usize,
    state_init: Vec<f64>,
    buffers: Vec<Array<f64>>,
    tables: Vec<Arc<Table>>,
    values: Vec<Array<usize>>,
    value_rates: Vec<Rate>,
    /// The length and rate of each lane.
    lanes: Vec<usize>,
    lane_rates: Vec<Rate>,
    frames: Vec<niminal_engine::ops::FrameScope>,
    ffts: Vec<usize>,
    /// Compiling the code of a frame scope, which can't hold another.
    in_frame: bool,
    natives: Vec<Native>,
    ports: Vec<Port>,
    resets: Vec<niminal_engine::ops::ResetScope>,
    /// Registers that hold a value known while compiling.
    consts: HashMap<usize, f64>,
    frame: Frame,
    /// Set inside a branch: state is only written when this register is not 0.
    guard: Option<usize>,
    /// This kernel is the body of a `control` scope, so everything in it already runs once per step.
    in_control: bool,
    /// Set while working out a value at a fixed rate (a state's starting value, `x.init`).
    force: Option<Section>,
    /// The functions being copied in, outermost first, to catch recursion.
    stack: Vec<String>,
    /// Constants turned into tables, by name.
    const_tables: HashMap<String, KVal>,
}

/// Compile `def` for a call that gives its parameters `args`, in order. Also gives the unit of
/// each result.
pub(crate) fn compile_kernel(
    def: &FnDef,
    args: &[KArg],
    registry: &Registry,
    names: &Names,
    tempo: Tempo,
) -> Res<(Kernel, KernelResult)> {
    compile_scoped_kernel(def, args, registry, names, tempo, false, false)
}

pub(crate) fn compile_scoped_kernel(
    def: &FnDef,
    args: &[KArg],
    registry: &Registry,
    names: &Names,
    tempo: Tempo,
    instrument_scope: bool,
    in_control: bool,
) -> Res<(Kernel, KernelResult)> {
    compile_scope_depth(
        def,
        args,
        registry,
        names,
        tempo,
        instrument_scope,
        usize::from(instrument_scope),
        in_control,
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_scope_depth(
    def: &FnDef,
    args: &[KArg],
    registry: &Registry,
    names: &Names,
    tempo: Tempo,
    instrument_scope: bool,
    scope_depth: usize,
    in_control: bool,
) -> Res<(Kernel, KernelResult)> {
    if scope_depth > 16 {
        return Err(Diagnostic::new(
            "fx scopes nest at most 16 levels",
            def.span,
        ));
    }
    let params = funcs::signature(&def.params, tempo)?;
    let mut kb = Kb {
        instrument_scope,
        scope_depth,
        in_control,
        registry,
        names,
        tempo,
        init: Vec::new(),
        block: Vec::new(),
        code: Vec::new(),
        registers: 0,
        state_init: Vec::new(),
        buffers: Vec::new(),
        tables: Vec::new(),
        values: Vec::new(),
        value_rates: Vec::new(),
        lanes: Vec::new(),
        lane_rates: Vec::new(),
        frames: Vec::new(),
        ffts: Vec::new(),
        in_frame: false,
        natives: Vec::new(),
        ports: Vec::new(),
        resets: Vec::new(),
        consts: HashMap::new(),
        frame: Frame::default(),
        guard: None,
        force: None,
        stack: vec![def.name.name.clone()],
        const_tables: HashMap::new(),
    };
    for (param, arg) in params.iter().zip(args) {
        let v = if matches!(arg, KArg::Missing) {
            let Some(default) = &param.default else {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` needs an argument `{}`",
                        def.name.name, param.name.name
                    ),
                    def.name.span,
                ));
            };
            kb.arg_value(param, default)?
        } else {
            kb.input_arg(&param.name.name, arg)
        };
        let v = kb.check_param(&def.name.name, param, &v, param.name.span)?;
        let v = kb.fit_rate(param, v, param.name.span)?;
        if let Kind::Choice(options) = &param.kind {
            kb.bind_choices(options);
        }
        kb.frame.scope.insert(param.name.name.clone(), v);
    }
    let result = kb.body(def)?;
    let mut outputs = Vec::new();
    let result = kb.output_shape(result, "out", &mut outputs, def.name.span)?;
    let mut kernel = Kernel {
        latency: 0,
        name: def.name.name.clone(),
        resets: kb.resets,
        ports: kb.ports,
        init: kb.init,
        block: kb.block,
        code: kb.code,
        registers: kb.registers,
        state_init: kb.state_init,
        buffers: kb.buffers,
        tables_f64: kb
            .tables
            .iter()
            .map(|t| t.as_f64().into_boxed_slice())
            .collect(),
        tables: kb.tables,
        lanes: kb.lanes,
        frames: kb.frames,
        ffts: kb.ffts,
        values: kb.values,
        outputs,
        natives: kb.natives,
    };
    kernel.compensate_latency();
    Ok((kernel, result))
}

fn not_a_number(e: &Expr) -> Diagnostic {
    Diagnostic::new("expected a number or a signal here, found an array", e.span)
}

impl KVal {
    fn array(items: Vec<KVal>) -> Self {
        Self::Array(items.into())
    }
}

impl Kb<'_> {
    fn input_arg(&mut self, name: &str, arg: &KArg) -> KVal {
        match arg {
            KArg::Buffer { selector, choices } => {
                let KVal::Num(selector) = self.input_arg(name, selector) else {
                    unreachable!()
                };
                KVal::Buffer {
                    selector,
                    choices: choices.clone(),
                }
            }
            KArg::Function { def, captures } => KVal::Function {
                def: def.clone(),
                captures: captures
                    .iter()
                    .map(|(n, v)| (n.clone(), self.input_arg(&format!("{name}::{n}"), v)))
                    .collect(),
            },
            KArg::Port { unit, rate } => {
                let port = self.ports.len();
                self.ports.push(Port::named(name.to_string(), None));
                let reg = self.emit(*rate, |dst| Instr::Input { dst, port });
                KVal::Num(Value {
                    reg,
                    unit: *unit,
                    rate: *rate,
                    port: Some(port),
                })
            }
            KArg::Const { unit, value } => KVal::Num(self.constant(*unit, *value)),
            KArg::Table { unit, values } => self.table(*unit, values.clone()),
            KArg::Array(items) => KVal::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| self.input_arg(&format!("{name}[{i}]"), item))
                    .collect(),
            ),
            KArg::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => KVal::Range {
                unit: *unit,
                lo: *lo,
                hi: *hi,
                inclusive: *inclusive,
            },
            KArg::Missing => unreachable!("defaults are evaluated in the callee"),
        }
    }

    fn output_shape(
        &mut self,
        value: KVal,
        name: &str,
        outputs: &mut Vec<(String, usize)>,
        span: Span,
    ) -> Res<KernelResult> {
        match value {
            KVal::Num(v) => {
                let index = outputs.len();
                outputs.push((name.to_string(), v.reg));
                Ok(KernelResult::Scalar {
                    index,
                    unit: v.unit,
                })
            }
            KVal::Record(fields) => Ok(KernelResult::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| {
                        let shape = self.output_shape(value, &name, outputs, span)?;
                        Ok((name, shape))
                    })
                    .collect::<Res<_>>()?,
            )),
            other if Self::is_array(&other) => {
                let items = self.array_elements(other, span)?;
                Ok(KernelResult::Array(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, value)| {
                            self.output_shape(value, &format!("{name}[{i}]"), outputs, span)
                        })
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "a function returns a number or an array",
                span,
            )),
        }
    }

    fn snapshot(&mut self, value: KVal, span: Span) -> Res<KVal> {
        match value {
            state @ KVal::State { .. } => self.lane_copy(state, span),
            KVal::Array(items) => Ok(KVal::Array(
                items
                    .into_iter()
                    .map(|v| self.snapshot(v, span))
                    .collect::<Res<_>>()?,
            )),
            KVal::Record(fields) => Ok(KVal::Record(
                fields
                    .into_iter()
                    .map(|(name, v)| Ok((name, self.snapshot(v, span)?)))
                    .collect::<Res<_>>()?,
            )),
            other => Ok(other),
        }
    }

    // ---- emitting ----------------------------------------------------

    fn section(&self, rate: Rate) -> Section {
        self.force.unwrap_or(section_of(rate))
    }

    fn rate(&self, rate: Rate) -> Rate {
        match self.force {
            Some(s) => rate.min(rate_of(s)),
            None => rate,
        }
    }

    fn push(&mut self, rate: Rate, instr: Instr) {
        match self.section(rate) {
            Section::Init => self.init.push(instr),
            Section::Block => self.block.push(instr),
            Section::Sample => self.code.push(instr),
        }
    }

    fn emit(&mut self, rate: Rate, make: impl FnOnce(usize) -> Instr) -> usize {
        let dst = self.registers;
        self.registers += 1;
        self.push(rate, make(dst));
        dst
    }

    fn constant(&mut self, unit: Unit, value: f64) -> Value {
        let reg = self.emit(Rate::Const, |dst| Instr::Const { dst, value });
        self.consts.insert(reg, value);
        Value {
            reg,
            unit,
            rate: Rate::Const,
            port: None,
        }
    }

    fn konst(&self, v: &Value) -> Option<f64> {
        self.consts.get(&v.reg).copied()
    }

    fn table(&mut self, unit: Unit, values: Arc<Table>) -> KVal {
        let id = self.tables.len();
        self.tables.push(values.clone());
        KVal::Table { id, unit, values }
    }

    /// A value read where the rate is forced: a port is read again there; anything faster than the
    /// forced rate can't be.
    fn read(&mut self, v: Value, span: Span) -> Res<Value> {
        let Some(forced) = self.force else {
            return Ok(v);
        };
        if v.rate <= rate_of(forced) {
            return Ok(v);
        }
        match v.port {
            Some(port) => {
                let rate = rate_of(forced);
                let reg = self.emit(rate, |dst| Instr::Input { dst, port });
                Ok(Value { reg, rate, ..v })
            }
            None => Err(Diagnostic::new(
                match forced {
                    Section::Init => {
                        "this changes while the note plays, so it isn't known at note start"
                    }
                    _ => "this changes every sample, so it can't be read once per block",
                },
                span,
            )
            .with_help("use the parameters and constants here")),
        }
    }

    fn bin(&mut self, op: Op, a: Value, b: Value, unit: Unit) -> Value {
        if let (Some(x), Some(y)) = (self.konst(&a), self.konst(&b)) {
            return self.constant(unit, op.apply(x, y));
        }
        let rate = self.rate(a.rate.max(b.rate));
        let reg = self.emit(rate, |dst| Instr::Bin {
            dst,
            op,
            a: a.reg,
            b: b.reg,
        });
        Value {
            reg,
            unit,
            rate,
            port: None,
        }
    }

    fn cmp(&mut self, op: Cmp, a: Value, b: Value) -> Value {
        if let (Some(x), Some(y)) = (self.konst(&a), self.konst(&b)) {
            return self.constant(Unit::Num, if op.holds(x, y) { 1.0 } else { 0.0 });
        }
        let rate = self.rate(a.rate.max(b.rate));
        let reg = self.emit(rate, |dst| Instr::Cmp {
            dst,
            op,
            a: a.reg,
            b: b.reg,
        });
        Value {
            reg,
            unit: Unit::Num,
            rate,
            port: None,
        }
    }

    fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        if let Some(k) = self.konst(&c) {
            return if k != 0.0 { a } else { b };
        }
        if a.reg == b.reg {
            return a;
        }
        let rate = self.rate(c.rate.max(a.rate).max(b.rate));
        let reg = self.emit(rate, |dst| Instr::Select {
            dst,
            cond: c.reg,
            a: a.reg,
            b: b.reg,
        });
        Value {
            reg,
            unit: a.unit,
            rate,
            port: None,
        }
    }

    fn call1(&mut self, f: Fn1, a: Value) -> Value {
        if let Some(x) = self.konst(&a) {
            return self.constant(Unit::Num, f.apply(x));
        }
        let rate = self.rate(a.rate);
        let reg = self.emit(rate, |dst| Instr::Call { dst, f, a: a.reg });
        Value {
            reg,
            unit: Unit::Num,
            rate,
            port: None,
        }
    }

    /// 1 where `v` is not 0.
    fn truthy(&mut self, v: Value) -> Value {
        let zero = self.constant(v.unit, 0.0);
        self.cmp(Cmp::Ne, v, zero)
    }

    /// The guard for a branch: the current one, and `c`.
    fn guard_with(&mut self, c: Value) -> Value {
        let c = self.truthy(c);
        match self.guard {
            None => c,
            Some(g) => {
                let g = Value {
                    reg: g,
                    unit: Unit::Num,
                    rate: Rate::Audio,
                    port: None,
                };
                self.bin(Op::Mul, g, c, Unit::Num)
            }
        }
    }

    /// `v` as it was at note start: a port is read in the note-start section, anything else that
    /// changes is caught on its first sample and held.
    fn at_init(&mut self, v: Value) -> Value {
        if v.rate <= Rate::Init {
            return v;
        }
        if let Some(port) = v.port {
            let reg = self.emit(Rate::Init, |dst| Instr::Input { dst, port });
            return Value {
                reg,
                rate: Rate::Init,
                ..v
            };
        }
        let (flag, held) = (self.state_init.len(), self.state_init.len() + 1);
        self.state_init.extend([0.0, 0.0]);
        let started = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot: flag });
        let stored = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot: held });
        let reg = self.emit(Rate::Audio, |dst| Instr::Select {
            dst,
            cond: started,
            a: stored,
            b: v.reg,
        });
        let one = self.emit(Rate::Audio, |dst| Instr::Const { dst, value: 1.0 });
        self.code.push(Instr::StoreState {
            slot: held,
            src: reg,
        });
        self.code.push(Instr::StoreState {
            slot: flag,
            src: one,
        });
        Value {
            reg,
            unit: v.unit,
            rate: Rate::Audio,
            port: None,
        }
    }

    /// `v` read once per block.
    fn at_control(&mut self, v: Value, span: Span) -> Res<Value> {
        if v.rate <= Rate::Control {
            return Ok(v);
        }
        match v.port {
            Some(port) => {
                let reg = self.emit(Rate::Control, |dst| Instr::Input { dst, port });
                Ok(Value {
                    reg,
                    rate: Rate::Control,
                    ..v
                })
            }
            None => Err(Diagnostic::new(
                "inside a function with state, only a parameter can be read once per block",
                span,
            )
            .with_help("apply `.control` where the function is called")),
        }
    }

    // ---- functions -----------------------------------------------------

    /// Run a function's body in the current frame, which holds its parameters, and give its result.
    fn body(&mut self, def: &FnDef) -> Res<KVal> {
        let n = def.body.len();
        let mut tail = None;
        for (i, stmt) in def.body.iter().enumerate() {
            let last = i + 1 == n && matches!(def.result, FnResult::Inferred | FnResult::Unit(_));
            if let Some(v) = self.stmt(stmt, last)? {
                tail = Some((v, stmt_span(stmt)));
            }
        }
        let result = match &def.result {
            FnResult::Named(outputs) => {
                let mut fields = Vec::with_capacity(outputs.len());
                for out in outputs {
                    let Some(v) = self.frame.scope.get(&out.name).cloned() else {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` is a result of `{}`, but the body never defines it",
                                out.name, def.name.name
                            ),
                            out.span,
                        ));
                    };
                    fields.push((out.name.clone(), v));
                }
                KVal::Record(fields)
            }
            _ => {
                let v = match (tail, self.frame.out) {
                    (Some(_), Some(_)) => {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` sets `out`, so its last line can't also be a value",
                                def.name.name
                            ),
                            def.name.span,
                        ));
                    }
                    (Some((v, span)), None) => (v, span),
                    (None, Some(out)) => (KVal::Num(out), def.name.span),
                    (None, None) => {
                        return Err(Diagnostic::new(
                            format!("`{}` must end with the value it gives", def.name.name),
                            def.name.span,
                        )
                        .with_help("the last line of a function is its result"));
                    }
                };
                if let Some(unit) = funcs::result_unit(&def.result)? {
                    self.check_result_unit(&v.0, unit, v.1)?;
                }
                v.0
            }
        };
        let result = self.snapshot(result, def.name.span)?;
        self.finish_frame()?;
        Ok(result)
    }

    fn check_result_unit(&self, value: &KVal, unit: Unit, span: Span) -> Res<()> {
        match value {
            KVal::Num(v) if v.unit == unit => Ok(()),
            KVal::Table { unit: u, .. } | KVal::State { unit: u, .. } if *u == unit => Ok(()),
            KVal::Array(items) => {
                for item in items {
                    self.check_result_unit(item, unit, span)?;
                }
                Ok(())
            }
            _ => Err(Diagnostic::new(
                format!("the result must have unit {}", unit.describe()),
                span,
            )),
        }
    }

    /// Store each state's value for the next sample: only where the guard holds, if there is one.
    fn finish_frame(&mut self) -> Res<()> {
        let guard = self.guard;
        let states = std::mem::take(&mut self.frame.states);
        for s in &states {
            let Some(KVal::Num(v)) = self.frame.scope.get(&s.name).cloned() else {
                continue;
            };
            let src = self.guarded(guard, v.reg, s.loaded);
            self.code.push(Instr::StoreState { slot: s.slot, src });
        }
        let forward: Vec<(String, (usize, Span))> = self.frame.forward.drain().collect();
        for (name, (slot, span)) in forward {
            let v = match self.frame.scope.get(&name) {
                Some(KVal::Num(v)) => *v,
                _ => {
                    return Err(Diagnostic::new(
                        format!("`prev({name})` reads `{name}`, which is never defined"),
                        span,
                    ));
                }
            };
            if v.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!(
                        "`prev({name})` is read before `{name}` is defined, so it is a plain number, but `{name}` is {}",
                        v.unit.describe()
                    ),
                    span,
                ));
            }
            let loaded = self.registers;
            self.registers += 1;
            self.code.push(Instr::LoadState { dst: loaded, slot });
            let src = self.guarded(guard, v.reg, loaded);
            self.code.push(Instr::StoreState { slot, src });
        }
        for (slot, reg) in std::mem::take(&mut self.frame.deferred) {
            self.code.push(Instr::StoreState { slot, src: reg });
        }
        Ok(())
    }

    fn guarded(&mut self, guard: Option<usize>, new: usize, old: usize) -> usize {
        match guard {
            None => new,
            Some(cond) => {
                let dst = self.registers;
                self.registers += 1;
                self.code.push(Instr::Select {
                    dst,
                    cond,
                    a: new,
                    b: old,
                });
                dst
            }
        }
    }

    fn bind_choices(&mut self, options: &[String]) {
        for (i, o) in options.iter().enumerate() {
            let v = self.constant(Unit::Num, i as f64);
            self.frame.scope.insert(o.clone(), KVal::Num(v));
        }
    }

    /// Check a value against a parameter, and fit it to the parameter's rate and range.
    fn check_param(&mut self, callee: &str, param: &Param, v: &KVal, span: Span) -> Res<KVal> {
        let name = &param.name.name;
        let mismatch = |found: &str| {
            Diagnostic::new(
                format!("`{name}` expects {}, found {found}", param.kind.describe()),
                span,
            )
            .with_help(format!("in the call of `{callee}`"))
        };
        match (&param.kind, v) {
            (Kind::Buffer | Kind::Any, KVal::Buffer { .. }) => return Ok(v.clone()),
            (Kind::Buffer, _) | (_, KVal::Buffer { .. }) => return Err(mismatch("a value")),
            (Kind::Function, KVal::Function { .. }) => return Ok(v.clone()),
            (Kind::Function, _) => return Err(mismatch("a value")),
            (_, KVal::Function { .. }) if !matches!(param.kind, Kind::Any) => {
                return Err(mismatch("a function"));
            }
            (Kind::Array(inner), value) => {
                if let Kind::Unit { unit, whole } = **inner {
                    match value {
                        KVal::Table {
                            unit: found,
                            values,
                            ..
                        } => {
                            if *found != unit {
                                return Err(mismatch(found.describe()));
                            }
                            if whole && values.as_f64().iter().any(|v| v.fract() != 0.0) {
                                return Err(mismatch("a fractional array element"));
                            }
                            return Ok(value.clone());
                        }
                        KVal::State { unit: found, .. } if param.rate().is_none() => {
                            if *found != unit {
                                return Err(mismatch(found.describe()));
                            }
                            return Ok(value.clone());
                        }
                        _ => {}
                    }
                }
                let entries = self.array_elements(value.clone(), span)?;
                let element_param = Param {
                    kind: *inner.clone(),
                    ..param.clone()
                };
                let checked = entries
                    .into_iter()
                    .map(|v| self.check_param(callee, &element_param, &v, span))
                    .collect::<Res<_>>()?;
                return Ok(KVal::array(checked));
            }
            (_, KVal::Num(value)) => {
                if let Some(unit) = param.kind.unit() {
                    let fits = value.unit == unit || (unit == Unit::Db && value.unit == Unit::Num);
                    if !fits {
                        return Err(mismatch(value.unit.describe()));
                    }
                }
                if param.rate() == Some(Rate::Const) && self.konst(value).is_none() {
                    return Err(Diagnostic::new(
                        format!("`{name}` must be known while compiling"),
                        span,
                    ));
                }
                if let Kind::Unit { whole: true, .. } = param.kind
                    && let Some(k) = self.konst(value)
                    && k.fract() != 0.0
                {
                    return Err(mismatch(&format!("{k}")));
                }
                let mut value = *value;
                if let Kind::Range { lo, hi, .. } = param.kind {
                    if let Some(k) = self.konst(&value) {
                        if !(lo..=hi).contains(&k) {
                            return Err(Diagnostic::new(
                                format!("`{name}` is between {lo} and {hi}, but this is {k}"),
                                span,
                            ));
                        }
                    } else {
                        let (l, h) = (self.constant(value.unit, lo), self.constant(value.unit, hi));
                        let unit = value.unit;
                        value = self.bin(Op::Max, value, l, unit);
                        value = self.bin(Op::Min, value, h, unit);
                    }
                }
                return Ok(KVal::Num(value));
            }
            (Kind::Any, _) => {}
            (_, other) if Self::is_array(other) => {
                let entries = self.array_elements(other.clone(), span)?;
                return Ok(KVal::Array(
                    entries
                        .into_iter()
                        .map(|v| self.check_param(callee, param, &v, span))
                        .collect::<Res<_>>()?,
                ));
            }
            (_, KVal::Range { .. }) => return Err(mismatch("a range")),
            (_, _) => return Err(mismatch("an array")),
        }
        Ok(v.clone())
    }

    /// A call of another user function: its body is compiled here, with its own state.
    fn call_fn(&mut self, def: &FnDef, name: &Ident, args: &[Arg]) -> Res<KVal> {
        self.call_captured(def, name, args, HashMap::new())
    }

    fn call_captured(
        &mut self,
        def: &FnDef,
        name: &Ident,
        args: &[Arg],
        captures: HashMap<String, KVal>,
    ) -> Res<KVal> {
        if self.stack.contains(&def.name.name) {
            let mut path = self.stack.clone();
            path.push(def.name.name.clone());
            return Err(Diagnostic::new(
                format!("functions can't call themselves: {}", path.join(" -> ")),
                name.span,
            ));
        }
        let params = funcs::signature(&def.params, self.tempo)?;
        let names: Vec<&str> = params.iter().map(|p| p.name.name.as_str()).collect();
        let positional = funcs::positional(params.iter().map(Param::required));
        let bound = funcs::bind(&name.name, name.span, &names, positional, args)?;
        let mut values = Vec::with_capacity(params.len());
        for (param, arg) in params.iter().zip(&bound) {
            values.push(match arg {
                Some(arg) => {
                    let value = self.arg_value(param, &arg.value)?;
                    Some((self.snapshot(value, arg.value.span)?, arg.value.span))
                }
                None => None,
            });
        }
        let saved = std::mem::take(&mut self.frame);
        self.frame.scope = captures;
        self.stack.push(def.name.name.clone());
        let result = (|| {
            for (param, given) in params.iter().zip(values) {
                let (v, span) = match given {
                    Some(g) => g,
                    None => {
                        let Some(default) = &param.default else {
                            return Err(Diagnostic::new(
                                format!("`{}` needs an argument `{}`", name.name, param.name.name),
                                name.span,
                            ));
                        };
                        (self.arg_value(param, default)?, default.span)
                    }
                };
                let v = self.check_param(&name.name, param, &v, span)?;
                let v = self.fit_rate(param, v, span)?;
                if let Kind::Choice(options) = &param.kind {
                    self.bind_choices(options);
                }
                self.frame.scope.insert(param.name.name.clone(), v);
            }
            self.body(def)
        })();
        self.stack.pop();
        self.frame = saved;
        result
    }

    /// A parameter's argument: a choice's name is its index.
    fn arg_value(&mut self, param: &Param, e: &Expr) -> Res<KVal> {
        if let Kind::Choice(options) = &param.kind {
            return match &e.kind {
                ExprKind::Name(n) if options.contains(n) => {
                    let i = options.iter().position(|o| o == n).unwrap();
                    Ok(KVal::Num(self.constant(Unit::Num, i as f64)))
                }
                _ => Err(Diagnostic::new(
                    format!("`{}` is one of {}", param.name.name, options.join(", ")),
                    e.span,
                )),
            };
        }
        self.value(e)
    }

    /// A parameter with a rate sees its value at that rate.
    fn fit_rate(&mut self, param: &Param, v: KVal, span: Span) -> Res<KVal> {
        let Some(rate) = param.rate() else {
            return Ok(v);
        };
        match v {
            KVal::Num(value) => Ok(KVal::Num(match rate {
                Rate::Init => self.at_init(value),
                Rate::Control => self.at_control(value, span)?,
                Rate::Const | Rate::Audio => value,
            })),
            other if Self::is_array(&other) => {
                let entries = self.array_elements(other, span)?;
                Ok(KVal::Array(
                    entries
                        .into_iter()
                        .map(|v| self.fit_rate(param, v, span))
                        .collect::<Res<_>>()?,
                ))
            }
            other => Ok(other),
        }
    }

    // ---- statements ----------------------------------------------------

    /// Run a statement. The value of a trailing expression, if `last`.
    fn stmt(&mut self, stmt: &Stmt, last: bool) -> Res<Option<KVal>> {
        match stmt {
            Stmt::Bind { name, value } => {
                self.bind(name, value)?;
                Ok(None)
            }
            Stmt::State { name, init } => {
                self.declare_state(name, init)?;
                Ok(None)
            }
            Stmt::AddAssign { name, value } => {
                if name.name != "out" {
                    return Err(Diagnostic::new(
                        format!("`+=` in a function only adds to `out`, not `{}`", name.name),
                        name.span,
                    )
                    .with_help(if self.names.bus(&name.name).is_some() {
                        "functions can't send to buses; send from the instrument that calls it"
                            .to_string()
                    } else {
                        format!("write `{0} = {0} + ...`", name.name)
                    }));
                }
                let v = self.num(value)?;
                let sum = match self.frame.out {
                    None => v,
                    Some(prev) => self.add(prev, v, value.span)?,
                };
                self.frame.out = Some(sum);
                Ok(None)
            }
            Stmt::IndexAssign { name, index, value } => {
                if !self.frame.buffers.contains(&name.name) {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` is not a state array, so it can't be written",
                            name.name
                        ),
                        name.span,
                    ));
                }
                let Some(KVal::State { id, unit, len }) = self.frame.scope.get(&name.name).cloned()
                else {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` is not a state array, so it can't be written",
                            name.name
                        ),
                        name.span,
                    )
                    .with_help(format!("declare it with `state {} = zeros(64)`", name.name)));
                };
                if len == 0 {
                    return Err(Diagnostic::new("this array is empty", name.span));
                }
                let i = self.num(index)?;
                if i.unit != Unit::Num {
                    return Err(Diagnostic::new("an index is a plain number", index.span));
                }
                let v = self.num(value)?;
                if v.unit != unit {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` holds {}, but this is {}",
                            name.name,
                            unit.describe(),
                            v.unit.describe()
                        ),
                        value.span,
                    ));
                }
                let cond = self.guard;
                self.code.push(Instr::Write {
                    list: id,
                    idx: i.reg,
                    src: v.reg,
                    cond,
                });
                Ok(None)
            }
            Stmt::Destructure { names, value } => {
                let array = self.value(value)?;
                let entries = self.array_elements(array, value.span)?;
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
                for (n, v) in names.iter().zip(entries) {
                    self.check_name(n)?;
                    self.frame.scope.insert(n.name.clone(), v);
                }
                Ok(None)
            }
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
                    otherwise, then, ..
                } if !(last && otherwise.is_some() && ends_with_value(then)) => {
                    self.if_stmt(e)?;
                    Ok(None)
                }
                ExprKind::For { body, .. } if !(last && ends_with_value(body)) => {
                    self.for_stmt(e)?;
                    Ok(None)
                }
                _ if last => Ok(Some(self.value(e)?)),
                _ => Err(Diagnostic::new("this value is never used", e.span)
                    .with_help("bind it with `name = ...`, or make it the last line")),
            },
        }
    }

    fn check_name(&self, name: &Ident) -> Res<()> {
        if matches!(
            name.name.as_str(),
            "in" | "it"
                | "note"
                | "clock"
                | "state"
                | "pi"
                | "sample_rate"
                | "true"
                | "false"
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
        self.names.check_local_in(name, true)
    }

    fn bind(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        if name.name == "out" {
            let v = self.num(value)?;
            self.frame.out = Some(v);
            return Ok(());
        }
        self.check_name(name)?;
        let v = self.value(value)?;
        if let Some(KVal::State { id, unit, len }) = self.frame.scope.get(&name.name).cloned() {
            if !self.frame.buffers.contains(&name.name) {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` is not a state array, so it can't be written",
                        name.name
                    ),
                    name.span,
                ));
            }
            // `buf = expr` writes every entry: an array of the same length, or one value for all
            let input = self.lane_of(v, value.span)?;
            if input.len.is_some_and(|n| n != len) {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` has {len} entries, but this array has {}",
                        name.name,
                        input.len.unwrap_or(0)
                    ),
                    value.span,
                ));
            }
            if input.unit != unit {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` holds {}, but this is {}",
                        name.name,
                        unit.describe(),
                        input.unit.describe()
                    ),
                    value.span,
                ));
            }
            let cond = self.guard;
            self.code.push(Instr::LaneStore {
                list: id,
                src: input.src,
                cond,
            });
            return Ok(());
        }
        if let Some(s) = self.frame.states.iter().find(|s| s.name == name.name) {
            let KVal::Num(v) = &v else {
                return Err(not_a_number(value));
            };
            if v.unit != s.unit {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` holds {}, but this is {}",
                        name.name,
                        s.unit.describe(),
                        v.unit.describe()
                    ),
                    value.span,
                ));
            }
        }
        let v = self.snapshot(v, value.span)?;
        self.frame.scope.insert(name.name.clone(), v);
        Ok(())
    }

    fn declare_state(&mut self, name: &Ident, init: &Expr) -> Res<()> {
        self.check_name(name)?;
        if self.frame.scope.contains_key(&name.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already defined", name.name),
                name.span,
            ));
        }
        let saved = self.force.replace(Section::Init);
        let start = self.value(init);
        self.force = saved;
        match start? {
            KVal::Num(v) => {
                let slot = self.state_init.len();
                match self.konst(&v) {
                    Some(k) => self.state_init.push(k),
                    None => {
                        self.state_init.push(0.0);
                        self.init.push(Instr::StoreState { slot, src: v.reg });
                    }
                }
                let loaded = self.registers;
                self.registers += 1;
                self.code.push(Instr::LoadState { dst: loaded, slot });
                self.frame.states.push(StateVar {
                    name: name.name.clone(),
                    slot,
                    loaded,
                    unit: v.unit,
                });
                self.frame.scope.insert(
                    name.name.clone(),
                    KVal::Num(Value {
                        reg: loaded,
                        unit: v.unit,
                        rate: Rate::Audio,
                        port: None,
                    }),
                );
            }
            KVal::Table { unit, values, .. } => {
                let id = self.buffers.len();
                self.buffers.push(values.as_f64().into());
                self.frame.buffers.insert(name.name.clone());
                self.frame.scope.insert(
                    name.name.clone(),
                    KVal::State {
                        id,
                        unit,
                        len: values.len(),
                    },
                );
            }
            KVal::Array(items) => {
                let unit = match items.first() {
                    Some(KVal::Num(first)) => first.unit,
                    None => Unit::Num,
                    _ => {
                        return Err(Diagnostic::new(
                            "a state array needs numeric elements",
                            init.span,
                        ));
                    }
                };
                let id = self.buffers.len();
                let len = items.len();
                let mut values = Vec::with_capacity(len);
                for (i, item) in items.into_iter().enumerate() {
                    let KVal::Num(value) = item else {
                        return Err(Diagnostic::new(
                            "a state array needs numeric elements",
                            init.span,
                        ));
                    };
                    if value.unit != unit {
                        return Err(Diagnostic::new(
                            "state array elements must have the same unit",
                            init.span,
                        ));
                    }
                    values.push(self.konst(&value).unwrap_or(0.0));
                    if self.konst(&value).is_none() {
                        let index = self.constant(Unit::Num, i as f64);
                        self.init.push(Instr::Write {
                            list: id,
                            idx: index.reg,
                            src: value.reg,
                            cond: None,
                        });
                    }
                }
                self.buffers.push(values.into());
                self.frame.buffers.insert(name.name.clone());
                self.frame
                    .scope
                    .insert(name.name.clone(), KVal::State { id, unit, len });
            }
            _ => {
                return Err(Diagnostic::new(
                    "a state starts as a number, or as an array such as `zeros(64)`",
                    init.span,
                ));
            }
        }
        Ok(())
    }

    fn block(&mut self, stmts: &[Stmt], value: bool, span: Span) -> Res<Option<KVal>> {
        let outer: Vec<String> = self.frame.scope.keys().cloned().collect();
        let mut tail = None;
        for (i, s) in stmts.iter().enumerate() {
            if matches!(s, Stmt::State { .. }) {
                let Stmt::State { name, .. } = s else {
                    unreachable!()
                };
                return Err(Diagnostic::new(
                    "state is declared at the top of a function, not inside a block",
                    name.span,
                ));
            }
            let last = value && i + 1 == stmts.len();
            if let Some(v) = self.stmt(s, last)? {
                tail = Some(v);
            }
        }
        self.frame.scope.retain(|k, _| outer.contains(k));
        if value && tail.is_none() {
            return Err(Diagnostic::new("this block must end with a value", span));
        }
        Ok(tail)
    }

    fn if_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::If {
            cond,
            then,
            otherwise,
        } = &e.kind
        else {
            unreachable!()
        };
        let c = self.num(cond)?;
        if let Some(k) = self.konst(&c) {
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
        let saved_scope = self.frame.scope.clone();
        let saved_out = self.frame.out;
        let saved_guard = self.guard;

        let g = self.guard_with(c);
        self.guard = Some(g.reg);
        self.block(then, false, e.span)?;
        let then_scope = std::mem::replace(&mut self.frame.scope, saved_scope.clone());
        let then_out = std::mem::replace(&mut self.frame.out, saved_out);

        let not_c = {
            let zero = self.constant(c.unit, 0.0);
            self.cmp(Cmp::Eq, c, zero)
        };
        self.guard = saved_guard;
        let g = self.guard_with(not_c);
        self.guard = Some(g.reg);
        if let Some(stmts) = otherwise {
            self.block(stmts, false, e.span)?;
        }
        let else_scope = std::mem::replace(&mut self.frame.scope, saved_scope);
        let else_out = std::mem::replace(&mut self.frame.out, saved_out);
        self.guard = saved_guard;

        let names: Vec<String> = self.frame.scope.keys().cloned().collect();
        for name in names {
            let (a, b) = (&then_scope[&name], &else_scope[&name]);
            let merged = if same(a, b) {
                a.clone()
            } else {
                self.select_value(c, a.clone(), b.clone(), e.span)?
            };
            self.frame.scope.insert(name, merged);
        }
        self.frame.out = match (then_out, else_out) {
            (Some(a), Some(b)) => Some(self.select(c, a, b)),
            (Some(a), None) => {
                let zero = self.constant(a.unit, 0.0);
                Some(self.select(c, a, zero))
            }
            (None, Some(b)) => {
                let zero = self.constant(b.unit, 0.0);
                Some(self.select(c, zero, b))
            }
            (None, None) => None,
        };
        Ok(())
    }

    /// The entries a `for` runs over.
    /// Materialize a fixed-length array's current elements. Storage is an
    /// implementation choice, never a restriction on array operations.
    fn array_elements(&mut self, value: KVal, span: Span) -> Res<Vec<KVal>> {
        match value {
            KVal::Array(items) => Ok(items.into_vec()),
            KVal::Table { unit, values, .. } => Ok(values
                .as_f64()
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect()),
            KVal::State { id, unit, len } => Ok((0..len)
                .map(|k| {
                    let index = self.constant(Unit::Num, k as f64);
                    KVal::Num(self.read_array(ArraySource::State(id), unit, index, None))
                })
                .collect()),
            KVal::Lane { id, unit, len } => Ok((0..len)
                .map(|k| {
                    let index = self.constant(Unit::Num, k as f64);
                    KVal::Num(self.read_array(ArraySource::Lane(id), unit, index, None))
                })
                .collect()),
            KVal::Num(v) => Ok(vec![KVal::Num(v)]),
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => Ok(range_numbers(lo, hi, inclusive, span)?
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect()),
            _ => Err(Diagnostic::new("expected an array here", span)),
        }
    }

    fn is_array(value: &KVal) -> bool {
        matches!(
            value,
            KVal::Array(_) | KVal::Table { .. } | KVal::State { .. } | KVal::Lane { .. }
        )
    }

    fn entries(&mut self, e: &Expr) -> Res<Vec<KVal>> {
        let value = self.value(e)?;
        match value {
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => Ok(range_numbers(lo, hi, inclusive, e.span)?
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect()),
            other => self.array_elements(other, e.span),
        }
    }

    fn for_stmt(&mut self, e: &Expr) -> Res<()> {
        let ExprKind::For { var, iter, body } = &e.kind else {
            unreachable!()
        };
        self.check_name(var)?;
        if self.frame.scope.contains_key(&var.name) {
            return Err(Diagnostic::new(
                format!("`{}` is already in use", var.name),
                var.span,
            ));
        }
        for entry in self.entries(iter)? {
            self.frame.scope.insert(var.name.clone(), entry);
            self.block(body, false, e.span)?;
            self.frame.scope.remove(&var.name);
        }
        Ok(())
    }

    // ---- expressions ---------------------------------------------------

    /// A number or a signal: anything else is an error.
    fn num(&mut self, e: &Expr) -> Res<Value> {
        match self.value(e)? {
            KVal::Num(v) => Ok(v),
            KVal::Record(fields) => match &fields[0].1 {
                KVal::Num(v) => Ok(*v),
                _ => Err(not_a_number(e)),
            },
            _ => Err(not_a_number(e)),
        }
    }

    fn value(&mut self, e: &Expr) -> Res<KVal> {
        match &e.kind {
            ExprKind::Num { value, unit } => {
                let (unit, value) = literal_sig(*value, unit.as_deref(), self.tempo, e.span)?;
                Ok(KVal::Num(self.constant(unit, value)))
            }
            ExprKind::Name(name) => self.name(name, e.span),
            ExprKind::Neg(inner) => {
                let value = self.value(inner)?;
                self.negate_value(value, e.span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                self.binary_value(*op, l, r, e.span)
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let (a, b) = (self.value(lhs)?, self.value(rhs)?);
                self.zip_values(a, b, e.span, |this, a, b| {
                    if a.unit != b.unit {
                        return Err(Diagnostic::new(
                            format!(
                                "can't compare {} and {}",
                                a.unit.describe(),
                                b.unit.describe()
                            ),
                            e.span,
                        ));
                    }
                    Ok(this.cmp(cmp_op(*op), a, b))
                })
            }
            ExprKind::Logic { op, lhs, rhs } => {
                let (l, r) = (self.num(lhs)?, self.num(rhs)?);
                let (l, r) = (self.truthy(l), self.truthy(r));
                Ok(KVal::Num(match op {
                    LogicOp::And => self.bin(Op::Mul, l, r, Unit::Num),
                    LogicOp::Or => self.bin(Op::Max, l, r, Unit::Num),
                }))
            }
            ExprKind::Not(inner) => {
                let v = self.num(inner)?;
                let zero = self.constant(v.unit, 0.0);
                Ok(KVal::Num(self.cmp(Cmp::Eq, v, zero)))
            }
            ExprKind::If {
                cond,
                then,
                otherwise,
            } => {
                let Some(otherwise) = otherwise else {
                    return Err(Diagnostic::new(
                        "an `if` used as a value needs an `else`",
                        e.span,
                    ));
                };
                let c = self.num(cond)?;
                if let Some(k) = self.konst(&c) {
                    let taken = if k != 0.0 { then } else { otherwise };
                    return Ok(self.block(taken, true, e.span)?.expect("a value"));
                }
                let saved = self.guard;
                let g = self.guard_with(c);
                self.guard = Some(g.reg);
                let a = self.block(then, true, e.span)?.expect("a value");
                let not_c = {
                    let zero = self.constant(c.unit, 0.0);
                    self.cmp(Cmp::Eq, c, zero)
                };
                self.guard = saved;
                let g = self.guard_with(not_c);
                self.guard = Some(g.reg);
                let b = self.block(otherwise, true, e.span)?.expect("a value");
                self.guard = saved;
                self.select_value(c, a, b, e.span)
            }
            ExprKind::For { var, iter, body } => {
                self.check_name(var)?;
                let mut results = Vec::new();
                for entry in self.entries(iter)? {
                    self.frame.scope.insert(var.name.clone(), entry);
                    let v = self.block(body, true, e.span);
                    self.frame.scope.remove(&var.name);
                    results.push(self.snapshot(v?.expect("a value"), e.span)?);
                }
                Ok(KVal::array(results))
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let (l, h) = (self.num(lo)?, self.num(hi)?);
                let (Some(a), Some(b)) = (self.konst(&l), self.konst(&h)) else {
                    return Err(Diagnostic::new(
                        "the ends of a range must be known while compiling",
                        e.span,
                    ));
                };
                if l.unit != h.unit {
                    return Err(Diagnostic::new(
                        "the ends of a range need the same unit",
                        e.span,
                    ));
                }
                Ok(KVal::Range {
                    unit: l.unit,
                    lo: a,
                    hi: b,
                    inclusive: *inclusive,
                })
            }
            ExprKind::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    let value = self.value(i)?;
                    out.push(self.snapshot(value, i.span)?);
                }
                Ok(KVal::array(out))
            }
            ExprKind::Index { list, index } => self.index(list, index, e.span),
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Lambda { params, body } => {
                let def = funcs::lambda(params, body, e.span);
                let captures = self
                    .frame
                    .scope
                    .iter()
                    .filter(|(n, _)| funcs::reads(&def, n))
                    .map(|(n, v)| (n.clone(), v.clone()))
                    .collect();
                Ok(KVal::Function { def, captures })
            }
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can't be used inside a function with state",
                e.span,
            )),
            ExprKind::Env(env) if self.instrument_scope => self.envelope(env, e.span),
            ExprKind::Env(_) => Err(Diagnostic::new(
                "envelopes can't be used inside a function with state",
                e.span,
            )
            .with_help("make the envelope in the instrument and pass it in as an argument")),
        }
    }

    fn emit_native(
        &mut self,
        op: Box<dyn niminal_engine::Opcode>,
        inputs: Vec<NativeInput>,
        unit: Unit,
    ) -> KVal {
        let index = self.natives.len();
        self.natives.push(Native { op, inputs });
        let guard = self.guard;
        let reg = self.emit(Rate::Audio, |dst| Instr::Native { dst, index, guard });
        KVal::Num(Value {
            reg,
            unit,
            rate: Rate::Audio,
            port: None,
        })
    }

    fn envelope(&mut self, env: &EnvLit, span: Span) -> Res<KVal> {
        use niminal_engine::ops::{Env, EnvField, Segment};
        let mut values = Vec::new();
        for step in env.values() {
            let value = match step {
                EnvValue::Lit(n) => {
                    let (unit, value) =
                        literal_sig(n.value, n.unit.as_deref(), self.tempo, n.span)?;
                    self.constant(unit, value)
                }
                EnvValue::Expr(e) => self.num(e)?,
            };
            values.push(value);
        }
        let start = values[0];
        if start.unit == Unit::Time || start.unit == Unit::Db {
            return Err(Diagnostic::new(
                "an envelope level cannot be a duration or db",
                span,
            ));
        }
        let mut fields = Vec::new();
        let mut inputs = Vec::new();
        if self.konst(&start).is_none() {
            fields.push(EnvField::Start);
            inputs.push(NativeInput::Reg(start.reg));
        }
        let mut segments = Vec::new();
        for (i, segment) in env.segments.iter().enumerate() {
            if values[1 + i * 2].unit != Unit::Time {
                return Err(Diagnostic::new(
                    "an envelope duration needs a time",
                    segment.dur.span(),
                ));
            }
            if values[2 + i * 2].unit != start.unit {
                return Err(Diagnostic::new(
                    "envelope levels need the same unit",
                    segment.level.span(),
                ));
            }
            let duration = values[1 + i * 2];
            let level = values[2 + i * 2];
            if self.konst(&duration).is_none() {
                fields.push(EnvField::Dur(i));
                inputs.push(NativeInput::Reg(duration.reg));
            }
            if self.konst(&level).is_none() {
                fields.push(EnvField::Level(i));
                inputs.push(NativeInput::Reg(level.reg));
            }
            segments.push(Segment::new(
                self.konst(&duration).unwrap_or(0.0) as f32,
                self.konst(&level).unwrap_or(0.0) as f32,
                crate::lower::curve_of(segment.curve),
            ));
        }
        if fields.len() > niminal_engine::MAX_INPUTS {
            return Err(Diagnostic::new(
                "too many envelope inputs in an fx scope",
                span,
            ));
        }
        let mut op = Env::new(
            self.konst(&start).unwrap_or(0.0) as f32,
            segments,
            env.sustain_at,
        )
        .with_inputs(fields);
        if env.looped {
            op = op.looped();
        }
        Ok(self.emit_native(Box::new(op), inputs, start.unit))
    }

    fn name(&mut self, name: &str, span: Span) -> Res<KVal> {
        if let Some(v) = self.frame.scope.get(name).cloned() {
            return match v {
                other if self.force.is_some() => {
                    self.map_value(other, span, |this, value| this.read(value, span))
                }
                other => Ok(other),
            };
        }
        if let Some((_, buffer)) = self.names.buffers.get(name).cloned() {
            return Ok(KVal::Buffer {
                selector: self.constant(Unit::Num, 0.0),
                choices: crate::lower::Choices::new(vec![(name.to_string(), buffer)]),
            });
        }
        if let Some(def) = self.names.fns.get(name) {
            return Ok(KVal::Function {
                def: def.clone(),
                captures: HashMap::new(),
            });
        }
        match name {
            "pi" => return Ok(KVal::Num(self.constant(Unit::Num, std::f64::consts::PI))),
            "true" => return Ok(KVal::Num(self.constant(Unit::Num, 1.0))),
            "false" => return Ok(KVal::Num(self.constant(Unit::Num, 0.0))),
            "sample_rate" => {
                let reg = self.emit(Rate::Init, |dst| Instr::SampleRate { dst });
                return Ok(KVal::Num(Value {
                    reg,
                    unit: Unit::Hz,
                    rate: Rate::Init,
                    port: None,
                }));
            }
            "note" => {
                return Err(
                    Diagnostic::new("`note` is only available in an instrument", span)
                        .with_help("pass `note.time` or `note.released` in as an argument"),
                );
            }
            "clock" => {
                return Err(Diagnostic::new(
                    "`clock` is only available in an instrument, node or bus",
                    span,
                )
                .with_help("pass `clock.time` in as an argument"));
            }
            _ => {}
        }
        if let Some(v) = self.const_tables.get(name) {
            return Ok(v.clone());
        }
        if let Some(d) = self.names.consts.get(name) {
            let v = self.const_value(d, span)?;
            if matches!(v, KVal::Table { .. }) {
                self.const_tables.insert(name.to_string(), v.clone());
            }
            return Ok(v);
        }
        if let Ok(note @ niminal_score::Value::Note(_)) = name.parse::<niminal_score::Value>() {
            let value = note
                .as_hz_in(&self.tempo.tuning)
                .expect("a note is a frequency");
            return Ok(KVal::Num(self.constant(Unit::Hz, value)));
        }
        let known = self
            .frame
            .scope
            .keys()
            .map(String::as_str)
            .chain(["pi", "sample_rate"]);
        Err(unknown_name(name, span, known))
    }

    /// A constant from a `const` definition.
    fn const_value(&mut self, d: &Datum, span: Span) -> Res<KVal> {
        match d {
            Datum::Function { .. } => Err(Diagnostic::new(
                "a function is not a numeric constant",
                span,
            )),
            Datum::Record(fields) => Ok(KVal::Record(
                fields
                    .iter()
                    .map(|(name, v)| Ok((name.clone(), self.const_value(v, span)?)))
                    .collect::<Res<_>>()?,
            )),
            Datum::Sig(s) => match s.konst() {
                Some(k) => Ok(KVal::Num(self.constant(s.unit(), k))),
                None => Err(Diagnostic::new("this constant isn't a number", span)),
            },
            Datum::Table(table) => Ok(self.table(Unit::Num, table.clone())),
            Datum::Buffer { .. } | Datum::BufferRef { .. } => Err(Diagnostic::new(
                "a shared buffer isn't a compile-time constant",
                span,
            )),
            Datum::Range { lo, hi, inclusive } => match (lo.konst(), hi.konst()) {
                (Some(a), Some(b)) => Ok(KVal::Range {
                    unit: lo.unit(),
                    lo: a,
                    hi: b,
                    inclusive: *inclusive,
                }),
                _ => Err(Diagnostic::new(
                    "this range isn't known while compiling",
                    span,
                )),
            },
            Datum::Array(items) => {
                let numbers: Option<Vec<(Unit, f64)>> = items
                    .iter()
                    .map(|i| match i {
                        Datum::Sig(s) => s.konst().map(|k| (s.unit(), k)),
                        _ => None,
                    })
                    .collect();
                match numbers {
                    Some(n) if !n.is_empty() && n.iter().all(|(u, _)| *u == n[0].0) => {
                        let values = Arc::new(Table::from_f64(n.iter().map(|(_, k)| *k), None));
                        Ok(self.table(n[0].0, values))
                    }
                    _ => {
                        let mut out = Vec::with_capacity(items.len());
                        for i in items {
                            out.push(self.const_value(i, span)?);
                        }
                        Ok(KVal::array(out))
                    }
                }
            }
        }
    }

    fn extreme_value(&mut self, a: KVal, b: KVal, minimum: bool, span: Span) -> Res<KVal> {
        let op = if minimum { Op::Min } else { Op::Max };
        if let Some(lane) = self.lane_binary(op, |a, b| (a == b).then_some(a), &a, &b, span)? {
            return Ok(lane);
        }
        self.zip_values(a, b, span, |this, a, b| {
            if a.unit != b.unit {
                return Err(Diagnostic::new(
                    "array elements must have the same unit",
                    span,
                ));
            }
            Ok(this.bin(if minimum { Op::Min } else { Op::Max }, a, b, a.unit))
        })
    }

    fn math_value(&mut self, f: Fn1, value: KVal, span: Span) -> Res<KVal> {
        if let KVal::State {
            unit: Unit::Num, ..
        }
        | KVal::Lane {
            unit: Unit::Num, ..
        } = value
            && self.force.is_none()
            && let Some(input) = self.lane_input(&value)
        {
            let len = input.len.expect("an array has a length");
            let rate = self.rate(input.rate);
            let dst = self.new_lane(len, rate);
            self.push(
                rate,
                Instr::LaneCall {
                    dst,
                    f,
                    a: input.src,
                },
            );
            return Ok(KVal::Lane {
                id: dst,
                unit: Unit::Num,
                len,
            });
        }
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
            Ok(this.call1(f, value))
        })
    }

    fn select_value(&mut self, condition: Value, a: KVal, b: KVal, span: Span) -> Res<KVal> {
        if let (KVal::Num(a), KVal::Num(b)) = (&a, &b) {
            if a.unit != b.unit {
                return Err(Diagnostic::new("both branches need the same unit", span));
            }
            return Ok(KVal::Num(self.select(condition, *a, *b)));
        }
        if Self::is_array(&a) && Self::is_array(&b) {
            let a = self.array_elements(a, span)?;
            let b = self.array_elements(b, span)?;
            if a.len() != b.len() {
                return Err(Diagnostic::new(
                    "both branches need the same array shape",
                    span,
                ));
            }
            return Ok(KVal::Array(
                a.into_iter()
                    .zip(b)
                    .map(|(a, b)| self.select_value(condition, a, b, span))
                    .collect::<Res<_>>()?,
            ));
        }
        Err(Diagnostic::new(
            "both branches need matching numbers or array shapes",
            span,
        ))
    }

    fn negate_value(&mut self, value: KVal, span: Span) -> Res<KVal> {
        if let KVal::State { unit, .. } | KVal::Lane { unit, .. } = value
            && unit != Unit::Db
            && self.force.is_none()
        {
            let minus_one = self.constant(Unit::Num, -1.0);
            if let Some(lane) =
                self.lane_binary(Op::Mul, |a, _| Some(a), &value, &KVal::Num(minus_one), span)?
            {
                return Ok(lane);
            }
        }
        self.map_value(value, span, |this, v| {
            if v.unit == Unit::Db {
                let one = this.constant(Unit::Num, 1.0);
                return Ok(this.bin(Op::Div, one, v, Unit::Db));
            }
            if let Some(k) = this.konst(&v) {
                return Ok(this.constant(v.unit, -k));
            }
            let rate = this.rate(v.rate);
            let reg = this.emit(rate, |dst| Instr::Neg { dst, a: v.reg });
            Ok(Value {
                reg,
                unit: v.unit,
                rate,
                port: None,
            })
        })
    }

    fn add(&mut self, a: Value, b: Value, span: Span) -> Res<Value> {
        self.binary(BinOp::Add, a, b, span)
    }

    fn binary_value(&mut self, op: BinOp, a: KVal, b: KVal, span: Span) -> Res<KVal> {
        type UnitRule = fn(Unit, Unit) -> Option<Unit>;
        let (engine_op, unit_of): (Op, UnitRule) = match op {
            BinOp::Add => (Op::Add, |a, b| (a == b && a != Unit::Db).then_some(a)),
            BinOp::Sub => (Op::Sub, |a, b| (a == b && a != Unit::Db).then_some(a)),
            BinOp::Mul => (Op::Mul, crate::unit::mul_unit),
            BinOp::Div => (Op::Div, crate::unit::div_unit),
        };
        if let Some(lane) = self.lane_binary(engine_op, unit_of, &a, &b, span)? {
            return Ok(lane);
        }
        self.zip_values(a, b, span, |this, a, b| this.binary(op, a, b, span))
    }

    fn zip_values(
        &mut self,
        a: KVal,
        b: KVal,
        span: Span,
        operation: impl Fn(&mut Self, Value, Value) -> Res<Value> + Copy,
    ) -> Res<KVal> {
        let a = if Self::is_array(&a) {
            KVal::array(self.array_elements(a, span)?)
        } else {
            a
        };
        let b = if Self::is_array(&b) {
            KVal::array(self.array_elements(b, span)?)
        } else {
            b
        };
        match (a, b) {
            (KVal::Num(a), KVal::Num(b)) => operation(self, a, b).map(KVal::Num),
            (KVal::Record(fields), b) => self.zip_values(fields[0].1.clone(), b, span, operation),
            (a, KVal::Record(fields)) => self.zip_values(a, fields[0].1.clone(), span, operation),
            (KVal::Array(a), KVal::Array(b)) => {
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
                Ok(KVal::Array(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| self.zip_values(a, b, span, operation))
                        .collect::<Res<_>>()?,
                ))
            }
            (KVal::Array(a), b @ KVal::Num(_)) => Ok(KVal::Array(
                a.into_iter()
                    .map(|a| self.zip_values(a, b.clone(), span, operation))
                    .collect::<Res<_>>()?,
            )),
            (a @ KVal::Num(_), KVal::Array(b)) => Ok(KVal::Array(
                b.into_iter()
                    .map(|b| self.zip_values(a.clone(), b, span, operation))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new("arithmetic doesn't work on ranges", span)),
        }
    }

    fn map_value(
        &mut self,
        value: KVal,
        span: Span,
        operation: impl Fn(&mut Self, Value) -> Res<Value> + Copy,
    ) -> Res<KVal> {
        match value {
            KVal::Num(value) => operation(self, value).map(KVal::Num),
            KVal::Record(fields) => self.map_value(fields[0].1.clone(), span, operation),
            other if Self::is_array(&other) => {
                let items = self.array_elements(other, span)?;
                Ok(KVal::Array(
                    items
                        .into_iter()
                        .map(|v| self.map_value(v, span, operation))
                        .collect::<Res<_>>()?,
                ))
            }
            _ => Err(Diagnostic::new(
                "this operation needs a number or an array",
                span,
            )),
        }
    }

    fn binary(&mut self, op: BinOp, l: Value, r: Value, span: Span) -> Res<Value> {
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            BinOp::Mul => "multiply",
            BinOp::Div => "divide",
        };
        let mismatch = || {
            Diagnostic::new(
                format!(
                    "can't {verb} {} and {}",
                    l.unit.describe(),
                    r.unit.describe()
                ),
                span,
            )
        };
        match op {
            BinOp::Add | BinOp::Sub => {
                if l.unit == Unit::Hz && r.unit == Unit::Semitones {
                    return Ok(self.transpose(l, r, op == BinOp::Sub));
                }
                if l.unit != r.unit {
                    return Err(mismatch());
                }
                if l.unit == Unit::Db {
                    // adding decibels multiplies gain factors
                    let r = if op == BinOp::Sub {
                        let one = self.constant(Unit::Num, 1.0);
                        self.bin(Op::Div, one, r, Unit::Db)
                    } else {
                        r
                    };
                    return Ok(self.bin(Op::Mul, l, r, Unit::Db));
                }
                let o = if op == BinOp::Add { Op::Add } else { Op::Sub };
                Ok(self.bin(o, l, r, l.unit))
            }
            BinOp::Mul => {
                let unit = crate::unit::mul_unit(l.unit, r.unit).ok_or_else(mismatch)?;
                Ok(self.bin(Op::Mul, l, r, unit))
            }
            BinOp::Div => {
                let unit = crate::unit::div_unit(l.unit, r.unit).ok_or_else(mismatch)?;
                Ok(self.bin(Op::Div, l, r, unit))
            }
        }
    }

    /// `freq + 7st`: a frequency scaled by 2^(st/12).
    fn transpose(&mut self, freq: Value, interval: Value, down: bool) -> Value {
        let k = if down { -1.0 } else { 1.0 } * std::f64::consts::LN_2 / 12.0;
        let scale = self.constant(Unit::Num, k);
        let e = self.bin(Op::Mul, interval, scale, Unit::Num);
        let ratio = self.call1(Fn1::Exp, e);
        self.bin(Op::Mul, freq, ratio, Unit::Hz)
    }

    fn index(&mut self, list: &Expr, index: &Expr, span: Span) -> Res<KVal> {
        if self.instrument_scope
            && let ExprKind::Name(name) = &list.kind
            && let Some((_, buffer)) = self.names.buffers.get(name).cloned()
        {
            let index = self.num(index)?;
            if index.unit != Unit::Num {
                return Err(Diagnostic::new("a buffer index is a plain number", span));
            }
            return Ok(self.emit_native(
                Box::new(niminal_engine::ops::BufferRead::new(buffer, Interp::None)),
                vec![NativeInput::Reg(index.reg)],
                Unit::Num,
            ));
        }
        let l = self.value(list)?;
        if let KVal::Buffer { selector, choices } = l {
            let pos = self.num(index)?;
            return self.read_buffer_ref(selector, choices, pos, Interp::None, span);
        }
        if let ExprKind::Range { .. } = index.kind {
            let KVal::Range {
                lo, hi, inclusive, ..
            } = self.value(index)?
            else {
                unreachable!()
            };
            let picks = range_numbers(lo, hi, inclusive, index.span)?;
            let entries = self.array_elements(l, list.span)?;
            let n = entries.len();
            if n == 0 && !picks.is_empty() {
                return Err(Diagnostic::new("this array is empty", span));
            }
            return Ok(KVal::Array(
                picks
                    .into_iter()
                    .map(|k| entries[niminal_engine::wrapped_index(k, n)].clone())
                    .collect(),
            ));
        }
        let i = self.num(index)?;
        if i.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("an index is a plain number, found {}", i.unit.describe()),
                index.span,
            ));
        }
        match l {
            KVal::Array(items) => {
                if items.is_empty() {
                    return Err(Diagnostic::new("this array is empty", span));
                }
                let n = items.len();
                if let Some(k) = self.konst(&i) {
                    return Ok(items[niminal_engine::wrapped_index(k, n)].clone());
                }
                self.index_values(items.into_vec(), i, span)
            }
            KVal::Table { id, unit, values } => {
                if values.is_empty() {
                    return Err(Diagnostic::new("this array is empty", span));
                }
                if let Some(k) = self.konst(&i) {
                    let n = values.len();
                    return Ok(KVal::Num(self.constant(
                        unit,
                        f64::from(values.get(niminal_engine::wrapped_index(k, n))),
                    )));
                }
                Ok(KVal::Num(self.read_array(
                    ArraySource::Table(id),
                    unit,
                    i,
                    None,
                )))
            }
            KVal::State { id, unit, len } => {
                if len == 0 {
                    return Err(Diagnostic::new("this array is empty", span));
                }
                Ok(KVal::Num(self.read_array(
                    ArraySource::State(id),
                    unit,
                    i,
                    None,
                )))
            }
            KVal::Lane { id, unit, .. } => Ok(KVal::Num(self.read_array(
                ArraySource::Lane(id),
                unit,
                i,
                None,
            ))),
            KVal::Num(v) => Ok(KVal::Num(v)),
            _ => Err(Diagnostic::new("only an array can be indexed", list.span)),
        }
    }

    fn index_values(&mut self, items: Vec<KVal>, index: Value, span: Span) -> Res<KVal> {
        if items.iter().all(|v| matches!(v, KVal::Num(_))) {
            let (array, unit) = self.numeric_array(KVal::array(items), span)?;
            return Ok(KVal::Num(self.read_array(array, unit, index, None)));
        }
        let rows = items
            .into_iter()
            .map(|v| {
                if !Self::is_array(&v) {
                    return Err(Diagnostic::new(
                        "an index that changes needs matching element shapes",
                        span,
                    ));
                }
                self.array_elements(v, span)
            })
            .collect::<Res<Vec<_>>>()?;
        let Some(first) = rows.first() else {
            return Err(Diagnostic::new("this array is empty", span));
        };
        let width = first.len();
        if rows.iter().any(|row| row.len() != width) {
            return Err(Diagnostic::new(
                "an index that changes needs matching element shapes",
                span,
            ));
        }
        Ok(KVal::Array(
            (0..width)
                .map(|column| {
                    let elements = rows.iter().map(|row| row[column].clone()).collect();
                    self.index_values(elements, index, span)
                })
                .collect::<Res<_>>()?,
        ))
    }

    fn numeric_array(&mut self, value: KVal, span: Span) -> Res<(ArraySource, Unit)> {
        match value {
            KVal::Table { values, .. } if values.is_empty() => {
                Err(Diagnostic::new("this array is empty", span))
            }
            KVal::State { len: 0, .. } => Err(Diagnostic::new("this array is empty", span)),
            KVal::Table { id, unit, .. } => Ok((ArraySource::Table(id), unit)),
            KVal::State { id, unit, .. } => Ok((ArraySource::State(id), unit)),
            KVal::Lane { id, unit, .. } => Ok((ArraySource::Lane(id), unit)),
            other => {
                let items = self.array_elements(other, span)?;
                let mut registers = Vec::with_capacity(items.len());
                let mut unit = None;
                let mut rate = Rate::Const;
                for item in items {
                    let KVal::Num(v) = item else {
                        return Err(Diagnostic::new(
                            "this operation needs a one-dimensional numeric array",
                            span,
                        ));
                    };
                    if unit.is_some_and(|unit| unit != v.unit) {
                        return Err(Diagnostic::new(
                            "array elements must have the same unit",
                            span,
                        ));
                    }
                    unit = Some(v.unit);
                    rate = rate.max(v.rate);
                    registers.push(v.reg);
                }
                let Some(unit) = unit else {
                    return Err(Diagnostic::new("this array is empty", span));
                };
                let id = self.values.len();
                self.values.push(registers.into());
                self.value_rates.push(rate);
                Ok((ArraySource::Values(id), unit))
            }
        }
    }

    fn read_array(
        &mut self,
        list: ArraySource,
        unit: Unit,
        i: Value,
        interp: Option<Interp>,
    ) -> Value {
        if let Some(pos) = self.konst(&i) {
            let data = match list {
                ArraySource::Table(id) => Some(self.tables[id].as_f64()),
                ArraySource::Values(id) => self.values[id]
                    .iter()
                    .map(|reg| self.consts.get(reg).copied())
                    .collect::<Option<Vec<_>>>(),
                ArraySource::State(_) | ArraySource::Lane(_) => None,
            };
            if let Some(data) = data {
                return self.constant(
                    unit,
                    niminal_engine::ops::read_at(&data, pos, interp.unwrap_or(Interp::None)),
                );
            }
        }
        let rate = match list {
            // a state list can change every sample
            ArraySource::State(_) => Rate::Audio,
            ArraySource::Lane(id) => self.rate(i.rate.max(self.lane_rates[id])),
            ArraySource::Table(_) => self.rate(i.rate),
            ArraySource::Values(id) => self.rate(i.rate.max(self.value_rates[id])),
        };
        let reg = match interp {
            None => self.emit(rate, |dst| Instr::Read {
                dst,
                list,
                idx: i.reg,
            }),
            Some(interp) => self.emit(rate, |dst| Instr::ReadAt {
                dst,
                list,
                pos: i.reg,
                interp,
            }),
        };
        Value {
            reg,
            unit,
            rate,
            port: None,
        }
    }

    // ---- calls -----------------------------------------------------------

    /// The receiver of a method call, if it is one.
    fn receiver<'e>(&self, name: &Ident, args: &'e [Arg]) -> Res<&'e Arg> {
        args.first().filter(|a| a.receiver).ok_or_else(|| {
            Diagnostic::new(
                format!(
                    "`{}` is called on a value, as in `x.{}`",
                    name.name, name.name
                ),
                name.span,
            )
        })
    }

    fn capture_arg(&mut self, value: KVal, wires: &mut Vec<NativeInput>, span: Span) -> Res<KArg> {
        Ok(match value {
            KVal::Buffer { selector, choices } => KArg::Buffer {
                selector: Box::new(self.capture_arg(KVal::Num(selector), wires, span)?),
                choices,
            },
            KVal::Num(v) => match self.konst(&v) {
                Some(value) => KArg::Const {
                    unit: v.unit,
                    value,
                },
                None => {
                    wires.push(NativeInput::Reg(v.reg));
                    KArg::Port {
                        unit: v.unit,
                        rate: v.rate,
                    }
                }
            },
            KVal::Function { def, captures } => {
                let mut captures: Vec<_> = captures.into_iter().collect();
                captures.sort_by(|a, b| a.0.cmp(&b.0));
                KArg::Function {
                    def,
                    captures: captures
                        .into_iter()
                        .map(|(n, v)| Ok((n, self.capture_arg(v, wires, span)?)))
                        .collect::<Res<_>>()?,
                }
            }
            KVal::Table { unit, values, .. } => KArg::Table { unit, values },
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => KArg::Range {
                unit,
                lo,
                hi,
                inclusive,
            },
            KVal::Record(fields) => return self.capture_arg(fields[0].1.clone(), wires, span),
            other => {
                let values = self.array_elements(other, span)?;
                KArg::Array(
                    values
                        .into_iter()
                        .map(|v| self.capture_arg(v, wires, span))
                        .collect::<Res<_>>()?,
                )
            }
        })
    }

    fn read_buffer_ref(
        &mut self,
        selector: Value,
        choices: crate::lower::BufferChoices,
        pos: Value,
        interp: Interp,
        span: Span,
    ) -> Res<KVal> {
        if pos.unit != Unit::Num {
            return Err(Diagnostic::new("a buffer position is a plain number", span));
        }
        let buffers = Arc::new(choices.iter().map(|(_, b)| b.clone()).collect());
        Ok(self.emit_native(
            Box::new(niminal_engine::ops::BufferReadBank::new(
                buffers,
                interp,
                choices.key,
            )),
            vec![NativeInput::Reg(selector.reg), NativeInput::Reg(pos.reg)],
            Unit::Num,
        ))
    }

    fn dsp_scope(&mut self, name: &str, args: &[Arg], span: Span) -> Res<KVal> {
        let usage = || {
            Diagnostic::new(
                match name {
                    "spawn" => "write `spawn(trigger, dur: 100ms, max: 8) { ... }`",
                    "control" => "write `control { ... }`",
                    _ => "write `oversample(4) { ... }`",
                },
                span,
            )
        };
        let Some((body, options)) = args.split_last() else {
            return Err(usage());
        };
        let ExprKind::Lambda { params, body } = &body.value.kind else {
            return Err(usage());
        };
        if !params.is_empty() {
            return Err(usage());
        }
        let control = name == "control";
        if control && self.in_control {
            // already once per step: the body runs here, with state of its own
            let def = funcs::lambda(&[], body, span);
            let captures = self
                .frame
                .scope
                .iter()
                .filter(|(n, _)| funcs::reads(&def, n))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            return self.call_captured(&def, &def.name, &[], captures);
        }
        if control && self.guard.is_some() {
            return Err(Diagnostic::new(
                "a `control` scope can't be inside an `if` or `match` branch",
                span,
            )
            .with_help("put the branch inside the scope instead"));
        }
        let (first, options) = if control {
            if !options.is_empty() {
                return Err(usage());
            }
            (None, options)
        } else {
            let Some((first, options)) = options.split_first() else {
                return Err(usage());
            };
            if first.name.is_some() {
                return Err(usage());
            }
            let first = self.num(&first.value)?;
            if first.unit != Unit::Num {
                return Err(usage());
            }
            (Some(first), options)
        };
        let mut duration = None;
        let mut max = None;
        for option in options {
            match option.name.as_ref().map(|n| n.name.as_str()) {
                Some("dur") if name == "spawn" && duration.is_none() => {
                    let value = self.num(&option.value)?;
                    if value.unit != Unit::Time
                        || self
                            .konst(&value)
                            .is_some_and(|v| !(0.0..=600.0).contains(&v) || v == 0.0)
                    {
                        return Err(usage());
                    }
                    duration = Some(value);
                }
                Some("max") if name == "spawn" && max.is_none() => {
                    let value = self.num(&option.value)?;
                    max = self
                        .konst(&value)
                        .filter(|v| {
                            value.unit == Unit::Num && v.fract() == 0.0 && (1.0..=64.0).contains(v)
                        })
                        .map(|v| v as usize);
                    if max.is_none() {
                        return Err(usage());
                    }
                }
                _ => return Err(usage()),
            }
        }
        let factor = if name == "oversample" {
            first
                .as_ref()
                .and_then(|first| self.konst(first))
                .filter(|f| [2.0, 4.0, 8.0].contains(f))
                .ok_or_else(usage)? as usize
        } else {
            1
        };
        if name == "spawn" && duration.is_none() {
            return Err(usage());
        }
        let mut def = funcs::lambda(&[], body, span);
        let mut captures: Vec<_> = self
            .frame
            .scope
            .iter()
            .filter(|(n, _)| funcs::reads(&def, n))
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        captures.sort_by(|a, b| a.0.cmp(&b.0));
        let mut wires = Vec::new();
        let mut kargs = Vec::new();
        for (n, v) in captures {
            let v = if control {
                self.at_block_start(&n, v, span)?
            } else {
                v
            };
            def.params.push(ParamDef {
                name: Ident { name: n, span },
                rate: None,
                ty: None,
                default: None,
            });
            kargs.push(self.capture_arg(v, &mut wires, span)?);
        }
        let (kernel, shape) = compile_scope_depth(
            &def,
            &kargs,
            self.registry,
            self.names,
            self.tempo,
            true,
            self.scope_depth + 1,
            control || self.in_control,
        )?;
        let custom = Box::new(niminal_engine::ops::Custom::new(Arc::new(kernel)));
        let op: Box<dyn niminal_engine::Opcode> = if name == "spawn" {
            wires.extend([
                NativeInput::Reg(first.expect("spawn has a trigger").reg),
                NativeInput::Reg(duration.unwrap().reg),
            ]);
            Box::new(niminal_engine::ops::Spawn::new(custom, max.unwrap_or(8)))
        } else if control {
            Box::new(niminal_engine::ops::ControlRate::new(custom))
        } else {
            Box::new(niminal_engine::ops::Oversample::new(custom, factor))
        };
        let outputs = op.outputs();
        let index = self.natives.len();
        self.natives.push(Native { op, inputs: wires });
        let first = self.registers;
        self.registers += outputs;
        let rate = if control { Rate::Control } else { Rate::Audio };
        self.push(
            rate,
            Instr::Native {
                dst: first,
                index,
                guard: self.guard,
            },
        );
        fn result(shape: KernelResult, first: usize, rate: Rate) -> KVal {
            match shape {
                KernelResult::Scalar { index, unit } => KVal::Num(Value {
                    reg: first + index,
                    unit,
                    rate,
                    port: None,
                }),
                KernelResult::Array(values) => {
                    KVal::Array(values.into_iter().map(|v| result(v, first, rate)).collect())
                }
                KernelResult::Record(values) => KVal::Record(
                    values
                        .into_iter()
                        .map(|(n, v)| (n, result(v, first, rate)))
                        .collect(),
                ),
            }
        }
        Ok(result(shape, first, rate))
    }

    /// A value a `control` scope captures, as it is at the start of the block: a parameter is read
    /// there, and anything that changes every sample can't be.
    fn at_block_start(&mut self, name: &str, v: KVal, span: Span) -> Res<KVal> {
        match v {
            KVal::Num(x) => self.read_at_block_start(name, x, span).map(KVal::Num),
            KVal::Buffer { selector, choices } => Ok(KVal::Buffer {
                selector: self.read_at_block_start(name, selector, span)?,
                choices,
            }),
            KVal::Function { def, captures } => Ok(KVal::Function {
                def,
                captures: captures
                    .into_iter()
                    .map(|(n, c)| Ok((n, self.at_block_start(name, c, span)?)))
                    .collect::<Res<_>>()?,
            }),
            KVal::Record(fields) => Ok(KVal::Record(
                fields
                    .into_iter()
                    .map(|(n, f)| Ok((n, self.at_block_start(name, f, span)?)))
                    .collect::<Res<_>>()?,
            )),
            other if Self::is_array(&other) => {
                let items = self.array_elements(other, span)?;
                Ok(KVal::Array(
                    items
                        .into_iter()
                        .map(|v| self.at_block_start(name, v, span))
                        .collect::<Res<_>>()?,
                ))
            }
            other => Ok(other),
        }
    }

    fn read_at_block_start(&mut self, name: &str, v: Value, span: Span) -> Res<Value> {
        self.at_control(v, span).map_err(|_| {
            Diagnostic::new(
                format!("`{name}` changes every sample, so a `control` scope can't read it"),
                span,
            )
            .with_help("pass a parameter in, or compute the value inside the scope")
        })
    }

    fn reset_scope(&mut self, args: &[Arg], span: Span) -> Res<KVal> {
        let (trigger, body) = funcs::reset_args(args, span)?;
        let trigger = self.num(trigger)?;
        if trigger.unit != Unit::Num {
            return Err(Diagnostic::new("a reset trigger is one plain signal", span));
        }
        let previous = self.state_init.len();
        self.state_init.push(0.0);
        let (states, buffers, natives) = (
            self.state_init.len(),
            self.buffers.len(),
            self.natives.len(),
        );
        let (init, block, code) = (self.init.len(), self.block.len(), self.code.len());
        let registers = self.registers;
        let def = funcs::lambda(&[], body, span);
        let captures = self
            .frame
            .scope
            .iter()
            .filter(|(n, _)| funcs::reads(&def, n))
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        let result = self.call_captured(&def, &def.name, &[], captures)?;
        let scope = self.resets.len();
        self.resets.push(niminal_engine::ops::ResetScope {
            states: states..self.state_init.len(),
            buffers: buffers..self.buffers.len(),
            natives: (natives..self.natives.len()).collect(),
            registers: registers..self.registers,
            init: self.init[init..].to_vec(),
            block: self.block[block..].to_vec(),
        });
        self.code.insert(
            code,
            Instr::Reset {
                scope,
                trigger: trigger.reg,
                previous,
                guard: self.guard,
            },
        );
        Ok(result)
    }

    fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let n = name.name.as_str();
        if matches!(n, "spawn" | "oversample")
            || (n == "control"
                && matches!(args, [a] if !a.receiver && matches!(a.value.kind, ExprKind::Lambda { .. })))
        {
            return self.dsp_scope(n, args, span);
        }
        if n == "reset_on" {
            return self.reset_scope(args, span);
        }
        match n {
            "frames" => return self.frames_scope(name, args, span),
            "fft" => return self.fft_call(name, args, span),
            "ifft" => return self.ifft_call(name, args, span),
            _ => {}
        }
        if let [receiver] = args
            && receiver.receiver
            && n == "time"
            && matches!(&receiver.value.kind, ExprKind::Name(c) if c == "clock")
        {
            use niminal_engine::ops::{Context, ReadContext};
            let op = Box::new(ReadContext::new(Context::ClockTime));
            return Ok(self.emit_native(op, Vec::new(), Unit::Time));
        }
        if self.instrument_scope
            && let [receiver] = args
            && receiver.receiver
        {
            use niminal_engine::ops::{Context, NoteStart, NoteTime, ReadContext, Released};
            if matches!(&receiver.value.kind, ExprKind::Name(n) if n == "note") {
                let (op, unit): (Box<dyn niminal_engine::Opcode>, Unit) = match n {
                    "time" => (Box::new(NoteTime::default()), Unit::Time),
                    "start" => (Box::new(NoteStart::default()), Unit::Num),
                    "released" => (Box::new(Released), Unit::Num),
                    "dur" => (Box::new(ReadContext::new(Context::NoteDur)), Unit::Time),
                    "index" => (Box::new(ReadContext::new(Context::NoteIndex)), Unit::Num),
                    _ => return Err(Diagnostic::new("unknown note field", name.span)),
                };
                return Ok(self.emit_native(op, Vec::new(), unit));
            }
        }
        if self.instrument_scope
            && let Some(def) = self.names.instrs.get(n).cloned()
        {
            let own_gain = def.params.iter().any(|p| p.name.name == "gain");
            let (gains, rest): (Vec<_>, Vec<_>) = args
                .iter()
                .cloned()
                .partition(|a| !own_gain && a.name.as_ref().is_some_and(|n| n.name == "gain"));
            if gains.len() > 1 {
                return Err(Diagnostic::new("gain given more than once", span));
            }
            let gain = if let Some(arg) = gains.first() {
                let mut value = self.num(&arg.value)?;
                if !matches!(value.unit, Unit::Num | Unit::Db) {
                    return Err(Diagnostic::new(
                        "gain needs db or a plain factor",
                        arg.value.span,
                    ));
                }
                value.unit = Unit::Num;
                Some(value)
            } else {
                None
            };
            let as_fn = FnDef {
                name: def.name,
                params: def.params,
                body: def.body,
                result: FnResult::Inferred,
                span: def.span,
            };
            let result = self.call_fn(&as_fn, name, &rest)?;
            return match gain {
                Some(gain) => self.binary_value(BinOp::Mul, result, KVal::Num(gain), span),
                None => Ok(result),
            };
        }
        if self.instrument_scope
            && n == "at"
            && let Some(receiver) = args.first().filter(|a| a.receiver)
            && let ExprKind::Name(buffer) = &receiver.value.kind
            && let Some((_, buffer)) = self.names.buffers.get(buffer).cloned()
        {
            let mut position = None;
            let mut interp = None;
            for arg in &args[1..] {
                match arg.name.as_ref().map(|n| n.name.as_str()) {
                    None | Some("pos") if position.is_none() => {
                        position = Some(self.num(&arg.value)?)
                    }
                    Some("interp") if interp.is_none() => interp = Some(interp_of(&arg.value)?),
                    _ => {
                        return Err(Diagnostic::new(
                            "write `buffer.at(pos, interp: linear)`",
                            arg.value.span,
                        ));
                    }
                }
            }
            let position =
                position.ok_or_else(|| Diagnostic::new("buffer.at needs a position", span))?;
            if position.unit != Unit::Num {
                return Err(Diagnostic::new("a buffer position is a plain number", span));
            }
            return Ok(self.emit_native(
                Box::new(niminal_engine::ops::BufferRead::new(
                    buffer,
                    interp.unwrap_or(Interp::Linear),
                )),
                vec![NativeInput::Reg(position.reg)],
                Unit::Num,
            ));
        }
        if n == "at"
            && let Some(receiver) = args.first().filter(|a| a.receiver)
            && let ExprKind::Name(id) = &receiver.value.kind
            && let Some(KVal::Buffer { selector, choices }) = self.frame.scope.get(id).cloned()
        {
            let mut pos = None;
            let mut interp = None;
            for arg in &args[1..] {
                match arg.name.as_ref().map(|n| n.name.as_str()) {
                    None if pos.is_none() => pos = Some(self.num(&arg.value)?),
                    Some("interp") if interp.is_none() => interp = Some(interp_of(&arg.value)?),
                    _ => {
                        return Err(Diagnostic::new(
                            "write `source.at(pos, interp: linear)`",
                            span,
                        ));
                    }
                }
            }
            return self.read_buffer_ref(
                selector,
                choices,
                pos.ok_or_else(|| Diagnostic::new("at needs a position", span))?,
                interp.unwrap_or(Interp::Linear),
                span,
            );
        }
        // Named result lookup must preserve an array field's shape.
        if let [receiver] = args
            && receiver.receiver
            && !self.names.fns.contains_key(n)
        {
            let record = match &receiver.value.kind {
                ExprKind::Name(name) => self.frame.scope.get(name).cloned(),
                ExprKind::Call { name, .. }
                    if self
                        .names
                        .fns
                        .get(&name.name)
                        .is_some_and(|def| matches!(def.result, FnResult::Named(_)))
                        || matches!(self.frame.scope.get(&name.name), Some(KVal::Function { def, .. }) if matches!(def.result, FnResult::Named(_))) =>
                {
                    Some(self.value(&receiver.value)?)
                }
                _ => None,
            };
            if let Some(KVal::Record(fields)) = record {
                return fields
                    .iter()
                    .find(|(field, _)| field == n)
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        Diagnostic::new(format!("this has no result `{n}`"), name.span)
                    });
            }
        }
        if let Some(KVal::Function { def, captures }) = self.frame.scope.get(n).cloned() {
            return self.call_captured(&def, name, args, captures);
        }
        if let Some(def) = self.names.fns.get(n) {
            let def = def.clone();
            return self.call_fn(&def, name, args);
        }
        match n {
            "prev" => return self.prev(name, args).map(KVal::Num),
            "min" | "max" if args.len() == 1 && args[0].receiver => {
                return self.array_call(name, args, span);
            }
            "min" | "max" | "pow" | "mod" | "atan2" => return self.combine(name, args),
            "clamp" => return self.clamp(name, args),
            "range" => return self.range(name, args, span),
            "hold" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                return self.map_value(value, span, |_, v| {
                    if v.rate > Rate::Control {
                        return Err(Diagnostic::new(
                            "`hold` is for values that change at most once per block",
                            span,
                        )
                        .with_help(
                            "compute it in a `control { ... }` scope, or read it with `.control`",
                        ));
                    }
                    Ok(v)
                });
            }
            "smooth" if args.len() == 1 && args[0].receiver => {
                let value = self.value(&args[0].value)?;
                return self.map_value(value, span, |this, v| {
                    if v.rate > Rate::Control {
                        return Err(Diagnostic::new(
                            "`smooth` is for values that change at most once per block",
                            span,
                        )
                        .with_help(
                            "compute it in a `control { ... }` scope, or read it with `.control`",
                        ));
                    }
                    match this.emit_native(
                        Box::new(niminal_engine::ops::Ramp::default()),
                        vec![NativeInput::Reg(v.reg)],
                        v.unit,
                    ) {
                        KVal::Num(smoothed) => Ok(smoothed),
                        _ => unreachable!("a native gives one number"),
                    }
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
                return self.map_value(value, span, |this, v| {
                    if n == "init" {
                        Ok(this.at_init(v))
                    } else {
                        this.at_control(v, args[0].value.span)
                    }
                });
            }
            "len" | "sum" | "map" | "indices" | "at" | "channels" | "step" | "dot" | "fir"
            | "taps" | "matvec" => {
                return self.array_call(name, args, span);
            }
            "zeros" => {
                let [count] = args else {
                    return Err(Diagnostic::new("`zeros` takes a count", name.span));
                };
                let c = self.num(&count.value)?;
                let n = match self.konst(&c) {
                    Some(k) if k.fract() == 0.0 && (0.0..=(1 << 22) as f64).contains(&k) => {
                        k as usize
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            "`zeros` takes a whole number known while compiling",
                            count.value.span,
                        ));
                    }
                };
                if n == 0 {
                    return Ok(KVal::array(vec![]));
                }
                return Ok(self.table(
                    Unit::Num,
                    Arc::new(Table::from_f32(vec![0.0f32; n].into_boxed_slice(), None)),
                ));
            }
            "split" => {
                return Err(Diagnostic::new(
                    "a function with state works on one channel at a time",
                    name.span,
                ));
            }
            _ => {}
        }
        if let Some(v) = self.unit_method(name, args)? {
            return Ok(v);
        }
        if let Some(f) = math_fn(n) {
            let [arg] = args else {
                return Err(Diagnostic::new(format!("`{n}` takes one value"), name.span));
            };
            if let Some(a) = &arg.name {
                return Err(Diagnostic::new(
                    format!("`{n}` takes its value without a name"),
                    a.span,
                ));
            }
            let value = self.value(&arg.value)?;
            return self.math_value(f, value, arg.value.span);
        }

        let registry = self.registry;
        if let Some(spec) = registry.find(n) {
            if let Build::Builtin(_) = &spec.build {
                return self.native(name, spec, args);
            }
            return Err(Diagnostic::new(
                format!("`{n}` can't be used inside a function with state"),
                name.span,
            )
            .with_help("it works on channels; use it where the function is called"));
        }
        if matches!(n, "osc_out" | "input") {
            return Err(Diagnostic::new(
                format!("`{n}` can't be used inside a function"),
                name.span,
            ));
        }
        if self.names.kind_of(n) == Some("instrument") {
            return Err(Diagnostic::new(
                format!("`{n}` is an instrument; a function with state can't play one"),
                name.span,
            ));
        }
        let known: Vec<&str> = registry
            .names()
            .chain(self.names.fns.keys().map(String::as_str))
            .chain(Fn1::ALL.iter().map(|f| f.name()))
            .chain([
                "min", "max", "pow", "mod", "atan2", "clamp", "prev", "zeros",
            ])
            .collect();
        let mut d = Diagnostic::new(format!("unknown function `{n}`"), name.span);
        if let Some(c) = closest(n, known) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        Err(d)
    }

    /// `x.hz`, `n.ms`, `(-6db).num`...: give a plain value a unit, or take it away.
    fn unit_method(&mut self, name: &Ident, args: &[Arg]) -> Res<Option<KVal>> {
        let scale = matches!(
            name.name.as_str(),
            "num"
                | "hz"
                | "khz"
                | "db"
                | "sec"
                | "ms"
                | "beat"
                | "beats"
                | "bar"
                | "bars"
                | "deg"
                | "st"
                | "ct"
        );
        if !scale || !(args.len() == 1 && args[0].receiver) {
            return Ok(None);
        }
        let value = self.value(&args[0].value)?;
        let span = args[0].value.span;
        self.map_value(value, span, |this, value| {
            this.unit_value(name, value, span)
        })
        .map(Some)
    }

    fn unit_value(&mut self, name: &Ident, v: Value, span: Span) -> Res<Value> {
        if name.name == "num" {
            return Ok(match v.unit {
                Unit::Db => {
                    // a db value is held as a gain factor
                    let ln = self.call1(
                        Fn1::Log,
                        Value {
                            unit: Unit::Num,
                            ..v
                        },
                    );
                    let k = self.constant(Unit::Num, 20.0 / std::f64::consts::LN_10);
                    self.bin(Op::Mul, ln, k, Unit::Num)
                }
                _ => Value {
                    unit: Unit::Num,
                    ..v
                },
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
            let k = self.constant(Unit::Num, std::f64::consts::LN_10 / 20.0);
            let e = self.bin(Op::Mul, v, k, Unit::Num);
            let g = self.call1(Fn1::Exp, e);
            return Ok(Value {
                unit: Unit::Db,
                ..g
            });
        }
        let f = self.constant(Unit::Num, factor);
        Ok(self.bin(Op::Mul, v, f, unit))
    }

    /// `prev(x)`: what `x` was one sample ago. `y = x + prev(y) * 0.5` is a feedback loop.
    fn prev(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let [arg] = args else {
            return Err(Diagnostic::new("`prev` takes one value", name.span));
        };
        if let Some(n) = &arg.name {
            return Err(Diagnostic::new(
                "`prev` takes its value without a name",
                n.span,
            ));
        }
        if self.force.is_some() {
            return Err(Diagnostic::new(
                "`prev` changes every sample, so it can't be used here",
                name.span,
            ));
        }
        if let ExprKind::Name(n) = &arg.value.kind
            && let Some(&(slot, _)) = self.frame.forward.get(n)
        {
            let reg = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot });
            return Ok(Value {
                reg,
                unit: Unit::Num,
                rate: Rate::Audio,
                port: None,
            });
        }
        let slot = self.state_init.len();
        self.state_init.push(0.0);
        let reg = self.emit(Rate::Audio, |dst| Instr::LoadState { dst, slot });
        if let ExprKind::Name(n) = &arg.value.kind
            && !self.frame.scope.contains_key(n)
            && !matches!(n.as_str(), "pi" | "sample_rate")
        {
            // a name defined further down: this is feedback
            self.frame.forward.insert(n.clone(), (slot, arg.value.span));
            return Ok(Value {
                reg,
                unit: Unit::Num,
                rate: Rate::Audio,
                port: None,
            });
        }
        let v = self.num(&arg.value)?;
        self.frame.deferred.push((slot, v.reg));
        Ok(Value {
            reg,
            unit: v.unit,
            rate: Rate::Audio,
            port: None,
        })
    }

    /// The values of a call such as `min(a, b)` or `a.mod(b)`, without names.
    fn plain_args(&mut self, name: &Ident, args: &[Arg], count: usize) -> Res<Vec<(KVal, Span)>> {
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
    fn combine(&mut self, name: &Ident, args: &[Arg]) -> Res<KVal> {
        let mut values = self.plain_args(name, args, 2)?;
        let (b, b_span) = values.pop().unwrap();
        let (a, _) = values.pop().unwrap();
        self.zip_values(a, b, b_span, |this, a, b| {
            this.combine_scalar(name, a, b, b_span)
        })
    }

    fn combine_scalar(&mut self, name: &Ident, a: Value, b: Value, b_span: Span) -> Res<Value> {
        match name.name.as_str() {
            "pow" => {
                if a.unit != Unit::Num || b.unit != Unit::Num {
                    return Err(Diagnostic::new("`pow` expects plain numbers", b_span));
                }
                Ok(self.bin(Op::Pow, a, b, Unit::Num))
            }
            "atan2" => {
                if a.unit != b.unit {
                    return Err(Diagnostic::new(
                        "`atan2` needs values with the same unit",
                        b_span,
                    ));
                }
                Ok(self.bin(Op::Atan2, a, b, Unit::Num))
            }
            "mod" => {
                let unit = crate::unit::rem_unit(a.unit, b.unit).ok_or_else(|| {
                    Diagnostic::new(
                        format!(
                            "can't take the remainder of {} and {}",
                            a.unit.describe(),
                            b.unit.describe()
                        ),
                        b_span,
                    )
                })?;
                Ok(self.bin(Op::Rem, a, b, unit))
            }
            m => {
                if a.unit != b.unit {
                    return Err(Diagnostic::new(
                        format!(
                            "`{m}` needs values with the same unit: this is {}, the first was {}",
                            b.unit.describe(),
                            a.unit.describe()
                        ),
                        b_span,
                    ));
                }
                Ok(self.bin(if m == "min" { Op::Min } else { Op::Max }, a, b, a.unit))
            }
        }
    }

    fn range(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
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
        let KVal::Range { unit, lo, hi, .. } = self.value(&range.value)? else {
            return Err(Diagnostic::new("`range` takes a range", range.value.span));
        };
        self.map_value(value, span, |this, x| {
            if x.unit != Unit::Num {
                return Err(Diagnostic::new(
                    "`range` maps plain numbers from 0 to 1",
                    span,
                ));
            }
            if unit == Unit::Db {
                let scale = this.constant(Unit::Num, (hi / lo).ln());
                let exponent = this.bin(Op::Mul, x, scale, Unit::Num);
                let grown = this.call1(Fn1::Exp, exponent);
                let base = this.constant(Unit::Db, lo);
                return Ok(this.bin(Op::Mul, grown, base, Unit::Db));
            }
            let width = this.constant(unit, hi - lo);
            let scaled = this.bin(Op::Mul, x, width, unit);
            let base = this.constant(unit, lo);
            Ok(this.bin(Op::Add, scaled, base, unit))
        })
    }

    /// `x.clamp(lo..hi)`
    fn clamp(&mut self, name: &Ident, args: &[Arg]) -> Res<KVal> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`clamp` takes a range", name.span)
                .with_help("for example `x.clamp(0..1)`"));
        };
        let v = self.value(&x.value)?;
        let ExprKind::Range { lo, hi, .. } = &range.value.kind else {
            return Err(Diagnostic::new(
                "`clamp` takes a range, such as `20hz..18khz`",
                range.value.span,
            ));
        };
        let (l, h) = (self.num(lo)?, self.num(hi)?);
        let up = self.extreme_value(v, KVal::Num(l), false, name.span)?;
        self.extreme_value(up, KVal::Num(h), true, name.span)
    }

    /// `list.len`, `list.sum`, `list.map(f)`, `list.indices` and `list.at(pos)`.
    fn array_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let receiver = self.receiver(name, args)?;
        let l = self.value(&receiver.value)?;
        if name.name == "step" {
            let KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } = l
            else {
                return Err(
                    Diagnostic::new("`step` is called on a range", receiver.value.span)
                        .with_help("for example `(0..1).step(0.25)`"),
                );
            };
            let [_, step] = args else {
                return Err(Diagnostic::new(
                    "`step` takes the size of a step",
                    name.span,
                ));
            };
            let size = self.num(&step.value)?;
            let size_unit = size.unit;
            let Some(size) = self.konst(&size) else {
                return Err(Diagnostic::new(
                    "the size of a step must be known while compiling",
                    step.value.span,
                ));
            };
            if size_unit != unit || unit == Unit::Db {
                return Err(Diagnostic::new(
                    "a step has the same unit as the range, and a db range can't be stepped",
                    step.value.span,
                ));
            }
            let values = step_numbers(lo, hi, inclusive, size, span)?;
            return Ok(KVal::array(
                values
                    .into_iter()
                    .map(|v| KVal::Num(self.constant(unit, v)))
                    .collect(),
            ));
        }
        let l = if matches!(l, KVal::Range { .. }) {
            if name.name == "channels" {
                return Err(Diagnostic::new("a range has no channels", span));
            }
            KVal::array(self.array_elements(l, span)?)
        } else {
            l
        };
        let len = |l: &KVal| match l {
            KVal::Array(items) => Some(items.len()),
            KVal::Table { values, .. } => Some(values.len()),
            KVal::State { len, .. } | KVal::Lane { len, .. } => Some(*len),
            KVal::Num(_) => Some(1),
            _ => None,
        };
        let Some(count) = len(&l) else {
            return Err(Diagnostic::new(
                format!("`{}` works on an array", name.name),
                receiver.value.span,
            ));
        };
        if matches!(
            name.name.as_str(),
            "len" | "indices" | "sum" | "min" | "max" | "channels"
        ) && args.len() != 1
        {
            return Err(Diagnostic::new(
                format!("`{}` takes no arguments", name.name),
                name.span,
            ));
        }
        match name.name.as_str() {
            "channels" => Ok(KVal::array(self.array_elements(l, span)?)),
            "len" => Ok(KVal::Num(self.constant(Unit::Num, count as f64))),
            "indices" => Ok(KVal::Range {
                unit: Unit::Num,
                lo: 0.0,
                hi: count as f64,
                inclusive: false,
            }),
            "sum" | "min" | "max" if matches!(l, KVal::State { .. } | KVal::Lane { .. }) => {
                let kind = match name.name.as_str() {
                    "sum" => Reduce::Sum,
                    "min" => Reduce::Min,
                    _ => Reduce::Max,
                };
                let input = self.lane_of(l, receiver.value.span)?;
                let rate = self.rate(input.rate);
                let reg = self.emit(rate, |dst| Instr::Reduce {
                    dst,
                    kind,
                    a: input.src,
                });
                Ok(KVal::Num(Value {
                    reg,
                    unit: input.unit,
                    rate,
                    port: None,
                }))
            }
            "dot" | "fir" | "taps" | "matvec" => self.vector_call(name, args, l, span),
            "sum" | "min" | "max" => {
                let entries = self.array_elements(l, receiver.value.span)?;
                let mut total: Option<KVal> = None;
                for item in entries {
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
            "at" => {
                let (list, unit) = self.numeric_array(l, receiver.value.span)?;
                let mut pos = None;
                let mut interp = Interp::Linear;
                for a in &args[1..] {
                    match a.name.as_ref().map(|n| n.name.as_str()) {
                        None if pos.is_none() => pos = Some(self.num(&a.value)?),
                        Some("interp") => interp = interp_of(&a.value)?,
                        _ => {
                            return Err(Diagnostic::new(
                                "`at` takes a position and `interp:`",
                                a.value.span,
                            ));
                        }
                    }
                }
                let Some(pos) = pos else {
                    return Err(Diagnostic::new("`at` takes a position", name.span));
                };
                if pos.unit != Unit::Num {
                    return Err(Diagnostic::new("a position is a plain number", span));
                }
                Ok(KVal::Num(self.read_array(list, unit, pos, Some(interp))))
            }
            _ => {
                // map
                let [_, f] = args else {
                    return Err(Diagnostic::new("`map` takes a function", name.span));
                };
                let mapped = match &f.value.kind {
                    ExprKind::Name(n)
                        if self.names.fns.contains_key(n)
                            || matches!(self.frame.scope.get(n), Some(KVal::Function { .. })) =>
                    {
                        Some(funcs::mapped_call(n, f.value.span))
                    }
                    _ => None,
                };
                let (param, body) = lambda_of(mapped.as_ref().unwrap_or(&f.value))?;
                if param != "it" {
                    self.check_name(&Ident {
                        name: param.clone(),
                        span: f.value.span,
                    })?;
                }
                if self.frame.scope.contains_key(&param) {
                    return Err(Diagnostic::new(
                        format!(
                            "`{param}` is already in use, so it can't be this function's parameter"
                        ),
                        f.value.span,
                    ));
                }
                let entries = self.array_elements(l, receiver.value.span)?;
                let mut out = Vec::with_capacity(entries.len());
                for entry in entries {
                    self.frame.scope.insert(param.clone(), entry);
                    let v = match &body {
                        Body::Block(stmts) => self
                            .block(stmts, true, f.value.span)
                            .map(|v| v.expect("a value")),
                        Body::Expr(e) => self.value(e),
                    };
                    self.frame.scope.remove(&param);
                    out.push(self.snapshot(v?, f.value.span)?);
                }
                Ok(KVal::array(out))
            }
        }
    }

    /// A call of a built-in opcode such as `lpf` or `osc`: it is run a sample at a time, with
    /// state of its own for each call.
    fn native(&mut self, name: &Ident, spec: &opcodes::OpSpec, args: &[Arg]) -> Res<KVal> {
        if self.force.is_some() {
            return Err(Diagnostic::new(
                format!(
                    "`{}` changes every sample, so it can't be used here",
                    name.name
                ),
                name.span,
            ));
        }
        let names: Vec<&str> = spec.params.iter().map(|p| p.name.as_str()).collect();
        let bound = funcs::bind(&name.name, name.span, &names, spec.positional, args)?;
        let mut vals: Vec<Option<ArgVal<Value>>> = vec![None; spec.params.len()];
        for (index, (param, arg)) in spec.params.iter().zip(bound).enumerate() {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(
                        format!("`{}` needs an argument `{}`", name.name, param.name),
                        name.span,
                    ));
                }
                continue;
            };
            vals[index] = Some(match param.kind {
                opcodes::Kind::Wave => ArgVal::Wave(self.wave_arg(&arg.value)?),
                opcodes::Kind::Table => ArgVal::Tables(self.table_arg(&arg.value, true)?),
                opcodes::Kind::Layout => {
                    return Err(Diagnostic::new(
                        format!("`{}` can't be used inside a function with state", name.name),
                        name.span,
                    ));
                }
                opcodes::Kind::Signal(_) | opcodes::Kind::Gain => {
                    let value = self.value(&arg.value)?;
                    self.arg_val(value, arg.value.span)?
                }
            });
        }
        self.native_call(name, spec, &vals)
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
        let tables = match self.table_arg(e, false) {
            Err(_) if matches!(e.kind, ExprKind::Name(_)) => return Err(not_a_wave()),
            tables => tables?,
        };
        opcodes::WaveArg::from_tables(&tables).map_err(|m| Diagnostic::new(m, e.span))
    }

    /// An argument of a built-in opcode: a number, or an array of them.
    fn arg_val(&mut self, value: KVal, span: Span) -> Res<ArgVal<Value>> {
        match value {
            KVal::Num(v) => Ok(ArgVal::Sig(v)),
            v if Self::is_array(&v) => Ok(ArgVal::Array(
                self.array_elements(v, span)?
                    .into_iter()
                    .map(|item| self.arg_val(item, span))
                    .collect::<Res<_>>()?,
            )),
            _ => Err(Diagnostic::new(
                "opcode arguments must be numbers or arrays",
                span,
            )),
        }
    }

    /// A table known while compiling, or one per channel.
    fn table_arg(&mut self, e: &Expr, nested: bool) -> Res<Vec<Arc<Table>>> {
        let value = self.value(e)?;
        let not_a_table = || {
            Diagnostic::new(
                "expected a one-dimensional table of numbers known while compiling",
                e.span,
            )
        };
        let constants = |this: &Self, items: &[KVal]| -> Option<Arc<Table>> {
            let numbers: Option<Vec<f64>> = items
                .iter()
                .map(|item| match item {
                    KVal::Num(v) => this.konst(v),
                    _ => None,
                })
                .collect();
            numbers
                .filter(|n| !n.is_empty())
                .map(|n| Arc::new(Table::from_f64(n, None)))
        };
        let tables = match value {
            KVal::Table { values, .. } => vec![values],
            KVal::Array(items)
                if !items.is_empty()
                    && items.iter().all(|i| {
                        matches!(i, KVal::Table { .. }) || (nested && matches!(i, KVal::Array(_)))
                    }) =>
            {
                items
                    .iter()
                    .map(|i| match i {
                        KVal::Table { values, .. } => Ok(values.clone()),
                        KVal::Array(items) => constants(self, items).ok_or_else(not_a_table),
                        _ => unreachable!(),
                    })
                    .collect::<Res<Vec<_>>>()?
            }
            KVal::Array(items) => vec![constants(self, &items).ok_or_else(not_a_table)?],
            _ => return Err(not_a_table()),
        };
        if tables.iter().any(|t| t.is_empty()) {
            return Err(Diagnostic::new("a table can't be empty", e.span));
        }
        Ok(tables)
    }

    /// One call of a built-in opcode, spread over the elements of any array argument.
    fn native_call(
        &mut self,
        name: &Ident,
        spec: &opcodes::OpSpec,
        vals: &[Option<ArgVal<Value>>],
    ) -> Res<KVal> {
        if let Some(rows) = call::rows(vals).map_err(|m| Diagnostic::new(m, name.span))? {
            return Ok(KVal::Array(
                rows.iter()
                    .map(|row| self.native_call(name, spec, row))
                    .collect::<Res<_>>()?,
            ));
        }
        for (param, val) in spec.params.iter().zip(vals) {
            if let Some(ArgVal::Sig(v)) = val
                && let Some(expected) = call::unit_expected(param.kind, v.unit)
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` expects {} for `{}`, found {}",
                        name.name,
                        call::expectation(param.kind, expected),
                        param.name,
                        v.unit.describe()
                    ),
                    name.span,
                ));
            }
        }
        let build = call::build_args(spec, vals, |v| self.konst(v), 0, self.natives.len() as u32)
            .map_err(|m| Diagnostic::new(m, name.span))?;
        let op = spec
            .instantiate(&build)
            .map_err(|e| Diagnostic::new(e, name.span))?
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("`{}` can't be used inside a function with state", name.name),
                    name.span,
                )
            })?;
        if !op.per_sample() {
            return Err(Diagnostic::new(
                format!(
                    "`{}` works on whole blocks, so it can't be used inside a function with state",
                    name.name
                ),
                name.span,
            ));
        }
        // one input for each of the opcode's ports, from an argument or the port's default
        let mut wired = Vec::with_capacity(op.ports().len());
        for port in op.ports() {
            let given = spec
                .params
                .iter()
                .position(|p| p.name == port.name)
                .and_then(|i| match &vals[i] {
                    Some(ArgVal::Sig(v)) => Some(NativeInput::Reg(v.reg)),
                    _ => None,
                });
            wired.push(match (given, port.default) {
                (Some(input), _) => input,
                (None, Some(default)) => NativeInput::Const(f64::from(default)),
                (None, None) => {
                    return Err(Diagnostic::new(
                        format!("`{}` needs an argument `{}`", name.name, port.name),
                        name.span,
                    ));
                }
            });
        }
        let outputs = op.outputs();
        let index = self.natives.len();
        self.natives.push(Native { op, inputs: wired });
        let first = self.registers;
        self.registers += outputs;
        self.push(
            Rate::Audio,
            Instr::Native {
                dst: first,
                index,
                guard: self.guard,
            },
        );
        let values: Vec<_> = (0..outputs)
            .map(|i| {
                KVal::Num(Value {
                    reg: first + i,
                    unit: Unit::Num,
                    rate: Rate::Audio,
                    port: None,
                })
            })
            .collect();
        if spec.name == "granular" && build.consts.get("monitor") == Some(&1.0) {
            let fields = niminal_engine::ops::GRANULAR_MONITOR_FIELDS;
            let channels = outputs - fields.len();
            let audio = if channels == 1 {
                values[0].clone()
            } else {
                KVal::Array(values[..channels].to_vec().into())
            };
            let mut result = vec![("audio".into(), audio)];
            result.extend(
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (name.to_string(), values[channels + i].clone())),
            );
            return Ok(KVal::Record(result));
        }
        Ok(if outputs == 1 {
            values.into_iter().next().unwrap()
        } else {
            KVal::Array(values.into())
        })
    }
}

// ---- lanes ------------------------------------------------------------------------------------

/// What a vector instruction reads, and what is known about it.
#[derive(Clone, Copy)]
struct LaneIn {
    src: LaneSrc,
    unit: Unit,
    /// `None` for a single value that stands for every entry.
    len: Option<usize>,
    rate: Rate,
}

impl Kb<'_> {
    fn new_lane(&mut self, len: usize, rate: Rate) -> usize {
        let id = self.lanes.len();
        self.lanes.push(len);
        self.lane_rates.push(rate);
        id
    }

    /// What a vector instruction can read of `v`, if it is a number or an array it holds in memory.
    fn lane_input(&self, v: &KVal) -> Option<LaneIn> {
        Some(match v {
            KVal::Num(n) => LaneIn {
                src: LaneSrc::Reg(n.reg),
                unit: n.unit,
                len: None,
                rate: n.rate,
            },
            KVal::State { id, unit, len } if *len > 0 => LaneIn {
                src: LaneSrc::State(*id),
                unit: *unit,
                len: Some(*len),
                rate: Rate::Audio,
            },
            KVal::Table { id, unit, values } if !values.is_empty() => LaneIn {
                src: LaneSrc::Table(*id),
                unit: *unit,
                len: Some(values.len()),
                rate: Rate::Const,
            },
            KVal::Lane { id, unit, len } if *len > 0 => LaneIn {
                src: LaneSrc::Lane(*id),
                unit: *unit,
                len: Some(*len),
                rate: self.lane_rates[*id],
            },
            _ => return None,
        })
    }

    /// An array (or a single value) as something a vector instruction reads, gathering computed
    /// elements into a lane.
    fn lane_of(&mut self, v: KVal, span: Span) -> Res<LaneIn> {
        if let Some(input) = self.lane_input(&v) {
            return Ok(input);
        }
        let v = match v {
            KVal::Range { .. } => KVal::array(self.array_elements(v, span)?),
            other => other,
        };
        if !matches!(v, KVal::Array(_)) {
            return Err(Diagnostic::new("expected an array of numbers here", span));
        }
        let (source, unit) = self.numeric_array(v, span)?;
        let ArraySource::Values(values) = source else {
            unreachable!("computed elements are held as values")
        };
        let (len, rate) = (self.values[values].len(), self.value_rates[values]);
        let dst = self.new_lane(len, rate);
        self.push(rate, Instr::LaneFromRegs { dst, values });
        Ok(LaneIn {
            src: LaneSrc::Lane(dst),
            unit,
            len: Some(len),
            rate,
        })
    }

    /// Like [`Kb::lane_of`], but `None` for anything that is not a number or a flat array of them.
    fn lane_operand(&mut self, v: &KVal, span: Span) -> Res<Option<LaneIn>> {
        if let Some(input) = self.lane_input(v) {
            return Ok(Some(input));
        }
        let flat = match v {
            KVal::Array(items) => {
                !items.is_empty() && items.iter().all(|i| matches!(i, KVal::Num(_)))
            }
            _ => false,
        };
        if flat {
            return self.lane_of(v.clone(), span).map(Some);
        }
        Ok(None)
    }

    /// A copy of a state array as it is now.
    fn lane_copy(&mut self, v: KVal, span: Span) -> Res<KVal> {
        let KVal::State { unit, len, .. } = v else {
            return Ok(v);
        };
        if len == 0 || self.force.is_some() {
            return Ok(KVal::array(self.array_elements(v, span)?));
        }
        let input = self.lane_input(&v).expect("a state array");
        let one = self.constant(Unit::Num, 1.0);
        let dst = self.new_lane(len, Rate::Audio);
        self.push(
            Rate::Audio,
            Instr::LaneBin {
                dst,
                op: Op::Mul,
                a: input.src,
                b: LaneSrc::Reg(one.reg),
            },
        );
        Ok(KVal::Lane { id: dst, unit, len })
    }

    /// `a op b` as one vector instruction, when one of them is a state array or a lane and the
    /// units combine plainly. `None` means the caller should work element by element.
    fn lane_binary(
        &mut self,
        op: Op,
        unit_of: impl Fn(Unit, Unit) -> Option<Unit>,
        a: &KVal,
        b: &KVal,
        span: Span,
    ) -> Res<Option<KVal>> {
        let held = |v: &KVal| matches!(v, KVal::State { .. } | KVal::Lane { .. });
        if self.force.is_some() || !(held(a) || held(b)) {
            return Ok(None);
        }
        let (Some(x), Some(y)) = (self.lane_operand(a, span)?, self.lane_operand(b, span)?) else {
            return Ok(None);
        };
        let len = match (x.len, y.len) {
            (Some(m), Some(n)) if m != n => {
                return Err(Diagnostic::new(
                    format!("can't combine an array of {m} with an array of {n}"),
                    span,
                ));
            }
            (Some(n), _) | (_, Some(n)) => n,
            (None, None) => return Ok(None),
        };
        let Some(unit) = unit_of(x.unit, y.unit) else {
            return Ok(None);
        };
        let rate = self.rate(x.rate.max(y.rate));
        let dst = self.new_lane(len, rate);
        self.push(
            rate,
            Instr::LaneBin {
                dst,
                op,
                a: x.src,
                b: y.src,
            },
        );
        Ok(Some(KVal::Lane { id: dst, unit, len }))
    }

    /// `x.frames(size, fn(w) { ... })`: the body runs every `size / overlap` samples on the last `size` samples
    /// of `x` (windowed, as an array `w`) and returns `size` samples, which are overlapped into the output.
    /// The output is `size` samples late.
    fn frames_scope(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        const WINDOW: &str = "<frame window>";
        if self.in_frame || self.guard.is_some() || self.force.is_some() {
            return Err(Diagnostic::new(
                "`frames` can't be used inside another frame, an `if` or here",
                name.span,
            ));
        }
        let bound = funcs::bind(
            "frames",
            name.span,
            &["x", "size", "body", "other", "overlap"],
            3,
            args,
        )?;
        let need = |i: usize, what: &str| {
            bound[i].ok_or_else(|| Diagnostic::new(format!("`frames` needs {what}"), name.span))
        };
        let x = self.num(&need(0, "a signal")?.value)?;
        if x.unit != Unit::Num {
            return Err(Diagnostic::new(
                "`frames` takes a plain signal",
                need(0, "a signal")?.value.span,
            ));
        }
        let other = bound[3].map(|arg| self.num(&arg.value)).transpose()?;
        if other.as_ref().is_some_and(|v| v.unit != Unit::Num) {
            return Err(Diagnostic::new("`frames` takes a plain signal", span));
        }
        let size_arg = need(1, "a size")?;
        let size = self.num(&size_arg.value)?;
        let size = match self.konst(&size) {
            Some(k)
                if k.fract() == 0.0
                    && (4.0..=65536.0).contains(&k)
                    && (k as usize).is_power_of_two() =>
            {
                k as usize
            }
            _ => {
                return Err(Diagnostic::new(
                    "a frame size is a power of two from 4 to 65536, known while compiling",
                    size_arg.value.span,
                ));
            }
        };
        let overlap = if let Some(arg) = bound[4] {
            let value = self.num(&arg.value)?;
            match self.konst(&value) {
                Some(n)
                    if value.unit == Unit::Num
                        && [2.0, 4.0, 8.0, 16.0].contains(&n)
                        && n <= size as f64 =>
                {
                    n as usize
                }
                _ => {
                    return Err(Diagnostic::new(
                        "frame overlap is 2, 4, 8 or 16, no larger than size, known while compiling",
                        arg.value.span,
                    ));
                }
            }
        } else {
            2
        };
        let body_arg = need(2, "a function")?;
        let KVal::Function { def, captures } = self.value(&body_arg.value)? else {
            return Err(Diagnostic::new(
                "the `body` argument of `frames` is a function, as in `fn(w) { ... }`",
                body_arg.value.span,
            ));
        };
        let window = self.new_lane(size, Rate::Audio);
        self.frame.scope.insert(
            WINDOW.into(),
            KVal::Lane {
                id: window,
                unit: Unit::Num,
                len: size,
            },
        );
        let given = Arg {
            name: None,
            receiver: false,
            value: Expr {
                kind: ExprKind::Name(WINDOW.into()),
                span: body_arg.value.span,
            },
        };
        let mut given = vec![given];
        let window2 = other.as_ref().map(|_| {
            let lane = self.new_lane(size, Rate::Audio);
            self.frame.scope.insert(
                "<frame window 2>".into(),
                KVal::Lane {
                    id: lane,
                    unit: Unit::Num,
                    len: size,
                },
            );
            given.push(Arg {
                name: None,
                receiver: false,
                value: Expr {
                    kind: ExprKind::Name("<frame window 2>".into()),
                    span,
                },
            });
            lane
        });
        // the body's code is kept apart, to run when a frame is ready
        let outer = std::mem::take(&mut self.code);
        self.in_frame = true;
        let called = self.call_captured(&def, name, &given, captures);
        let result = called.and_then(|result| self.lane_of(result, span));
        self.in_frame = false;
        let code = std::mem::replace(&mut self.code, outer);
        self.frame.scope.remove(WINDOW);
        self.frame.scope.remove("<frame window 2>");
        let result = result?;
        if result.len != Some(size) || result.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("the function of `frames` returns {size} plain samples"),
                body_arg.value.span,
            ));
        }
        let scope = self.frames.len();
        self.frames.push(niminal_engine::ops::FrameScope {
            size,
            hop: size / overlap,
            window,
            window2,
            result: result.src,
            code,
        });
        let reg = self.emit(Rate::Audio, |dst| Instr::Frame {
            scope,
            input: x.reg,
            input2: other.map(|v| v.reg),
            dst,
        });
        Ok(KVal::Num(Value {
            reg,
            unit: Unit::Num,
            rate: Rate::Audio,
            port: None,
        }))
    }

    /// The FFT plan for `size`, made once.
    fn fft_plan(&mut self, size: usize) -> usize {
        match self.ffts.iter().position(|&n| n == size) {
            Some(plan) => plan,
            None => {
                self.ffts.push(size);
                self.ffts.len() - 1
            }
        }
    }

    /// `fft(w)`: the spectrum of an array of `n` samples, as `[re, im]` with `n / 2 + 1` entries each.
    fn fft_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let [arg] = args else {
            return Err(Diagnostic::new("`fft` takes an array", name.span));
        };
        let value = self.value(&arg.value)?;
        let input = self.lane_of(value, arg.value.span)?;
        let n = input
            .len
            .filter(|n| n.is_power_of_two() && *n >= 4)
            .ok_or_else(|| {
                Diagnostic::new(
                    "`fft` takes an array whose length is a power of two, at least 4",
                    span,
                )
            })?;
        if input.unit != Unit::Num {
            return Err(Diagnostic::new("`fft` takes plain numbers", arg.value.span));
        }
        let rate = self.rate(input.rate);
        let (re, im) = (
            self.new_lane(n / 2 + 1, rate),
            self.new_lane(n / 2 + 1, rate),
        );
        let plan = self.fft_plan(n);
        self.push(
            rate,
            Instr::Fft {
                re,
                im,
                src: input.src,
                plan,
            },
        );
        let lane = |id| KVal::Lane {
            id,
            unit: Unit::Num,
            len: n / 2 + 1,
        };
        Ok(KVal::array(vec![lane(re), lane(im)]))
    }

    /// `ifft(re, im)` (or `ifft([re, im])`): the `n` samples with that spectrum.
    fn ifft_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let parts = match args {
            [spectrum] => match self.value(&spectrum.value)? {
                KVal::Array(items) if items.len() == 2 => items.into_vec(),
                _ => {
                    return Err(Diagnostic::new(
                        "`ifft` takes the real and imaginary parts, as `ifft(re, im)`",
                        name.span,
                    ));
                }
            },
            [re, im] => vec![self.value(&re.value)?, self.value(&im.value)?],
            _ => {
                return Err(Diagnostic::new(
                    "`ifft` takes the real and imaginary parts, as `ifft(re, im)`",
                    name.span,
                ));
            }
        };
        let mut lanes = Vec::new();
        for part in parts {
            lanes.push(self.lane_of(part, span)?);
        }
        let (re, im) = (lanes[0], lanes[1]);
        let bins = match (re.len, im.len) {
            (Some(a), Some(b)) if a == b && a >= 3 => a,
            _ => {
                return Err(Diagnostic::new(
                    "`ifft` takes two arrays of the same length, `n / 2 + 1` for a signal of `n`",
                    span,
                ));
            }
        };
        let n = (bins - 1) * 2;
        if !n.is_power_of_two() || re.unit != Unit::Num || im.unit != Unit::Num {
            return Err(Diagnostic::new(
                "`ifft` makes a signal whose length is a power of two, from plain numbers",
                span,
            ));
        }
        let rate = self.rate(re.rate.max(im.rate));
        let dst = self.new_lane(n, rate);
        let plan = self.fft_plan(n);
        self.push(
            rate,
            Instr::Ifft {
                dst,
                re: re.src,
                im: im.src,
                plan,
            },
        );
        Ok(KVal::Lane {
            id: dst,
            unit: Unit::Num,
            len: n,
        })
    }

    /// `dot`, `fir`, `taps` and `matvec`: arrays in, one instruction.
    fn vector_call(&mut self, name: &Ident, args: &[Arg], l: KVal, span: Span) -> Res<KVal> {
        let op = name.name.as_str();
        let (params, positional): (&[&str], usize) = match op {
            "dot" => (&["a", "b"], 2),
            "fir" => (&["ring", "taps", "from"], 3),
            "taps" => (&["ring", "offsets", "from", "interp"], 3),
            _ => (&["matrix", "v"], 2),
        };
        let bound = funcs::bind(op, name.span, params, positional, args)?;
        let given = |i: usize| bound[i];
        let need = |i: usize| {
            given(i)
                .ok_or_else(|| Diagnostic::new(format!("`{op}` needs `{}`", params[i]), name.span))
        };
        let first = need(0)?.value.span;
        let rate_of = |this: &Self, rates: &[Rate]| {
            this.rate(rates.iter().copied().max().unwrap_or(Rate::Const))
        };
        let product = |a: Unit, b: Unit| {
            crate::unit::mul_unit(a, b).ok_or_else(|| {
                Diagnostic::new(
                    format!("can't multiply {} and {}", a.describe(), b.describe()),
                    span,
                )
            })
        };
        let scalar = |this: &mut Self, reg: usize, unit: Unit, rate: Rate| {
            KVal::Num(Value {
                reg: {
                    let _ = this;
                    reg
                },
                unit,
                rate,
                port: None,
            })
        };
        match op {
            "dot" => {
                let a = self.lane_of(l, first)?;
                let other = self.value(&need(1)?.value)?;
                let b = self.lane_of(other, need(1)?.value.span)?;
                if a.len.is_none() || a.len != b.len {
                    return Err(Diagnostic::new(
                        "`dot` takes two arrays of the same length",
                        span,
                    ));
                }
                let unit = product(a.unit, b.unit)?;
                let rate = rate_of(self, &[a.rate, b.rate]);
                let reg = self.emit(rate, |dst| Instr::Dot {
                    dst,
                    a: a.src,
                    b: b.src,
                });
                Ok(scalar(self, reg, unit, rate))
            }
            "fir" | "taps" => {
                let KVal::State {
                    id: ring,
                    unit,
                    len: ring_len,
                } = l
                else {
                    return Err(Diagnostic::new(
                        format!("`{op}` reads a state array, such as `state buf = zeros(1024)`"),
                        first,
                    ));
                };
                if ring_len == 0 {
                    return Err(Diagnostic::new("this array is empty", first));
                }
                let array = self.value(&need(1)?.value)?;
                let array = self.lane_of(array, need(1)?.value.span)?;
                let Some(count) = array.len else {
                    return Err(Diagnostic::new(
                        format!("`{op}` takes an array of {}", params[1]),
                        need(1)?.value.span,
                    ));
                };
                let from = self.num(&need(2)?.value)?;
                if from.unit != Unit::Num {
                    return Err(Diagnostic::new(
                        "a position is a plain number",
                        need(2)?.value.span,
                    ));
                }
                if op == "fir" {
                    if count > ring_len {
                        return Err(Diagnostic::new(
                            format!("{count} taps are more than the {ring_len} entries to read"),
                            need(1)?.value.span,
                        ));
                    }
                    let unit = product(array.unit, unit)?;
                    let rate = Rate::Audio;
                    let reg = self.emit(rate, |dst| Instr::Fir {
                        dst,
                        taps: array.src,
                        ring,
                        from: from.reg,
                    });
                    return Ok(scalar(self, reg, unit, rate));
                }
                if array.unit != Unit::Num {
                    return Err(Diagnostic::new(
                        "offsets are plain numbers of samples",
                        need(1)?.value.span,
                    ));
                }
                let interp = match given(3) {
                    Some(a) => interp_of(&a.value)?,
                    None => Interp::Linear,
                };
                let dst = self.new_lane(count, Rate::Audio);
                self.push(
                    Rate::Audio,
                    Instr::Taps {
                        dst,
                        ring,
                        offsets: array.src,
                        from: from.reg,
                        interp,
                    },
                );
                Ok(KVal::Lane {
                    id: dst,
                    unit,
                    len: count,
                })
            }
            _ => {
                let m = self.lane_of(l, first)?;
                let other = self.value(&need(1)?.value)?;
                let v = self.lane_of(other, need(1)?.value.span)?;
                let (Some(total), Some(cols)) = (m.len, v.len) else {
                    return Err(Diagnostic::new(
                        "`matvec` takes a matrix and a vector",
                        span,
                    ));
                };
                if total % cols != 0 {
                    return Err(Diagnostic::new(
                        format!("a matrix of {total} entries does not have rows of {cols}"),
                        span,
                    ));
                }
                let unit = product(m.unit, v.unit)?;
                let rate = rate_of(self, &[m.rate, v.rate]);
                let dst = self.new_lane(total / cols, rate);
                self.push(
                    rate,
                    Instr::MatVec {
                        dst,
                        matrix: m.src,
                        cols,
                        v: v.src,
                    },
                );
                Ok(KVal::Lane {
                    id: dst,
                    unit,
                    len: total / cols,
                })
            }
        }
    }
}

/// Whether two values are the same, for names a branch didn't change.
fn same(a: &KVal, b: &KVal) -> bool {
    match (a, b) {
        (KVal::Num(x), KVal::Num(y)) => x.reg == y.reg,
        (KVal::Table { id: a, .. }, KVal::Table { id: b, .. })
        | (KVal::State { id: a, .. }, KVal::State { id: b, .. }) => a == b,
        (KVal::Array(a), KVal::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (KVal::Range { lo: a, hi: b, .. }, KVal::Range { lo: c, hi: d, .. }) => a == c && b == d,
        (KVal::Record(a), KVal::Record(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.0 == y.0 && same(&x.1, &y.1))
        }
        _ => false,
    }
}

/// Whether a block's last statement is an expression that gives a value.
pub(crate) fn ends_with_value(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(Stmt::Expr(e)) => match &e.kind {
            ExprKind::If {
                then,
                otherwise: Some(o),
                ..
            } => ends_with_value(then) && ends_with_value(o),
            ExprKind::If {
                otherwise: None, ..
            } => false,
            ExprKind::For { body, .. } => ends_with_value(body),
            _ => true,
        },
        _ => false,
    }
}

fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Expr(e) => e.span,
        Stmt::Bind { name, .. }
        | Stmt::State { name, .. }
        | Stmt::AddAssign { name, .. }
        | Stmt::IndexAssign { name, .. } => name.span,
        Stmt::Destructure { value, .. } => value.span,
    }
}

pub(crate) fn cmp_op(op: CmpOp) -> Cmp {
    match op {
        CmpOp::Lt => Cmp::Lt,
        CmpOp::Gt => Cmp::Gt,
        CmpOp::Le => Cmp::Le,
        CmpOp::Ge => Cmp::Ge,
        CmpOp::Eq => Cmp::Eq,
        CmpOp::Ne => Cmp::Ne,
    }
}

/// The whole numbers a range runs over.
pub(crate) fn range_numbers(lo: f64, hi: f64, inclusive: bool, span: Span) -> Res<Vec<f64>> {
    if lo.fract() != 0.0 || hi.fract() != 0.0 {
        return Err(Diagnostic::new(
            "a range runs over whole numbers; use `.step(...)` for others",
            span,
        ));
    }
    let end = if inclusive { hi } else { hi - 1.0 };
    if end < lo {
        return Ok(Vec::new());
    }
    let count = (end - lo) as usize + 1;
    if count > 1 << 20 {
        return Err(Diagnostic::new("this range is too long", span));
    }
    Ok((0..count).map(|i| lo + i as f64).collect())
}

/// The values a range takes in steps of `step`: `(0..1).step(0.25)` is 0, 0.25, 0.5, 0.75 and 1.
pub(crate) fn step_numbers(
    lo: f64,
    hi: f64,
    inclusive: bool,
    step: f64,
    span: Span,
) -> Res<Vec<f64>> {
    if step <= 0.0 {
        return Err(Diagnostic::new("a step must be above 0", span));
    }
    let count = ((hi - lo) / step + 1e-9).floor();
    if count < 0.0 {
        return Ok(Vec::new());
    }
    if count > 65_536.0 {
        return Err(Diagnostic::new("this range has too many steps", span));
    }
    let mut values: Vec<f64> = (0..=count as usize).map(|i| lo + i as f64 * step).collect();
    // `..<` leaves the end out, even when a step lands on it
    if !inclusive
        && values
            .last()
            .is_some_and(|last| (hi - last).abs() < step * 1e-9)
    {
        values.pop();
    }
    Ok(values)
}

/// `interp: none`, `linear` or `cubic`.
pub(crate) fn interp_of(e: &Expr) -> Res<Interp> {
    match &e.kind {
        ExprKind::Name(n) if n == "none" => Ok(Interp::None),
        ExprKind::Name(n) if n == "linear" => Ok(Interp::Linear),
        ExprKind::Name(n) if n == "cubic" => Ok(Interp::Cubic),
        _ => Err(Diagnostic::new(
            "`interp` is `none`, `linear` or `cubic`",
            e.span,
        )),
    }
}

pub(crate) fn edge_of(e: &Expr) -> Res<niminal_engine::Edge> {
    match &e.kind {
        ExprKind::Name(n) if n == "wrap" => Ok(niminal_engine::Edge::Wrap),
        ExprKind::Name(n) if n == "clamp" => Ok(niminal_engine::Edge::Clamp),
        _ => Err(Diagnostic::new("`edge` is `wrap` or `clamp`", e.span)),
    }
}

/// A function given to `map`: `fn(x) { ... }`, or an expression of `it`.
pub(crate) enum Body<'e> {
    Block(&'e [Stmt]),
    Expr(&'e Expr),
}

pub(crate) fn lambda_of(e: &Expr) -> Res<(String, Body<'_>)> {
    match &e.kind {
        ExprKind::Lambda { params, body } => {
            let [param] = params.as_slice() else {
                return Err(Diagnostic::new(
                    "the function given to `map` takes one parameter",
                    e.span,
                ));
            };
            Ok((param.name.clone(), Body::Block(body)))
        }
        _ => Ok(("it".to_string(), Body::Expr(e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use niminal_engine::ops::Fn1;

    fn kernel(src: &str, args: &[KArg]) -> Kernel {
        let items = parse(src).unwrap();
        let Item::Fn(def) = &items[0] else { panic!() };
        let names = Names::default();
        compile_kernel(def, args, &Registry::builtin(), &names, Tempo::new(120.0))
            .unwrap()
            .0
    }

    const ONE_POLE: &str = "fn one_pole(x, cutoff: hz) {\n  state y = 0\n  a = exp(-2 * pi * cutoff / sample_rate)\n  y = x * (1 - a) + y * a\n  y\n}";

    fn has_exp(code: &[Instr]) -> bool {
        code.iter()
            .any(|i| matches!(i, Instr::Call { f: Fn1::Exp, .. }))
    }

    #[test]
    fn work_runs_at_the_rate_of_what_it_depends_on() {
        let x = KArg::Port {
            unit: Unit::Num,
            rate: Rate::Audio,
        };
        // a cutoff known while compiling: the coefficient is worked out once, at note start
        let k = kernel(
            ONE_POLE,
            &[
                x.clone(),
                KArg::Const {
                    unit: Unit::Hz,
                    value: 800.0,
                },
            ],
        );
        assert!(has_exp(&k.init) && !has_exp(&k.block) && !has_exp(&k.code));
        // a cutoff that changes once per block: once per block
        let k = kernel(
            ONE_POLE,
            &[
                x.clone(),
                KArg::Port {
                    unit: Unit::Hz,
                    rate: Rate::Control,
                },
            ],
        );
        assert!(has_exp(&k.block) && !has_exp(&k.code));
        // one that changes every sample: every sample
        let k = kernel(
            ONE_POLE,
            &[
                x,
                KArg::Port {
                    unit: Unit::Hz,
                    rate: Rate::Audio,
                },
            ],
        );
        assert!(has_exp(&k.code));
    }

    #[test]
    fn several_results_come_from_one_kernel() {
        let k = kernel(
            "fn split(x, c: hz) -> (low, high) {\n state y = 0\n y = y + (x - y) * 0.1\n low = y\n high = x - y\n}",
            &[
                KArg::Port {
                    unit: Unit::Num,
                    rate: Rate::Audio,
                },
                KArg::Const {
                    unit: Unit::Hz,
                    value: 1.0,
                },
            ],
        );
        assert_eq!(
            k.outputs
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["low", "high"]
        );
        assert_eq!(k.state_init.len(), 1, "the body ran once for both results");
    }

    #[test]
    fn a_state_starts_from_a_note_start_value() {
        let k = kernel(
            "fn glide(x: hz) {\n state y = x\n y = y + (x - y) * 0.01\n y\n}",
            &[KArg::Port {
                unit: Unit::Hz,
                rate: Rate::Audio,
            }],
        );
        assert!(
            k.init.iter().any(|i| matches!(i, Instr::StoreState { .. })),
            "the start is stored at note start"
        );
        assert!(
            k.init.iter().any(|i| matches!(i, Instr::Input { .. })),
            "from the input's first sample"
        );
    }
}
