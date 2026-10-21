use crate::diag::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Instr(InstrDef),
    /// `ctl name = value`
    Control {
        name: Ident,
        value: Expr,
    },
    Fn(FnDef),
    /// `const name = expr`: a value known while compiling, such as a number, a range or a table.
    Const {
        name: Ident,
        value: Expr,
    },
    Bus(BusDecl),
    /// `buffer take = zeros(48000)` or `buffer take = 4bars`: shared writable table.
    Buffer(BufferDecl),
    Node(NodeDecl),
    Fx(NodeDecl),
    Sample(SampleDecl),
    MidiOut(MidiOutDecl),
    Config(ConfigDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstrDef {
    pub name: Ident,
    pub params: Vec<ParamDef>,
    pub body: Vec<Stmt>,
    pub span: Span,
    /// `mono bass(...)`: one note sounds at a time.
    pub mono: bool,
    /// `with(max: 8, same_note: release)`
    pub options: Vec<Arg>,
}

/// `kick = sample("drums/kick.wav").root(c4)`, or `drums = kit("drums/808")`.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleDecl {
    pub is_kit: bool,
    pub name: Ident,
    pub source: SampleSource,
    pub source_span: Span,
    pub options: Vec<Arg>,
    pub span: Span,
}

/// `midi_out drums(port: "IAC Driver", channel: 10)`: an instrument that plays external gear.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiOutDecl {
    pub name: Ident,
    pub args: Vec<Arg>,
    pub span: Span,
}

/// Where a sample or kit comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum SampleSource {
    /// A file, or for a kit a folder.
    Path(String),
    /// `amen.slices(16)`: a kit of equal pieces of a sample declared earlier.
    Slices { sample: Ident, count: f64 },
    /// A const table (or expression that yields one): `pad = sample(organ)`.
    Table(Expr),
}

/// `bus space` or `bus space: stereo`
#[derive(Debug, Clone, PartialEq)]
pub struct BusDecl {
    pub name: Ident,
    /// A layout such as `stereo` or `surround(5.1)`.
    pub layout: Option<Expr>,
}

/// `buffer take = zeros(48000)` or `buffer take = 4bars`
#[derive(Debug, Clone, PartialEq)]
pub struct BufferDecl {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// `config { channels: stereo }`
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDecl {
    pub entries: Vec<(Ident, Expr)>,
    /// `tuning: { a4: 432hz, scale: just }`: how note names become frequencies.
    pub tuning: Vec<(Ident, Expr)>,
    pub span: Span,
}

/// An audio collection (`node`) or persistent DSP instance (`fx`).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeDecl {
    pub name: Ident,
    /// Fx parameters become live controls; collection nodes have none.
    pub params: Vec<ParamDef>,
    pub body: Vec<Stmt>,
    pub span: Span,
    /// `with(quantize: 1bar)` after the body.
    pub options: Vec<Arg>,
}

/// `fn name(x, cutoff: hz) -> hz { ... }`
#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    pub name: Ident,
    pub params: Vec<ParamDef>,
    pub result: FnResult,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// What follows `->` in a function's signature.
#[derive(Debug, Clone, PartialEq)]
pub enum FnResult {
    /// No `->`: the unit is whatever the body gives.
    Inferred,
    /// `-> hz`
    Unit(Ident),
    /// `-> (low, band, high)`: the names the body binds as its results.
    Named(Vec<Ident>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub name: Ident,
    /// `init`, `control` or `const`, before the type.
    pub rate: Option<(Rate, Span)>,
    /// `None` in a function means any unit (generic).
    pub ty: Option<TypeSpec>,
    pub default: Option<Expr>,
}

/// How often a value may change, slowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rate {
    /// Known while compiling.
    Const,
    /// Fixed at note start.
    Init,
    /// Once per block.
    Control,
    /// Every sample.
    Audio,
}

