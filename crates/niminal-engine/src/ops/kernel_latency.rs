//! Align audio-rate register dependencies before constructing a running kernel.
use super::{Instr, Kernel, Native, NativeInput, SampleDelay};

impl Kernel {
    pub fn compensate_latency(&mut self) {
        let mut delays = vec![0usize; self.registers];
        let mut audio = vec![false; self.registers];
        let mut code = Vec::new();
        for mut instruction in std::mem::take(&mut self.code) {
            let owner = match instruction {
                Instr::Bin { dst, .. }
                | Instr::Cmp { dst, .. }
                | Instr::Select { dst, .. }
                | Instr::Neg { dst, .. }
                | Instr::Copy { dst, .. }
                | Instr::Call { dst, .. }
                | Instr::Native { dst, .. }
                | Instr::Frame { dst, .. }
                | Instr::Read { dst, .. }
                | Instr::ReadAt { dst, .. } => Some(dst),
                _ => None,
            };
            let mut inputs = match instruction {
                Instr::Bin { a, b, .. } | Instr::Cmp { a, b, .. } => vec![a, b],
                Instr::Select { cond, a, b, .. } => vec![cond, a, b],
                Instr::Neg { a, .. } | Instr::Copy { src: a, .. } | Instr::Call { a, .. } => {
                    vec![a]
                }
                Instr::Frame { input, input2, .. } => {
                    let mut inputs = vec![input];
                    inputs.extend(input2);
                    inputs
                }
                Instr::Native { index, .. } => self.natives[index]
                    .inputs
                    .iter()
                    .filter_map(|v| {
                        if let NativeInput::Reg(r) = v {
                            Some(*r)
                        } else {
                            None
                        }
                    })
                    .collect(),
                Instr::Read { idx, list, .. } | Instr::ReadAt { pos: idx, list, .. } => {
                    let mut inputs = vec![idx];
                    if let super::ArraySource::Values(id) = list {
                        inputs.extend(self.values[id].iter().copied());
                    }
                    inputs
                }
                _ => Vec::new(),
            };
            let latency = inputs.iter().map(|&r| delays[r]).max().unwrap_or(0);
            let old = inputs.clone();
            for r in &mut inputs {
                let init = if let Instr::Native { index, .. } = instruction {
                    self.natives[index]
                        .inputs
                        .iter()
                        .enumerate()
                        .any(|(port, input)| {
                            matches!(input, NativeInput::Reg(input) if input == r)
                                && self.natives[index].op.input_is_init(port)
                        })
                } else {
                    false
                };
                *r = self.align_register(
                    *r,
                    latency,
                    owner.map(|owner| (owner, init)),
                    &mut delays,
                    &mut audio,
                    &mut code,
                );
            }
            let remap = |r: &mut usize| {
                if let Some(i) = old.iter().position(|v| v == r) {
                    *r = inputs[i];
                }
            };
            let (dst, count, own) = match &mut instruction {
                Instr::Bin { dst, a, b, .. } | Instr::Cmp { dst, a, b, .. } => {
                    remap(a);
                    remap(b);
                    (Some(*dst), 1, 0)
                }
                Instr::Select { dst, cond, a, b } => {
                    remap(cond);
                    remap(a);
                    remap(b);
                    (Some(*dst), 1, 0)
                }
                Instr::Neg { dst, a } | Instr::Call { dst, a, .. } => {
                    remap(a);
                    (Some(*dst), 1, 0)
                }
                Instr::Copy { dst, src } => {
                    remap(src);
                    (Some(*dst), 1, 0)
                }
                Instr::Native { dst, index, .. } => {
                    for input in &mut self.natives[*index].inputs {
                        if let NativeInput::Reg(r) = input {
                            remap(r);
                        }
                    }
                    (
                        Some(*dst),
                        self.natives[*index].op.outputs(),
                        self.natives[*index].op.latency(),
                    )
                }
                Instr::Const { dst, .. } | Instr::SampleRate { dst } => {
                    delays[*dst] = 0;
                    code.push(instruction);
                    continue;
                }
                Instr::Read { dst, list, idx }
                | Instr::ReadAt {
                    dst,
                    list,
                    pos: idx,
                    ..
                } => {
                    remap(idx);
                    if let super::ArraySource::Values(id) = list {
                        let mut values = self.values[*id].clone();
                        for r in values.iter_mut() {
                            remap(r);
                        }
                        *id = self.values.len();
                        self.values.push(values);
                    }
                    (Some(*dst), 1, 0)
                }
                Instr::Frame {
                    dst,
                    input,
                    input2,
                    scope,
                } => {
                    remap(input);
                    if let Some(r) = input2 {
                        remap(r);
                    }
                    (Some(*dst), 1, self.frames[*scope].size)
                }
                Instr::Input { dst, .. } | Instr::LoadState { dst, .. } => (Some(*dst), 1, 0),
                _ => (None, 0, 0),
            };
            if let Some(dst) = dst {
                for k in 0..count {
                    delays[dst + k] = latency + own;
                    audio[dst + k] = true;
                }
            }
            code.push(instruction);
        }
        self.latency = self
            .outputs
            .iter()
            .map(|(_, r)| delays[*r])
            .max()
            .unwrap_or(0);
        for i in 0..self.outputs.len() {
            let r = self.outputs[i].1;
            self.outputs[i].1 = self.align_register(
                r,
                self.latency,
                Some((r, false)),
                &mut delays,
                &mut audio,
                &mut code,
            );
        }
        self.code = code;
    }

    fn align_register(
        &mut self,
        reg: usize,
        target: usize,
        owner: Option<(usize, bool)>,
        delays: &mut Vec<usize>,
        audio: &mut Vec<bool>,
        code: &mut Vec<Instr>,
    ) -> usize {
        if !audio[reg] || delays[reg] >= target {
            return reg;
        }
        let index = self.natives.len();
        self.natives.push(Native {
            op: Box::new(if owner.is_some_and(|(_, init)| init) {
                SampleDelay::warm(target - delays[reg])
            } else {
                SampleDelay::new(target - delays[reg])
            }),
            inputs: vec![NativeInput::Reg(reg)],
        });
        if let Some((owner, _)) = owner {
            for scope in &mut self.resets {
                if scope.registers.contains(&owner) {
                    scope.natives.push(index);
                }
            }
        }
        let dst = self.registers;
        self.registers += 1;
        delays.push(target);
        audio.push(true);
        code.push(Instr::Native {
            dst,
            index,
            guard: None,
        });
        dst
    }
}
