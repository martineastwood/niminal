//! The wasm backend must give the interpreter's output bit for bit, whatever the operands.
//!
//! Random kernels over a pool of awkward values (NaN, infinities, signed zeros, huge and tiny
//! numbers) run through both, block after block, and every register is compared.
#![cfg(any(feature = "wasm-kernel", all(feature = "jit-dynasm", any(target_arch = "aarch64", target_arch = "x86_64"))))]
#![allow(dead_code)]

use niminal_engine::ops::{
    ArraySource, BinOp, Cmp, Custom, Fn1, Instr, Interp, Kernel, MemoMath,
};
use niminal_engine::{Array, Opcode, Port, ProcessCtx};
use std::sync::Arc;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

const POOL: [f64; 22] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    2.0,
    3.7,
    -3.7,
    1e300,
    -1e300,
    1e-300,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    1e10,
    48_000.0,
    7.0,
    -0.25,
    6.0,
    -2.5,
    0.999,
    100.0,
];
const REGS: usize = 16;
const OPS: [BinOp; 9] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Min,
    BinOp::Max,
    BinOp::Pow,
    BinOp::Atan2,
    BinOp::Rem,
];
const CMPS: [Cmp; 6] = [Cmp::Lt, Cmp::Gt, Cmp::Le, Cmp::Ge, Cmp::Eq, Cmp::Ne];
const INTERPS: [Interp; 3] = [Interp::None, Interp::Linear, Interp::Cubic];

/// The operations a JavaScript engine computes exactly as Rust does: everything but the
/// transcendental functions, whose last bits depend on the maths library.
const EXACT_OPS: [BinOp; 7] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Min,
    BinOp::Max,
    BinOp::Rem,
];
const EXACT_FNS: [Fn1; 6] = [
    Fn1::Abs,
    Fn1::Floor,
    Fn1::Ceil,
    Fn1::Sqrt,
    Fn1::Sign,
    Fn1::Fract,
];

fn random_kernel(rng: &mut Rng) -> Kernel {
    random_kernel_with(rng, &OPS, &Fn1::ALL)
}

/// Without the list instructions, which the dynasm spike does not generate.
fn random_scalar_kernel(rng: &mut Rng) -> Kernel {
    random_kernel(rng)
}

#[allow(dead_code)]
fn random_scalar_kernel_without_lists(rng: &mut Rng) -> Kernel {
    let mut kernel = random_kernel(rng);
    for instr in &mut kernel.code {
        if matches!(
            instr,
            Instr::Read { .. } | Instr::ReadAt { .. } | Instr::Write { .. }
        ) {
            *instr = Instr::Neg { dst: 3, a: 5 };
        }
    }
    kernel
}

