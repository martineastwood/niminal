use crate::common::*;

fn raw(src: &str) -> Vec<f32> {
    mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap())
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

// A short click sent only to a bus, which a track delays.
const ECHO: &str = "
tempo 120bpm
bus space
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track fx { out = space.delay(time: 1/4beat, feedback: 0.5) }
click() for 1/8beat
";

#[test]
fn echoes_land_on_the_beat_and_decay() {
    let out = raw(ECHO);
    let window = |n: usize| peak(&out[n..n + 200]);
    // 1/4 beat at 120bpm is 6000 samples
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
fn a_reverb_track_rings_after_the_notes_stop() {
    let out = raw("
bus space
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
    assert!(out.len() > SR as usize, "a reverb tail is longer than a second: {}", out.len() as f64 / SR);
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(peak(&out) > 0.001);
}

#[test]
fn a_delay_time_given_as_a_parameter_matches_the_same_literal_time() {
    let with_param = raw("
instr echo(t: sec) { out = (osc(saw, 220hz) * env[1 | 10ms 0]).delay(time: t, max: 1sec, feedback: 0.3) }
echo(t: 100ms) for 1beat");
    let with_literal = raw("
instr echo() { out = (osc(saw, 220hz) * env[1 | 10ms 0]).delay(time: 100ms, feedback: 0.3) }
echo() for 1beat");
    assert!(peak(&with_param) > 0.1);
    assert_eq!(with_param, with_literal);
}

#[test]
fn a_reverb_inside_an_instrument_rings_out_as_it_does_on_a_track() {
    let in_instrument = raw("
instr click() { out = (osc(sine, 1000hz) * env[1 0.5ms 0]).reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
    let on_track = raw("
bus space
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
    assert!(in_instrument.len() > SR as usize, "the voice outlived its note: {}", in_instrument.len());
    assert_eq!(in_instrument.len(), on_track.len());
    let worst = in_instrument.iter().zip(&on_track).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(worst < 1e-6, "largest difference {worst}");
}

#[test]
fn a_delay_inside_an_instrument_is_not_cut_off_at_note_end() {
    let out = raw("
tempo 120bpm
instr click() { out = (osc(sine, 1000hz) * env[1 0.5ms 0]).delay(time: 1/2beat, feedback: 0.5) }
click() for 1/16beat");
    // the note ends after 1/16 beat (~31ms); echoes at 250ms intervals still sound
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
