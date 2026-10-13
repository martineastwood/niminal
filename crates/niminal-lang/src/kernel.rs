//! Compiles a function with state to an engine [`Kernel`]: straight-line
//! register code run once per sample. A kernel is compiled for each call, so a
//! function's untyped parameters take whatever units the call gives them.
//!
//! Every value has a rate, and the code that makes it runs at that rate: once
//! at note start, once per block, or every sample. Functions the body calls are
//! copied in, with their own state.

use std::collections::HashMap;
use std::sync::Arc;

use niminal_engine::Port;
use niminal_engine::ops::{
    BinOp as Op, Cmp, Fn1, Instr, Interp, Kernel, List, Native, NativeInput,
};
use niminal_score::Tempo;

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::funcs::{self, Kind, Param};
use crate::lower::{Datum, Names, literal_sig, unknown_name};
use crate::opcodes::{self, Build, BuildArgs, Registry, math_fn};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

/// What a call gives one parameter of a function compiled to a kernel.
#[derive(Clone, Debug)]
pub(crate) enum KArg {
    /// A signal wired into one of the kernel's ports.
    Port { unit: Unit, rate: Rate },
    /// A value known while compiling.
    Const { unit: Unit, value: f64 },
    /// A list of numbers known while compiling.
    Table { unit: Unit, values: Arc<[f64]> },
    /// A range known while compiling.
    Range {
        unit: Unit,
        lo: f64,
        hi: f64,
        inclusive: bool,
    },
    /// Not given: the parameter's default.
    Missing,
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
    Num(Value),
    /// A list known while compiling.
    List(Vec<KVal>),
    /// A list of numbers shared by every voice.
    Table {
        id: usize,
        unit: Unit,
        values: Arc<[f64]>,
    },
    /// A list this kernel writes.
    State { id: usize, unit: Unit, len: usize },
    Range {
        unit: Unit,
        lo: f64,
        hi: f64,
        inclusive: bool,
    },
    /// The results of a function with several, by name.
    Record(Vec<(String, Value)>),
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
    /// `prev(y)` written before `y` is: the state it reads.
    forward: HashMap<String, (usize, Span)>,
    /// `prev(expr)`: the state to store the value in at the end of the sample.
    deferred: Vec<(usize, usize)>,
    /// The value of `out`, if the body builds it.
    out: Option<Value>,
}

struct Kb<'a> {
    registry: &'a Registry,
    names: &'a Names,
    tempo: Tempo,
    init: Vec<Instr>,
    block: Vec<Instr>,
    code: Vec<Instr>,
    registers: usize,
    state_init: Vec<f64>,
    lists: Vec<Vec<f64>>,
    tables: Vec<Arc<[f64]>>,
    natives: Vec<Native>,
    ports: Vec<Port>,
    /// Registers that hold a value known while compiling.
    consts: HashMap<usize, f64>,
    frame: Frame,
    /// Set inside a branch: state is only written when this register is not 0.
    guard: Option<usize>,
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
) -> Res<(Kernel, Vec<Unit>)> {
    let params = funcs::signature(&def.params, tempo)?;
    let mut kb = Kb {
        registry,
        names,
        tempo,
        init: Vec::new(),
        block: Vec::new(),
        code: Vec::new(),
        registers: 0,
        state_init: Vec::new(),
        lists: Vec::new(),
        tables: Vec::new(),
        natives: Vec::new(),
        ports: Vec::new(),
        consts: HashMap::new(),
        frame: Frame::default(),
        guard: None,
        force: None,
        stack: vec![def.name.name.clone()],
        const_tables: HashMap::new(),
    };
    for (param, arg) in params.iter().zip(args) {
        let v = match arg {
            KArg::Port { unit, rate } => {
                let port = kb.ports.len();
                kb.ports.push(Port::named(param.name.name.clone(), None));
                let reg = kb.emit(*rate, |dst| Instr::Input { dst, port });
                KVal::Num(Value {
                    reg,
                    unit: *unit,
                    rate: *rate,
                    port: Some(port),
                })
            }
            KArg::Const { unit, value } => KVal::Num(kb.constant(*unit, *value)),
            KArg::Table { unit, values } => kb.table(*unit, values.clone()),
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
            KArg::Missing => {
                let Some(default) = &param.default else {
                    return Err(Diagnostic::new(
                        format!("`{}` needs an argument `{}`", def.name.name, param.name.name),
                        def.name.span,
                    ));
                };
                let v = kb.arg_value(param, default)?;
                kb.check_param(&def.name.name, param, &v, default.span)?
            }
        };
        if let Kind::Choice(options) = &param.kind {
            kb.bind_choices(options);
        }
        kb.frame.scope.insert(param.name.name.clone(), v);
    }
    if kb.ports.len() > niminal_engine::MAX_INPUTS {
        return Err(Diagnostic::new(
            format!(
                "`{}` is given {} signals; a function with state takes at most {}",
                def.name.name,
                kb.ports.len(),
                niminal_engine::MAX_INPUTS
            ),
            def.name.span,
        ));
    }
    let result = kb.body(def)?;
    let (outputs, units): (Vec<(String, usize)>, Vec<Unit>) = match result {
        KVal::Num(v) => (vec![("out".to_string(), v.reg)], vec![v.unit]),
        KVal::Record(fields) => fields.into_iter().map(|(n, v)| ((n, v.reg), v.unit)).unzip(),
        _ => unreachable!("`body` gives a number or a record"),
    };
    let kernel = Kernel {
        name: def.name.name.clone(),
        ports: kb.ports,
        init: kb.init,
        block: kb.block,
        code: kb.code,
        registers: kb.registers,
        state_init: kb.state_init,
        lists: kb.lists,
        tables: kb.tables,
        outputs,
        natives: kb.natives,
    };
    Ok((kernel, units))
}

