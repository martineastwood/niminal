//! Conservative scalar liveness and conditional sample-work elimination.
use super::{Instr, Kernel, LaneSrc, NativeInput};

/// Only scalar operations without effects or mutable-array reads can be discarded.
fn pure(instruction: &Instr) -> Option<(usize, [usize; 3], usize)> {
    match *instruction {
        Instr::Const { dst, .. } | Instr::SampleRate { dst } | Instr::Input { dst, .. } => {
            Some((dst, [0; 3], 0))
        }
        Instr::Bin { dst, a, b, .. } | Instr::Cmp { dst, a, b, .. } => Some((dst, [a, b, 0], 2)),
        Instr::Neg { dst, a } | Instr::Call { dst, a, .. } | Instr::Copy { dst, src: a } => {
            Some((dst, [a, 0, 0], 1))
        }
        Instr::Select { dst, cond, a, b } => Some((dst, [cond, a, b], 3)),
        _ => None,
    }
}

fn lane(src: LaneSrc, used: &mut Vec<usize>) {
    if let LaneSrc::Reg(reg) = src {
        used.push(reg);
    }
}

/// Dependencies of instructions which we always retain. Arrays, natives, and state updates are
/// deliberately conservative roots: removing their effects needs a separate analysis.
fn roots(instruction: &Instr, used: &mut Vec<usize>) {
    match *instruction {
        Instr::BeginBranch { cond } | Instr::SkipIfZero { cond, .. } => used.push(cond),
        Instr::EndBranch | Instr::LoadState { .. } => {}
        Instr::Reset { trigger, guard, .. } => {
            used.push(trigger);
            used.extend(guard);
        }
        Instr::StoreState { src, .. } => used.push(src),
        Instr::Native { guard, .. } => used.extend(guard),
        Instr::Read { idx, .. } | Instr::ReadAt { pos: idx, .. } => used.push(idx),
        Instr::Write { idx, src, cond, .. } => {
            used.extend([idx, src]);
            used.extend(cond);
        }
        Instr::LaneBin { a, b, .. } | Instr::Dot { a, b, .. } => {
            lane(a, used);
            lane(b, used);
        }
        Instr::LaneCall { a, .. } | Instr::Reduce { a, .. } => lane(a, used),
        Instr::LaneFromRegs { .. } => {} // All computed-array registers are roots below.
        Instr::LaneStore { src, cond, .. } => {
            lane(src, used);
            used.extend(cond);
        }
        Instr::Fir { taps, from, .. } => {
            lane(taps, used);
            used.push(from);
        }
        Instr::Taps { offsets, from, .. } => {
            lane(offsets, used);
            used.push(from);
        }
        Instr::Frame { input, input2, .. } => {
            used.push(input);
            used.extend(input2);
        }
        Instr::Fft { src, .. } => lane(src, used),
        Instr::Ifft { re, im, .. } => {
            lane(re, used);
            lane(im, used);
        }
        Instr::MatVec { matrix, v, .. } => {
            lane(matrix, used);
            lane(v, used);
        }
        Instr::Const { .. }
        | Instr::SampleRate { .. }
        | Instr::Input { .. }
        | Instr::Bin { .. }
        | Instr::Cmp { .. }
        | Instr::Neg { .. }
        | Instr::Call { .. }
        | Instr::Copy { .. }
        | Instr::Select { .. } => unreachable!("pure instructions have explicit dependencies"),
    }
}

