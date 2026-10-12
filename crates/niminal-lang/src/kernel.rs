//! Compiles an `opcode` body to an engine [`Kernel`]: straight-line register
//! code run once per sample. Types are checked with the same unit rules as
//! instruments, but values are computed at run time, so (unlike in an
//! instrument) division by a changing value is fine.

use std::collections::HashMap;
use std::sync::Arc;

use niminal_engine::Port;
use niminal_engine::ops::{BinOp as Op, Cmp, Fn1, Instr, Kernel, Native, NativeInput};
use niminal_score::Tempo;

use crate::ast::*;
use crate::lower::{Lower, Names, literal_sig, param_type, unknown_name};
use crate::diag::{Diagnostic, Span, closest};
use crate::opcodes::{Build, BuildArgs, Kind, OpSpec, ParamSpec, Registry, math_fn};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

#[derive(Clone, Copy)]
struct Value {
    reg: usize,
    unit: Unit,
}

struct Kb<'a> {
    registry: &'a Registry,
    code: Vec<Instr>,
    registers: usize,
    scope: HashMap<String, Value>,
    /// State variable name to (slot, unit).
    states: HashMap<String, (usize, Unit)>,
    state_init: Vec<f64>,
    /// `prev(y)` written before `y` is: the state it reads, and where, so `y = ...` can store into it.
    forward: HashMap<String, (usize, Span)>,
    /// `prev(expr)`: the state to store the value in at the end of the sample.
    deferred: Vec<(usize, usize)>,
    /// Built-in opcodes the body calls, run a sample at a time.
    natives: Vec<Native>,
    /// Registers that hold a constant, and what it is (a built-in may need one, like a delay's `max`).
    constants: HashMap<usize, f64>,
    /// Names bound to a call of an opcode with several results (`f = svf(x)`), and those results.
    records: HashMap<String, Vec<(String, Value)>>,
    tempo: Tempo,
}

/// Compile an opcode definition. `Err` means its signature is unusable. `Ok`
/// carries the opcode and, if its body had a mistake, the diagnostic; the
/// opcode is then a stub that callers are still checked against.
pub fn compile_opcode(def: &OpcodeDef, registry: &Registry, names: &Names, tempo: Tempo) -> Res<(OpSpec, Option<Diagnostic>)> {
    let name = def.name.name.clone();
    if registry.find(&name).is_some() {
        return Err(Diagnostic::new(format!("`{name}` is already an opcode"), def.name.span));
    }

    // Signature.
    let mut params = Vec::new();
    let mut ports = Vec::new();
    let mut units = Vec::new();
    let mut defaults = Lower::new(registry, names, tempo);
    for p in &def.params {
        if params.iter().any(|q: &ParamSpec| q.name == p.name.name) {
            return Err(Diagnostic::new(format!("parameter `{}` is declared twice", p.name.name), p.name.span));
        }
        let (unit, range) = match &p.ty {
            Some(ty) => param_type(ty)?,
            None => (Unit::Num, None),
        };
        let default = match &p.default {
            Some(e) => Some(defaults.param_default(e, unit, range, &p.name.name)?),
            None => None,
        };
        let kind = if unit == Unit::Db { Kind::Gain } else { Kind::Signal(unit) };
        params.push(ParamSpec { name: p.name.name.clone(), kind, required: default.is_none(), wired: true });
        ports.push(Port::named(&p.name.name, default));
        units.push(unit);
    }

    // Body.
    let build = match compile_body(def, &ports, &units, tempo, registry) {
        Ok(kernel) => (Build::Custom(Arc::new(Kernel { name: name.clone(), ports, ..kernel })), None),
        Err(d) => (Build::Invalid, Some(d)),
    };
    let spec = OpSpec { name, doc: String::new(), params, positional: 1, build: build.0 };
    Ok((spec, build.1))
}

