use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap())
}

fn close(a: &[f32], b: &[f32]) {
    assert_eq!(a.len(), b.len());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert!((x - y).abs() < 1e-5, "sample {i}: {x} vs {y}");
    }
}

#[test]
fn a_send_reaches_a_track_that_reads_the_bus() {
    let dry = raw("instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }\ntone(f: 220hz) for 1beat");
    let wet = raw("
bus space
instr tone(f: hz) {
  out = osc(sine, f) * env[1 1sec 1]
  space += out
}
track room { out = space }
tone(f: 220hz) for 1beat");
    // dry plus an identical copy through the bus
    let doubled: Vec<f32> = dry.iter().map(|s| s * 2.0).collect();
    close(&wet, &doubled);
}

#[test]
fn declaration_order_makes_no_difference_and_adds_no_latency() {
    let a = raw("
bus space
instr tone(f: hz) { out = osc(sine, f) * env[1 1sec 1]\nspace += out }
track room { out = space.gain(0.5) }
tone(f: 220hz) for 1beat");
    let b = raw("
track room { out = space.gain(0.5) }
instr tone(f: hz) { out = osc(sine, f) * env[1 1sec 1]\nspace += out }
bus space
tone(f: 220hz) for 1beat");
    close(&a, &b);
    // the reader hears the send in the same block: 1.5 x the dry signal from sample 0
    let dry = raw("instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }\ntone(f: 220hz) for 1beat");
    let scaled: Vec<f32> = dry.iter().map(|s| s * 1.5).collect();
    close(&a, &scaled);
}

#[test]
fn a_track_chain_processes_its_instrument() {
    let dry = raw("instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }\ntone(f: 220hz) for 1beat");
    let through = raw("
instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }
track lead { instrument = tone\nout = it.gain(0.25) }
lead(f: 220hz) for 1beat");
    let quarter: Vec<f32> = dry.iter().map(|s| s * 0.25).collect();
    close(&through, &quarter);
}

#[test]
fn routing_a_track_to_a_bus_moves_its_sound() {
    let direct = raw("
instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }
track lead { instrument = tone\nout = it.gain(0.5) }
lead(f: 220hz) for 1beat");
    let via_bus = raw("
bus mix
instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }
track lead { instrument = tone\nout = it.gain(0.5).to(mix) }
track out_track { out = mix }
lead(f: 220hz) for 1beat");
    close(&direct, &via_bus);
}

#[test]
fn track_defaults_apply_and_notes_override_them() {
    let src = |track_arg: &str, note_arg: &str| {
        format!(
            "instr tone(f: hz, amp: db = 0db) {{ osc(sine, f).gain(amp) * env[1 1sec 1] }}
track lead {{ instrument = tone({track_arg}) }}
lead({note_arg}) for 1beat"
        )
    };
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let loud = peak(&raw(&src("f: 220hz", "f: 220hz")));
    let quiet_default = peak(&raw(&src("f: 220hz, amp: -12db", "f: 220hz")));
    let overridden = peak(&raw(&src("f: 220hz, amp: -12db", "f: 220hz, amp: 0db")));
    assert!((quiet_default - loud * 0.2512).abs() < 0.01, "{quiet_default} vs {loud}");
    assert!((overridden - loud).abs() < 1e-4);
}

#[test]
fn a_chains_filter_state_persists_across_notes() {
    // One track-level filter serves both notes. When the second note
    // starts, the filter still holds charge from the first, so its output
    // starts well above where the first note's did.
    let src = "
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}
instr dc(f: hz) { 1.0 * env[1 | 1ms 0] }
track lead { instrument = dc\nout = it.one_pole(cutoff: 2hz) }
lead(f: 1hz) for 1/8beat
at 1/4beat lead(f: 1hz) for 1/8beat
";
    let out = raw(src);
    let second_start = (0.125 * SR) as usize; // 1/4 beat at 120bpm
    assert!(out[1] < 0.01, "the filter starts empty: {}", out[1]);
    assert!(out[second_start + 1] > 0.1, "and still holds charge later: {}", out[second_start + 1]);
}

#[test]
fn feedback_is_rejected_when_compiling() {
    let errs = compile("bus x\nbus y\ntrack a { out = x.to(y) }\ntrack b { out = y.to(x) }").err().unwrap();
    assert!(errs[0].message.starts_with("tracks feed back through buses"));
}