impl Kernel {
    /// Remove unused scalar work without removing, reordering, or combining stateful operations.
    pub fn eliminate_dead_expressions(&mut self) {
        let mut dependencies = vec![None; self.registers];
        let mut used: Vec<_> = self.outputs.iter().map(|(_, reg)| *reg).collect();
        for values in &self.values {
            used.extend(values.iter().copied());
        }
        for native in &self.natives {
            used.extend(native.inputs.iter().filter_map(|input| match input {
                NativeInput::Reg(r) => Some(*r),
                NativeInput::Const(_) => None,
            }));
        }
        for frame in &self.frames {
            lane(frame.result, &mut used);
        }
        let code = self
            .init
            .iter()
            .chain(&self.block)
            .chain(&self.code)
            .chain(self.frames.iter().flat_map(|f| &f.code))
            .chain(
                self.resets
                    .iter()
                    .flat_map(|r| r.init.iter().chain(&r.block)),
            );
        for instruction in code {
            if let Some((dst, inputs, count)) = pure(instruction) {
                dependencies[dst] = Some((inputs, count));
            } else {
                roots(instruction, &mut used);
            }
        }
        let mut live = vec![false; self.registers];
        while let Some(reg) = used.pop() {
            if std::mem::replace(&mut live[reg], true) {
                continue;
            }
            if let Some((inputs, count)) = &dependencies[reg] {
                used.extend(&inputs[..*count]);
            }
        }
        let retain = |instruction: &Instr| pure(instruction).is_none_or(|(dst, _, _)| live[dst]);
        prune(&mut self.init, retain);
        prune(&mut self.block, retain);
        prune(&mut self.code, retain);
        for frame in &mut self.frames {
            prune(&mut frame.code, retain);
        }
        for reset in &mut self.resets {
            prune(&mut reset.init, retain);
            prune(&mut reset.block, retain);
        }
    }

    /// Resolve compiler branch markers after latency compensation has inserted its stateful work.
    pub fn optimize_branches(&mut self) {
        // A delayed merge can consume an untaken branch's past values. Keep those histories
        // unconditional until control-flow-aware latency alignment is available.
        let allow_skips =
            self.frames.is_empty() && self.natives.iter().all(|n| n.op.latency() == 0);
        self.code = branches(&self.code, &self.natives, allow_skips);
        for frame in &mut self.frames {
            frame.code = branches(&frame.code, &self.natives, false);
        }
    }
}

/// Retain instruction boundaries so existing forward skips survive scalar dead-code removal.
fn prune(code: &mut Vec<Instr>, retain: impl Fn(&Instr) -> bool) {
    let mut positions = vec![0; code.len() + 1];
    let mut result = Vec::with_capacity(code.len());
    let mut skips = Vec::new();
    for (i, &instruction) in code.iter().enumerate() {
        positions[i] = result.len();
        if retain(&instruction) {
            if let Instr::SkipIfZero { count, .. } = instruction {
                skips.push((result.len(), i + count + 1));
            }
            result.push(instruction);
        }
    }
    positions[code.len()] = result.len();
    for (start, target) in skips {
        let Instr::SkipIfZero { count, .. } = &mut result[start] else {
            unreachable!()
        };
        *count = positions[target] - start - 1;
    }
    *code = result;
}

fn worthwhile(code: &[Instr]) -> bool {
    use super::{BinOp, Fn1};
    code.iter().any(|i| {
        matches!(
            i,
            Instr::Call {
                f: Fn1::Sin | Fn1::Cos | Fn1::Tan | Fn1::Tanh | Fn1::Exp | Fn1::Log,
                ..
            } | Instr::Bin {
                op: BinOp::Pow | BinOp::Atan2 | BinOp::Rem,
                ..
            } | Instr::Read { .. }
                | Instr::ReadAt { .. }
        )
    }) || code
        .iter()
        .filter(|i| !matches!(i, Instr::BeginBranch { .. } | Instr::EndBranch))
        .count()
        >= 4
}

