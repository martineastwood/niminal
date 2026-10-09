//! Compiles an `opcode` body to an engine [`Kernel`]: straight-line register
//! code run once per sample. Types are checked with the same unit rules as
//! instruments, but values are computed at run time, so (unlike in an
//! instrument) division by a changing value is fine.

use std::collections::HashMap;
use std::sync::Arc;

use niminal_engine::Port;
use niminal_engine::ops::{BinOp as Op, Instr, Kernel};
use niminal_score::Tempo;

use crate::ast::*;
use crate::lower::{Lower, Names, literal_sig, param_type, unknown_name};
use crate::diag::{Diagnostic, Span, closest};
use crate::opcodes::{Build, Kind, OpSpec, ParamSpec, Registry, math_fn};
use crate::unit::Unit;

type Res<T> = Result<T, Diagnostic>;

#[derive(Clone, Copy)]
struct Value {
    reg: usize,
    unit: Unit,
}

struct Kb {
    code: Vec<Instr>,
    registers: usize,
    scope: HashMap<String, Value>,
    /// State variable name to (slot, unit).
    states: HashMap<String, (usize, Unit)>,
    state_init: Vec<f64>,
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
        params.push(ParamSpec { name: p.name.name.clone(), kind, required: default.is_none() });
        ports.push(Port::named(&p.name.name, default));
        units.push(unit);
    }

    // Body.
    let build = match compile_body(def, &ports, &units, tempo) {
        Ok(kernel) => (Build::Custom(Arc::new(Kernel { name: name.clone(), ports, ..kernel })), None),
        Err(d) => (Build::Invalid, Some(d)),
    };
    let spec = OpSpec { name, params, positional: 1, build: build.0 };
    Ok((spec, build.1))
}

fn compile_body(def: &OpcodeDef, ports: &[Port], units: &[Unit], tempo: Tempo) -> Res<Kernel> {
    let mut kb = Kb {
        code: Vec::new(),
        registers: 0,
        scope: HashMap::new(),
        states: HashMap::new(),
        state_init: Vec::new(),
        tempo,
    };

    for (i, (port, unit)) in ports.iter().zip(units).enumerate() {
        let reg = kb.emit(|dst| Instr::Input { dst, port: i });
        kb.scope.insert(port.name.to_string(), Value { reg, unit: *unit });
    }

    let Some((last, init)) = def.body.split_last() else {
        return Err(Diagnostic::new(format!("opcode `{}` is empty", def.name.name), def.name.span)
            .with_help("the last line of an opcode is the value it outputs"));
    };
    for stmt in init {
        match stmt {
            Stmt::Bind { name, value } => kb.bind(name, value)?,
            Stmt::State { name, init } => kb.declare_state(name, init)?,
            Stmt::AddAssign { name, .. } => return Err(add_assign_in_opcode(name)),
            Stmt::Expr(e) => {
                return Err(Diagnostic::new("this value is never used", e.span)
                    .with_help("bind it with `name = ...`, or make it the last line"));
            }
        }
    }
    let Stmt::Expr(out_expr) = last else {
        let name = match last {
            Stmt::Bind { name, .. } | Stmt::State { name, .. } | Stmt::AddAssign { name, .. } => name,
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

    Ok(Kernel {
        name: String::new(),
        ports: Vec::new(),
        code: kb.code,
        registers: kb.registers,
        state_init: kb.state_init,
        output: out.reg,
    })
}

fn add_assign_in_opcode(name: &Ident) -> Diagnostic {
    Diagnostic::new("`+=` isn't available inside an opcode", name.span)
        .with_help(format!("write `{0} = {0} + ...`", name.name))
}

impl Kb {
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
        let v = self.expr(value)?;
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
                Ok(Value { reg: self.emit(|dst| Instr::Const { dst, value }), unit })
            }
            ExprKind::Name(name) => self.name(name, e.span),
            ExprKind::Neg(inner) => {
                let v = self.expr(inner)?;
                if v.unit == Unit::Db {
                    return Err(Diagnostic::new("negating a db value isn't supported inside opcodes yet", e.span));
                }
                Ok(Value { reg: self.emit(|dst| Instr::Neg { dst, a: v.reg }), unit: v.unit })
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                self.binary(*op, l, r, e.span)
            }
            ExprKind::Call { name, args } => self.call(name, args),
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
        };
        let op = match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
        };
        Ok(Value { reg: self.emit(|dst| Instr::Bin { dst, op, a: l.reg, b: r.reg }), unit })
    }

    fn call(&mut self, name: &Ident, args: &[Arg]) -> Res<Value> {
        let Some(f) = math_fn(&name.name) else {
            let math = ["sin", "cos", "tanh", "exp", "abs"];
            let help = format!("opcodes are built from arithmetic and these functions: {}", math.join(", "));
            let mut d = Diagnostic::new(format!("`{}` can't be used inside an opcode", name.name), name.span);
            if let Some(c) = closest(&name.name, math) {
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
