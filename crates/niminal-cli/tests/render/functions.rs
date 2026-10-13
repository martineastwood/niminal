//! Functions, lists, rates and the instrument features of the language spec, heard.

use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap())
}

fn near(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

fn error(src: &str) -> String {
    compile(src).err().map(|e| e.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// The period of `x` in samples, by autocorrelation.
fn period(x: &[f32]) -> usize {
    (20..1500)
        .max_by(|&a, &b| {
            let c = |lag: usize| x.iter().zip(&x[lag..]).map(|(p, q)| p * q).sum::<f32>();
            c(a).partial_cmp(&c(b)).unwrap()
        })
        .unwrap()
}

#[test]
fn a_function_without_a_type_takes_the_unit_it_is_given() {
    let generic = raw("fn twice(x) { x * 2 }\ninstr a(f: hz) { osc(saw, f.twice) * 0.5.twice }\na(f: 110hz) for 1beat");
    let by_hand = raw("instr a(f: hz) { osc(saw, f * 2) * 1 }\na(f: 110hz) for 1beat");
    assert!(near(&generic, &by_hand));
    // a declared result unit is checked
    assert!(error("fn f(x) -> hz { x }\ninstr a() { osc(saw, 1hz.f) * f(2) }").contains("declared as hz"));
}

#[test]
fn functions_can_be_defined_in_any_order_but_not_call_themselves() {
    let later = raw("instr a(f: hz) { osc(saw, f).soft }\nfn soft(x) { x.dull * 0.5 }\nfn dull(x) { x.lpf(800hz) }\na(f: 110hz) for 1beat");
    let by_hand = raw("instr a(f: hz) { osc(saw, f).lpf(800hz) * 0.5 }\na(f: 110hz) for 1beat");
    assert!(near(&later, &by_hand));
    assert!(error("fn a(x) { x.b }\nfn b(x) { x.a }\ninstr i(f: hz) { osc(saw, f).a }").contains("can't call themselves"));
}

#[test]
fn a_statement_if_changes_state_only_when_it_holds() {
    // sample and hold: the output only changes when the trigger crosses 0 going up, so it is a staircase
    let held = raw(
        "fn hold(x, trig) {\n state h = 0\n if trig > 0 and prev(trig) <= 0 { h = x }\n h\n}\ninstr a() { osc(saw, 3hz).hold(osc(square, 20hz)) * env[1 1sec 1] }\na() for 1beat",
    );
    let changes = held[..(0.5 * SR) as usize].windows(2).filter(|w| (w[0] - w[1]).abs() > 1e-6).count();
    assert!((9..=11).contains(&changes), "about 10 steps in half a second: {changes}");
}

#[test]
fn a_statement_for_layers_into_out() {
    let organ = raw("instr organ(f: hz) {\n for n in 1..3 { out += osc(sine, f * n) / n }\n}\norgan(f: 110hz) for 1beat");
    let by_hand = raw("instr organ(f: hz) { osc(sine, f) + osc(sine, f * 2) / 2 + osc(sine, f * 3) / 3 }\norgan(f: 110hz) for 1beat");
    assert!(near(&organ, &by_hand), "`1..3` includes 3");
}

const SVF: &str = "fn svf(x, cutoff: hz, res: 0..1 = 0.5) -> (low, band, high) {
  state low = 0
  state band = 0
  f = 2 * sin(pi * cutoff / sample_rate)
  high = x - low - 2 * (1 - res) * band
  band = band + f * high
  low = low + f * band
}
fn filter(x, cutoff: hz, mode: (low | band | high) = low) {
  f = x.svf(cutoff: cutoff)
  if mode == low { f.low } else if mode == band { f.band } else { f.high }
}
";

#[test]
fn a_choice_picks_its_branch_while_compiling() {
    let play = |body: &str| raw(&format!("{SVF}instr a() {{ {body} * env[1 1sec 1] }}\na() for 1beat"));
    let band = play("osc(saw, 220hz).filter(1khz, mode: band)");
    assert!(near(&band, &play("osc(saw, 220hz).svf(cutoff: 1khz).band")));
    assert!(near(&play("osc(saw, 220hz).filter(1khz)"), &play("osc(saw, 220hz).svf(cutoff: 1khz).low")));
    assert!(!near(&band, &play("osc(saw, 220hz).filter(1khz, mode: high)")));
    assert!(error(&format!("{SVF}instr a() {{ osc(saw, 220hz).filter(1khz, mode: notch) }}")).contains("one of low, band, high"));
}

#[test]
fn a_state_list_makes_a_plucked_string() {
    let string = "fn string(x, freq: init hz, decay: 0..1 = 0.996, size: const int = 4096) {
  state buf = zeros(size)
  state pos = 0
  len = sample_rate / freq
  delayed = buf.at(pos - len, interp: linear)
  y = x + (delayed + prev(delayed)) * 0.5 * decay
  buf[pos] = y
  pos = (pos + 1).mod(size)
  y
}
fn noise() {
  state s = 12345
  s = (s * 16807).mod(2147483647)
  s / 2147483647 * 2 - 1
}
";
    let out = raw(&format!("{string}instr pluck(freq: init hz) with(tail: 2sec) {{ (noise() * env[1 5ms 0]).string(freq) }}\npluck(freq: 220hz) for 1beat"));
    let at = |t: f64| (t * SR) as usize;
    assert!(out.len() > at(2.0), "the tail keeps it ringing after the note: {}", out.len());
    let p = period(&out[at(0.3)..at(0.3) + 4096]);
    assert!((216..=222).contains(&p), "a period of about 218 samples is 220hz: {p}");
    assert!(rms(&out[at(1.5)..at(1.6)]) < rms(&out[at(0.2)..at(0.3)]) / 3.0, "it dies away");
}

#[test]
fn an_fir_filter_reads_a_constant_list_and_writes_its_history() {
    let fir = "fn fir(x, taps: [num]) {
  state hist = zeros(taps.len)
  state pos = 0
  hist[pos] = x
  acc = 0
  for i in taps.indices { acc = acc + taps[i] * hist[pos - i] }
  pos = (pos + 1).mod(taps.len)
  acc
}
const taps = [0.25, 0.5, 0.25]
";
    let filtered = raw(&format!("{fir}instr a() {{ osc(saw, 300hz).fir(taps) * env[1 1sec 1] }}\na() for 1beat"));
    let by_hand = raw("instr a() { x = osc(saw, 300hz)\n (x * 0.25 + prev(x) * 0.5 + prev(prev(x)) * 0.25) * env[1 1sec 1] }\na() for 1beat");
    assert!(near(&filtered, &by_hand));
}

