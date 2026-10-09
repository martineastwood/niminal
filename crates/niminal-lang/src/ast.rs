use crate::diag::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Instr(InstrDef),
    Opcode(OpcodeDef),
    Tempo(Expr),
    Note(NoteStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstrDef {
    pub name: Ident,
    pub params: Vec<ParamDef>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `opcode name(x, cutoff: hz) { ... }`
#[derive(Debug, Clone, PartialEq)]
pub struct OpcodeDef {
    pub name: Ident,
    pub params: Vec<ParamDef>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub name: Ident,
    /// Optional in an opcode, where an untyped parameter is a plain signal.
    pub ty: Option<TypeSpec>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeSpec {
    /// `hz`, `db`, `sec`
    Unit(Ident),
    /// `0..1`: a bounded unitless parameter.
    Range { lo: f64, hi: f64, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Bind { name: Ident, value: Expr },
    /// `state y = 0.0`, only inside an opcode.
    State { name: Ident, init: Expr },
    Expr(Expr),
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

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Num { value: f64, unit: Option<String> },
    Name(String),
    /// `f(a, b)`. `x.f(a)` is parsed as `f(x, a)` with `x` marked as the receiver.
    Call { name: Ident, args: Vec<Arg> },
    Neg(Box<Expr>),
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Env(EnvLit),
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

#[derive(Debug, Clone, PartialEq)]
pub struct EnvSegment {
    pub dur: EnvNum,
    pub curve: Option<CurveKind>,
    pub level: EnvNum,
}

/// `env[0 5ms 1 120ms 0.4 | 600ms 0]`
#[derive(Debug, Clone, PartialEq)]
pub struct EnvLit {
    pub start: EnvNum,
    pub segments: Vec<EnvSegment>,
    /// Number of segments before the `|`.
    pub sustain_at: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteStmt {
    pub at: Option<Expr>,
    pub target: Ident,
    pub args: Vec<Arg>,
    pub dur: Expr,
    pub span: Span,
}

/// Units that measure time.
pub fn is_time_unit(unit: &str) -> bool {
    matches!(unit, "sec" | "ms" | "beat" | "beats")
}
