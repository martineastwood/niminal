//! Opt-in executed sample-instruction counters. Profiling builds are not timing baselines.
use super::{BinOp, Instr};
use std::cell::RefCell;

pub const NAMES: [&str; 29] = [
    "add",
    "sub",
    "mul",
    "div",
    "min",
    "max",
    "pow",
    "atan2",
    "rem",
    "load_state",
    "store_state",
    "mul_add",
    "constant",
    "input",
    "sample_rate",
    "compare",
    "compare_skip",
    "select",
    "skip",
    "memo",
    "negate",
    "copy",
    "math_call",
    "native",
    "read",
    "read_at",
    "write",
    "reset",
    "vector_frame",
];
const N: usize = NAMES.len();
#[derive(Clone)]
pub struct Counts {
    pub instructions: [u64; N],
    pub state_reads: u64,
    pub state_writes: u64,
    pub pairs: [[u64; N]; N],
}
thread_local! {
    static COUNTS: RefCell<Counts> = const { RefCell::new(Counts {
        instructions: [0; N], pairs: [[0; N]; N], state_reads: 0, state_writes: 0,
    }) };
}
/// Reset this thread's counters outside rendering.
pub fn reset() {
    COUNTS.with(|counts| {
        *counts.borrow_mut() = Counts {
            instructions: [0; N],
            pairs: [[0; N]; N],
            state_reads: 0,
            state_writes: 0,
        }
    });
}
/// Copy this thread's counters outside rendering.
pub fn snapshot() -> Counts {
    COUNTS.with(|counts| counts.borrow().clone())
}

pub(super) fn record(instr: &Instr, pc: usize, previous: &mut Option<(usize, usize)>) {
    let kind = match instr {
        Instr::Bin { op, .. } => match op {
            BinOp::Add => 0,
            BinOp::Sub => 1,
            BinOp::Mul => 2,
            BinOp::Div => 3,
            BinOp::Min => 4,
            BinOp::Max => 5,
            BinOp::Pow => 6,
            BinOp::Atan2 => 7,
            BinOp::Rem => 8,
        },
        Instr::LoadState { .. } => 9,
        Instr::StoreState { .. } => 10,
        Instr::MulAdd { .. } | Instr::MulSub { .. } | Instr::SubMul { .. } => 11,
        Instr::Clamp { .. } => 4,
        Instr::Const { .. } => 12,
        Instr::Input { .. } => 13,
        Instr::SampleRate { .. } => 14,
        Instr::Cmp { .. } => 15,
        Instr::CmpSkip { .. } => 16,
        Instr::Select { .. } => 17,
        Instr::SkipIfZero { .. } => 18,
        Instr::Memo { .. } => 19,
        Instr::Neg { .. } => 20,
        Instr::Copy { .. } => 21,
        Instr::Call { .. } => 22,
        Instr::Native { .. } => 23,
        Instr::Read { .. } => 24,
        Instr::ReadAt { .. } => 25,
        Instr::Write { .. } => 26,
        Instr::Reset { .. } => 27,
        _ => 28,
    };
    COUNTS.with(|counts| {
        let mut counts = counts.borrow_mut();
        counts.instructions[kind] += 1;
        match instr {
            Instr::LoadState { .. } => counts.state_reads += 1,
            Instr::StoreState { .. } => counts.state_writes += 1,
            _ => {}
        }
        if let Some((last_pc, last_kind)) = *previous
            && last_pc + 1 == pc
        {
            counts.pairs[last_kind][kind] += 1;
        }
    });
    *previous = Some((pc, kind));
}