fn random_kernel_with(rng: &mut Rng, ops: &[BinOp], fns: &[Fn1]) -> Kernel {
    let reg = |rng: &mut Rng| rng.below(REGS);
    let len = 40;
    let mut code = Vec::new();
    for at in 0..len {
        let left = len - at - 1;
        let (d, a, b, c) = (reg(rng), reg(rng), reg(rng), reg(rng));
        let instr = match rng.below(24) {
            0 => Instr::Const {
                dst: d,
                value: rng.pick(&POOL),
            },
            1 => Instr::Input {
                dst: d,
                port: rng.below(2),
            },
            2 => Instr::SampleRate { dst: d },
            3 => Instr::LoadState {
                dst: d,
                slot: rng.below(3),
            },
            4 => Instr::StoreState {
                slot: rng.below(3),
                src: a,
            },
            5..=8 => Instr::Bin {
                dst: d,
                op: rng.pick(ops),
                a,
                b,
            },
            9 => Instr::Neg { dst: d, a },
            10 => Instr::Copy { dst: d, src: a },
            11 => match rng.below(3) {
                0 => Instr::MulAdd { dst: d, a, b, c },
                1 => Instr::MulSub { dst: d, a, b, c },
                _ => Instr::SubMul { dst: d, a, b, c },
            },
            12 => Instr::Clamp {
                dst: d,
                x: a,
                lo: b,
                hi: c,
            },
            13 => Instr::Cmp {
                dst: d,
                op: rng.pick(&CMPS),
                a,
                b,
            },
            14 => Instr::Select {
                dst: d,
                cond: a,
                a: b,
                b: c,
            },
            15 | 16 => Instr::Call {
                dst: d,
                f: rng.pick(fns),
                a,
            },
            17 => Instr::SkipIfZero {
                cond: a,
                count: rng.below(left.min(6) + 1),
            },
            18 => Instr::CmpSkip {
                dst: d,
                op: rng.pick(&CMPS),
                a,
                b,
                count: rng.below(left.min(6) + 1),
            },
            19 => Instr::Memo {
                dst: d,
                slot: rng.below(2),
                math: if rng.below(2) == 0 {
                    MemoMath::Unary(rng.pick(fns))
                } else {
                    MemoMath::Binary(rng.pick(ops), b)
                },
                a,
            },
            20 => Instr::Read {
                dst: d,
                list: if rng.below(2) == 0 {
                    ArraySource::State(0)
                } else {
                    ArraySource::Table(0)
                },
                idx: a,
            },
            21 => Instr::ReadAt {
                dst: d,
                list: if rng.below(2) == 0 {
                    ArraySource::State(0)
                } else {
                    ArraySource::Table(0)
                },
                pos: a,
                interp: rng.pick(&INTERPS),
            },
            22 => Instr::Write {
                list: 0,
                idx: a,
                src: b,
                cond: if rng.below(2) == 0 { Some(c) } else { None },
            },
            _ => Instr::Neg { dst: d, a },
        };
        code.push(instr);
    }
    Kernel {
        ports: vec![Port::required("a"), Port::required("b")],
        registers: REGS,
        memo_slots: 2,
        init: (0..8)
            .map(|r| Instr::Const {
                dst: r,
                value: rng.pick(&POOL),
            })
            .collect(),
        code,
        state_init: vec![0.0, 1.0, -0.5],
        buffers: vec![Array::from(vec![0.1, -0.2, 0.3, 0.4, -0.5, 0.6, 0.7])],
        tables_f64: vec![vec![1.0, 2.0, -3.0, 0.25, 8.0].into_boxed_slice()],
        values: vec![Array::from(vec![3usize, 5, 7, 9])],
        outputs: (0..REGS).map(|r| (format!("r{r}"), r)).collect(),
        ..Kernel::default()
    }
}

fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[cfg(feature = "wasm-kernel")]
fn run_pair(seed: u64) {
    run_pair_with(seed, random_kernel, Custom::new_wasm)
}

fn run_pair_with(
    seed: u64,
    make_kernel: fn(&mut Rng) -> Kernel,
    make: fn(Arc<Kernel>) -> Result<Custom, String>,
) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let kernel = Arc::new(make_kernel(&mut rng));
    let mut interp = Custom::new_interpreted(kernel.clone());
    let mut wasm = make(kernel.clone()).expect("kernel compiles");
    let ctx = ProcessCtx::new(48_000.0, true);
    for block in 0..4 {
        if block == 3 {
            interp.reset();
            wasm.reset();
        }
        let frames = 64;
        let input: Vec<Vec<f32>> = (0..2)
            .map(|_| {
                (0..frames)
                    .map(|_| {
                        if rng.below(3) == 0 {
                            rng.pick(&POOL) as f32
                        } else {
                            (rng.below(2000) as f32 - 1000.0) / 37.0
                        }
                    })
                    .collect()
            })
            .collect();
        let inputs: Vec<&[f32]> = input.iter().map(|v| v.as_slice()).collect();
        let mut out_i = vec![vec![0.0f32; frames]; REGS];
        let mut out_w = vec![vec![0.0f32; frames]; REGS];
        interp.process_outputs(&ctx, &inputs, &mut out_i, frames);
        wasm.process_outputs(&ctx, &inputs, &mut out_w, frames);
        for r in 0..REGS {
            for i in 0..frames {
                assert!(
                    same(out_i[r][i], out_w[r][i]),
                    "seed {seed} block {block} reg {r} sample {i}: \
                     interpreter {} wasm {}\n{:#?}",
                    out_i[r][i],
                    out_w[r][i],
                    kernel.code
                );
            }
        }
    }
}

#[cfg(feature = "wasm-kernel")]
#[test]
fn random_scalar_and_list_kernels_match_the_interpreter() {
    for seed in 1..=400 {
        run_pair(seed);
    }
}

