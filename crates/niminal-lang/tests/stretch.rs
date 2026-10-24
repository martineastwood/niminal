use niminal_lang::compile;

#[test]
fn samplers_and_slices_accept_forwarded_stretch() {
    compile(
        r#"
const wave = harmonics([1], size: 4800)
pad = sample(wave).root(c4).loop(forward)
chops = pad.slices(4)
instr stretched(freq: hz, stretch: 0.25..4 = 2, stretch_mode: 0..2 = 2) {
    pad(freq: freq, stretch: stretch, stretch_mode: stretch_mode, loop_start: 0.2, loop_end: 0.8)
}
instr sliced(stretch: 0.25..4 = 0.5) {
    chops(sample: 1, stretch: stretch, stretch_mode: 1, rate: -1)
}
"#,
    )
    .unwrap();
}
