use crate::common::*;

fn raw(level: &str) -> Vec<f32> {
    let src = format!(
        "tempo 120bpm\ninstr a(freq: hz) {{ osc(sine, freq) * {level} }}\na(freq: 220hz) for 1beat"
    );
    mono(
        render_with(
            &program(&src),
            &[],
            &RenderOptions {
                limiter: false,
                until: None,
                ..Default::default()
            },
        )
        .unwrap(),
    )
}

fn errors(level: &str) -> String {
    let src = format!("instr a(freq: hz) {{ osc(sine, freq) * {level} }}");
    compile(&src)
        .err()
        .map(|e| e.iter().map(|d| d.render("test", &src)).collect())
        .expect("an error")
}

#[test]
fn adsr_and_perc_are_the_envelopes_they_stand_for() {
    assert_eq!(
        raw("adsr(attack: 5ms, decay: 120ms, sustain: 0.4, release: 600ms)"),
        raw("env[0 5ms 1 120ms 0.4 | 600ms 0]")
    );
    assert_eq!(
        raw("adsr(5ms, 120ms, 0.4, 600ms)"),
        raw("env[0 5ms 1 120ms 0.4 | 600ms 0]")
    );
    // a bare value after a named one still fills the next positional slot (attack), as opcodes do
    assert_eq!(
        raw("adsr(decay: 120ms, 5ms, sustain: 0.4, release: 600ms)"),
        raw("env[0 5ms 1 120ms 0.4 | 600ms 0]")
    );
    assert_eq!(raw("perc(decay: 400ms)"), raw("env[0 2ms 1 400ms.exp 0]"));
    assert_eq!(
        raw("perc(attack: 10ms, decay: 1/4beat)"),
        raw("env[0 10ms 1 1/4beat.exp 0]")
    );
}

#[test]
fn shortcut_mistakes_are_reported() {
    assert!(
        errors("adsr(attack: 5ms, decay: 120ms, sustain: 0.4)").contains("`adsr` needs `release`")
    );
    assert!(errors("adsr(attack: 5ms, atack: 1ms)").contains("`adsr` has no `atack`"));
    assert!(errors("perc()").contains("`perc` needs `decay`"));
    assert!(errors("perc(decay: 1ms, decay: 2ms)").contains("`decay` is given twice"));
    assert!(errors("adsr(5ms, 120ms, 0.4hz, 600ms)").contains("same unit"));
}

#[test]
fn a_user_opcode_with_the_same_name_wins() {
    // if the shortcut won, `adsr(1)` would need four envelope parameters
    let src = "fn adsr(x) { x }\ninstr a(freq: hz) { osc(sine, freq) * adsr(1) }\na(freq: 220hz) for 1beat";
    assert!(compile(src).is_ok(), "{:?}", compile(src).err());
}