#[cfg(feature = "wasm-kernel")]
#[test]
fn kernels_that_need_the_interpreter_are_refused() {
    for seed in 1..=20 {
        let mut rng = Rng(seed * 77 + 1);
        let mut kernel = random_kernel(&mut rng);
        kernel.code.push(Instr::Read {
            dst: 0,
            list: ArraySource::Values(0),
            idx: 1,
        });
        assert!(Custom::new_wasm(Arc::new(kernel)).is_err());
    }
}

/// Writes kernels and their interpreter results for `tests/wasm_node/check.mjs`, which runs the
/// same modules in V8. `NIMINAL_WASM_DUMP=path cargo test ... dump_for_node -- --ignored`.
#[cfg(feature = "wasm-kernel")]
#[test]
#[ignore]
fn dump_for_node() {
    use niminal_engine::ops::wasm::emit;
    let Ok(path) = std::env::var("NIMINAL_WASM_DUMP") else {
        return;
    };
    let hex = |v: u64| format!("\"{v:x}\"");
    let mut cases = Vec::new();
    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let kernel = random_kernel_with(&mut rng, &EXACT_OPS, &EXACT_FNS);
        let frames = niminal_engine::block_size();
        let emitted = emit(&kernel, frames).expect("emits");
        let input: Vec<Vec<f32>> = (0..2)
            .map(|_| {
                (0..frames)
                    .map(|_| {
                        if rng.below(3) == 0 {
                            rng.pick(&POOL) as f32
                        } else {
                            (rng.below(2000) as f32 - 1000.0) / 37.0
                        }
                    })
                    .collect()
            })
            .collect();
        let inputs: Vec<&[f32]> = input.iter().map(|v| v.as_slice()).collect();
        let mut interp = Custom::new_interpreted(Arc::new(kernel.clone()));
        let mut out = vec![vec![0.0f32; frames]; REGS];
        interp.process_outputs(&ProcessCtx::new(48_000.0, true), &inputs, &mut out, frames);
        let mut regs0 = vec![0.0f64; REGS];
        for instr in &kernel.init {
            if let Instr::Const { dst, value } = *instr {
                regs0[dst] = value;
            }
        }
        let list = |v: &[f64]| v.iter().map(|x| hex(x.to_bits())).collect::<Vec<_>>().join(",");
        let list32 = |v: &[f32]| v.iter().map(|x| hex(u64::from(x.to_bits()))).collect::<Vec<_>>().join(",");
        let l = &emitted.layout;
        cases.push(format!(
            "{{\"seed\":{seed},\"wasm\":\"{}\",\"layout\":{{\"regs\":{},\"state\":{},\"sr\":{},\"inputs\":{},\"outputs\":{},\"block\":{},\"tables\":[{}],\"buffers\":[{}]}},\"regs\":[{}],\"state\":[{}],\"tables\":[{}],\"buffers\":[{}],\"inputs\":[{}],\"expected\":[{}],\"frames\":{frames}}}",
            emitted.bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            l.regs, l.state, l.sample_rate, l.inputs, l.outputs, l.block,
            l.tables.iter().map(|t| t.0.to_string()).collect::<Vec<_>>().join(","),
            l.buffers.iter().map(|t| t.0.to_string()).collect::<Vec<_>>().join(","),
            list(&regs0),
            list(&kernel.state_init),
            kernel.tables_f64.iter().map(|t| format!("[{}]", list(t))).collect::<Vec<_>>().join(","),
            kernel.buffers.iter().map(|t| format!("[{}]", list(t))).collect::<Vec<_>>().join(","),
            input.iter().map(|v| format!("[{}]", list32(v))).collect::<Vec<_>>().join(","),
            out.iter().map(|v| format!("[{}]", list32(v))).collect::<Vec<_>>().join(","),
        ));
    }
    std::fs::write(path, format!("[{}]", cases.join(",\n"))).expect("writes");
}

#[cfg(all(feature = "jit-dynasm", any(target_arch = "aarch64", target_arch = "x86_64")))]
#[test]
fn random_scalar_kernels_match_the_interpreter_on_the_jit() {
    for seed in 1..=400 {
        run_pair_with(seed, random_scalar_kernel, Custom::new_jit);
    }
}

