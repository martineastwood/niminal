//! Redundant scalar work in the sample section: identical expressions computed twice, and tests
//! of values that are already 0 or 1 against 0. Nothing is reordered, and nothing with an effect
//! (state, arrays, native opcodes, frames) is touched, so the audio is bit-for-bit the same.
//!
//! The pass relies on every register having one defining instruction. It checks that first and
//! leaves a kernel alone if it does not hold. An expression computed inside a conditional region
//! is not reused after the region ends, since the region may not have run.
use std::collections::{HashMap, HashSet};

use super::custom::MemoMath;
use super::{ArraySource, BinOp, Cmp, Fn1, Instr, Interp, Kernel, LaneSrc, NativeInput};

/// The registers an instruction writes: the first and how many.
fn defines(instr: &Instr, natives: &[super::Native]) -> Option<(usize, usize)> {
    match *instr {
        Instr::Const { dst, .. }
        | Instr::Input { dst, .. }
        | Instr::SampleRate { dst }
        | Instr::LoadState { dst, .. }
        | Instr::Bin { dst, .. }
        | Instr::MulAdd { dst, .. }
        | Instr::MulSub { dst, .. }
        | Instr::SubMul { dst, .. }
        | Instr::Clamp { dst, .. }
        | Instr::Memo { dst, .. }
        | Instr::Neg { dst, .. }
        | Instr::Copy { dst, .. }
        | Instr::Cmp { dst, .. }
        | Instr::CmpSkip { dst, .. }
        | Instr::Select { dst, .. }
        | Instr::Call { dst, .. }
        | Instr::Read { dst, .. }
        | Instr::ReadAt { dst, .. }
        | Instr::Reduce { dst, .. }
        | Instr::Dot { dst, .. }
        | Instr::Fir { dst, .. }
        | Instr::Frame { dst, .. } => Some((dst, 1)),
        Instr::Native { dst, index, .. } => Some((dst, natives[index].op.outputs().max(1))),
        _ => None,
    }
}

/// Rewrites every register an instruction writes.
fn map_writes(instr: &mut Instr, f: &mut impl FnMut(usize) -> usize) {
    match instr {
        Instr::Const { dst, .. }
        | Instr::Input { dst, .. }
        | Instr::SampleRate { dst }
        | Instr::LoadState { dst, .. }
        | Instr::Bin { dst, .. }
        | Instr::MulAdd { dst, .. }
        | Instr::MulSub { dst, .. }
        | Instr::SubMul { dst, .. }
        | Instr::Clamp { dst, .. }
        | Instr::Memo { dst, .. }
        | Instr::Neg { dst, .. }
        | Instr::Copy { dst, .. }
        | Instr::Cmp { dst, .. }
        | Instr::CmpSkip { dst, .. }
        | Instr::Select { dst, .. }
        | Instr::Call { dst, .. }
        | Instr::Read { dst, .. }
        | Instr::ReadAt { dst, .. }
        | Instr::Reduce { dst, .. }
        | Instr::Dot { dst, .. }
        | Instr::Fir { dst, .. }
        | Instr::Frame { dst, .. }
        | Instr::Native { dst, .. } => *dst = f(*dst),
        _ => {}
    }
}

