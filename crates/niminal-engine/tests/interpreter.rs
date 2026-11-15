//! Dispatch optimizations must preserve skipped instructions and intermediate rounding.
use niminal_engine::ops::{BinOp, Custom, Instr, Kernel};
use niminal_engine::{Opcode, Port, ProcessCtx};
use std::sync::Arc;

#[test]
fn skipped_products_retain_their_register_and_products_round_before_addition() {
    let kernel = Kernel {
        ports: vec![Port::required("trigger")],
        registers: 6,
        init: vec![
            Instr::Const {
                dst: 0,
                value: 1.0 + 2f64.powi(-27),
            },
            Instr::Const {
                dst: 1,
                value: 1.0 - 2f64.powi(-27),
            },
            Instr::Const {
                dst: 2,
                value: -1.0,
            },
        ],
        code: vec![
            Instr::Input { dst: 3, port: 0 },
            Instr::SkipIfZero { cond: 3, count: 1 },
            Instr::Bin {
                dst: 4,
                op: BinOp::Mul,
                a: 0,
                b: 1,
            },
            Instr::Bin {
                dst: 5,
                op: BinOp::Add,
                a: 4,
                b: 2,
            },
        ],
        outputs: vec![("out".into(), 5)],
        ..Kernel::default()
    };
    let mut op = Custom::new(Arc::new(kernel));
    let ctx = ProcessCtx::new(48_000.0, true);
    let mut out = [0.0; 5];
    op.process(&ctx, &[&[0.0, 1.0, 0.0, 1.0, 0.0]], &mut out);
    // The first skipped product leaves register 4 at zero. Later skips preserve 1.
    // A contracted multiply-add would produce -2^-54, rather than zero.
    let expected: [f32; 5] = [-1.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(out.map(f32::to_bits), expected.map(f32::to_bits));
    op.reset();
    op.process(&ctx, &[&[0.0; 5]], &mut out);
    assert_eq!(out.map(f32::to_bits), [-1.0f32; 5].map(f32::to_bits));
}

#[test]
fn direct_arithmetic_matches_generic_math_for_edge_inputs() {
    for op in [
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::Min,
        BinOp::Max,
    ] {
        for a in [
            0.0,
            -0.0,
            1.0,
            -1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            for b in [
                0.0,
                -0.0,
                1.0,
                -1.0,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NAN,
            ] {
                let expected = std::hint::black_box(op).apply(a, b) as f32;
                let mut custom = Custom::new(Arc::new(Kernel {
                    registers: 3,
                    init: vec![
                        Instr::Const { dst: 0, value: a },
                        Instr::Const { dst: 1, value: b },
                    ],
                    code: vec![Instr::Bin {
                        dst: 2,
                        op,
                        a: 0,
                        b: 1,
                    }],
                    outputs: vec![("out".into(), 2)],
                    ..Kernel::default()
                }));
                let mut out = [0.0];
                custom.process(&ProcessCtx::new(48_000.0, true), &[], &mut out);
                if expected.is_nan() {
                    assert!(out[0].is_nan());
                } else {
                    assert_eq!(out[0].to_bits(), expected.to_bits(), "{op:?}({a}, {b})");
                }
            }
        }
    }
}