#[cfg(all(feature = "jit-dynasm", any(target_arch = "aarch64", target_arch = "x86_64")))]
#[test]
fn every_operation_matches_on_edge_values_on_the_jit() {
    let mut bad = Vec::new();
    let mut check = |label: String, code: Instr, a: f64, b: f64, c: f64| {
        let kernel = Arc::new(Kernel {
            registers: 5,
            init: vec![
                Instr::Const { dst: 0, value: a },
                Instr::Const { dst: 1, value: b },
                Instr::Const { dst: 2, value: c },
            ],
            code: vec![code],
            outputs: vec![("out".into(), 4)],
            ..Kernel::default()
        });
        let ctx = ProcessCtx::new(48_000.0, true);
        let mut want = [0.0f32];
        let mut got = [0.0f32];
        Custom::new_interpreted(kernel.clone()).process(&ctx, &[], &mut want);
        Custom::new_jit(kernel).expect("generates").process(&ctx, &[], &mut got);
        if !same(want[0], got[0]) {
            bad.push(format!("{label}({a}, {b}, {c}): interpreter {} jit {}", want[0], got[0]));
        }
    };
    for &a in &POOL {
        for &b in &POOL {
            for op in OPS {
                check(format!("{op:?}"), Instr::Bin { dst: 4, op, a: 0, b: 1 }, a, b, 0.0);
            }
            for op in CMPS {
                check(format!("{op:?}"), Instr::Cmp { dst: 4, op, a: 0, b: 1 }, a, b, 0.0);
            }
            check("Select".into(), Instr::Select { dst: 4, cond: 0, a: 1, b: 2 }, a, b, 7.0);
            check("MulAdd".into(), Instr::MulAdd { dst: 4, a: 0, b: 1, c: 2 }, a, b, 0.5);
            check("Clamp".into(), Instr::Clamp { dst: 4, x: 0, lo: 1, hi: 2 }, a, b, 3.0);
        }
        for f in Fn1::ALL {
            check(format!("{f:?}"), Instr::Call { dst: 4, f, a: 0 }, a, 0.0, 0.0);
        }
        check("Neg".into(), Instr::Neg { dst: 4, a: 0 }, a, 0.0, 0.0);
    }
    bad.truncate(25);
    assert!(bad.is_empty(), "{} mismatches:\n{}", bad.len(), bad.join("\n"));
}

#[cfg(all(feature = "jit-dynasm", any(target_arch = "aarch64", target_arch = "x86_64")))]
#[test]
fn the_jit_hands_back_to_the_interpreter_for_buffers_it_cannot_use() {
    // two outputs, but `process` renders into a single buffer
    let kernel = Arc::new(Kernel {
        registers: 4,
        state_init: vec![0.0],
        init: vec![Instr::Const { dst: 0, value: 0.25 }],
        code: vec![
            Instr::LoadState { dst: 1, slot: 0 },
            Instr::Bin { dst: 2, op: BinOp::Add, a: 1, b: 0 },
            Instr::StoreState { slot: 0, src: 2 },
            Instr::Neg { dst: 3, a: 2 },
        ],
        outputs: vec![("a".into(), 2), ("b".into(), 3)],
        ..Kernel::default()
    });
    let ctx = ProcessCtx::new(48_000.0, true);
    let mut want = [0.0f32; 8];
    let mut got = [0.0f32; 8];
    Custom::new_interpreted(kernel.clone()).process(&ctx, &[], &mut want);
    Custom::new_jit(kernel.clone()).expect("generates").process(&ctx, &[], &mut got);
    assert_eq!(want.map(f32::to_bits), got.map(f32::to_bits));
    // more samples than the block size the voice was made for
    let frames = niminal_engine::block_size() + 5;
    let mut out_i = vec![vec![0.0f32; frames]; 2];
    let mut out_j = vec![vec![0.0f32; frames]; 2];
    Custom::new_interpreted(kernel.clone()).process_outputs(&ctx, &[], &mut out_i, frames);
    Custom::new_jit(kernel).expect("generates").process_outputs(&ctx, &[], &mut out_j, frames);
    assert_eq!(out_i, out_j);
}