#[test]
fn a_table_is_read_between_its_entries() {
    let wavetable = raw(
        "const sine = harmonics([1], size: 512)\nfn phasor(freq: hz) {\n state p = 0\n p = (p + freq / sample_rate).fract\n p\n}\ninstr a() { sine.at(phasor(200hz) * sine.len) * env[1 1sec 1] }\na() for 1beat",
    );
    let sine = raw("instr a() { osc(sine, 200hz) * env[1 1sec 1] }\na() for 1beat");
    assert_eq!(period(&wavetable[1000..5000]), period(&sine[1000..5000]));
    assert!((rms(&wavetable) - rms(&sine)).abs() < 0.01, "{} vs {}", rms(&wavetable), rms(&sine));
}

#[test]
fn an_index_that_changes_switches_between_entries() {
    let stepped = raw("instr a() { osc(sine, [200hz, 400hz][note.time.num * 4]) * env[1 1sec 1] }\na() for 1beat");
    let at = |t: f64| (t * SR) as usize;
    assert_eq!(period(&stepped[at(0.05)..at(0.05) + 4000]), 240, "200hz first");
    assert_eq!(period(&stepped[at(0.3)..at(0.3) + 4000]), 120, "400hz a quarter second on");
}

#[test]
fn init_holds_a_value_from_note_start() {
    // a rising ramp, frozen at its first value, in an instrument and in a function with state
    let frozen = raw("instr a() { (env[0.5 1sec 1]).init * env[1 1sec 1] }\na() for 1beat");
    assert!(frozen[100..20_000].iter().all(|v| (v - 0.5).abs() < 1e-6));
    let in_fn = raw("fn f(x: init) {\n state s = 0\n s = x\n s\n}\ninstr a() { env[0.5 1sec 1].f * env[1 1sec 1] }\na() for 1beat");
    assert!(in_fn[100..20_000].iter().all(|v| (v - 0.5).abs() < 1e-6));
    // a parameter declared `init` doesn't follow the note's later changes either
    let p = program("instr a(freq: init hz) { osc(sine, freq) * env[1 1sec 1] }\na(freq: 220hz) for 1beat");
    assert!(rms(&mono(render_with(&p, &[], &RenderOptions::default()).unwrap())) > 0.1);
}

#[test]
fn control_holds_a_value_through_each_block() {
    let stepped = raw("instr a() { env[0 1sec 1].control * env[1 1sec 1] }\na() for 1beat");
    // within a block of 32 the value doesn't change; between blocks it does
    for block in stepped[..9600].chunks(32) {
        assert!(block.iter().all(|v| (v - block[0]).abs() < 1e-6));
    }
    assert!(stepped[64] > stepped[32]);
}

#[test]
fn envelope_steps_can_follow_parameters() {
    let pad = |attack: &str| raw(&format!("instr pad(attack: sec = 400ms) {{ env[0 attack 1 | 10ms 0] }}\npad(attack: {attack}) for 1beat"));
    let at = |t: f64| (t * SR) as usize;
    assert!((pad("100ms")[at(0.05)] - 0.5).abs() < 0.01, "halfway up after half the attack");
    assert!((pad("400ms")[at(0.2)] - 0.5).abs() < 0.01);
    // adsr takes them too
    let a = raw("instr a(r: sec = 300ms) { adsr(10ms, 50ms, 0.5, r) }\na() for 1beat");
    assert!((a[at(0.2)] - 0.5).abs() < 0.01);
}

