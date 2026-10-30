//! The rules for calling a built-in opcode that both compilers share. Each compiler evaluates the
//! arguments in its own terms (a signal in the graph, a register in a kernel) and wraps them in an
//! [`ArgVal`]; what is checked, how arrays spread over a call and what the engine is told when the
//! opcode is built is decided here, once.

use std::sync::Arc;

use niminal_engine::Table;

use crate::layout::Layout;
use crate::opcodes::{BuildArgs, Kind, OpSpec, WaveArg};
use crate::unit::Unit;

/// An evaluated argument of a built-in opcode, where `S` is a compiler's own kind of signal.
#[derive(Clone)]
pub(crate) enum ArgVal<S> {
    Wave(WaveArg),
    /// Tables known while compiling: one, or one per channel.
    Tables(Vec<Arc<Table>>),
    Layout(Layout),
    Sig(S),
    Array(Vec<ArgVal<S>>),
}

/// The unit an argument was expected to have, if `found` is not it. Gains take decibels or a plain
/// factor; every other signal parameter takes exactly its unit.
pub(crate) fn unit_expected(kind: Kind, found: Unit) -> Option<Unit> {
    match kind {
        Kind::Signal(expected) if found != expected => Some(expected),
        Kind::Gain if !matches!(found, Unit::Db | Unit::Num) => Some(Unit::Db),
        _ => None,
    }
}

/// What a parameter of this kind expects, for an error message.
pub(crate) fn expectation(kind: Kind, expected: Unit) -> String {
    match kind {
        Kind::Gain => "db or a plain factor".into(),
        _ => expected.describe().to_string(),
    }
}

type CallRow<S> = Vec<Option<ArgVal<S>>>;

/// The calls an array argument spreads a call into: one per element, with the arguments that are not
/// arrays repeated. `None` when no argument is an array. Arrays in one call must have the same length.
pub(crate) fn rows<S: Clone>(
    vals: &[Option<ArgVal<S>>],
) -> Result<Option<Vec<CallRow<S>>>, &'static str> {
    let mut lengths = vals.iter().filter_map(|v| match v {
        Some(ArgVal::Array(items)) => Some(items.len()),
        _ => None,
    });
    let Some(count) = lengths.next() else {
        return Ok(None);
    };
    if lengths.any(|n| n != count) {
        return Err("opcode array arguments must have matching shapes");
    }
    Ok(Some(
        (0..count)
            .map(|i| {
                vals.iter()
                    .map(|v| match v {
                        Some(ArgVal::Array(items)) => Some(items[i].clone()),
                        other => other.clone(),
                    })
                    .collect()
            })
            .collect(),
    ))
}

/// What the engine is told when the opcode is built for one channel: its waveform or table, and the
/// arguments that are constants. `konst` says whether a signal is one.
pub(crate) fn build_args<S>(
    spec: &OpSpec,
    vals: &[Option<ArgVal<S>>],
    konst: impl Fn(&S) -> Option<f64>,
    channel: usize,
    salt: u32,
) -> Result<BuildArgs, String> {
    let mut build = BuildArgs {
        channel,
        salt,
        ..Default::default()
    };
    for (param, val) in spec.params.iter().zip(vals) {
        match val {
            Some(ArgVal::Wave(w)) => build.wave = Some(w.clone()),
            Some(ArgVal::Tables(t)) => {
                build.table = Some(t[channel % t.len()].clone());
                build.tables.insert(param.name.clone(), t.clone());
            }
            Some(ArgVal::Sig(s)) => {
                if let Some(k) = konst(s) {
                    build.consts.insert(param.name.clone(), k);
                } else if !param.wired {
                    return Err(format!("`{}` must be known while compiling", param.name));
                }
            }
            Some(ArgVal::Layout(_) | ArgVal::Array(_)) | None => {}
        }
    }
    Ok(build)
}