fn branches(code: &[Instr], natives: &[super::Native], allow_skips: bool) -> Vec<Instr> {
    // Mark regions where skipping would change the unconditional history of a frame or latency
    // compensator. Guarded zero-latency natives already pause in untaken branches.
    let mut stack = Vec::new();
    let mut pairs = vec![None; code.len()];
    for (i, instruction) in code.iter().enumerate() {
        match *instruction {
            Instr::BeginBranch { .. } => stack.push((i, allow_skips)),
            Instr::EndBranch => {
                let (start, safe) = stack.pop().expect("balanced compiler branch markers");
                pairs[start] = Some((i, safe));
                if !safe && let Some((_, parent)) = stack.last_mut() {
                    *parent = false;
                }
            }
            Instr::Frame { .. } | Instr::StoreState { .. } => {
                if let Some((_, safe)) = stack.last_mut() {
                    *safe = false;
                }
            }
            Instr::Native { index, guard, .. }
                if guard.is_none() || natives[index].op.latency() != 0 =>
            {
                if let Some((_, safe)) = stack.last_mut() {
                    *safe = false;
                }
            }
            _ => {}
        }
    }
    assert!(stack.is_empty(), "balanced compiler branch markers");
    let mut result = Vec::with_capacity(code.len());
    let mut positions = vec![0; code.len() + 1];
    let mut skips = Vec::new();
    for (i, instruction) in code.iter().enumerate() {
        positions[i] = result.len();
        match *instruction {
            Instr::BeginBranch { cond } => {
                let (end, safe) = pairs[i].expect("branch has an end marker");
                if safe && worthwhile(&code[i + 1..end]) {
                    skips.push((result.len(), end));
                    result.push(Instr::SkipIfZero { cond, count: 0 });
                }
            }
            Instr::EndBranch => {}
            other => result.push(other),
        }
    }
    positions[code.len()] = result.len();
    for (start, end) in skips {
        let Instr::SkipIfZero { count, .. } = &mut result[start] else {
            unreachable!()
        };
        *count = positions[end] - start - 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{BinOp, Fn1, Native, SampleDelay};

    #[test]
    fn dead_pure_chains_are_removed_but_state_updates_still_run() {
        let mut kernel = Kernel {
            registers: 8,
            init: vec![
                Instr::Const { dst: 0, value: 1.0 },
                Instr::Const { dst: 1, value: 2.0 },
            ],
            code: vec![
                Instr::Bin {
                    dst: 2,
                    op: BinOp::Mul,
                    a: 0,
                    b: 1,
                },
                Instr::Call {
                    dst: 3,
                    f: Fn1::Exp,
                    a: 2,
                },
                Instr::Bin {
                    dst: 4,
                    op: BinOp::Add,
                    a: 0,
                    b: 1,
                },
                Instr::StoreState { slot: 0, src: 4 },
                Instr::Copy { dst: 5, src: 0 },
            ],
            outputs: vec![("out".into(), 5)],
            state_init: vec![0.0],
            ..Kernel::default()
        };
        kernel.eliminate_dead_expressions();
        assert_eq!(kernel.code.len(), 3);
        assert!(matches!(kernel.code[0], Instr::Bin { dst: 4, .. }));
        assert!(matches!(kernel.code[1], Instr::StoreState { .. }));
        assert_eq!(kernel.init.len(), 2);
    }

    #[test]
    fn nested_branches_have_correct_forward_skip_lengths() {
        let mut kernel = Kernel {
            code: vec![
                Instr::BeginBranch { cond: 0 },
                Instr::Call {
                    dst: 1,
                    f: Fn1::Exp,
                    a: 0,
                },
                Instr::BeginBranch { cond: 1 },
                Instr::Call {
                    dst: 2,
                    f: Fn1::Exp,
                    a: 1,
                },
                Instr::EndBranch,
                Instr::Copy { dst: 3, src: 1 },
                Instr::EndBranch,
            ],
            ..Kernel::default()
        };
        kernel.optimize_branches();
        assert_eq!(
            kernel.code,
            vec![
                Instr::SkipIfZero { cond: 0, count: 4 },
                Instr::Call {
                    dst: 1,
                    f: Fn1::Exp,
                    a: 0
                },
                Instr::SkipIfZero { cond: 1, count: 1 },
                Instr::Call {
                    dst: 2,
                    f: Fn1::Exp,
                    a: 1
                },
                Instr::Copy { dst: 3, src: 1 },
            ]
        );
    }

    #[test]
    fn dead_code_removal_remaps_forward_skip_targets() {
        let mut kernel = Kernel {
            registers: 5,
            code: vec![
                Instr::SkipIfZero { cond: 0, count: 3 },
                Instr::Call {
                    dst: 1,
                    f: Fn1::Exp,
                    a: 0,
                },
                Instr::Call {
                    dst: 2,
                    f: Fn1::Sin,
                    a: 0,
                },
                Instr::Copy { dst: 3, src: 2 },
                Instr::Copy { dst: 4, src: 2 },
            ],
            outputs: vec![("out".into(), 4)],
            ..Kernel::default()
        };
        kernel.eliminate_dead_expressions();
        assert_eq!(
            kernel.code,
            vec![
                Instr::SkipIfZero { cond: 0, count: 1 },
                Instr::Call {
                    dst: 2,
                    f: Fn1::Sin,
                    a: 0
                },
                Instr::Copy { dst: 4, src: 2 },
            ]
        );
    }

    #[test]
    fn forward_skips_execute_only_when_the_condition_is_nonzero() {
        use crate::{Opcode, Port, ProcessCtx};
        let kernel = Kernel {
            registers: 4,
            ports: vec![Port::required("x")],
            init: vec![Instr::Const { dst: 1, value: 1.0 }],
            code: vec![
                Instr::Input { dst: 0, port: 0 },
                Instr::SkipIfZero { cond: 0, count: 1 },
                Instr::Bin {
                    dst: 2,
                    op: BinOp::Add,
                    a: 0,
                    b: 1,
                },
                Instr::Copy { dst: 3, src: 2 },
            ],
            outputs: vec![("out".into(), 3)],
            ..Kernel::default()
        };
        let mut op = crate::ops::Custom::new(std::sync::Arc::new(kernel));
        op.prepare(48000.0);
        let mut out = [0.0; 5];
        op.process(
            &ProcessCtx::new(48000.0, true),
            &[&[0.0, 1.0, 0.0, 2.0, 0.0]],
            &mut out,
        );
        assert_eq!(out, [0.0, 2.0, 2.0, 3.0, 3.0]);
    }

    #[test]
    fn cheap_selects_stay_branchless() {
        let mut kernel = Kernel {
            code: vec![
                Instr::BeginBranch { cond: 0 },
                Instr::Neg { dst: 1, a: 0 },
                Instr::EndBranch,
            ],
            ..Kernel::default()
        };
        kernel.optimize_branches();
        assert_eq!(kernel.code, vec![Instr::Neg { dst: 1, a: 0 }]);
    }

    #[test]
    fn latency_histories_and_unconditional_state_updates_are_not_skipped() {
        let code = vec![
            Instr::BeginBranch { cond: 0 },
            Instr::Call {
                dst: 1,
                f: Fn1::Exp,
                a: 0,
            },
            Instr::EndBranch,
        ];
        let mut kernel = Kernel {
            code: code.clone(),
            natives: vec![Native {
                op: Box::new(SampleDelay::new(2)),
                inputs: vec![NativeInput::Reg(0)],
            }],
            ..Kernel::default()
        };
        kernel.optimize_branches();
        assert_eq!(
            kernel.code,
            vec![Instr::Call {
                dst: 1,
                f: Fn1::Exp,
                a: 0
            }]
        );
        let mut kernel = Kernel {
            code: vec![
                code[0],
                code[1],
                Instr::StoreState { slot: 0, src: 1 },
                code[2],
            ],
            ..Kernel::default()
        };
        kernel.optimize_branches();
        assert_eq!(
            kernel.code,
            vec![code[1], Instr::StoreState { slot: 0, src: 1 }]
        );
    }
}