/// Rewrites every register an instruction reads (not the ones it writes).
pub(super) fn map_reads(instr: &mut Instr, f: &mut impl FnMut(usize) -> usize) {
    let lane = |src: &mut LaneSrc, f: &mut dyn FnMut(usize) -> usize| {
        if let LaneSrc::Reg(r) = src {
            *r = f(*r);
        }
    };
    match instr {
        Instr::BeginBranch { cond } | Instr::SkipIfZero { cond, .. } => *cond = f(*cond),
        Instr::EndBranch
        | Instr::Const { .. }
        | Instr::Input { .. }
        | Instr::SampleRate { .. }
        | Instr::LoadState { .. }
        | Instr::LaneFromRegs { .. } => {}
        Instr::Reset { trigger, guard, .. } => {
            *trigger = f(*trigger);
            if let Some(g) = guard {
                *g = f(*g);
            }
        }
        Instr::StoreState { src, .. } => *src = f(*src),
        Instr::Bin { a, b, .. } | Instr::Cmp { a, b, .. } | Instr::CmpSkip { a, b, .. } => {
            *a = f(*a);
            *b = f(*b);
        }
        Instr::MulAdd { a, b, c, .. }
        | Instr::MulSub { a, b, c, .. }
        | Instr::SubMul { a, b, c, .. }
        | Instr::Clamp {
            x: a, lo: b, hi: c, ..
        } => {
            *a = f(*a);
            *b = f(*b);
            *c = f(*c);
        }
        Instr::Memo { a, math, .. } => {
            *a = f(*a);
            if let MemoMath::Binary(_, b) = math {
                *b = f(*b);
            }
        }
        Instr::Neg { a, .. } | Instr::Call { a, .. } | Instr::Copy { src: a, .. } => *a = f(*a),
        Instr::Select { cond, a, b, .. } => {
            *cond = f(*cond);
            *a = f(*a);
            *b = f(*b);
        }
        Instr::Native { guard, .. } => {
            if let Some(g) = guard {
                *g = f(*g);
            }
        }
        Instr::Read { idx, .. } | Instr::ReadAt { pos: idx, .. } => *idx = f(*idx),
        Instr::Write { idx, src, cond, .. } => {
            *idx = f(*idx);
            *src = f(*src);
            if let Some(c) = cond {
                *c = f(*c);
            }
        }
        Instr::LaneBin { a, b, .. } | Instr::Dot { a, b, .. } => {
            lane(a, f);
            lane(b, f);
        }
        Instr::LaneCall { a, .. } | Instr::Reduce { a, .. } => lane(a, f),
        Instr::LaneStore { src, cond, .. } => {
            lane(src, f);
            if let Some(c) = cond {
                *c = f(*c);
            }
        }
        Instr::Fir { taps, from, .. } => {
            lane(taps, f);
            *from = f(*from);
        }
        Instr::Taps { offsets, from, .. } => {
            lane(offsets, f);
            *from = f(*from);
        }
        Instr::Frame { input, input2, .. } => {
            *input = f(*input);
            if let Some(i) = input2 {
                *i = f(*i);
            }
        }
        Instr::Fft { src, .. } => lane(src, f),
        Instr::Ifft { re, im, .. } => {
            lane(re, f);
            lane(im, f);
        }
        Instr::MatVec { matrix, v, .. } => {
            lane(matrix, f);
            lane(v, f);
        }
    }
}

/// What a pure expression is, for finding another like it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Bin(BinOp, usize, usize),
    Cmp(Cmp, usize, usize),
    Neg(usize),
    Call(Fn1, usize),
    Select(usize, usize, usize),
    Input(usize),
    Read(ArraySource, usize),
    ReadAt(ArraySource, usize, Interp),
}

fn reads_state(key: &Key, list: usize) -> bool {
    matches!(
        key,
        Key::Read(ArraySource::State(s), _) | Key::ReadAt(ArraySource::State(s), _, _) if *s == list
    )
}

