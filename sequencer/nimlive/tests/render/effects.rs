use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap())
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

// A short click sent only to a bus, which a node delays.
const ECHO: &str = "\ntempo 120bpm\nbus space\ninstr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }\nprocessor fx { out = space.delay(time: 1/4beat, feedback: 0.5) }\nclick() for 1/8beat\n";

#[test]
fn echoes_land_on_the_beat_and_decay() {
    let out = raw(ECHO);
    let window = |n: usize| peak(&out[n..n + 200]);
    // 1/4beat at 120bpm is 6000 samples
    let (first, second, third) = (window(6000), window(12_000), window(18_000));
    assert!(first > 0.5, "{first}");
    assert!((second / first - 0.5).abs() < 0.05, "{second} vs {first}");
    assert!((third / first - 0.25).abs() < 0.05);
    assert!(peak(&out[..6000]) < 1e-6, "silent before the first echo: the instrument itself makes no sound");
    assert!(peak(&out[2000..5900]) < 1e-6);
}

#[test]
fn rendering_waits_for_the_tail_and_trims_the_silence() {
    let out = raw(ECHO);
    // 0.5^n falls below -90db after about 15 echoes of 125ms
    assert!(out.len() > (1.5 * SR) as usize, "{}", out.len() as f64 / SR);
    assert!(out.len() < (3.0 * SR) as usize);
    assert!(out.last().unwrap().abs() >= 3.1e-5, "ends on the last audible sample");
}

#[test]
fn a_reverb_node_rings_after_the_notes_stop() {
    let out = raw("\nbus space\ninstr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }\nprocessor hall { out = space.reverb(room: 0.8, damp: 0.5) }\nclick() for 1/8beat");
    assert!(out.len() > SR as usize, "a reverb tail is longer than a second: {}", out.len() as f64 / SR);
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(peak(&out) > 0.001);
}

#[test]
fn a_delay_time_given_as_a_parameter_matches_the_same_literal_time() {
    let with_param = raw("\ninstr echo(t: sec) { out = (osc(saw, 220hz) * env[1 | 10ms 0]).delay(time: t, max: 1sec, feedback: 0.3) }\necho(t: 100ms) for 1beat");
    let with_literal = raw("\ninstr echo() { out = (osc(saw, 220hz) * env[1 | 10ms 0]).delay(time: 100ms, feedback: 0.3) }\necho() for 1beat");
    assert!(peak(&with_param) > 0.1);
    assert_eq!(with_param, with_literal);
}

#[test]
fn a_reverb_inside_an_instrument_rings_out_as_it_does_on_a_node() {
    let in_instrument = raw("\ninstr click() { out = (osc(sine, 1000hz) * env[1 0.5ms 0]).reverb(room: 0.8, damp: 0.5) }\nclick() for 1/8beat");
    let on_node = raw("\nbus space\ninstr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }\nprocessor hall { out = space.reverb(room: 0.8, damp: 0.5) }\nclick() for 1/8beat");
    assert!(in_instrument.len() > SR as usize, "the voice outlived its note: {}", in_instrument.len());
    assert_eq!(in_instrument.len(), on_node.len());
    let worst = in_instrument.iter().zip(&on_node).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(worst < 1e-6, "largest difference {worst}");
}

#[test]
fn a_delay_inside_an_instrument_is_not_cut_off_at_note_end() {
    let out = raw("\ntempo 120bpm\ninstr click() { out = (osc(sine, 1000hz) * env[1 0.5ms 0]).delay(time: 1/2beat, feedback: 0.5) }\nclick() for 1/16beat");
    // the note ends after 1/16beat (~31ms); echoes at 250ms intervals still sound
    let at = |n: usize| peak(&out[n..n + 200]);
    assert!(at(12_000) > 0.5, "first echo {}", at(12_000));
    assert!(at(24_000) > 0.2, "second echo {}", at(24_000));
}

#[test]
fn the_example_with_effects_renders() {
    let out = raw(include_str!("../../../../examples/room.nml"));
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(rms(&out) > 0.01);
}

#[test]
fn noise_and_pinknoise_make_sound() {
    let white = raw("instr n() { noise() * env[1 20ms 0] * 0.5 }\nn() for 1/8beat");
    let pink = raw("instr n() { pinknoise() * env[1 20ms 0] * 0.5 }\nn() for 1/8beat");
    assert!(peak(&white) > 0.1, "white {}", peak(&white));
    assert!(peak(&pink) > 0.01, "pink {}", peak(&pink));
    assert!(white.iter().chain(pink.iter()).all(|s| s.is_finite()));
}

#[test]
fn fold_clip_chorus_bitcrush_and_compress_render() {
    let out = raw("\ninstr tone() {\n  s = osc(saw, 110hz) * env[1 | 80ms 0]\n  s = s.wavefold(drive: 3) * 0.3\n  s = s.clip(level: 0.8)\n  s = s.chorus(rate: 0.8hz, depth: 0.4, time: 8ms, mix: 0.4)\n  s = s.bitcrush(rate: 8khz, bits: 8)\n  s.compress(threshold: -12db, ratio: 4, attack: 5ms, release: 40ms, makeup: 3db)\n}\ntone() for 1beat");
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(peak(&out) > 0.05, "{}", peak(&out));
}

#[test]
fn compress_with_a_sidechain_ducks_the_signal() {
    // a node ducks a pad bus when a click hits the key bus
    let ducked = raw("\nbus pad\nbus key\ninstr tone() { pad += osc(sine, 220hz) * env[1 200ms 1] }\ninstr click() { key += osc(sine, 1000hz) * env[1 2ms 0] }\nprocessor ducked {\n  out = pad.compress(threshold: -24db, ratio: 20, attack: 1ms, release: 50ms, sidechain: key)\n}\ntone() for 1beat\nat 50ms click() for 10ms");
    assert!(ducked.iter().all(|s| s.is_finite()));
    assert!(peak(&ducked) > 0.01);
    // around the click the pad should be quieter than before it
    let before = peak(&ducked[1000..2000]);
    let during = peak(&ducked[2400..2800]);
    assert!(during < before * 0.7, "before {before} during {during}");
}

#[test]
fn the_synth_drums_example_renders() {
    let out = render_with(
        &program(include_str!("../../../../examples/synth_drums.nml")),
        &[],
        &RenderOptions { limiter: false, until: None, ..Default::default() },
    ).unwrap();
    assert_eq!(out.channels.len(), 2);
    let mix: Vec<f32> = out.channels[0].iter().zip(&out.channels[1]).map(|(l, r)| (l + r) * 0.5).collect();
    assert!(mix.iter().all(|s| s.is_finite()));
    assert!(rms(&mix) > 0.01);
}
