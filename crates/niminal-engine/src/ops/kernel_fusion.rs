//! Fuse adjacent scalar work without changing its floating-point operation order.
use super::kernel_cse::map_reads;
use super::kernel_opt::prune_where;
use super::{BinOp, Instr, Kernel, LaneSrc, NativeInput};

impl Kernel {
    /// Replace a multiply and its sole consumer with one dispatch. Run after scalar
    /// cleanup and branch resolution. Products observed elsewhere remain ordinary instructions.
    pub fn fuse_scalar_chains(&mut self) -> usize {
        if !self.code.windows(2).any(|pair| {
            matches!(pair, [Instr::Bin { dst, op: BinOp::Mul, .. },
                Instr::Bin { op: BinOp::Add, a, b, .. }] if dst == a || dst == b)
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
        for pc in 1..self.code.len() {
            if remove[pc - 1] || targets[pc] {
                continue;
            }
            if let Instr::Bin {
                dst: product,
                op: BinOp::Mul,
                a,
                b,
            } = self.code[pc - 1]
                && uses[product] == 1
                && let Instr::Bin {
                    dst,
                    op: BinOp::Add,
                    a: left,
                    b: right,
                } = self.code[pc]
                && (left == product || right == product)
            {
                self.code[pc] = Instr::MulAdd {
                    dst,
                    a,
                    b,
                    c: if left == product { right } else { left },
                };
                remove[pc - 1] = true;
            }
        }
        let removed = remove.iter().filter(|&&r| r).count();
        if removed > 0 {
            prune_where(&mut self.code, |pc, _| !remove[pc]);
        }
        removed
    }
}