fn key_of(instr: &Instr) -> Option<(usize, Key)> {
    Some(match *instr {
        Instr::Bin { dst, op, a, b } => (dst, Key::Bin(op, a, b)),
        Instr::Cmp { dst, op, a, b } => {
            let (a, b) = if matches!(op, Cmp::Eq | Cmp::Ne) && a > b {
                (b, a)
            } else {
                (a, b)
            };
            (dst, Key::Cmp(op, a, b))
        }
        Instr::Neg { dst, a } => (dst, Key::Neg(a)),
        Instr::Call { dst, f, a } => (dst, Key::Call(f, a)),
        Instr::Select { dst, cond, a, b } => (dst, Key::Select(cond, a, b)),
        Instr::Input { dst, port } => (dst, Key::Input(port)),
        Instr::Read {
            dst,
            list: list @ (ArraySource::Table(_) | ArraySource::Values(_) | ArraySource::State(_)),
            idx,
        } => (dst, Key::Read(list, idx)),
        Instr::ReadAt {
            dst,
            list: list @ (ArraySource::Table(_) | ArraySource::Values(_) | ArraySource::State(_)),
            pos,
            interp,
        } => (dst, Key::ReadAt(list, pos, interp)),
        _ => return None,
    })
}

struct OpenRegion {
    end: usize,
    cond: Option<usize>,
    keys: Vec<Key>,
}