fn compile_body(def: &OpcodeDef, ports: &[Port], units: &[Unit], tempo: Tempo, registry: &Registry) -> Res<Kernel> {
    let mut kb = Kb {
        registry,
        code: Vec::new(),
        registers: 0,
        scope: HashMap::new(),
        states: HashMap::new(),
        state_init: Vec::new(),
        forward: HashMap::new(),
        deferred: Vec::new(),
        records: HashMap::new(),
        natives: Vec::new(),
        constants: HashMap::new(),
        tempo,
    };

    for (i, (port, unit)) in ports.iter().zip(units).enumerate() {
        let reg = kb.emit(|dst| Instr::Input { dst, port: i });
        kb.scope.insert(port.name.to_string(), Value { reg, unit: *unit });
    }

    let multi = !def.outputs.is_empty();
    let (init, last) = if multi { (&def.body[..], None) } else { def.body.split_last().map(|(l, i)| (i, Some(l))).unwrap_or((&[][..], None)) };
    if def.body.is_empty() || (!multi && last.is_none()) {
        return Err(Diagnostic::new(format!("opcode `{}` is empty", def.name.name), def.name.span)
            .with_help("the last line of an opcode is the value it outputs"));
    }
    for stmt in init {
        match stmt {
            Stmt::Bind { name, value } => kb.bind(name, value)?,
            Stmt::State { name, init } => kb.declare_state(name, init)?,
            Stmt::AddAssign { name, .. } => return Err(add_assign_in_opcode(name)),
            Stmt::Destructure { value, .. } => return Err(channels_in_opcode(value.span)),
            Stmt::Expr(e) => {
                return Err(Diagnostic::new("this value is never used", e.span).with_help(if multi {
                    "bind it with `name = ...`; the opcode's results are the names after `->`"
                } else {
                    "bind it with `name = ...`, or make it the last line"
                }));
            }
        }
    }
    let mut outputs: Vec<(String, usize)> = Vec::new();
    if multi {
        for out in &def.outputs {
            let Some(v) = kb.scope.get(&out.name).copied() else {
                return Err(Diagnostic::new(format!("`{}` is a result of `{}`, but the body never defines it", out.name, def.name.name), out.span)
                    .with_help(format!("add a line `{} = ...`", out.name)));
            };
            if v.unit != Unit::Num {
                return Err(Diagnostic::new(format!("an opcode outputs plain signals, but `{}` is {}", out.name, v.unit.describe()), out.span));
            }
            outputs.push((out.name.clone(), v.reg));
        }
    }
    if let Some(last) = last {
        let Stmt::Expr(out_expr) = last else {
            let name = match last {
                Stmt::Bind { name, .. } | Stmt::State { name, .. } | Stmt::AddAssign { name, .. } => name,
                Stmt::Destructure { value, .. } => return Err(channels_in_opcode(value.span)),
                Stmt::Expr(_) => unreachable!(),
            };
            return Err(Diagnostic::new("an opcode must end with the value it outputs", name.span)
                .with_help(format!("add a last line such as `{}`", name.name)));
        };
        let out = kb.expr(out_expr)?;
        if out.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("an opcode outputs a plain signal, but this is {}", out.unit.describe()),
                out_expr.span,
            ));
        }
        outputs.push(("out".to_string(), out.reg));
    }
    if let Some((name, (_, span))) = kb.forward.iter().next() {
        return Err(Diagnostic::new(format!("`prev({name})` reads `{name}`, which is never defined"), *span));
    }
    for (slot, src) in std::mem::take(&mut kb.deferred) {
        kb.code.push(Instr::StoreState { slot, src });
    }

    Ok(Kernel {
        name: String::new(),
        ports: Vec::new(),
        code: kb.code,
        registers: kb.registers,
        state_init: kb.state_init,
        outputs,
        natives: kb.natives,
    })
}

