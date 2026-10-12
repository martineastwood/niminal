use crate::common::*;

const LIB: &str = "
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}

opcode drive(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 9)).tanh
}

opcode dc_block(x) {
  state x1 = 0.0
  state y1 = 0.0
  y = x - x1 + 0.995 * y1
  x1 = x
  y1 = y
  y
}
";

fn raw(src: &str) -> Vec<f32> {
    let p = program(&format!("{LIB}{src}"));
    mono(render_with(&p, &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap())
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// What `body` makes of the constant `x`, with `x` fed in as a plain number for a while.
fn through(body: &str, x: f64) -> f32 {
    let out = raw(&format!("opcode t(x) {{ {body} }}\ninstr a() {{ v = {x} * env[1 1sec 1]\n v.t }}\na() for 1beat"));
    // silence at the end of a render is trimmed, so a note of zeros comes back empty
    out.get(4800).copied().unwrap_or(0.0)
}

fn close(a: f32, b: f64) -> bool {
    (f64::from(a) - b).abs() < 1e-5
}

#[test]
fn if_picks_one_of_two_values_and_else_if_chains() {
    let pick = "if x > 0.5 { 1 } else { 0.25 }";
    assert!(close(through(pick, 0.8), 1.0) && close(through(pick, 0.3), 0.25));
    let chain = "if x > 0.8 { 3 } else if x > 0.4 { 2 } else { 1 }";
    assert!(close(through(chain, 0.9), 3.0) && close(through(chain, 0.5), 2.0) && close(through(chain, 0.1), 1.0));
    // the branches can be written over several lines
    let long = "if x >= 0.5 {\n 2 * x\n} else {\n -x\n}";
    assert!(close(through(long, 0.75), 1.5) && close(through(long, 0.25), -0.25));
}

#[test]
fn comparisons_give_one_or_zero_and_combine_by_multiplying() {
    for (body, x, want) in [
        ("x < 0.5", 0.25, 1.0), ("x < 0.5", 0.75, 0.0), ("x <= 0.5", 0.5, 1.0), ("x >= 0.5", 0.25, 0.0),
        ("x == 0.5", 0.5, 1.0), ("x != 0.5", 0.5, 0.0), ("(x > 0.2) * (x < 0.6)", 0.4, 1.0), ("(x > 0.2) * (x < 0.6)", 0.8, 0.0),
    ] {
        assert!(close(through(body, x), want), "{body} with {x}");
    }
    // a state can be changed by a decision, as in a peak follower
    let held = raw("opcode hold(x) {\n state m = 0.0\n m = if x > m { x } else { m * 0.999 }\n m\n}\ninstr a() { (osc(sine, 100hz) * env[1 1sec 1]).hold }\na() for 1beat");
    let lowest = held[10_000..20_000].iter().fold(f32::MAX, |m, v| m.min(*v));
    assert!(lowest > 0.5, "the follower never falls back to the sine's zero crossings: {lowest}");
}

#[test]
fn the_extra_math_functions() {
    for (body, x, want) in [
        ("min(x, 0.5)", 0.8, 0.5), ("max(x, 0.5)", 0.2, 0.5), ("clamp(x, 0.2, 0.6)", 0.9, 0.6), ("clamp(x, 0.2, 0.6)", 0.1, 0.2),
        ("clamp(x, 0.2, 0.6)", 0.4, 0.4), ("pow(x, 2)", 3.0, 9.0), ("floor(x)", 2.75, 2.0), ("ceil(x)", 2.25, 3.0),
        ("sqrt(x)", 0.25, 0.5), ("log(x)", std::f64::consts::E, 1.0), ("sign(x)", -4.0, -1.0), ("fract(x)", 2.75, 0.75),
        ("tan(x)", 0.5, 0.5f64.tan()),
    ] {
        assert!(close(through(body, x), want), "{body} with {x}: {}", through(body, x));
    }
}

#[test]
fn the_new_functions_are_ordinary_instrument_opcodes_too() {
    let floor = raw("instr a() { (2.75 * env[1 1sec 1]).floor }\na() for 1beat");
    assert!(close(floor[4800], 2.0), "{}", floor[4800]);
}

#[test]
fn opcode_branching_mistakes_are_reported() {
    let error = |src: &str| compile(src).err().map(|e| e.iter().map(|d| d.message.clone() + &d.help.clone().unwrap_or_default()).collect::<String>()).unwrap_or_default();
    assert!(error("opcode t(x) { if x > 0 { 1 } }").contains("needs an `else`"));
    assert!(error("opcode t(x, f: hz) { if x > 0 { f } else { 1 } }").contains("same unit"));
    assert!(error("opcode t(x, f: hz) { if f { 1 } else { 0 } }").contains("plain number to decide on"));
    assert!(error("opcode t(x) { x < 1 < 2 }").contains("don't chain"));
    assert!(error("opcode t(x, f: hz) { x < f }").contains("can't compare"));
    assert!(error("opcode t(x) { min(x) }").contains("takes 2 values"));
    assert!(error("opcode t(x, f: hz) { pow(f, f) }").contains("plain numbers"));
    assert!(error("opcode t(x, f: hz) { min(x, f) }").contains("same unit"));
    assert!(error("instr a(f: hz) { osc(sine, f) * if f { 1 } else { 0 } }").contains("plain number to decide on"));
    assert!(error("instr a(f: hz) { osc(sine, f) * if f > 1 { 1 } else { 0 } }").contains("can't compare"));
    assert!(error("instr a(f: hz) { osc(sine, f) * if f > 1hz { 1 } else { 1hz } }").contains("same unit"));
    assert!(error("opcode t(x, f: hz) { x % f }").contains("can't take the remainder of"));
    assert!(error("instr a(f: hz) { osc(sine, f) * (3 % f) }").contains("can't take the remainder of"));
    assert!(error("opcode t(x) { sinn(x) }").contains("did you mean `sin`?"));
}

const SPLIT: &str = "
opcode split(x, cutoff: hz) -> (low, high) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  low = y
  high = x - y
}
";

#[test]
fn an_opcode_with_several_outputs_gives_each_as_a_field() {
    let play = |body: &str| raw(&format!("{SPLIT}instr a() {{ {body} }}\na() for 1beat"));
    let whole = play("osc(saw, 300hz)");
    let summed = play("f = osc(saw, 300hz).split(cutoff: 500hz)\n f.low + f.high");
    assert_eq!(whole.len(), summed.len());
    assert!(whole.iter().zip(&summed).all(|(a, b)| (a - b).abs() < 1e-5), "the two parts add back up to the input");
    // a 2khz sine is nearly all in the high part
    let (low, high) = (play("osc(sine, 2000hz).split(cutoff: 500hz).low"), play("osc(sine, 2000hz).split(cutoff: 500hz).high"));
    assert!(rms(&low) < rms(&high) / 3.0, "{} vs {}", rms(&low), rms(&high));
    // used on its own, the first result stands for it
    assert_eq!(play("osc(sine, 2000hz).split(cutoff: 500hz)"), low);
}

#[test]
fn an_opcode_can_use_the_several_results_of_another() {
    let nested = raw(&format!(
        "{SPLIT}opcode band(x, lo: hz, hi: hz) {{ top = x.split(cutoff: hi)\n mid = top.low.split(cutoff: lo)\n mid.high }}\ninstr a() {{ osc(saw, 300hz).band(lo: 200hz, hi: 900hz) }}\na() for 1beat"
    ));
    let by_hand = raw(&format!(
        "{SPLIT}instr a() {{ top = osc(saw, 300hz).split(cutoff: 900hz)\n mid = top.low.split(cutoff: 200hz)\n mid.high }}\na() for 1beat"
    ));
    assert!(rms(&nested) > 0.01);
    assert!(nested.iter().zip(&by_hand).all(|(a, b)| (a - b).abs() < 1e-5), "the same as writing it out");
    // a call on its own inside an opcode gives its first result, as in an instrument
    let first = raw(&format!("{SPLIT}opcode u(x) {{ x.split(cutoff: 500hz) }}\ninstr a() {{ osc(saw, 300hz).u }}\na() for 1beat"));
    let direct = raw(&format!("{SPLIT}instr a() {{ osc(saw, 300hz).split(cutoff: 500hz).low }}\na() for 1beat"));
    assert!(first.iter().zip(&direct).all(|(a, b)| (a - b).abs() < 1e-6));
}

#[test]
fn an_opcode_can_use_built_in_opcodes() {
    let near = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
    // a filter, and the arguments it is given, inside an opcode
    let wrapped = raw("opcode dull(x, c: hz) { x.lpf(cutoff: c * 2, res: 0.3) * 0.5 }\ninstr a() { osc(saw, 220hz).dull(c: 1khz) }\na() for 1beat");
    let by_hand = raw("instr a() { osc(saw, 220hz).lpf(cutoff: 1khz * 2, res: 0.3) * 0.5 }\na() for 1beat");
    assert!(rms(&wrapped) > 0.01);
    assert!(near(&wrapped, &by_hand), "the same as writing it out");
    // an oscillator, with a waveform, as an LFO that moves the filter
    let lfo = raw("opcode wah(x) { x.lpf(cutoff: 1khz + osc(sine, 3hz) * 800hz) }\ninstr a() { osc(saw, 220hz).wah }\na() for 1beat");
    let lfo_by_hand = raw("instr a() { osc(saw, 220hz).lpf(cutoff: 1khz + osc(sine, 3hz) * 800hz) }\na() for 1beat");
    assert!(near(&lfo, &lfo_by_hand));
    // each use has its own state: two calls are two filters
    let twice = raw("opcode dull(x) { x.lpf(cutoff: 500hz) }\ninstr a() { osc(saw, 220hz).dull.dull }\na() for 1beat");
    let twice_by_hand = raw("instr a() { osc(saw, 220hz).lpf(cutoff: 500hz).lpf(cutoff: 500hz) }\na() for 1beat");
    assert!(near(&twice, &twice_by_hand));
    // a delay needs its longest time as a constant, as it does in an instrument
    let echo = raw("opcode echo(x) { x + x.delay(time: 100ms, feedback: 0.3, max: 200ms) * 0.5 }\ninstr a() { osc(sine, 440hz) * env[1 | 10ms 0] }\ninstr b() { osc(sine, 440hz) * env[1 | 10ms 0].echo }\nb() for 1beat");
    assert!(rms(&echo) > 0.001);
}

#[test]
fn several_outputs_keep_separate_state_per_channel_and_note() {
    let one = raw(&format!("{SPLIT}instr a(f: hz) {{ osc(saw, f).split(cutoff: 500hz).high * env[1 1sec 1] }}\na(f: 200hz) for 1beat"));
    let two = raw(&format!("{SPLIT}instr a(f: hz) {{ osc(saw, f).split(cutoff: 500hz).high * env[1 1sec 1] }}\na(f: 200hz) for 1beat\nat 100ms a(f: 330hz) for 1beat"));
    assert_eq!(&one[..4700], &two[..4700]);
}

#[test]
fn an_opcode_can_call_an_opcode_defined_before_it() {
    let nested = raw("opcode smooth(x, c: hz) { x.one_pole(cutoff: c).one_pole(cutoff: c) }\ninstr a() { osc(saw, 2000hz).smooth(c: 300hz) }\na() for 1beat");
    let by_hand = raw("instr a() { osc(saw, 2000hz).one_pole(cutoff: 300hz).one_pole(cutoff: 300hz) }\na() for 1beat");
    let near = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
    assert!(near(&nested, &by_hand), "each inlined copy has its own state");
    // defaults and later opcodes using earlier ones
    let driven = raw("opcode hot(x) { x.drive }\nopcode hotter(x) { x.hot.hot }\ninstr a() { osc(sine, 100hz).hotter }\na() for 1beat");
    let by_hand = raw("instr a() { osc(sine, 100hz).drive.drive }\na() for 1beat");
    assert!(near(&driven, &by_hand));
}

#[test]
fn opcode_composition_and_output_mistakes_are_reported() {
    let error = |src: &str| compile(src).err().map(|e| e.iter().map(|d| d.message.clone() + &d.help.clone().unwrap_or_default()).collect::<String>()).unwrap_or_default();
    assert!(error(&format!("{SPLIT}instr a() {{ osc(sine, 1hz).split(cutoff: 1hz).mid }}")).contains("no result `mid`its results are low, high"));
    assert!(error("opcode t(x) -> (a, b) { a = x }").contains("`b` is a result of `t`, but the body never defines it"));
    assert!(error("opcode t(x) -> (a) { a = x }").contains("at least two outputs"));
    assert!(error("opcode t(x) -> (a, a) { a = x }").contains("named twice"));
    assert!(error("instr t() -> (a, b) { 1 }").contains("only an opcode"));
    assert!(error("opcode t(x) -> (a, b) { a = x\n b = x\n x }").contains("never used"));
    assert!(error(&format!("{SPLIT}opcode u(x) {{ f = x.split(cutoff: 1hz)\n f.mid }}")).contains("no result `mid`"));
    assert!(error("opcode u(x) { x.nonsense(cutoff: 1khz) }").contains("can't be used inside an opcode"));
    assert!(error("opcode u(x) { x.lpf(cutoff: 1) }").contains("expects"));
    assert!(error("opcode u(x) { x.lpf }").contains("needs an argument `cutoff`"));
    assert!(error("opcode u(x) { to_layout(x, stereo) }").contains("can't be used inside an opcode"));
    assert!(error(&format!("{LIB}opcode u(x) {{ x.one_pole(cutoff: 0.5) }}")).contains("expects"));
    assert!(error(&format!("{LIB}opcode u(x) {{ x.one_pole }}")).contains("needs an argument `cutoff`"));
}

#[test]
fn an_instrument_can_decide_with_if_on_what_it_is_given() {
    let note = |freq: &str| raw(&format!("instr a(f: hz) {{ osc(sine, f) * (if f < 1khz {{ 1 }} else {{ 0.25 }}) }}\na(f: {freq}) for 1beat"));
    assert!(peak(&note("500hz")) > 0.95);
    let high = peak(&note("2000hz"));
    assert!((high - 0.25).abs() < 0.01, "{high}");
    // a decision between decibel values, as a bright note is turned down
    let db = raw("instr a(f: hz) { osc(sine, f).gain(if f < 1khz { 0db } else { -12db }) }\na(f: 2000hz) for 1beat");
    assert!((peak(&db) - 0.251).abs() < 0.01, "{}", peak(&db));
    // a decision known while compiling leaves nothing in the graph to run
    let folded = raw("instr a() { osc(sine, 440hz) * (if 2 > 1 { 0.5 } else { 0.1 }) }\na() for 1beat");
    assert!((peak(&folded) - 0.5).abs() < 0.01);
    // a changing condition, on a signal
    let gated = raw("instr a() { osc(sine, 100hz) * (if osc(sine, 100hz) > 0 { 1 } else { 0 }) }\na() for 1beat");
    assert!(gated.iter().all(|s| *s > -1e-6), "only the positive half is left");
}

#[test]
fn modulo_and_dividing_by_a_signal_in_instruments_and_opcodes() {
    let via = |expr: &str| raw(&format!("instr a() {{ v = 1.3 * env[1 1sec 1]\n {expr} }}\na() for 1beat"))[4800];
    assert!((via("v % 0.5") - 0.3).abs() < 1e-5);
    assert!((via("v % 1") - 0.3).abs() < 1e-5);
    assert!((via("1 / v") - 1.0 / 1.3).abs() < 1e-5);
    assert!((via("7 % 3 * v") - 1.3).abs() < 1e-5, "a constant remainder is worked out while compiling");
    assert!(close(through("x % 0.3", 1.0), 0.1));
    assert!(close(through("x % 3", -1.0), 2.0), "the remainder takes the sign of the divisor");
    assert!(close(through("x % 0", 1.0), 0.0), "and is 0 for a divisor of 0");
    assert!(close(through("x / 0", 1.0), 0.0));
    // a wrapping phase, as a sawtooth built by hand
    assert!(close(through("(x * 3) % 1", 0.5), 0.5));
}

#[test]
fn a_user_lowpass_filters() {
    let bright = raw("instr a() { osc(saw, 2000hz) }\na() for 1beat");
    let dull = raw("instr a() { osc(saw, 2000hz).one_pole(cutoff: 300hz) }\na() for 1beat");
    let mid = bright.len() / 2;
    let (b, d) = (rms(&bright[mid..mid + 9600]), rms(&dull[mid..mid + 9600]));
    assert!(d < b / 4.0, "300hz one-pole should take most of a 2khz saw away: {d} vs {b}");
}

#[test]
fn a_user_waveshaper_bounds_the_signal() {
    let loud = raw("instr a() { osc(sine, 100hz) * 20 }\na() for 1beat");
    let driven = raw("instr a() { (osc(sine, 100hz) * 20).drive(amount: 1) }\na() for 1beat");
    assert!(peak(&loud) > 15.0);
    assert!(peak(&driven) <= 1.0);
    assert!(peak(&driven) > 0.99, "hard-driven tanh nearly reaches 1");
}

#[test]
fn a_user_dc_blocker_removes_offset() {
    let offset = raw("instr a() { osc(sine, 440hz) * 0.3 + 0.5 }\na() for 2beats");
    let blocked = raw("instr a() { (osc(sine, 440hz) * 0.3 + 0.5).dc_block }\na() for 2beats");
    let mean = |x: &[f32]| x.iter().sum::<f32>() / x.len() as f32;
    let tail = |x: &Vec<f32>| x[x.len() / 2..].to_vec();
    assert!((mean(&tail(&offset)) - 0.5).abs() < 0.01);
    assert!(mean(&tail(&blocked)).abs() < 0.02, "{}", mean(&tail(&blocked)));
}

#[test]
fn state_belongs_to_the_voice_so_repeated_notes_sound_identical() {
    let out = raw("instr a() { osc(saw, 300hz).one_pole(cutoff: 500hz) * env[1 0.2sec 1] }\na() for 1/4beat\nat 1beat a() for 1/4beat");
    let second = (0.5 * SR) as usize; // 1 beat at 120bpm
    assert_eq!(&out[..4800], &out[second..second + 4800]);
}

#[test]
fn overlapping_notes_do_not_share_state() {
    let solo = raw("instr a(f: hz) { osc(saw, f).one_pole(cutoff: 500hz) * env[1 1sec 1] }\na(f: 200hz) for 1beat");
    let both = raw("instr a(f: hz) { osc(saw, f).one_pole(cutoff: 500hz) * env[1 1sec 1] }\na(f: 200hz) for 1beat\nat 100ms a(f: 330hz) for 1beat");
    // before the second note starts the output is identical
    assert_eq!(&solo[..4700], &both[..4700]);
    assert_ne!(&solo[5000..6000], &both[5000..6000]);
}