impl Kernel {
    /// Reuse identical sample-rate expressions and drop redundant tests on values that are
    /// already 0 or 1. Returns how many instructions it removed.
    pub fn eliminate_redundant_expressions(&mut self) -> usize {
        if !self.single_definitions() {
            return 0;
        }
        let mut last_read = vec![0usize; self.registers];
        for (pc, instr) in self.code.iter().enumerate() {
            let mut copy = *instr;
            map_reads(&mut copy, &mut |r| {
                if let Some(slot) = last_read.get_mut(r) {
                    *slot = (*slot).max(pc);
                }
                r
            });
        }
        let mut mark_external = |r: usize| {
            if let Some(slot) = last_read.get_mut(r) {
                *slot = usize::MAX;
            }
            r
        };
        for instr in self
            .init
            .iter()
            .chain(&self.block)
            .chain(self.frames.iter().flat_map(|f| &f.code))
            .chain(
                self.resets
                    .iter()
                    .flat_map(|r| r.init.iter().chain(&r.block)),
            )
        {
            let mut copy = *instr;
            map_reads(&mut copy, &mut mark_external);
        }
        for frame in &self.frames {
            if let LaneSrc::Reg(r) = frame.result {
                mark_external(r);
            }
        }
        for native in &self.natives {
            for input in &native.inputs {
                if let NativeInput::Reg(r) = input {
                    mark_external(*r);
                }
            }
        }
        for values in &self.values {
            for &r in values.iter() {
                mark_external(r);
            }
        }
        for &(_, r) in &self.outputs {
            mark_external(r);
        }

        // registers holding a constant, and the ones that are +0.0 or 1.0 by construction
        let mut constant: HashMap<usize, f64> = HashMap::new();
        let mut boolean: HashSet<usize> = HashSet::new();
        for instr in self.init.iter().chain(&self.block).chain(&self.code) {
            match *instr {
                Instr::Const { dst, value } => {
                    constant.insert(dst, value);
                    if value.to_bits() == 0.0f64.to_bits() || value == 1.0 {
                        boolean.insert(dst);
                    }
                }
                Instr::Cmp { dst, .. } | Instr::CmpSkip { dst, .. } => {
                    boolean.insert(dst);
                }
                _ => {}
            }
        }
        let has_resets = !self.resets.is_empty();
        let mut alias: HashMap<usize, usize> = HashMap::new();
        let mut remove = vec![false; self.code.len()];
        let mut seen: HashMap<Key, usize> = HashMap::new();
        // the expressions each open conditional region added, and where it ends
        let mut regions: Vec<OpenRegion> = Vec::new();
        for (pc, instr) in self.code.iter_mut().enumerate() {
            while regions.last().is_some_and(|r| r.end <= pc) {
                if let Some(region) = regions.pop() {
                    for key in region.keys {
                        seen.remove(&key);
                    }
                }
            }
            map_reads(instr, &mut |r| alias.get(&r).copied().unwrap_or(r));
            if let Instr::Reset { .. } = instr
                && has_resets
            {
                // a reset reruns code that changes registers read below
                seen.clear();
                for region in &mut regions {
                    region.keys.clear();
                    region.cond = None;
                }
                continue;
            }
            if let Instr::Write {
                list,
                idx,
                src,
                cond,
            } = instr
            {
                if let Some(c) = cond
                    && regions.iter().any(|r| r.cond == Some(*c))
                {
                    *cond = None;
                }
                let key = Key::Read(ArraySource::State(*list), *idx);
                if cond.is_none() && seen.get(&key) == Some(src) {
                    remove[pc] = true;
                    continue;
                }
                let len = self.buffers.get(*list).map_or(0, |b| b.len());
                let w_slot = constant
                    .get(idx)
                    .filter(|_| len > 0)
                    .map(|&v| crate::array::wrapped_index(v, len));
                let invalidates = |k: &Key| match k {
                    Key::ReadAt(ArraySource::State(s), _, _) => *s == *list,
                    Key::Read(ArraySource::State(s), r_idx) if *s == *list => {
                        match (w_slot, constant.get(r_idx).filter(|_| len > 0)) {
                            (Some(ws), Some(&rv)) => crate::array::wrapped_index(rv, len) == ws,
                            _ => true,
                        }
                    }
                    _ => false,
                };
                seen.retain(|k, _| !invalidates(k));
                for region in &mut regions {
                    region.keys.retain(|k| !invalidates(k));
                }
                if cond.is_none() {
                    seen.insert(key, *src);
                    if let Some(region) = regions.last_mut() {
                        region.keys.push(key);
                    }
                }
                continue;
            }
            if let Instr::LaneStore { list, cond, .. } = instr {
                if let Some(c) = cond
                    && regions.iter().any(|r| r.cond == Some(*c))
                {
                    *cond = None;
                }
                seen.retain(|k, _| !reads_state(k, *list));
                for region in &mut regions {
                    region.keys.retain(|k| !reads_state(k, *list));
                }
                continue;
            }
            if let Instr::SkipIfZero { cond, count } = *instr {
                regions.push(OpenRegion {
                    end: pc + 1 + count,
                    cond: Some(cond),
                    keys: Vec::new(),
                });
                continue;
            }
            let is_zero = |r: usize| constant.get(&r) == Some(&0.0);
            let is_pos_zero =
                |r: usize| constant.get(&r).is_some_and(|v| v.to_bits() == 0.0f64.to_bits());
            let is_one =
                |r: usize| constant.get(&r).is_some_and(|v| v.to_bits() == 1.0f64.to_bits());
            match *instr {
                Instr::Copy { dst, src } if boolean.contains(&src) => {
                    boolean.insert(dst);
                }
                Instr::Select { dst, a, b, .. }
                    if boolean.contains(&a) && boolean.contains(&b) =>
                {
                    boolean.insert(dst);
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Mul | BinOp::Min | BinOp::Max,
                    a,
                    b,
                } if boolean.contains(&a) && boolean.contains(&b) => {
                    boolean.insert(dst);
                }
                Instr::Bin {
                    dst,
                    op: BinOp::Sub,
                    a,
                    b,
                } if is_one(a) && boolean.contains(&b) => {
                    boolean.insert(dst);
                }
                _ => {}
            }
            if let Instr::Select { dst, cond, a, b } = *instr {
                let same = if a == b {
                    Some(a)
                } else if boolean.contains(&cond) && is_one(a) && is_pos_zero(b) {
                    Some(cond)
                } else {
                    None
                };
                if let Some(same) = same {
                    alias.insert(dst, same);
                    last_read[same] = last_read[same].max(last_read[dst]);
                    remove[pc] = true;
                    continue;
                }
            }
            if let Instr::Bin {
                dst,
                op: BinOp::Mul,
                a,
                b,
            } = *instr
            {
                let dst_last = last_read[dst];
                let same = if boolean.contains(&a)
                    && regions
                        .iter()
                        .any(|r| r.cond == Some(a) && dst_last < r.end)
                {
                    Some(b)
                } else if boolean.contains(&b)
                    && regions
                        .iter()
                        .any(|r| r.cond == Some(b) && dst_last < r.end)
                {
                    Some(a)
                } else {
                    None
                };
                if let Some(same) = same {
                    alias.insert(dst, same);
                    last_read[same] = last_read[same].max(dst_last);
                    remove[pc] = true;
                    continue;
                }
            }
            // Tests of values that are already 0 or 1 against a constant
            if let Instr::Cmp { dst, op, a, b } = *instr {
                let same = if boolean.contains(&a)
                    && let Some(&k) = constant.get(&b)
                {
                    if (op == Cmp::Ne && k == 0.0)
                        || (op == Cmp::Gt && (0.0..1.0).contains(&k))
                        || (op == Cmp::Ge && k > 0.0 && k <= 1.0)
                        || (op == Cmp::Eq && k == 1.0)
                    {
                        Some(a)
                    } else {
                        if op == Cmp::Le && k == 0.0 {
                            *instr = Instr::Cmp {
                                dst,
                                op: Cmp::Eq,
                                a,
                                b,
                            };
                        }
                        None
                    }
                } else if boolean.contains(&b)
                    && let Some(&k) = constant.get(&a)
                {
                    if (op == Cmp::Ne && k == 0.0)
                        || (op == Cmp::Lt && (0.0..1.0).contains(&k))
                        || (op == Cmp::Le && k > 0.0 && k <= 1.0)
                        || (op == Cmp::Eq && k == 1.0)
                    {
                        Some(b)
                    } else {
                        if op == Cmp::Ge && is_zero(a) {
                            *instr = Instr::Cmp {
                                dst,
                                op: Cmp::Eq,
                                a: b,
                                b: a,
                            };
                        }
                        None
                    }
                } else {
                    None
                };
                if let Some(same) = same {
                    alias.insert(dst, same);
                    last_read[same] = last_read[same].max(last_read[dst]);
                    remove[pc] = true;
                    continue;
                }
            }
            if let Some((dst, key)) = key_of(instr) {
                if let Some(&earlier) = seen.get(&key) {
                    alias.insert(dst, earlier);
                    last_read[earlier] = last_read[earlier].max(last_read[dst]);
                    remove[pc] = true;
                } else {
                    seen.insert(key, dst);
                    if let Some(region) = regions.last_mut() {
                        region.keys.push(key);
                    }
                }
            }
        }
        let removed = remove.iter().filter(|r| **r).count();
        if removed == 0 {
            return 0;
        }
        let mut rename = |r: usize| alias.get(&r).copied().unwrap_or(r);
        for instr in self
            .init
            .iter_mut()
            .chain(&mut self.block)
            .chain(self.frames.iter_mut().flat_map(|f| &mut f.code))
            .chain(
                self.resets
                    .iter_mut()
                    .flat_map(|r| r.init.iter_mut().chain(&mut r.block)),
            )
        {
            map_reads(instr, &mut rename);
        }
        for frame in &mut self.frames {
            if let LaneSrc::Reg(r) = &mut frame.result {
                *r = rename(*r);
            }
        }
        for native in &mut self.natives {
            for input in &mut native.inputs {
                if let NativeInput::Reg(r) = input {
                    *r = rename(*r);
                }
            }
        }
        for values in &mut self.values {
            for r in values.iter_mut() {
                *r = rename(*r);
            }
        }
        for (_, r) in &mut self.outputs {
            *r = rename(*r);
        }
        super::kernel_opt::prune_where(&mut self.code, |pc, _| !remove[pc]);
        removed
    }