/// Which argument goes with each parameter of `spec`, by position and by name.
fn bind_args<'a>(name: &Ident, spec: &OpSpec, args: &'a [Arg]) -> Res<Vec<Option<&'a Arg>>> {
    let mut bound: Vec<Option<&Arg>> = vec![None; spec.params.len()];
    let mut next_positional = 0;
    for arg in args {
        let index = match &arg.name {
            None => {
                let i = next_positional;
                next_positional += 1;
                if i >= spec.positional || i >= spec.params.len() {
                    let mut d = Diagnostic::new(format!("`{}` takes its other arguments by name", name.name), arg.value.span);
                    if let Some(p) = spec.params.get(i) {
                        d = d.with_help(format!("write `{}: ...`", p.name));
                    }
                    return Err(d);
                }
                i
            }
            Some(n) => spec.params.iter().position(|p| p.name == n.name).ok_or_else(|| {
                let mut d = Diagnostic::new(format!("`{}` has no argument named `{}`", name.name, n.name), n.span);
                if let Some(c) = closest(&n.name, spec.params.iter().map(|p| p.name.as_str())) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            })?,
        };
        if bound[index].is_some() {
            return Err(Diagnostic::new(
                format!("`{}` argument `{}` given more than once", name.name, spec.params[index].name),
                arg.value.span,
            ));
        }
        bound[index] = Some(arg);
    }
    Ok(bound)
}

fn channels_in_opcode(span: Span) -> Diagnostic {
    Diagnostic::new("channel lists aren't available inside an opcode", span)
        .with_help("an opcode works on one channel; applied to a multichannel signal, it runs once per channel")
}

fn add_assign_in_opcode(name: &Ident) -> Diagnostic {
    Diagnostic::new("`+=` isn't available inside an opcode", name.span)
        .with_help(format!("write `{0} = {0} + ...`", name.name))
}