impl Rate {
    pub fn keyword(self) -> &'static str {
        match self {
            Rate::Const => "const",
            Rate::Init => "init",
            Rate::Control => "control",
            Rate::Audio => "audio",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum TypeSpec {
    /// `hz`, `db`, `sec`, `num`, `int`
    Unit(Ident),
    /// `0..1` or `20hz..20khz`: a bounded value.
    Range { lo: Expr, hi: Expr, span: Span },
    /// `(low | band | high)`: one of these names, known while compiling.
    Choice(Vec<Ident>),
    /// `[num]`: a list whose entries have this type.
    Array(Box<TypeSpec>, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Bind {
        name: Ident,
        value: Expr,
    },
    /// `state y = 0.0`, only inside a function.
    State {
        name: Ident,
        init: Expr,
    },
    /// `[l, r] = src` names the channels of a multichannel signal.
    Destructure {
        names: Vec<Ident>,
        value: Expr,
    },
    /// `out += expr` layers a signal; `space += expr` sends to a bus.
    AddAssign {
        name: Ident,
        value: Expr,
    },
    /// `buf[i] = expr` writes an entry of a state list.
    IndexAssign {
        name: Ident,
        index: Expr,
        value: Expr,
    },
    Expr(Expr),
}

impl Stmt {
    /// The expressions directly inside this statement.
    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Stmt::Bind { value, .. }
            | Stmt::Destructure { value, .. }
            | Stmt::AddAssign { value, .. }
            | Stmt::Expr(value) => vec![value],
            Stmt::State { init, .. } => vec![init],
            Stmt::IndexAssign { index, value, .. } => vec![index, value],
        }
    }

    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Stmt::Bind { value, .. }
            | Stmt::Destructure { value, .. }
            | Stmt::AddAssign { value, .. }
            | Stmt::Expr(value) => vec![value],
            Stmt::State { init, .. } => vec![init],
            Stmt::IndexAssign { index, value, .. } => vec![index, value],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// `and` / `or`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Num {
        value: f64,
        unit: Option<String>,
    },
    Name(String),
    /// `f(a, b)`. `x.f(a)` is parsed as `f(x, a)` with `x` marked as the receiver.
    Call {
        name: Ident,
        args: Vec<Arg>,
    },
    Neg(Box<Expr>),
    /// `[a, b]`: a fixed-length array. Audio output interprets its elements as channels.
    Array(Vec<Expr>),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `a < b`: 1 when it holds and 0 when it doesn't.
    Compare {
        op: CmpOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `a and b`, `a or b`: 1 or 0.
    Logic {
        op: LogicOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `not a`
    Not(Box<Expr>),
    /// `if cond { ... } else { ... }`. As a value, both blocks end with an expression; as a
    /// statement the `else` is optional. `else if` is an `otherwise` holding one `if`.
    If {
        cond: Box<Expr>,
        then: Vec<Stmt>,
        otherwise: Option<Vec<Stmt>>,
    },
    /// `for x in xs { ... }`: as a value, the list of what each pass ends with.
    For {
        var: Ident,
        iter: Box<Expr>,
        body: Vec<Stmt>,
    },
    Env(EnvLit),
    /// `lo..hi`, or `lo..<hi` when it stops before `hi`.
    Range {
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
    },
    /// `list[2]`: one entry of a list, by a whole number known while compiling.
    Index {
        list: Box<Expr>,
        index: Box<Expr>,
    },
    /// `fn(r) { osc(saw, freq * r) }`, passed where a function is expected.
    Lambda {
        params: Vec<Ident>,
        body: Vec<Stmt>,
    },
    /// `"/fx/width"`: text, as an argument such as an OSC address.
    Str(String),
}

impl Expr {
    /// Call `f` on each expression directly inside this one, including those in nested blocks.
    #[inline(never)]
    pub fn for_each_child_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        let block = |stmts: &mut Vec<Stmt>, f: &mut dyn FnMut(&mut Expr)| {
            for s in stmts {
                for e in s.exprs_mut() {
                    f(e);
                }
            }
        };
        match &mut self.kind {
            ExprKind::Num { .. } | ExprKind::Name(_) | ExprKind::Str(_) => {}
            ExprKind::Call { args, .. } => args.iter_mut().for_each(|a| f(&mut a.value)),
            ExprKind::Neg(inner) | ExprKind::Not(inner) => f(inner),
            ExprKind::Array(items) => items.iter_mut().for_each(&mut *f),
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Compare { lhs, rhs, .. }
            | ExprKind::Logic { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            ExprKind::If {
                cond,
                then,
                otherwise,
            } => {
                f(cond);
                block(then, f);
                if let Some(o) = otherwise {
                    block(o, f);
                }
            }
            ExprKind::For { iter, body, .. } => {
                f(iter);
                block(body, f);
            }
            ExprKind::Env(env) => {
                for v in env.values_mut() {
                    if let EnvValue::Expr(e) = v {
                        f(e);
                    }
                }
            }
            ExprKind::Range { lo, hi, .. } => {
                f(lo);
                f(hi);
            }
            ExprKind::Index { list, index } => {
                f(list);
                f(index);
            }
            ExprKind::Lambda { body, .. } => block(body, f),
        }
    }

    /// Like [`Expr::for_each_child_mut`], read-only.
    pub fn for_each_child(&self, f: &mut dyn FnMut(&Expr)) {
        let block = |stmts: &Vec<Stmt>, f: &mut dyn FnMut(&Expr)| {
            for s in stmts {
                for e in s.exprs() {
                    f(e);
                }
            }
        };
        match &self.kind {
            ExprKind::Num { .. } | ExprKind::Name(_) | ExprKind::Str(_) => {}
            ExprKind::Call { args, .. } => args.iter().for_each(|a| f(&a.value)),
            ExprKind::Neg(inner) | ExprKind::Not(inner) => f(inner),
            ExprKind::Array(items) => items.iter().for_each(&mut *f),
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Compare { lhs, rhs, .. }
            | ExprKind::Logic { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            ExprKind::If {
                cond,
                then,
                otherwise,
            } => {
                f(cond);
                block(then, f);
                if let Some(o) = otherwise {
                    block(o, f);
                }
            }
            ExprKind::For { iter, body, .. } => {
                f(iter);
                block(body, f);
            }
            ExprKind::Env(env) => {
                for v in env.values() {
                    if let EnvValue::Expr(e) = v {
                        f(e);
                    }
                }
            }
            ExprKind::Range { lo, hi, .. } => {
                f(lo);
                f(hi);
            }
            ExprKind::Index { list, index } => {
                f(list);
                f(index);
            }
            ExprKind::Lambda { body, .. } => block(body, f),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
    pub receiver: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurveKind {
    Exp,
    Log,
    Custom(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvNum {
    pub value: f64,
    pub unit: Option<String>,
    pub span: Span,
}

/// One step of an envelope: a literal, or a name or `(expression)` read at note start.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvValue {
    Lit(EnvNum),
    Expr(Box<Expr>),
}

impl EnvValue {
    /// The literal, if this step is one.
    #[cfg(test)]
    pub fn lit(&self) -> Option<&EnvNum> {
        match self {
            EnvValue::Lit(n) => Some(n),
            EnvValue::Expr(_) => None,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            EnvValue::Lit(n) => n.span,
            EnvValue::Expr(e) => e.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvSegment {
    pub dur: EnvValue,
    pub curve: Option<CurveKind>,
    pub level: EnvValue,
}

/// `env[0 5ms 1 120ms 0.4 | 600ms 0]`
#[derive(Debug, Clone, PartialEq)]
pub struct EnvLit {
    pub start: EnvValue,
    pub segments: Vec<EnvSegment>,
    /// Number of segments before the `|`.
    pub sustain_at: Option<usize>,
    /// `env[...].loop`: starts over when it ends.
    pub looped: bool,
}

impl EnvLit {
    pub fn values(&self) -> Vec<&EnvValue> {
        let mut v = vec![&self.start];
        for s in &self.segments {
            v.push(&s.dur);
            v.push(&s.level);
        }
        v
    }

    pub fn values_mut(&mut self) -> Vec<&mut EnvValue> {
        let mut v = vec![&mut self.start];
        for s in &mut self.segments {
            v.push(&mut s.dur);
            v.push(&mut s.level);
        }
        v
    }
}

/// Units that measure time.
pub fn is_time_unit(unit: &str) -> bool {
    matches!(unit, "sec" | "ms" | "beat" | "beats" | "bar" | "bars")
}