    /// `if c { list[i] = v }` is compiled as read the old entry, select between it and `v`, write
    /// it back. This makes it the one write that checks `c` itself, and returns how many
    /// instructions went.
    pub fn fuse_conditional_writes(&mut self) -> usize {
        let mut uses = vec![0usize; self.registers];
        let mut count = |r: usize| {
            uses[r] += 1;
            r
        };
        for instr in self
            .init
            .iter_mut()
            .chain(&mut self.block)
            .chain(&mut self.code)
            .chain(self.frames.iter_mut().flat_map(|f| &mut f.code))
            .chain(
                self.resets
                    .iter_mut()
                    .flat_map(|r| r.init.iter_mut().chain(&mut r.block)),
            )
        {
            map_reads(instr, &mut count);
        }
        for frame in &self.frames {
            if let LaneSrc::Reg(r) = frame.result {
                count(r);
            }
        }
        for native in &self.natives {
            for input in &native.inputs {
                if let NativeInput::Reg(r) = input {
                    count(*r);
                }
            }
        }
        for values in &self.values {
            for &r in values.iter() {
                count(r);
            }
        }
        for &(_, r) in &self.outputs {
            count(r);
        }
        // a forward skip must not land inside a sequence that is being merged
        let mut targets: HashMap<usize, usize> = HashMap::new();
        for (pc, instr) in self.code.iter().enumerate() {
            if let Instr::SkipIfZero { count, .. } = *instr {
                *targets.entry(pc + 1 + count).or_default() += 1;
            }
        }
        let mut remove = vec![false; self.code.len()];
        let mut fused = Vec::new();
        let mut region_ends: Vec<usize> = Vec::new();
        let mut state_reads: HashMap<usize, (usize, usize, usize)> = HashMap::new();
        let mut pc = 0;
        while pc < self.code.len() {
            while region_ends.last().is_some_and(|&end| end <= pc) {
                region_ends.pop();
                let depth = region_ends.len();
                state_reads.retain(|_, (_, _, d)| *d <= depth);
            }
            // optional skip over the read, then read, select, write
            let skip = matches!(self.code[pc], Instr::SkipIfZero { count: 1, .. });
            let at = pc + usize::from(skip);
            if at + 2 < self.code.len()
                && let (
                    Instr::Read {
                        dst: old,
                        list: super::ArraySource::State(list),
                        idx,
                    },
                    Instr::Select { dst: new, cond, a, b },
                    Instr::Write {
                        list: written,
                        idx: written_idx,
                        src,
                        cond: None,
                    },
                ) = (self.code[at], self.code[at + 1], self.code[at + 2])
                && b == old
                && src == new
                && written == list
                && written_idx == idx
                && uses[old] == 1
                && uses[new] == 1
                // only this sequence's own skip may land on its select
                && targets.get(&(at + 1)).copied().unwrap_or(0) == usize::from(skip)
                && !targets.contains_key(&(at + 2))
            {
                remove[pc..at + 2].fill(true);
                fused.push((
                    at + 2,
                    Instr::Write {
                        list,
                        idx,
                        src: a,
                        cond: Some(cond),
                    },
                ));
                state_reads.retain(|_, (l, _, _)| *l != list);
                pc = at + 3;
                continue;
            }
            // When the read was shared with an earlier read of the same unmodified entry.
            let skip0 = matches!(self.code[pc], Instr::SkipIfZero { count: 0, .. });
            let at0 = pc + usize::from(skip0);
            if at0 + 1 < self.code.len()
                && let (
                    Instr::Select { dst: new, cond, a, b },
                    Instr::Write {
                        list,
                        idx,
                        src,
                        cond: None,
                    },
                ) = (self.code[at0], self.code[at0 + 1])
                && src == new
                && uses[new] == 1
                && state_reads
                    .get(&b)
                    .is_some_and(|&(l, i, _)| l == list && i == idx)
                && targets.get(&at0).copied().unwrap_or(0) == usize::from(skip0)
                && !targets.contains_key(&(at0 + 1))
            {
                remove[pc..at0 + 1].fill(true);
                fused.push((
                    at0 + 1,
                    Instr::Write {
                        list,
                        idx,
                        src: a,
                        cond: Some(cond),
                    },
                ));
                state_reads.retain(|_, (l, _, _)| *l != list);
                pc = at0 + 2;
                continue;
            }
            match self.code[pc] {
                Instr::SkipIfZero { count, .. } | Instr::CmpSkip { count, .. } => {
                    region_ends.push(pc + 1 + count);
                }
                Instr::Reset { .. } => state_reads.clear(),
                Instr::Write { list, .. } | Instr::LaneStore { list, .. } => {
                    state_reads.retain(|_, (l, _, _)| *l != list);
                }
                Instr::Read {
                    dst,
                    list: ArraySource::State(list),
                    idx,
                } => {
                    state_reads.insert(dst, (list, idx, region_ends.len()));
                }
                _ => {}
            }
            pc += 1;
        }
        if fused.is_empty() {
            return 0;
        }
        for (at, instr) in &fused {
            self.code[*at] = *instr;
        }
        let removed = remove.iter().filter(|r| **r).count();
        super::kernel_opt::prune_where(&mut self.code, |pc, _| !remove[pc]);
        removed
    }

