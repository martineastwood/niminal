//! Compiles a function with state to an engine [`Kernel`]: straight-line
//! register code run once per sample. A kernel is compiled for each call, so a
//! function's untyped parameters take whatever units the call gives them.
//!
//! Every value has a rate, and the code that makes it runs at that rate: once
//! at note start, once per block, or every sample. Functions the body calls are
//! copied in, with their own state.

mod emit;
mod functions;
mod stmts;
mod exprs;
mod scopes;
mod calls;
mod natives;
mod lanes;
mod helpers;
pub(crate) use helpers::*;

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