impl Kb<'_> {
    fn emit(&mut self, make: impl FnOnce(usize) -> Instr) -> usize {
        let dst = self.registers;
        self.registers += 1;
        self.code.push(make(dst));
        dst
    }

    fn declare_state(&mut self, name: &Ident, init: &Expr) -> Res<()> {
        if self.scope.contains_key(&name.name) {
            return Err(Diagnostic::new(format!("`{}` is already defined", name.name), name.span));
        }
        let (unit, value) = self.constant(init)?;
        let slot = self.state_init.len();
        self.state_init.push(value);
        self.states.insert(name.name.clone(), (slot, unit));
        let reg = self.emit(|dst| Instr::LoadState { dst, slot });
        self.scope.insert(name.name.clone(), Value { reg, unit });
        Ok(())
    }

    /// A number with an optional unit (and sign), for a state's starting value.
    fn constant(&self, e: &Expr) -> Res<(Unit, f64)> {
        match &e.kind {
            ExprKind::Num { value, unit } => literal_sig(*value, unit.as_deref(), self.tempo, e.span),
            ExprKind::Neg(inner) => self.constant(inner).map(|(u, v)| (u, -v)),
            _ => Err(Diagnostic::new("the starting value of a state must be a number", e.span)
                .with_help("for example `state y = 0.0`")),
        }
    }

    fn bind(&mut self, name: &Ident, value: &Expr) -> Res<()> {
        // `f = svf(x)` of an opcode with several results keeps them all, to be read as `f.band`
        if let ExprKind::Call { name: callee, args } = &value.kind {
            let registry = self.registry;
            if let Some(spec) = registry.find(&callee.name)
                && let Build::Custom(kernel) = &spec.build
                && kernel.outputs.len() > 1
            {
                let results = self.inline_all(callee, spec, kernel, args)?;
                self.records.insert(name.name.clone(), results);
                return Ok(());
            }
        }
        let v = self.expr(value)?;
        if let Some((slot, _)) = self.forward.remove(&name.name) {
            if v.unit != Unit::Num {
                return Err(Diagnostic::new(
                    format!("`prev({0})` is a plain number, but `{0}` is {1}", name.name, v.unit.describe()),
                    value.span,
                ));
            }
            self.code.push(Instr::StoreState { slot, src: v.reg });
        }
        if let Some(&(slot, unit)) = self.states.get(&name.name) {
            if v.unit != unit {
                return Err(Diagnostic::new(
                    format!("`{}` holds {}, but this is {}", name.name, unit.describe(), v.unit.describe()),
                    value.span,
                ));
            }
            self.code.push(Instr::StoreState { slot, src: v.reg });
        }
        self.scope.insert(name.name.clone(), v);
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> Res<Value> {
        match &e.kind {
            ExprKind::Num { value, unit } => {
                let (unit, value) = literal_sig(*value, unit.as_deref(), self.tempo, e.span)?;
                let reg = self.emit(|dst| Instr::Const { dst, value });
                self.constants.insert(reg, value);
                Ok(Value { reg, unit })
            }
            ExprKind::Name(name) => self.name(name, e.span),
            ExprKind::Neg(inner) => {
                let v = self.expr(inner)?;
                if v.unit == Unit::Db {
                    return Err(Diagnostic::new("negating a db value isn't supported inside opcodes yet", e.span));
                }
                let reg = self.emit(|dst| Instr::Neg { dst, a: v.reg });
                if let Some(c) = self.constants.get(&v.reg).copied() {
                    self.constants.insert(reg, -c);
                }
                Ok(Value { reg, unit: v.unit })
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                self.binary(*op, l, r, e.span)
            }
            ExprKind::Call { name, args } => self.call(name, args),
            ExprKind::Compare { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                if l.unit != r.unit {
                    return Err(Diagnostic::new(
                        format!("can't compare {} and {}", l.unit.describe(), r.unit.describe()),
                        e.span,
                    ));
                }
                let op = match op {
                    CmpOp::Lt => Cmp::Lt,
                    CmpOp::Gt => Cmp::Gt,
                    CmpOp::Le => Cmp::Le,
                    CmpOp::Ge => Cmp::Ge,
                    CmpOp::Eq => Cmp::Eq,
                    CmpOp::Ne => Cmp::Ne,
                };
                Ok(Value { reg: self.emit(|dst| Instr::Cmp { dst, op, a: l.reg, b: r.reg }), unit: Unit::Num })
            }
            ExprKind::If { cond, then, otherwise } => {
                let c = self.expr(cond)?;
                if c.unit != Unit::Num {
                    return Err(Diagnostic::new(
                        format!("an `if` needs a plain number to decide on, but this is {}", c.unit.describe()),
                        cond.span,
                    )
                    .with_help("a comparison gives 1 or 0: `if x > 0.5hz`"));
                }
                // both ways are worked out every sample and one is kept
                let (a, b) = (self.expr(then)?, self.expr(otherwise)?);
                if a.unit != b.unit {
                    return Err(Diagnostic::new(
                        format!("the two ways of an `if` must have the same unit: this is {}, the first was {}", b.unit.describe(), a.unit.describe()),
                        otherwise.span,
                    ));
                }
                Ok(Value { reg: self.emit(|dst| Instr::Select { dst, cond: c.reg, a: a.reg, b: b.reg }), unit: a.unit })
            }
            ExprKind::Channels(_) => Err(channels_in_opcode(e.span)),
            ExprKind::Pattern(_) | ExprKind::Grid(_) | ExprKind::Rows(_) | ExprKind::Rest => {
                Err(Diagnostic::new("patterns can't be used inside an opcode", e.span))
            }
            ExprKind::Str(_) => Err(Diagnostic::new("text can't be used inside an opcode", e.span)),
            ExprKind::Env(_) => Err(Diagnostic::new("envelopes can't be used inside an opcode", e.span)
                .with_help("compute the envelope in the instrument and pass it in as an argument")),
        }
    }

    fn name(&mut self, name: &str, span: Span) -> Res<Value> {
        if let Some(v) = self.scope.get(name) {
            return Ok(*v);
        }
        match name {
            "pi" => {
                let value = std::f64::consts::PI;
                return Ok(Value { reg: self.emit(|dst| Instr::Const { dst, value }), unit: Unit::Num });
            }
            "sample_rate" => {
                return Ok(Value { reg: self.emit(|dst| Instr::SampleRate { dst }), unit: Unit::Hz });
            }
            _ => {}
        }
        if let Ok(note @ niminal_score::Value::Note(_)) = name.parse::<niminal_score::Value>() {
            let value = note.as_hz().expect("a note is a frequency");
            return Ok(Value { reg: self.emit(|dst| Instr::Const { dst, value }), unit: Unit::Hz });
        }
        let known = self.scope.keys().map(String::as_str).chain(["pi", "sample_rate"]);
        Err(unknown_name(name, span, known))
    }

    fn binary(&mut self, op: BinOp, l: Value, r: Value, span: Span) -> Res<Value> {
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            BinOp::Mul => "multiply",
            BinOp::Div => "divide",
            BinOp::Rem => "take the remainder of",
        };
        let mismatch = || Diagnostic::new(format!("can't {verb} {} and {}", l.unit.describe(), r.unit.describe()), span);

        let unit = match op {
            BinOp::Add | BinOp::Sub => match (l.unit == r.unit, l.unit) {
                (false, _) => return Err(mismatch()),
                (true, Unit::Db) => {
                    return Err(Diagnostic::new("adding db values isn't supported inside opcodes yet", span));
                }
                (true, u) => u,
            },
            BinOp::Mul => match (l.unit, r.unit) {
                // Applying a gain to a plain value.
                (Unit::Num, Unit::Db) | (Unit::Db, Unit::Num) => Unit::Num,
                (Unit::Db, _) | (_, Unit::Db) => return Err(mismatch()),
                (Unit::Num, u) | (u, Unit::Num) => u,
                _ => return Err(mismatch()),
            },
            BinOp::Div => match (l.unit, r.unit) {
                (Unit::Db, _) | (_, Unit::Db) => return Err(mismatch()),
                (u, Unit::Num) => u,
                (a, b) if a == b => Unit::Num,
                _ => return Err(mismatch()),
            },
            BinOp::Rem => match (l.unit, r.unit) {
                (Unit::Db, _) | (_, Unit::Db) => return Err(mismatch()),
                (u, Unit::Num) => u,
                (a, b) if a == b => a,
                _ => return Err(mismatch()),
            },
        };
        let op = match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Rem => Op::Rem,
        };
        Ok(Value { reg: self.emit(|dst| Instr::Bin { dst, op, a: l.reg, b: r.reg }), unit })
    }

    /// `prev(x)`: what `x` was one sample ago. `y = x + prev(y) * 0.5` is a feedback loop.
    fn prev(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let [arg] = args else {
            return Err(Diagnostic::new("`prev` takes one value", name.span));
        };
        if let Some(n) = &arg.name {
            return Err(Diagnostic::new("`prev` takes its value without a name", n.span));
        }
        // a second `prev(y)` of a name still to come reads the same state
        if let ExprKind::Name(n) = &arg.value.kind
            && let Some(&(slot, _)) = self.forward.get(n)
        {
            return Ok(Value { reg: self.emit(|dst| Instr::LoadState { dst, slot }), unit: Unit::Num });
        }
        let slot = self.state_init.len();
        self.state_init.push(0.0);
        let reg = self.emit(|dst| Instr::LoadState { dst, slot });
        if let ExprKind::Name(n) = &arg.value.kind
            && !self.scope.contains_key(n)
            && n != "pi"
            && n != "sample_rate"
        {
            // a name defined further down: this is feedback
            self.forward.insert(n.clone(), (slot, arg.value.span));
            return Ok(Value { reg, unit: Unit::Num });
        }
        let v = self.expr(&arg.value)?;
        self.deferred.push((slot, v.reg));
        Ok(Value { reg, unit: v.unit })
    }

    /// A call to an opcode defined earlier: its instructions are copied in here, their registers and
    /// state moved out of the way, with the call's arguments standing for its inputs.
    fn inline(&mut self, name: &Ident, spec: &OpSpec, kernel: &Kernel, args: &[Arg]) -> Res<Value> {
        // used on its own, the first result stands for the call, as in an instrument
        Ok(self.inline_all(name, spec, kernel, args)?.remove(0).1)
    }

    /// Like [`Kb::inline`], giving every result of the callee, named.
    fn inline_all(&mut self, name: &Ident, spec: &OpSpec, kernel: &Kernel, args: &[Arg]) -> Res<Vec<(String, Value)>> {
        // match arguments to parameters, as a call in an instrument does
        let bound = bind_args(name, spec, args)?;
        let mut given: Vec<Option<(Value, Span)>> = Vec::with_capacity(spec.params.len());
        for arg in bound {
            given.push(match arg {
                Some(arg) => Some((self.expr(&arg.value)?, arg.value.span)),
                None => None,
            });
        }
        let mut inputs = Vec::with_capacity(spec.params.len());
        for (i, param) in spec.params.iter().enumerate() {
            let wanted = if matches!(param.kind, Kind::Gain) { Unit::Db } else if let Kind::Signal(u) = param.kind { u } else { Unit::Num };
            match given[i] {
                Some((v, span)) => {
                    if v.unit != wanted {
                        return Err(Diagnostic::new(
                            format!("`{}` expects {} for `{}`, found {}", name.name, wanted.describe(), param.name, v.unit.describe()),
                            span,
                        ));
                    }
                    inputs.push(v.reg);
                }
                None => match kernel.ports.get(i).and_then(|p| p.default) {
                    Some(value) => inputs.push(self.emit(|dst| Instr::Const { dst, value: f64::from(value) })),
                    None => {
                        return Err(Diagnostic::new(format!("`{}` needs an argument `{}`", name.name, param.name), name.span));
                    }
                },
            }
        }

        let (base, state_base, native_base) = (self.registers, self.state_init.len(), self.natives.len());
        self.registers += kernel.registers;
        for native in &kernel.natives {
            let inputs = native
                .inputs
                .iter()
                .map(|i| match *i {
                    NativeInput::Reg(r) => NativeInput::Reg(r + base),
                    NativeInput::Const(c) => NativeInput::Const(c),
                })
                .collect();
            self.natives.push(Native { op: native.op.box_clone(), inputs });
        }
        self.state_init.extend(&kernel.state_init);
        for instr in &kernel.code {
            let r = |reg: usize| reg + base;
            let moved = match *instr {
                Instr::Const { dst, value } => Instr::Const { dst: r(dst), value },
                Instr::Input { dst, port } => Instr::Copy { dst: r(dst), src: inputs[port] },
                Instr::SampleRate { dst } => Instr::SampleRate { dst: r(dst) },
                Instr::LoadState { dst, slot } => Instr::LoadState { dst: r(dst), slot: slot + state_base },
                Instr::StoreState { slot, src } => Instr::StoreState { slot: slot + state_base, src: r(src) },
                Instr::Bin { dst, op, a, b } => Instr::Bin { dst: r(dst), op, a: r(a), b: r(b) },
                Instr::Neg { dst, a } => Instr::Neg { dst: r(dst), a: r(a) },
                Instr::Copy { dst, src } => Instr::Copy { dst: r(dst), src: r(src) },
                Instr::Cmp { dst, op, a, b } => Instr::Cmp { dst: r(dst), op, a: r(a), b: r(b) },
                Instr::Select { dst, cond, a, b } => Instr::Select { dst: r(dst), cond: r(cond), a: r(a), b: r(b) },
                Instr::Call { dst, f, a } => Instr::Call { dst: r(dst), f, a: r(a) },
                Instr::Native { dst, index } => Instr::Native { dst: r(dst), index: index + native_base },
            };
            self.code.push(moved);
        }
        Ok(kernel.outputs.iter().map(|(n, reg)| (n.clone(), Value { reg: reg + base, unit: Unit::Num })).collect())
    }

    /// A call of a built-in opcode such as `lpf` or `osc`: it is run a sample at a time, with state of its
    /// own for each call.
    fn native(&mut self, name: &Ident, spec: &OpSpec, args: &[Arg]) -> Res<Value> {
        let bound = bind_args(name, spec, args)?;
        let mut build = BuildArgs::default();
        let mut inputs: Vec<(usize, NativeInput)> = Vec::new();
        for (index, (param, arg)) in spec.params.iter().zip(bound).enumerate() {
            let Some(arg) = arg else {
                if param.required {
                    return Err(Diagnostic::new(format!("`{}` needs an argument `{}`", name.name, param.name), name.span));
                }
                continue;
            };
            match param.kind {
                Kind::Wave => {
                    let found = match &arg.value.kind {
                        ExprKind::Name(n) => crate::opcodes::wave(n),
                        _ => None,
                    };
                    let Some(w) = found else {
                        let waves: Vec<_> = crate::opcodes::WAVES.iter().map(|(n, _)| *n).collect();
                        return Err(Diagnostic::new("expected a waveform", arg.value.span).with_help(format!("one of {}", waves.join(", "))));
                    };
                    build.wave = Some(w);
                }
                Kind::Layout => {
                    return Err(Diagnostic::new(format!("`{}` can't be used inside an opcode", name.name), name.span));
                }
                Kind::Signal(_) | Kind::Gain => {
                    let wanted = if let Kind::Signal(u) = param.kind { u } else { Unit::Db };
                    let v = self.expr(&arg.value)?;
                    let fits = v.unit == wanted || (wanted == Unit::Db && v.unit == Unit::Num);
                    if !fits {
                        return Err(Diagnostic::new(
                            format!("`{}` expects {} for `{}`, found {}", name.name, wanted.describe(), param.name, v.unit.describe()),
                            arg.value.span,
                        ));
                    }
                    if let Some(c) = self.constants.get(&v.reg) {
                        build.consts.insert(param.name.clone(), *c);
                    }
                    inputs.push((index, NativeInput::Reg(v.reg)));
                }
            }
        }
        let op = spec
            .instantiate(&build)
            .map_err(|e| Diagnostic::new(e, name.span))?
            .ok_or_else(|| Diagnostic::new(format!("`{}` can't be used inside an opcode", name.name), name.span))?;
        // one input for each of the opcode's ports, from an argument or the port's default
        let mut wired = Vec::with_capacity(op.ports().len());
        for port in op.ports() {
            let given = spec.params.iter().position(|p| p.name == port.name).and_then(|i| inputs.iter().find(|(k, _)| *k == i));
            wired.push(match (given, port.default) {
                (Some((_, input)), _) => *input,
                (None, Some(default)) => NativeInput::Const(f64::from(default)),
                (None, None) => {
                    return Err(Diagnostic::new(format!("`{}` needs an argument `{}`", name.name, port.name), name.span));
                }
            });
        }
        let index = self.natives.len();
        self.natives.push(Native { op, inputs: wired });
        Ok(Value { reg: self.emit(|dst| Instr::Native { dst, index }), unit: Unit::Num })
    }

    /// `min(a, b)`, `max(a, b)`, `pow(x, y)` and `clamp(x, lo, hi)`.
    fn combine(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let wanted = if name.name == "clamp" { 3 } else { 2 };
        if args.len() != wanted {
            return Err(Diagnostic::new(format!("`{}` takes {wanted} values", name.name), name.span));
        }
        if let Some(n) = args.iter().find_map(|a| a.name.as_ref()) {
            return Err(Diagnostic::new(format!("`{}` takes its values without names", name.name), n.span));
        }
        let mut values = Vec::with_capacity(wanted);
        for a in args {
            values.push((self.expr(&a.value)?, a.value.span));
        }
        let unit = values[0].0.unit;
        for (v, span) in &values[1..] {
            if v.unit != unit {
                return Err(Diagnostic::new(
                    format!("`{}` needs values with the same unit: this is {}, the first was {}", name.name, v.unit.describe(), unit.describe()),
                    *span,
                ));
            }
        }
        if name.name == "pow" && unit != Unit::Num {
            return Err(Diagnostic::new(format!("`pow` expects plain numbers, found {}", unit.describe()), args[0].value.span));
        }
        let reg = |v: &(Value, Span)| v.0.reg;
        let out = match name.name.as_str() {
            "min" => self.emit(|dst| Instr::Bin { dst, op: Op::Min, a: reg(&values[0]), b: reg(&values[1]) }),
            "max" => self.emit(|dst| Instr::Bin { dst, op: Op::Max, a: reg(&values[0]), b: reg(&values[1]) }),
            "pow" => self.emit(|dst| Instr::Bin { dst, op: Op::Pow, a: reg(&values[0]), b: reg(&values[1]) }),
            // the lower bound wins if the two are the wrong way round
            _ => {
                let capped = self.emit(|dst| Instr::Bin { dst, op: Op::Min, a: reg(&values[0]), b: reg(&values[2]) });
                self.emit(|dst| Instr::Bin { dst, op: Op::Max, a: capped, b: reg(&values[1]) })
            }
        };
        Ok(Value { reg: out, unit })
    }

    fn call(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        if name.name == "prev" {
            return self.prev(name, args);
        }
        if matches!(name.name.as_str(), "min" | "max" | "pow" | "clamp") {
            return self.combine(name, args);
        }
        let registry = self.registry;
        if let Some(spec) = registry.find(&name.name)
            && let Build::Custom(kernel) = &spec.build
        {
            return self.inline(name, spec, kernel, args);
        }
        // `f.band`: one result of an earlier call with several
        if let [receiver] = args
            && receiver.receiver
            && let ExprKind::Name(record) = &receiver.value.kind
            && let Some(results) = self.records.get(record)
        {
            return results.iter().find(|(n, _)| *n == name.name).map(|(_, v)| *v).ok_or_else(|| {
                let names: Vec<&str> = results.iter().map(|(n, _)| n.as_str()).collect();
                Diagnostic::new(format!("this opcode has no result `{}`", name.name), name.span)
                    .with_help(format!("its results are {}", names.join(", ")))
            });
        }
        if math_fn(&name.name).is_none()
            && let Some(spec) = registry.find(&name.name)
            && let Build::Builtin(_) = &spec.build
        {
            return self.native(name, spec, args);
        }
        let Some(f) = math_fn(&name.name) else {
            let math: Vec<&str> = Fn1::ALL.iter().map(|f| f.name()).chain(["min", "max", "pow", "clamp", "prev"]).collect();
            let help = format!("opcodes are built from arithmetic, `if`, and these functions: {}", math.join(", "));
            let mut d = Diagnostic::new(format!("`{}` can't be used inside an opcode", name.name), name.span);
            if let Some(c) = closest(&name.name, math.iter().copied()) {
                d = d.with_help(format!("did you mean `{c}`?"));
            } else {
                d = d.with_help(help);
            }
            return Err(d);
        };
        let [arg] = args else {
            return Err(Diagnostic::new(format!("`{}` takes one argument", name.name), name.span));
        };
        if let Some(n) = &arg.name {
            return Err(Diagnostic::new(format!("`{}` takes its argument without a name", name.name), n.span));
        }
        let v = self.expr(&arg.value)?;
        if v.unit != Unit::Num {
            return Err(Diagnostic::new(
                format!("`{}` expects a plain number, found {}", name.name, v.unit.describe()),
                arg.value.span,
            )
            .with_help("divide by a value with the same unit to get a plain ratio"));
        }
        Ok(Value { reg: self.emit(|dst| Instr::Call { dst, f, a: v.reg }), unit: Unit::Num })
    }
}