    /// Renumber live registers into `0..live_count` after optimization passes have removed
    /// dead and fused intermediates.
    pub fn compact_registers(&mut self) {
        if self.registers == 0 {
            return;
        }
        let mut used = vec![false; self.registers];
        let mut mark = |r: usize| {
            if let Some(slot) = used.get_mut(r) {
                *slot = true;
            }
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
            if let Some((first, count)) = defines(instr, &self.natives) {
                for r in first..first + count {
                    mark(r);
                }
            }
            let mut copy = *instr;
            map_reads(&mut copy, &mut mark);
        }
        for frame in &self.frames {
            if let LaneSrc::Reg(r) = frame.result {
                mark(r);
            }
        }
        for native in &self.natives {
            for input in &native.inputs {
                if let NativeInput::Reg(r) = input {
                    mark(*r);
                }
            }
        }
        for values in &self.values {
            for &r in values.iter() {
                mark(r);
            }
        }
        for &(_, r) in &self.outputs {
            mark(r);
        }
        let live_count = used.iter().filter(|&&u| u).count();
        if live_count == self.registers {
            return;
        }
        let mut remap = vec![0usize; self.registers];
        let mut next = 0usize;
        for (r, &u) in used.iter().enumerate() {
            if u {
                remap[r] = next;
                next += 1;
            }
        }
        let mut rename = |r: usize| remap[r];
        for instr in self
            .init
            .iter_mut()
            .chain(&mut self.block)
            .chain(&mut self.code)
            .chain(self.frames.iter_mut().flat_map(|f| &mut f.code))
            .chain(
                self.resets
                    .iter_mut()
                    .flat_map(|r| r.init.iter_mut().chain(&mut r.block)),
            )
        {
            map_reads(instr, &mut rename);
            map_writes(instr, &mut rename);
        }
        for frame in &mut self.frames {
            if let LaneSrc::Reg(r) = &mut frame.result {
                *r = rename(*r);
            }
        }
        for native in &mut self.natives {
            for input in &mut native.inputs {
                if let NativeInput::Reg(r) = input {
                    *r = rename(*r);
                }
            }
        }
        for values in &mut self.values {
            for r in values.iter_mut() {
                *r = rename(*r);
            }
        }
        for (_, r) in &mut self.outputs {
            *r = rename(*r);
        }
        self.registers = next;
    }

    /// Whether every register is written by at most one instruction.
    pub(super) fn single_definitions(&self) -> bool {
        let mut written = vec![false; self.registers];
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
        for instr in code {
            if let Some((first, count)) = defines(instr, &self.natives) {
                for slot in first..first + count {
                    match written.get_mut(slot) {
                        Some(w) if !*w => *w = true,
                        _ => return false,
                    }
                }
            }
        }
        true
    }
}
