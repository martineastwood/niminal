use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap())
}

fn near(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn a_control_from_zero_to_one_is_mapped_where_it_is_used() {
    let mapped = raw("ctl k = 0.5\ninstr a() { osc(saw, 220hz).lpf(cutoff: k.range(200hz..4khz)) }\na() for 1beat");
    let by_hand = raw("instr a() { osc(saw, 220hz).lpf(cutoff: 2100hz) }\na() for 1beat");
    assert!(rms(&mapped) > 0.01);
    assert!(near(&mapped, &by_hand));

}

#[test]
fn decibels_move_evenly_in_db() {
    let mapped = raw("ctl v = 0.5\ninstr a() { osc(saw, 220hz) * v.range(-60db..0db) }\na() for 1beat");
    let by_hand = raw("instr a() { osc(saw, 220hz).gain(-30db) }\na() for 1beat");
    assert!(near(&mapped, &by_hand));
}

#[test]
fn range_mistakes_are_reported() {
    let error = |src: &str| compile(src).err().map(|e| e.iter().map(|d| d.message.clone()).collect::<String>()).unwrap_or_default();
    assert!(error("ctl k = 0.5\ninstr a() { osc(saw, 220hz).lpf(cutoff: k.range(200hz..4)) }").contains("same unit"));
    assert!(error("ctl k = 0.5hz\ninstr a() { osc(saw, 220hz).lpf(cutoff: k.range(200hz..4khz)) }").contains("plain signal from 0 to 1"));
    assert!(error("ctl k = 0.5\ninstr a() { osc(saw, 220hz).lpf(cutoff: k.range(200hz)) }").contains("takes a range"));
    // one spelling: a range, never two ends
    assert!(error("ctl k = 0.5\ninstr a() { osc(saw, 220hz).lpf(cutoff: k.range(200hz, 4khz)) }").contains("takes a range"));
}
