#[test]
fn probe() {
    for (label, src) in [
        ("instr then node", "instr bass(freq: hz) { osc(sine, freq) * 0.2 }\nnode bass { instrument = bass }\n"),
        ("node then instr", "node bass { instrument = bass }\ninstr bass(freq: hz) { osc(sine, freq) * 0.2 }\n"),
    ] {
        let c = niminal_lang::compile_with(src, &Default::default());
        println!("{label}: {:?}", c.map(|_| ()).map_err(|e| e.into_iter().map(|d| d.message).collect::<Vec<_>>()));
    }
}
