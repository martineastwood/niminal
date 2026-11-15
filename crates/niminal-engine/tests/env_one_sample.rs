//! The kernel's single-sample entry must match block processing through all envelope phases.
use niminal_engine::ops::{Curve, Env, EnvField, Segment};
use niminal_engine::{Opcode, ProcessCtx};

fn compare(mut block: Env, rate: f32, release: usize, fields: &[f32]) {
    block.prepare(rate);
    let mut one = block.clone();
    assert_eq!(one.has_process_one(), !fields.is_empty());
    let mut pos = 0;
    for chunk in [0, 1, 7, 31, 64, 3, 64, 64, 64, 64, 64] {
        let mut left = chunk;
        while left > 0 {
            let n = if pos < release {
                left.min(release - pos)
            } else {
                left
            };
            let mut ctx = ProcessCtx::new(rate, pos < release);
            let inputs: Vec<Vec<f32>> = fields
                .iter()
                .map(|v| vec![if pos == 0 { *v } else { *v + 0.1 }; n])
                .collect();
            let refs: Vec<_> = inputs.iter().map(Vec::as_slice).collect();
            let mut out = vec![0.0; n];
            block.process(&ctx, &refs, &mut out);
            for (i, expected) in out.iter().enumerate() {
                ctx.gate = pos + i < release;
                let values: Vec<_> = fields
                    .iter()
                    .map(|v| if pos + i == 0 { *v } else { *v + 0.1 })
                    .collect();
                let actual = one.process_one(&ctx, &values);
                assert_eq!(
                    expected.to_bits(),
                    actual.to_bits(),
                    "sample {} at {rate}Hz",
                    pos + i
                );
            }
            assert_eq!(block.is_active(), one.is_active());
            pos += n;
            left -= n;
        }
    }
    Opcode::reset(&mut block);
    Opcode::reset(&mut one);
    let ctx = ProcessCtx::new(rate, true);
    let refs: Vec<_> = fields.iter().map(std::slice::from_ref).collect();
    let mut out = [0.0];
    block.process(&ctx, &refs, &mut out);
    assert_eq!(out[0].to_bits(), one.process_one(&ctx, fields).to_bits());
}
#[test]
fn scalar_envelopes_match_blocks_across_rates_curves_and_release_points() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for curve in [Curve::Linear, Curve::Exp, Curve::Log, Curve::Custom(-2.5)] {
            for release in [0, 5, 63, 128, 256] {
                compare(
                    Env::new(
                        0.0,
                        vec![
                            Segment::new(0.001, 1.0, curve),
                            Segment::linear(0.002, 0.3),
                            Segment::new(0.001, 0.0, curve),
                        ],
                        Some(2),
                    ),
                    rate,
                    release,
                    &[],
                );
            }
        }
    }
}
#[test]
fn scalar_envelopes_match_initial_inputs_zero_segments_and_loops() {
    for release in [0, 5, 128, 256] {
        compare(
            Env::new(
                0.0,
                vec![
                    Segment::linear(0.1, 0.5),
                    Segment::new(0.1, 0.0, Curve::Exp),
                ],
                Some(1),
            )
            .with_inputs(vec![
                EnvField::Start,
                EnvField::Dur(0),
                EnvField::Level(0),
                EnvField::Dur(1),
            ]),
            48_000.0,
            release,
            &[0.25, 0.001, 1.2, 0.002],
        );
        compare(
            Env::new(
                0.0,
                vec![
                    Segment::linear(0.0, 0.8),
                    Segment::new(0.001, 0.0, Curve::Exp),
                ],
                None,
            )
            .looped(),
            48_000.0,
            release,
            &[],
        );
        compare(
            Env::new(
                0.0,
                vec![Segment::linear(0.0, 1.0), Segment::linear(0.0, 0.0)],
                None,
            )
            .looped(),
            48_000.0,
            release,
            &[],
        );
    }
}
#[test]
fn empty_block_does_not_consume_note_start_inputs() {
    let mut env =
        Env::new(0.0, vec![Segment::linear(0.001, 0.0)], None).with_inputs(vec![EnvField::Start]);
    let ctx = ProcessCtx::new(48_000.0, true);
    env.process(&ctx, &[&[9.0]], &mut []);
    assert_eq!(env.process_one(&ctx, &[0.25]), 0.25);
}
