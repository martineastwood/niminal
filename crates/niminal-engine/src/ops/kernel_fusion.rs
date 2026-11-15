//! Fuse adjacent scalar work without changing its floating-point operation order.
use super::kernel_cse::map_reads;
use super::kernel_opt::prune_where;
use super::{BinOp, Instr, Kernel, LaneSrc, NativeInput};

impl Kernel {
    /// Replace a single-use `Mul` or `Max` and its sole consumer (`Add`, `Sub`, or `Min`)
    /// in the same basic block with one fused instruction. Run after scalar cleanup and
    /// branch resolution. Intermediates observed elsewhere remain ordinary instructions.
    pub fn fuse_scalar_chains(&mut self) -> usize {
        if !self.code.iter().any(|i| {
            matches!(
                i,
                Instr::Bin {
                    op: BinOp::Add | BinOp::Sub | BinOp::Min,
                    ..
                }
            )
        }) {
            return 0;
        }
        if !self.single_definitions() {
            return 0;
        }
        let mut uses = vec![0usize; self.registers];
        let mut count = |r: usize| {
            uses[r] = uses[r].saturating_add(1);
            r
        };
        for instr in self
            .init
            .iter()
            .chain(&self.block)
            .chain(&self.code)
            .chain(self.frames.iter().flat_map(|f| &f.code))
            .chain(
                self.resets
                    .iter()
                    .flat_map(|r| r.init.iter().chain(&r.block)),
            )
        {
            let mut instruction = *instr;
            map_reads(&mut instruction, &mut count);
        }
        for (_, reg) in &self.outputs {
            count(*reg);
        }
        for values in &self.values {
            for &reg in values.iter() {
                count(reg);
            }
        }
        for native in &self.natives {
            for input in &native.inputs {
                if let NativeInput::Reg(reg) = input {
                    count(*reg);
                }
            }
        }
        for frame in &self.frames {
            if let LaneSrc::Reg(reg) = frame.result {
                count(reg);
            }
        }
        let mut targets = vec![false; self.code.len() + 1];
        for (pc, instr) in self.code.iter().enumerate() {
            if let Instr::SkipIfZero { count, .. } | Instr::CmpSkip { count, .. } = *instr {
                targets[pc + count + 1] = true;
            }
        }
        let mut remove = vec![false; self.code.len()];
        let mut candidates: Vec<Option<(usize, BinOp, usize, usize)>> = vec![None; self.registers];
        let mut active_candidates: Vec<usize> = Vec::new();
        for (pc, instr) in self.code.iter_mut().enumerate() {
            if targets[pc] {
                for reg in active_candidates.drain(..) {
                    candidates[reg] = None;
                }
            }
            match *instr {
                Instr::SkipIfZero { .. }
                | Instr::CmpSkip { .. }
                | Instr::BeginBranch { .. }
                | Instr::EndBranch
                | Instr::Reset { .. } => {
                    for reg in active_candidates.drain(..) {
                        candidates[reg] = None;
                    }
                    continue;
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Add,
                    a: left,
                    b: right,
                } => {
                    if let Some((def_pc, BinOp::Mul, a, b)) = candidates[left] {
                        *instr = Instr::MulAdd {
                            dst,
                            a,
                            b,
                            c: right,
                        };
                        remove[def_pc] = true;
                        candidates[left] = None;
                        continue;
                    }
                    if let Some((def_pc, BinOp::Mul, a, b)) = candidates[right] {
                        *instr = Instr::MulAdd { dst, a, b, c: left };
                        remove[def_pc] = true;
                        candidates[right] = None;
                        continue;
                    }
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Sub,
                    a: left,
                    b: right,
                } => {
                    if let Some((def_pc, BinOp::Mul, a, b)) = candidates[left] {
                        *instr = Instr::MulSub {
                            dst,
                            a,
                            b,
                            c: right,
                        };
                        remove[def_pc] = true;
                        candidates[left] = None;
                        continue;
                    }
                    if let Some((def_pc, BinOp::Mul, a, b)) = candidates[right] {
                        *instr = Instr::SubMul {
                            dst,
                            a,
                            b,
                            c: left,
                        };
                        remove[def_pc] = true;
                        candidates[right] = None;
                        continue;
                    }
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Min,
                    a: left,
                    b: right,
                } => {
                    if let Some((def_pc, BinOp::Max, x, lo)) = candidates[left] {
                        *instr = Instr::Clamp {
                            dst,
                            x,
                            lo,
                            hi: right,
                        };
                        remove[def_pc] = true;
                        candidates[left] = None;
                        continue;
                    }
                }
                _ => {}
            }
            if let Instr::Bin {
                dst,
                op: op @ (BinOp::Mul | BinOp::Max),
                a,
                b,
            } = *instr
                && uses[dst] == 1
            {
                candidates[dst] = Some((pc, op, a, b));
                active_candidates.push(dst);
            }
        }
        let removed = remove.iter().filter(|&&r| r).count();
        if removed > 0 {
            prune_where(&mut self.code, |pc, _| !remove[pc]);
        }
        removed
    }
}
