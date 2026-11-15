use niminal_engine::ops::{BinOp, Custom, Instr, Kernel};
use niminal_engine::{Opcode, ProcessCtx};
use std::sync::Arc;

fn chain(a: f64, b: f64, c: f64, product_left: bool) -> Kernel {
    Kernel {
        registers: 5,
        init: vec![
            Instr::Const { dst: 0, value: a },
            Instr::Const { dst: 1, value: b },
            Instr::Const { dst: 2, value: c },
        ],
        code: vec![
            Instr::Bin {
                dst: 3,
                op: BinOp::Mul,
                a: 0,
                b: 1,
            },
            Instr::Bin {
                dst: 4,
                op: BinOp::Add,
                a: if product_left { 3 } else { 2 },
                b: if product_left { 2 } else { 3 },
            },
        ],
        outputs: vec![("out".into(), 4)],
        ..Kernel::default()
    }
}
fn render(kernel: Kernel) -> [f32; 5] {
    let mut op = Custom::new(Arc::new(kernel));
    let mut out = [0.0; 5];
    op.process(&ProcessCtx::new(48_000.0, true), &[], &mut out);
    out
}
#[test]
fn fusion_preserves_numeric_results_and_rounding_through_cleanup() {
    for (a, b, c) in [
        (1.0 + 2f64.powi(-27), 1.0 - 2f64.powi(-27), -1.0),
        (-0.0, 1.0, 0.0),
        (-0.0, 1.0, -0.0),
        (f64::INFINITY, 0.0, 1.0),
        (f64::MAX, 2.0, -f64::MAX),
        (f64::from_bits(0x7ff8_1234_5678_0000), 2.0, f64::NAN),
    ] {
        for left in [false, true] {
            let original = chain(a, b, c, left);
            let mut fused = original.clone();
            assert_eq!(fused.fuse_scalar_chains(), 1);
            assert_eq!(fused.code.len(), 1);
            fused.eliminate_dead_expressions();
            fused.eliminate_redundant_expressions();
            for (expected, actual) in render(original).into_iter().zip(render(fused)) {
                if expected.is_nan() {
                    // Rust does not specify arithmetic NaN payloads. Non-finite audio is
                    // independently silenced by Voice/the master safety tests.
                    // https://doc.rust-lang.org/std/primitive.f64.html#nan-bit-patterns
                    assert!(actual.is_nan());
                } else {
                    assert_eq!(expected.to_bits(), actual.to_bits());
                }
            }
        }
    }
}
#[test]
fn observed_products_and_multiple_definitions_are_not_fused() {
    let original = chain(2.0, 3.0, 4.0, true);
    let mut output = original.clone();
    output.outputs.push(("product".into(), 3));
    assert_eq!(output.fuse_scalar_chains(), 0);
    let mut values = original.clone();
    values.values.push(vec![3].into());
    assert_eq!(values.fuse_scalar_chains(), 0);
    let mut state = original.clone();
    state.state_init.push(0.0);
    state.code.push(Instr::StoreState { slot: 0, src: 3 });
    assert_eq!(state.fuse_scalar_chains(), 0);
    let mut overwritten = original;
    overwritten.code.push(Instr::Copy { dst: 3, src: 0 });
    assert_eq!(overwritten.fuse_scalar_chains(), 0);
}
#[test]
fn branch_entries_are_preserved_and_skip_counts_are_remapped() {
    let mut target_add = chain(2.0, 3.0, 4.0, true);
    target_add
        .code
        .insert(0, Instr::SkipIfZero { cond: 0, count: 1 });
    assert_eq!(target_add.fuse_scalar_chains(), 0);
    let mut skip_pair = chain(2.0, 3.0, 4.0, true);
    skip_pair
        .code
        .insert(0, Instr::SkipIfZero { cond: 0, count: 2 });
    assert_eq!(skip_pair.fuse_scalar_chains(), 1);
    assert!(matches!(
        skip_pair.code[0],
        Instr::SkipIfZero { count: 1, .. }
    ));
    assert_eq!(render(chain(2.0, 3.0, 4.0, true)), render(skip_pair));
}

