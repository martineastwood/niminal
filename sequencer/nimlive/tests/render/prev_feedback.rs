use crate::common::*;

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

// A click into a bus. The node reads the bus at half level and sends its own output back to the bus a block late.
const LOOP: &str = "\ntempo 120bpm\nbus fb\ninstr click() { fb += osc(sine, 1000hz) * env[1 0.5ms 0] }\nfx again { out = fb.gain(0.5)\n  fb += late(out) }\nclick() for 1beat\n";

#[test]
fn a_late_send_makes_a_feedback_loop_one_block_long() {
    let out = render_with(
        &program(LOOP),
        &[],
        &RenderOptions {
            limiter: false,
            ..Default::default()
        },
    )
    .unwrap()
    .channels
    .remove(0);
    let block = niminal_engine::DEFAULT_BLOCK;
    let echo = |k: usize| peak(&out[k * block..(k + 1) * block]);
    assert!(echo(0) > 0.2, "the click, at half level");
    for k in 1..5 {
        assert!(
            (echo(k) / echo(k - 1) - 0.5).abs() < 0.05,
            "echo {k} is half the one before: {} vs {}",
            echo(k),
            echo(k - 1)
        );
    }
}