fn not_a_number(e: &Expr) -> Diagnostic {
    Diagnostic::new("expected a number or a signal here, found a list", e.span)
}

impl Kb<'_> {
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

    fn table(&mut self, unit: Unit, values: Arc<[f64]>) -> KVal {
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
                    Section::Init => "this changes while the note plays, so it isn't known at note start",
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
        self.code.push(Instr::StoreState { slot: held, src: reg });
        self.code.push(Instr::StoreState { slot: flag, src: one });
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
                    let v = match self.frame.scope.get(&out.name) {
                        Some(KVal::Num(v)) => *v,
                        Some(_) => {
                            return Err(Diagnostic::new(
                                format!("result `{}` must be a number or a signal", out.name),
                                out.span,
                            ));
                        }
                        None => {
                            return Err(Diagnostic::new(
                                format!(
                                    "`{}` is a result of `{}`, but the body never defines it",
                                    out.name, def.name.name
                                ),
                                out.span,
                            )
                            .with_help(format!("add a line `{} = ...`", out.name)));
                        }
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
                    (Some((v, span)), None) => match v {
                        KVal::Num(v) => (v, span),
                        KVal::Record(fields) => (fields[0].1, span),
                        _ => {
                            return Err(Diagnostic::new(
                                "a function with state gives a number or a signal",
                                span,
                            ));
                        }
                    },
                    (None, Some(out)) => (out, def.name.span),
                    (None, None) => {
                        return Err(Diagnostic::new(
                            format!("`{}` must end with the value it gives", def.name.name),
                            def.name.span,
                        )
                        .with_help("the last line of a function is its result"));
                    }
                };
                if let Some(unit) = funcs::result_unit(&def.result)?
                    && v.0.unit != unit
                {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` gives {}, but its result is declared as {}",
                            def.name.name,
                            v.0.unit.describe(),
                            unit.describe()
                        ),
                        v.1,
                    ));
                }
                KVal::Num(v.0)
            }
        };
        self.finish_frame()?;
        Ok(result)
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
            (Kind::List(_), KVal::List(_) | KVal::Table { .. } | KVal::State { .. }) => {}
            (Kind::List(_), _) => return Err(mismatch("a single value")),
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
            (_, KVal::Range { .. }) => return Err(mismatch("a range")),
            (_, _) => return Err(mismatch("a list")),
        }
        Ok(v.clone())
    }

    /// A call of another user function: its body is compiled here, with its own state.
    fn call_fn(&mut self, def: &FnDef, name: &Ident, args: &[Arg]) -> Res<KVal> {
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
                Some(arg) => Some((self.arg_value(param, &arg.value)?, arg.value.span)),
                None => None,
            });
        }
        let saved = std::mem::take(&mut self.frame);
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
        let (Some(rate), KVal::Num(value)) = (param.rate(), &v) else {
            return Ok(v);
        };
        Ok(KVal::Num(match rate {
            Rate::Init => self.at_init(*value),
            Rate::Control => self.at_control(*value, span)?,
            Rate::Const | Rate::Audio => *value,
        }))
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
                let Some(KVal::State { id, unit, .. }) = self.frame.scope.get(&name.name).cloned()
                else {
                    return Err(Diagnostic::new(
                        format!("`{}` is not a state list, so it can't be written", name.name),
                        name.span,
                    )
                    .with_help(format!("declare it with `state {} = zeros(64)`", name.name)));
                };
                let i = self.num(index)?;
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
                let entries = match self.value(value)? {
                    KVal::List(items) => items,
                    KVal::Table { id, unit, values } => (0..values.len())
                        .map(|k| {
                            let _ = id;
                            KVal::Num(self.constant(unit, values[k]))
                        })
                        .collect(),
                    _ => {
                        return Err(Diagnostic::new(
                            "only a list can be taken apart with `[a, b] = ...`",
                            value.span,
                        ));
                    }
                };
                if entries.len() != names.len() {
                    return Err(Diagnostic::new(
                        format!(
                            "this list has {} entries, but {} name(s) are given",
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
                ExprKind::If { otherwise, then, .. }
                    if !(last && otherwise.is_some() && ends_with_value(then)) =>
                {
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
        if matches!(name.name.as_str(), "it" | "note" | "state" | "pi" | "sample_rate" | "true" | "false") {
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
        if let Some(KVal::State { .. }) = self.frame.scope.get(&name.name) {
            return Err(Diagnostic::new(
                format!("`{0}` is a state list: write its entries with `{0}[i] = ...`", name.name),
                name.span,
            ));
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
                let id = self.lists.len();
                self.lists.push(values.to_vec());
                self.frame.scope.insert(
                    name.name.clone(),
                    KVal::State {
                        id,
                        unit,
                        len: values.len(),
                    },
                );
            }
            KVal::List(items) => {
                let (unit, values) = self.numbers(&items, init.span)?;
                let id = self.lists.len();
                let len = values.len();
                self.lists.push(values);
                self.frame
                    .scope
                    .insert(name.name.clone(), KVal::State { id, unit, len });
            }
            _ => {
                return Err(Diagnostic::new(
                    "a state starts as a number, or as a list such as `zeros(64)`",
                    init.span,
                ));
            }
        }
        Ok(())
    }

    /// The numbers of a list known while compiling, and their unit.
    fn numbers(&self, items: &[KVal], span: Span) -> Res<(Unit, Vec<f64>)> {
        let mut unit = None;
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            let KVal::Num(v) = item else {
                return Err(Diagnostic::new("a state list holds numbers", span));
            };
            let Some(k) = self.konst(v) else {
                return Err(Diagnostic::new(
                    "a state list starts from numbers known while compiling",
                    span,
                ));
            };
            match unit {
                None => unit = Some(v.unit),
                Some(u) if u != v.unit => {
                    return Err(Diagnostic::new(
                        "the entries of a list must have the same unit",
                        span,
                    ));
                }
                _ => {}
            }
            values.push(k);
        }
        if values.is_empty() {
            return Err(Diagnostic::new("a state list needs at least one entry", span));
        }
        Ok((unit.unwrap_or(Unit::Num), values))
    }

    /// Run a block's statements. Names first bound inside it stay inside it. The trailing
    /// expression's value, if `value` is asked for.
    fn block(&mut self, stmts: &[Stmt], value: bool, span: Span) -> Res<Option<KVal>> {
        let outer: Vec<String> = self.frame.scope.keys().cloned().collect();
        let mut tail = None;
        for (i, s) in stmts.iter().enumerate() {
            if matches!(s, Stmt::State { .. }) {
                let Stmt::State { name, .. } = s else { unreachable!() };
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
            let taken = if k != 0.0 { Some(then) } else { otherwise.as_ref() };
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
            let merged = match (a, b) {
                (KVal::Num(a), KVal::Num(b)) => {
                    if a.unit != b.unit {
                        return Err(Diagnostic::new(
                            format!(
                                "`{name}` is {} on one side of the `if` and {} on the other",
                                a.unit.describe(),
                                b.unit.describe()
                            ),
                            e.span,
                        ));
                    }
                    KVal::Num(self.select(c, *a, *b))
                }
                _ if same(a, b) => a.clone(),
                _ => {
                    return Err(Diagnostic::new(
                        format!("only numbers and signals can change inside a branch, not `{name}`"),
                        e.span,
                    ));
                }
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
    fn entries(&mut self, e: &Expr) -> Res<Vec<KVal>> {
        Ok(match self.value(e)? {
            KVal::List(items) => items,
            KVal::Table { unit, values, .. } => values
                .iter()
                .map(|&k| KVal::Num(self.constant(unit, k)))
                .collect(),
            KVal::Range {
                unit,
                lo,
                hi,
                inclusive,
            } => range_numbers(lo, hi, inclusive, e.span)?
                .into_iter()
                .map(|k| KVal::Num(self.constant(unit, k)))
                .collect(),
            _ => {
                return Err(Diagnostic::new(
                    "`for` runs over a list or a range known while compiling",
                    e.span,
                ));
            }
        })
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
            KVal::Record(fields) => Ok(fields[0].1),
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
                let v = self.num(inner)?;
                if v.unit == Unit::Db {
                    let one = self.constant(Unit::Num, 1.0);
                    return Ok(KVal::Num(self.bin(Op::Div, one, v, Unit::Db)));
                }
                if let Some(k) = self.konst(&v) {
                    return Ok(KVal::Num(self.constant(v.unit, -k)));
                }
                let rate = self.rate(v.rate);
                let reg = self.emit(rate, |dst| Instr::Neg { dst, a: v.reg });
                Ok(KVal::Num(Value {
                    reg,
                    unit: v.unit,
                    rate,
                    port: None,
                }))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                self.binary_value(*op, l, r, e.span)
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let (l, r) = (self.num(lhs)?, self.num(rhs)?);
                if l.unit != r.unit {
                    return Err(Diagnostic::new(
                        format!(
                            "can't compare {} and {}",
                            l.unit.describe(),
                            r.unit.describe()
                        ),
                        e.span,
                    ));
                }
                Ok(KVal::Num(self.cmp(cmp_op(*op), l, r)))
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
                match (a, b) {
                    (KVal::Num(a), KVal::Num(b)) => {
                        if a.unit != b.unit {
                            return Err(Diagnostic::new(
                                format!(
                                    "the two ways of an `if` must have the same unit: one is {}, the other {}",
                                    a.unit.describe(),
                                    b.unit.describe()
                                ),
                                e.span,
                            ));
                        }
                        Ok(KVal::Num(self.select(c, a, b)))
                    }
                    _ => Err(Diagnostic::new(
                        "the two ways of an `if` that changes while playing must be numbers or signals",
                        e.span,
                    )),
                }
            }
            ExprKind::For { var, iter, body } => {
                self.check_name(var)?;
                let mut results = Vec::new();
                for entry in self.entries(iter)? {
                    self.frame.scope.insert(var.name.clone(), entry);
                    let v = self.block(body, true, e.span);
                    self.frame.scope.remove(&var.name);
                    results.push(v?.expect("a value"));
                }
                Ok(KVal::List(results))
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
            ExprKind::Channels(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(self.value(i)?);
                }
                Ok(KVal::List(out))
            }
            ExprKind::Index { list, index } => self.index(list, index, e.span),
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Lambda { .. } => Err(Diagnostic::new(
                "a function value can only be given to `map`",
                e.span,
            )),
            ExprKind::Pattern(_) | ExprKind::Grid(_) | ExprKind::Rows(_) | ExprKind::Rest => Err(
                Diagnostic::new("patterns can't be used inside a function with state", e.span),
            ),
            ExprKind::Str(_) => Err(Diagnostic::new(
                "text can't be used inside a function with state",
                e.span,
            )),
            ExprKind::Env(_) => Err(Diagnostic::new(
                "envelopes can't be used inside a function with state",
                e.span,
            )
            .with_help("make the envelope in the instrument and pass it in as an argument")),
        }
    }

    fn name(&mut self, name: &str, span: Span) -> Res<KVal> {
        if let Some(v) = self.frame.scope.get(name).cloned() {
            return match v {
                KVal::Num(v) => Ok(KVal::Num(self.read(v, span)?)),
                other => Ok(other),
            };
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
                return Err(Diagnostic::new("`note` is only available in an instrument", span)
                    .with_help("pass `note.time` or `note.released` in as an argument"));
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
            let value = note.as_hz().expect("a note is a frequency");
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
            Datum::Sig(s) => match s.konst() {
                Some(k) => Ok(KVal::Num(self.constant(s.unit(), k))),
                None => Err(Diagnostic::new("this constant isn't a number", span)),
            },
            Datum::Range { lo, hi, inclusive } => match (lo.konst(), hi.konst()) {
                (Some(a), Some(b)) => Ok(KVal::Range {
                    unit: lo.unit(),
                    lo: a,
                    hi: b,
                    inclusive: *inclusive,
                }),
                _ => Err(Diagnostic::new("this range isn't known while compiling", span)),
            },
            Datum::List(items) => {
                let numbers: Option<Vec<(Unit, f64)>> = items
                    .iter()
                    .map(|i| match i {
                        Datum::Sig(s) => s.konst().map(|k| (s.unit(), k)),
                        _ => None,
                    })
                    .collect();
                match numbers {
                    Some(n) if !n.is_empty() && n.iter().all(|(u, _)| *u == n[0].0) => {
                        let values: Arc<[f64]> = n.iter().map(|(_, k)| *k).collect();
                        Ok(self.table(n[0].0, values))
                    }
                    _ => {
                        let mut out = Vec::with_capacity(items.len());
                        for i in items {
                            out.push(self.const_value(i, span)?);
                        }
                        Ok(KVal::List(out))
                    }
                }
            }
        }
    }

    fn add(&mut self, a: Value, b: Value, span: Span) -> Res<Value> {
        self.binary(BinOp::Add, a, b, span)
    }

    fn binary_value(&mut self, op: BinOp, l: KVal, r: KVal, span: Span) -> Res<KVal> {
        match (l, r) {
            (KVal::Num(a), KVal::Num(b)) => Ok(KVal::Num(self.binary(op, a, b, span)?)),
            (KVal::List(a), KVal::List(b)) => {
                if a.len() != b.len() {
                    return Err(Diagnostic::new(
                        format!("can't combine a list of {} with a list of {}", a.len(), b.len()),
                        span,
                    ));
                }
                let mut out = Vec::with_capacity(a.len());
                for (x, y) in a.into_iter().zip(b) {
                    out.push(self.binary_value(op, x, y, span)?);
                }
                Ok(KVal::List(out))
            }
            (KVal::List(a), r @ KVal::Num(_)) => {
                let mut out = Vec::with_capacity(a.len());
                for x in a {
                    out.push(self.binary_value(op, x, r.clone(), span)?);
                }
                Ok(KVal::List(out))
            }
            (l @ KVal::Num(_), KVal::List(b)) => {
                let mut out = Vec::with_capacity(b.len());
                for y in b {
                    out.push(self.binary_value(op, l.clone(), y, span)?);
                }
                Ok(KVal::List(out))
            }
            (KVal::Record(f), r) => self.binary_value(op, KVal::Num(f[0].1), r, span),
            (l, KVal::Record(f)) => self.binary_value(op, l, KVal::Num(f[0].1), span),
            _ => Err(Diagnostic::new(
                "arithmetic works on numbers, signals and lists of them",
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
        let l = self.value(list)?;
        if let ExprKind::Range { .. } = index.kind {
            let KVal::Range { lo, hi, inclusive, .. } = self.value(index)? else {
                unreachable!()
            };
            let picks = range_numbers(lo, hi, inclusive, index.span)?;
            let entries = match l {
                KVal::List(items) => items,
                KVal::Table { unit, values, .. } => values
                    .iter()
                    .map(|&k| KVal::Num(self.constant(unit, k)))
                    .collect(),
                _ => {
                    return Err(Diagnostic::new(
                        "only a list known while compiling can be sliced",
                        list.span,
                    ));
                }
            };
            let n = entries.len();
            return Ok(KVal::List(
                picks
                    .into_iter()
                    .map(|k| entries[(k.rem_euclid(n as f64)) as usize].clone())
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
            KVal::List(items) => {
                if items.is_empty() {
                    return Err(Diagnostic::new("this list is empty", span));
                }
                let n = items.len();
                if let Some(k) = self.konst(&i) {
                    return Ok(items[(k.floor().rem_euclid(n as f64)) as usize].clone());
                }
                // an index that changes picks among numbers, every sample
                let mut nums = Vec::with_capacity(n);
                for item in &items {
                    match item {
                        KVal::Num(v) => nums.push(*v),
                        _ => {
                            return Err(Diagnostic::new(
                                "an index that changes can only pick among numbers or signals",
                                index.span,
                            ));
                        }
                    }
                }
                if nums.iter().any(|v| v.unit != nums[0].unit) {
                    return Err(Diagnostic::new(
                        "an index that changes needs entries with the same unit",
                        span,
                    ));
                }
                let n_reg = self.constant(Unit::Num, n as f64);
                let floored = self.call1(Fn1::Floor, i);
                let wrapped = self.bin(Op::Rem, floored, n_reg, Unit::Num);
                let mut result = nums[n - 1];
                for k in (0..n - 1).rev() {
                    let kk = self.constant(Unit::Num, k as f64);
                    let hit = self.cmp(Cmp::Eq, wrapped, kk);
                    result = self.select(hit, nums[k], result);
                }
                Ok(KVal::Num(result))
            }
            KVal::Table { id, unit, values } => {
                if let Some(k) = self.konst(&i) {
                    let n = values.len();
                    return Ok(KVal::Num(
                        self.constant(unit, values[(k.floor().rem_euclid(n as f64)) as usize]),
                    ));
                }
                Ok(KVal::Num(self.read_list(List::Table(id), unit, i, None)))
            }
            KVal::State { id, unit, .. } => {
                Ok(KVal::Num(self.read_list(List::State(id), unit, i, None)))
            }
            _ => Err(Diagnostic::new("only a list can be indexed", list.span)),
        }
    }

    fn read_list(&mut self, list: List, unit: Unit, i: Value, interp: Option<Interp>) -> Value {
        let rate = match list {
            // a state list can change every sample
            List::State(_) => Rate::Audio,
            List::Table(_) => self.rate(i.rate),
        };
        let reg = match interp {
            None => self.emit(rate, |dst| Instr::Read { dst, list, idx: i.reg }),
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
                format!("`{}` is called on a value, as in `x.{}`", name.name, name.name),
                name.span,
            )
        })
    }

    fn call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let n = name.name.as_str();
        // `f.band`: one result of an earlier call with several
        if let [receiver] = args
            && receiver.receiver
            && let ExprKind::Name(record) = &receiver.value.kind
            && let Some(KVal::Record(fields)) = self.frame.scope.get(record)
        {
            return fields
                .iter()
                .find(|(f, _)| f == n)
                .map(|(_, v)| KVal::Num(*v))
                .ok_or_else(|| {
                    let names: Vec<&str> = fields.iter().map(|(f, _)| f.as_str()).collect();
                    Diagnostic::new(format!("this has no result `{n}`"), name.span)
                        .with_help(format!("its results are {}", names.join(", ")))
                });
        }
        if let Some(def) = self.names.fns.get(n) {
            let def = def.clone();
            return self.call_fn(&def, name, args);
        }
        match n {
            "prev" => return self.prev(name, args).map(KVal::Num),
            "min" | "max" | "pow" | "mod" => return self.combine(name, args).map(KVal::Num),
            "clamp" => return self.clamp(name, args).map(KVal::Num),
            "init" | "control" if args.len() == 1 && args[0].receiver => {
                let v = self.num(&args[0].value)?;
                return Ok(KVal::Num(if n == "init" {
                    self.at_init(v)
                } else {
                    self.at_control(v, args[0].value.span)?
                }));
            }
            "len" | "sum" | "map" | "indices" | "at" => return self.list_call(name, args, span),
            "zeros" => {
                let [count] = args else {
                    return Err(Diagnostic::new("`zeros` takes a count", name.span));
                };
                let c = self.num(&count.value)?;
                let n = match self.konst(&c) {
                    Some(k) if k.fract() == 0.0 && (1.0..=(1 << 22) as f64).contains(&k) => k as usize,
                    _ => {
                        return Err(Diagnostic::new(
                            "`zeros` takes a whole number known while compiling",
                            count.value.span,
                        ));
                    }
                };
                return Ok(self.table(Unit::Num, vec![0.0; n].into()));
            }
            "split" | "channels" => {
                return Err(Diagnostic::new(
                    "a function with state works on one channel at a time",
                    name.span,
                ));
            }
            _ => {}
        }
        if let Some(v) = self.unit_method(name, args)? {
            return Ok(KVal::Num(v));
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
            let v = self.num(&arg.value)?;
            if v.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!("`{n}` expects a plain number, found {}", v.unit.describe()),
                    arg.value.span,
                )
                .with_help("divide by a value with the same unit to get a plain ratio"));
            }
            return Ok(KVal::Num(self.call1(f, v)));
        }
        let registry = self.registry;
        if let Some(spec) = registry.find(n) {
            if let Build::Builtin(_) = &spec.build {
                return self.native(name, spec, args).map(KVal::Num);
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
            .chain(["min", "max", "pow", "mod", "clamp", "prev", "zeros"])
            .collect();
        let mut d = Diagnostic::new(format!("unknown function `{n}`"), name.span);
        if let Some(c) = closest(n, known) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        Err(d)
    }

    /// `x.hz`, `n.ms`, `(-6db).num`...: give a plain value a unit, or take it away.
    fn unit_method(&mut self, name: &Ident, args: &[Arg]) -> Res<Option<Value>> {
        let scale = matches!(
            name.name.as_str(),
            "num" | "hz" | "khz" | "db" | "sec" | "ms" | "beat" | "beats" | "bar" | "bars" | "deg" | "st"
        );
        if !scale || !(args.len() == 1 && args[0].receiver) {
            return Ok(None);
        }
        let v = self.num(&args[0].value)?;
        let span = args[0].value.span;
        if name.name == "num" {
            return Ok(Some(match v.unit {
                Unit::Db => {
                    // a db value is held as a gain factor
                    let ln = self.call1(Fn1::Log, Value { unit: Unit::Num, ..v });
                    let k = self.constant(Unit::Num, 20.0 / std::f64::consts::LN_10);
                    self.bin(Op::Mul, ln, k, Unit::Num)
                }
                _ => Value { unit: Unit::Num, ..v },
            }));
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
            return Ok(Some(Value { unit: Unit::Db, ..g }));
        }
        let f = self.constant(Unit::Num, factor);
        Ok(Some(self.bin(Op::Mul, v, f, unit)))
    }

    /// `prev(x)`: what `x` was one sample ago. `y = x + prev(y) * 0.5` is a feedback loop.
    fn prev(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let [arg] = args else {
            return Err(Diagnostic::new("`prev` takes one value", name.span));
        };
        if let Some(n) = &arg.name {
            return Err(Diagnostic::new("`prev` takes its value without a name", n.span));
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
    fn plain_args(&mut self, name: &Ident, args: &[Arg], count: usize) -> Res<Vec<(Value, Span)>> {
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
            values.push((self.num(&a.value)?, a.value.span));
        }
        Ok(values)
    }

    /// `min(a, b)`, `max(a, b)`, `pow(x, y)` and `a.mod(b)`.
    fn combine(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let values = self.plain_args(name, args, 2)?;
        let ((a, _), (b, b_span)) = (values[0], values[1]);
        match name.name.as_str() {
            "pow" => {
                if a.unit != Unit::Num || b.unit != Unit::Num {
                    return Err(Diagnostic::new(
                        "`pow` expects plain numbers",
                        b_span,
                    ));
                }
                Ok(self.bin(Op::Pow, a, b, Unit::Num))
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

    /// `x.clamp(lo..hi)`
    fn clamp(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let [x, range] = args else {
            return Err(Diagnostic::new("`clamp` takes a range", name.span)
                .with_help("for example `x.clamp(0..1)`"));
        };
        let v = self.num(&x.value)?;
        let ExprKind::Range { lo, hi, .. } = &range.value.kind else {
            return Err(Diagnostic::new("`clamp` takes a range, such as `20hz..18khz`", range.value.span));
        };
        let (l, h) = (self.num(lo)?, self.num(hi)?);
        if l.unit != v.unit || h.unit != v.unit {
            return Err(Diagnostic::new(
                format!("the range's unit must match the value's: {}", v.unit.describe()),
                range.value.span,
            ));
        }
        let up = self.bin(Op::Max, v, l, v.unit);
        Ok(self.bin(Op::Min, up, h, v.unit))
    }

    /// `list.len`, `list.sum`, `list.map(f)`, `list.indices` and `list.at(pos)`.
    fn list_call(&mut self, name: &Ident, args: &[Arg], span: Span) -> Res<KVal> {
        let receiver = self.receiver(name, args)?;
        let l = self.value(&receiver.value)?;
        let len = |l: &KVal| match l {
            KVal::List(items) => Some(items.len()),
            KVal::Table { values, .. } => Some(values.len()),
            KVal::State { len, .. } => Some(*len),
            _ => None,
        };
        let Some(count) = len(&l) else {
            return Err(Diagnostic::new(
                format!("`{}` works on a list", name.name),
                receiver.value.span,
            ));
        };
        match name.name.as_str() {
            "len" => Ok(KVal::Num(self.constant(Unit::Num, count as f64))),
            "indices" => Ok(KVal::Range {
                unit: Unit::Num,
                lo: 0.0,
                hi: count as f64,
                inclusive: false,
            }),
            "sum" => {
                let entries = match l {
                    KVal::List(items) => items,
                    KVal::Table { unit, values, .. } => values
                        .iter()
                        .map(|&k| KVal::Num(self.constant(unit, k)))
                        .collect(),
                    _ => {
                        return Err(Diagnostic::new(
                            "`sum` works on a list known while compiling",
                            receiver.value.span,
                        ));
                    }
                };
                let mut total: Option<Value> = None;
                for e in entries {
                    let KVal::Num(v) = e else {
                        return Err(Diagnostic::new("`sum` adds numbers or signals", span));
                    };
                    total = Some(match total {
                        None => v,
                        Some(t) => self.add(t, v, span)?,
                    });
                }
                Ok(KVal::Num(total.expect("a list is never empty")))
            }
            "at" => {
                let (list, unit) = match l {
                    KVal::Table { id, unit, .. } => (List::Table(id), unit),
                    KVal::State { id, unit, .. } => (List::State(id), unit),
                    _ => {
                        return Err(Diagnostic::new(
                            "`at` reads a table or a state list",
                            receiver.value.span,
                        ));
                    }
                };
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
                Ok(KVal::Num(self.read_list(list, unit, pos, Some(interp))))
            }
            _ => {
                // map
                let [_, f] = args else {
                    return Err(Diagnostic::new("`map` takes a function", name.span));
                };
                let (param, body) = lambda_of(&f.value)?;
                let entries = match l {
                    KVal::List(items) => items,
                    KVal::Table { unit, values, .. } => values
                        .iter()
                        .map(|&k| KVal::Num(self.constant(unit, k)))
                        .collect(),
                    _ => {
                        return Err(Diagnostic::new(
                            "`map` works on a list known while compiling",
                            receiver.value.span,
                        ));
                    }
                };
                let mut out = Vec::with_capacity(entries.len());
                for entry in entries {
                    self.frame.scope.insert(param.clone(), entry);
                    let v = match &body {
                        Body::Block(stmts) => self.block(stmts, true, f.value.span).map(|v| v.expect("a value")),
                        Body::Expr(e) => self.value(e),
                    };
                    self.frame.scope.remove(&param);
                    out.push(v?);
                }
                Ok(KVal::List(out))
            }
        }
    }

    /// A call of a built-in opcode such as `lpf` or `osc`: it is run a sample at a time, with
    /// state of its own for each call.
    fn native(&mut self, name: &Ident, spec: &opcodes::OpSpec, args: &[Arg]) -> Res<Value> {
        if self.force.is_some() {
            return Err(Diagnostic::new(
                format!("`{}` changes every sample, so it can't be used here", name.name),
                name.span,
            ));
        }
        let names: Vec<&str> = spec.params.iter().map(|p| p.name.as_str()).collect();
        let bound = funcs::bind(&name.name, name.span, &names, spec.positional, args)?;
        let mut build = BuildArgs::default();
        let mut inputs: Vec<(usize, NativeInput)> = Vec::new();
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
            match param.kind {
                opcodes::Kind::Wave => {
                    let found = match &arg.value.kind {
                        ExprKind::Name(n) => opcodes::wave(n),
                        _ => None,
                    };
                    let Some(w) = found else {
                        let waves: Vec<_> = opcodes::WAVES.iter().map(|(n, _)| *n).collect();
                        return Err(Diagnostic::new("expected a waveform", arg.value.span)
                            .with_help(format!("one of {}", waves.join(", "))));
                    };
                    build.wave = Some(w);
                }
                opcodes::Kind::Layout => {
                    return Err(Diagnostic::new(
                        format!("`{}` can't be used inside a function with state", name.name),
                        name.span,
                    ));
                }
                opcodes::Kind::Signal(_) | opcodes::Kind::Gain => {
                    let wanted = match param.kind {
                        opcodes::Kind::Signal(u) => u,
                        _ => Unit::Db,
                    };
                    let v = self.num(&arg.value)?;
                    let fits = v.unit == wanted || (wanted == Unit::Db && v.unit == Unit::Num);
                    if !fits {
                        return Err(Diagnostic::new(
                            format!(
                                "`{}` expects {} for `{}`, found {}",
                                name.name,
                                wanted.describe(),
                                param.name,
                                v.unit.describe()
                            ),
                            arg.value.span,
                        ));
                    }
                    if let Some(c) = self.konst(&v) {
                        build.consts.insert(param.name.clone(), c);
                    }
                    inputs.push((index, NativeInput::Reg(v.reg)));
                }
            }
        }
        let op = spec
            .instantiate(&build)
            .map_err(|e| Diagnostic::new(e, name.span))?
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("`{}` can't be used inside a function with state", name.name),
                    name.span,
                )
            })?;
        // one input for each of the opcode's ports, from an argument or the port's default
        let mut wired = Vec::with_capacity(op.ports().len());
        for port in op.ports() {
            let given = spec
                .params
                .iter()
                .position(|p| p.name == port.name)
                .and_then(|i| inputs.iter().find(|(k, _)| *k == i));
            wired.push(match (given, port.default) {
                (Some((_, input)), _) => *input,
                (None, Some(default)) => NativeInput::Const(f64::from(default)),
                (None, None) => {
                    return Err(Diagnostic::new(
                        format!("`{}` needs an argument `{}`", name.name, port.name),
                        name.span,
                    ));
                }
            });
        }
        let index = self.natives.len();
        self.natives.push(Native { op, inputs: wired });
        let reg = self.emit(Rate::Audio, |dst| Instr::Native { dst, index });
        Ok(Value {
            reg,
            unit: Unit::Num,
            rate: Rate::Audio,
            port: None,
        })
    }
}

/// Whether two values are the same, for names a branch didn't change.
fn same(a: &KVal, b: &KVal) -> bool {
    match (a, b) {
        (KVal::Num(x), KVal::Num(y)) => x.reg == y.reg,
        (KVal::Table { id: a, .. }, KVal::Table { id: b, .. })
        | (KVal::State { id: a, .. }, KVal::State { id: b, .. }) => a == b,
        (KVal::List(a), KVal::List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y)),
        (KVal::Range { lo: a, hi: b, .. }, KVal::Range { lo: c, hi: d, .. }) => a == c && b == d,
        (KVal::Record(a), KVal::Record(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.1.reg == y.1.reg),
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
            ExprKind::If { otherwise: None, .. } => false,
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
        return Err(Diagnostic::new("this range is empty", span));
    }
    let count = (end - lo) as usize + 1;
    if count > 1 << 20 {
        return Err(Diagnostic::new("this range is too long", span));
    }
    Ok((0..count).map(|i| lo + i as f64).collect())
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
        compile_kernel(def, args, &Registry::builtin(), &names, Tempo { bpm: 120.0, beats_per_bar: 4.0 })
            .unwrap()
            .0
    }

    const ONE_POLE: &str = "fn one_pole(x, cutoff: hz) {
  state y = 0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}";

    fn has_exp(code: &[Instr]) -> bool {
        code.iter().any(|i| matches!(i, Instr::Call { f: Fn1::Exp, .. }))
    }

    #[test]
    fn work_runs_at_the_rate_of_what_it_depends_on() {
        let x = KArg::Port { unit: Unit::Num, rate: Rate::Audio };
        // a cutoff known while compiling: the coefficient is worked out once, at note start
        let k = kernel(ONE_POLE, &[x.clone(), KArg::Const { unit: Unit::Hz, value: 800.0 }]);
        assert!(has_exp(&k.init) && !has_exp(&k.block) && !has_exp(&k.code));
        // a cutoff that changes once per block: once per block
        let k = kernel(ONE_POLE, &[x.clone(), KArg::Port { unit: Unit::Hz, rate: Rate::Control }]);
        assert!(has_exp(&k.block) && !has_exp(&k.code));
        // one that changes every sample: every sample
        let k = kernel(ONE_POLE, &[x, KArg::Port { unit: Unit::Hz, rate: Rate::Audio }]);
        assert!(has_exp(&k.code));
    }

    #[test]
    fn several_results_come_from_one_kernel() {
        let k = kernel(
            "fn split(x, c: hz) -> (low, high) {\n state y = 0\n y = y + (x - y) * 0.1\n low = y\n high = x - y\n}",
            &[KArg::Port { unit: Unit::Num, rate: Rate::Audio }, KArg::Const { unit: Unit::Hz, value: 1.0 }],
        );
        assert_eq!(k.outputs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["low", "high"]);
        assert_eq!(k.state_init.len(), 1, "the body ran once for both results");
    }

    #[test]
    fn a_state_starts_from_a_note_start_value() {
        let k = kernel(
            "fn glide(x: hz) {\n state y = x\n y = y + (x - y) * 0.01\n y\n}",
            &[KArg::Port { unit: Unit::Hz, rate: Rate::Audio }],
        );
        assert!(k.init.iter().any(|i| matches!(i, Instr::StoreState { .. })), "the start is stored at note start");
        assert!(k.init.iter().any(|i| matches!(i, Instr::Input { .. })), "from the input's first sample");
    }
}