#[test]
fn mul_sub_sub_mul_and_clamp_fusion_preserve_exact_results() {
    for (a, b, c) in [
        (1.0 + 2f64.powi(-27), 1.0 - 2f64.powi(-27), 1.0),
        (-0.0, 1.0, 0.0),
        (3.5, 2.0, 10.0),
        (-4.0, 0.5, -1.0),
    ] {
        for product_left in [false, true] {
            let original = Kernel {
                registers: 5,
                init: vec![
                    Instr::Const { dst: 0, value: a },
                    Instr::Const { dst: 1, value: b },
                    Instr::Const { dst: 2, value: c },
                ],
                code: vec![
                    Instr::Bin {
                        dst: 3,
                        op: BinOp::Mul,
                        a: 0,
                        b: 1,
                    },
                    Instr::Bin {
                        dst: 4,
                        op: BinOp::Sub,
                        a: if product_left { 3 } else { 2 },
                        b: if product_left { 2 } else { 3 },
                    },
                ],
                outputs: vec![("out".into(), 4)],
                ..Kernel::default()
            };
            let mut fused = original.clone();
            assert_eq!(fused.fuse_scalar_chains(), 1);
            assert!(matches!(
                fused.code.as_slice(),
                [Instr::MulSub { .. }] | [Instr::SubMul { .. }]
            ));
            for (expected, actual) in render(original).into_iter().zip(render(fused)) {
                assert_eq!(expected.to_bits(), actual.to_bits());
            }
        }
        let clamp_orig = Kernel {
            registers: 5,
            init: vec![
                Instr::Const { dst: 0, value: a },
                Instr::Const { dst: 1, value: -1.0 },
                Instr::Const { dst: 2, value: 1.0 },
            ],
            code: vec![
                Instr::Bin {
                    dst: 3,
                    op: BinOp::Max,
                    a: 0,
                    b: 1,
                },
                Instr::Bin {
                    dst: 4,
                    op: BinOp::Min,
                    a: 3,
                    b: 2,
                },
            ],
            outputs: vec![("out".into(), 4)],
            ..Kernel::default()
        };
        let mut clamp_fused = clamp_orig.clone();
        assert_eq!(clamp_fused.fuse_scalar_chains(), 1);
        assert!(matches!(clamp_fused.code.as_slice(), [Instr::Clamp { .. }]));
        assert_eq!(render(clamp_orig), render(clamp_fused));
    }
}

#[test]
fn state_reads_share_unmodified_slots_and_forward_stores() {
    use niminal_engine::ops::ArraySource;
    let mut k = Kernel {
        registers: 7,
        buffers: vec![vec![2.0, 5.0].into()],
        init: vec![
            Instr::Const { dst: 0, value: 0.0 },
            Instr::Const { dst: 1, value: 1.0 },
        ],
        code: vec![
            Instr::Read {
                dst: 2,
                list: ArraySource::State(0),
                idx: 0,
            },
            Instr::Read {
                dst: 3,
                list: ArraySource::State(0),
                idx: 1,
            },
            // Writing slot 0 does not invalidate slot 1, and forwards to a subsequent read of slot 0.
            Instr::Write {
                list: 0,
                idx: 0,
                src: 3,
                cond: None,
            },
            Instr::Read {
                dst: 4,
                list: ArraySource::State(0),
                idx: 0,
            },
            Instr::Read {
                dst: 5,
                list: ArraySource::State(0),
                idx: 1,
            },
            Instr::Bin {
                dst: 6,
                op: BinOp::Add,
                a: 4,
                b: 5,
            },
        ],
        outputs: vec![("out".into(), 6)],
        ..Kernel::default()
    };
    let expected = render(k.clone());
    k.eliminate_redundant_expressions();
    k.eliminate_dead_expressions();
    k.compact_registers();
    assert_eq!(
        k.code
            .iter()
            .filter(|i| matches!(i, Instr::Read { .. }))
            .count(),
        1
    );
    assert!(k.registers <= 4);
    assert_eq!(render(k), expected);
}
