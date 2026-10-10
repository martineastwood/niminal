use crate::diag::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Instr(InstrDef),
    Control { name: Ident, value: Expr },
    Opcode(OpcodeDef),
    Bus(BusDecl),
    Track(TrackDecl),
    Sample(SampleDecl),
    Arrangement(ArrangementDecl),
    Config(ConfigDecl),
    Tempo(TempoStmt),
    /// `meter 3/4`
    Meter(Expr),
    /// `name = expr` at the top level: a pattern, clip or scene value.
    Bind { name: Ident, value: Expr },
    Clip(NamedBlock),
    Scene(NamedBlock),
    Command(CommandStmt),
    Note(NoteStmt),
}

/// `tempo 124bpm`, optionally `@ next bar over 2 bars` for a change while playing.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoStmt {
    pub value: Expr,
    pub quantize: Option<QuantizeSpec>,
    pub over: Option<Expr>,
    pub span: Span,
}

/// `clip name { key: value ... }` or `scene name { track: clip ... }`
#[derive(Debug, Clone, PartialEq)]
pub struct NamedBlock {
    pub name: Ident,
    pub entries: Vec<(Ident, Expr)>,
    pub span: Span,
}

/// Where a statement happens: a time, or `bar 5 beat 3`.
#[derive(Debug, Clone, PartialEq)]
pub enum AtPos {
    Time(Expr),
    /// Bars and beats count from 1.
    Bar { bar: Expr, beat: Option<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play { track: Ident, what: Expr },
    Stop(Ident),
    Mute(Ident),
    Unmute(Ident),
    Solo(Ident),
    Unsolo(Option<Ident>),
    Hush,
    Panic,
    Launch(Ident),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandStmt {
    pub at: Option<AtPos>,
    pub command: Command,
    pub quantize: Option<QuantizeSpec>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Land on the grid: `@ bar`, `@ 4 bars`.
    OnGrid,
    /// The next line of the grid: `@ next 4 bars`.
    Next,
    /// Counted from now: `@ in 4 bars`.
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantUnit {
    Now,
    Beat,
    Bar,
    Cycle,
}

/// When a change should land, relative to a musical grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantizeSpec {
    pub relation: Relation,
    pub count: f64,
    pub unit: QuantUnit,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstrDef {
    pub name: Ident,
    pub params: Vec<ParamDef>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `sample kick = "drums/kick.wav" with(root: c4)`, or `kit drums = "drums/808"`.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleDecl {
    pub is_kit: bool,
    pub name: Ident,
    pub source: SampleSource,
    pub source_span: Span,
    pub options: Vec<Arg>,
    pub span: Span,
}

/// `arrangement song { sections: [verse.over(8 bars) chorus.over(8 bars)] }`
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangementDecl {
    pub name: Ident,
    pub sections: Vec<SectionRef>,
    pub span: Span,
}

/// One scene in an arrangement, and how long it lasts.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionRef {
    pub scene: Ident,
    pub length: Expr,
}

/// Where a sample or kit comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum SampleSource {
    /// A file, or for a kit a folder.
    Path(String),
    /// `amen.slices(16)`: a kit of equal pieces of a sample declared earlier.
    Slices { sample: Ident, count: f64 },
}

/// `bus space` or `bus space: stereo`
#[derive(Debug, Clone, PartialEq)]
pub struct BusDecl {
    pub name: Ident,
    /// A layout such as `stereo` or `surround(5.1)`.
    pub layout: Option<Expr>,
}

/// `config { channels: stereo }`
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDecl {
    pub entries: Vec<(Ident, Expr)>,
    pub span: Span,
}

/// `track name { instrument = ...  out = ... }`
#[derive(Debug, Clone, PartialEq)]
pub struct TrackDecl {
    pub name: Ident,
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
    /// `[l, r] = src` names the channels of a multichannel signal.
    Destructure { names: Vec<Ident>, value: Expr },
    /// `out += expr` layers a signal; `space += expr` sends to a bus.
    AddAssign { name: Ident, value: Expr },
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
    /// A mini-notation pattern: the raw text between the brackets.
    Pattern(String),
    /// `grid[x.x.]`: the raw text between the brackets.
    Grid(String),
    /// `~`: nothing, or stop.
    Rest,
    /// `[a, b]`: a comma list is a multichannel signal, one expression per channel.
    Channels(Vec<Expr>),
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
    pub at: Option<AtPos>,
    pub target: Ident,
    pub args: Vec<Arg>,
    pub dur: Expr,
    pub span: Span,
}

/// Units that measure time.
pub fn is_time_unit(unit: &str) -> bool {
    matches!(unit, "sec" | "ms" | "beat" | "beats" | "bar" | "bars")
}