#[test]
fn note_time_and_released_describe_the_voice() {
    let time = raw("instr a() with(tail: 100ms) { note.time.num * env[1 1sec 1] }\na() for 1beat");
    let at = |t: f64| (t * SR) as usize;
    assert!((time[at(0.25)] - 0.25).abs() < 1e-3);
    let released = raw("instr a() with(tail: 200ms) { (note.released + 0.5) * 0.5 }\na() for 1beat");
    assert!((released[at(0.25)] - 0.25).abs() < 1e-6, "held");
    assert!((released[at(0.6)] - 0.75).abs() < 1e-6, "released at half a second");
    assert!(error("fn f(x) { x * note.time.num }").contains("only available in an instrument"));
}

#[test]
fn an_instrument_plays_another_inside_it() {
    let layered = raw("instr click(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\ninstr two(freq: hz) { click(freq: freq) + click(freq: freq * 1.5, gain: -6db) }\ntwo(freq: 220hz) for 1beat");
    let by_hand = raw("instr two(freq: hz) { e = env[1 | 10ms 0]\n osc(sine, freq) * e + osc(sine, freq * 1.5) * e * -6db }\ntwo(freq: 220hz) for 1beat");
    assert!(near(&layered, &by_hand));
}

#[test]
fn an_instrument_can_take_the_notes_gain_over() {
    let unused = |gain: &str| raw(&format!("instr a(gain: db = 0db) {{ osc(sine, 220hz) * env[1 | 10ms 0] }}\na(gain: {gain}) for 1beat"));
    let (quiet, loud) = (unused("-12db"), unused("0db"));
    // the instrument didn't apply it, so both are as loud
    assert!((rms(&quiet) - rms(&loud)).abs() < 1e-6);
    let applied = raw("instr a(gain: db = 0db) { osc(sine, 220hz).gain(gain) * env[1 | 10ms 0] }\na(gain: -12db) for 1beat");
    assert!((rms(&applied) / rms(&loud) - 0.251).abs() < 0.01);
}

#[test]
fn feedback_in_an_instrument_is_one_sample_long() {
    let inline = raw("instr a() { x = env[1 | 10ms 0] * 0.25\n y = x + prev(y) * 0.5\n y }\na() for 1beat");
    let in_fn = raw("fn loop(x) { y = x + prev(y) * 0.5\n y }\ninstr a() { (env[1 | 10ms 0] * 0.25).loop }\na() for 1beat");
    assert!(near(&inline, &in_fn));
    assert!((inline[1000] - 0.5).abs() < 1e-4, "settles at twice the input");
    // and `prev` of a name already defined is that name a sample ago
    let late = raw("instr a() { x = env[0 100ms 1 | 10ms 0]\n prev(x) * 0.5 }\na() for 1beat");
    let now = raw("instr a() { x = env[0 100ms 1 | 10ms 0]\n x * 0.5 }\na() for 1beat");
    for i in 100..400 {
        assert!((late[i] - now[i - 1]).abs() < 1e-6, "sample {i}");
    }
}

#[test]
fn constants_are_worked_out_while_compiling() {
    let a4 = raw("fn mtof(n: num) -> hz { 440hz * pow(2, (n - 69) / 12) }\nconst a4 = mtof(69)\ninstr a() { osc(sine, a4) * env[1 1sec 1] }\na() for 1beat");
    let by_hand = raw("instr a() { osc(sine, 440hz) * env[1 1sec 1] }\na() for 1beat");
    assert!(near(&a4, &by_hand));
    assert!(error("const c = osc(sine, 1hz)").contains("known while compiling"));
    assert!(error("fn f(x) {\n state s = 0\n s\n}\nconst c = f(1)").contains("runs while playing"));
}

#[test]
fn ranges_include_their_end_unless_written_with_less_than() {
    let count = |range: &str| raw(&format!("instr a() {{ {range}.len.hz.num * 0.01 * env[1 1sec 1] }}\na() for 1beat"))[100];
    assert!((count("(1..8)") - 0.08).abs() < 1e-6);
    assert!((count("(0..<8)") - 0.08).abs() < 1e-6);
    assert!((count("[1, 2, 3, 4][1..2]") - 0.02).abs() < 1e-6, "a slice");
    assert!((raw("instr a() { 0.75.clamp(0..0.5) * env[1 1sec 1] }\na() for 1beat")[100] - 0.5).abs() < 1e-6);
}

#[test]
fn the_functions_example_plays() {
    let out = render_with(&program(include_str!("../../../../examples/functions.nml")), &[], &RenderOptions::default()).unwrap();
    assert_eq!(out.channels.len(), 2);
    assert!(out.channels.iter().all(|c| rms(c) > 0.01 && c.iter().all(|s| s.is_finite())));
}
