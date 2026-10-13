use crate::common::*;

const STAGE: &str = "
tempo 120bpm
instr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: freq * (2 + bright * 6)) * env[1 | 20ms 0] }
track lead { instrument = pluck }
track bass { instrument = pluck(bright: 0.1) }
";

fn render_with_bars(body: &str, bars: Option<f64>) -> Result<Vec<f32>, niminal_cli::render::RenderError> {
    let p = program(&format!("{STAGE}{body}"));
    let until = bars.map(|b| b * 2.0); // a 4/4bar at 120bpm is two seconds
    render_with(&p, &[], &RenderOptions { limiter: false, until, ..Default::default() }).map(mono)
}

fn raw(body: &str, bars: f64) -> Vec<f32> {
    render_with_bars(body, Some(bars)).unwrap()
}

#[test]
fn a_played_pattern_renders_exactly_like_the_same_notes_written_out() {
    let played = raw("lead.play([c4 e4])", 2.0);
    // two bars of two one-second steps: four notes, two beats each
    let written = raw(
        "lead(freq: c4) for 2beats\nat 2beats lead(freq: e4) for 2beats\nat 4beats lead(freq: c4) for 2beats\nat 6beats lead(freq: e4) for 2beats",
        0.0,
    );
    assert!(!played.is_empty());
    assert_eq!(played, written);
}

#[test]
fn lanes_become_note_parameters() {
    let lane = raw("clip c { notes: [c4 e4]\nbright: [0.1 0.9]\ngain: [-6db] }\nlead.play(c)", 1.0);
    let written = raw(
        "lead(freq: c4, bright: 0.1, gain: -6db) for 2beats\nat 2beats lead(freq: e4, bright: 0.9, gain: -6db) for 2beats",
        0.0,
    );
    assert_eq!(lane, written);
}

#[test]
fn a_lane_can_set_a_track_parameter_as_each_note_starts() {
    let stage = "tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }\n";
    let render = |body: &str| {
        let p = program(&format!("{stage}{body}"));
        render_with(&p, &[], &RenderOptions { limiter: false, until: Some(2.0), ..Default::default() }).map(mono).unwrap()
    };
    let dull = render("pad.play([c5 c5])");
    let bright = render("clip c { notes: [c5 c5]\ncut: [300hz 8khz] }\npad.play(c)");
    let half = dull.len() / 2;
    // the first note is the same, the second is brighter (more of the saw gets through)
    assert_eq!(&dull[..half - 4800], &bright[..half - 4800]);
    assert!(rms(&bright[half..]) > rms(&dull[half..]) * 1.5, "{} vs {}", rms(&bright[half..]), rms(&dull[half..]));
    // with no notes there is nothing to step on, which is said rather than ignored
    let p = compile(&format!("{stage}clip c {{ cut: [300hz 8khz] }}\npad.play(c)"));
    let e = p.err().expect("a lane without notes is refused");
    assert!(e[0].message.contains("has no notes"), "{}", e[0].message);
    // a mistake in the lane's unit is reported
    let p = program(&format!("{stage}clip c {{ notes: [c5]\ncut: [0.5] }}\npad.play(c)"));
    let Err(e) = render_with(&p, &[], &RenderOptions { limiter: false, until: Some(1.0), ..Default::default() }) else { panic!("a bare number is not a frequency") };
    assert!(e.to_string().contains("cut"), "{e}");
}

const SWEEP_STAGE: &str = "tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }\n";

fn sweep(body: &str, seconds: f64) -> Vec<f32> {
    let p = program(&format!("{SWEEP_STAGE}{body}"));
    render_with(&p, &[], &RenderOptions { limiter: false, until: Some(seconds), ..Default::default() }).map(mono).unwrap()
}

fn eighths(x: &[f32]) -> Vec<f32> {
    x.chunks(x.len() / 8).take(8).map(rms).collect()
}

#[test]
fn an_envelope_lane_sweeps_a_track_parameter_over_the_clip() {
    let notes = "notes: [c5 c5 c5 c5 c5 c5 c5 c5]";
    let steady = eighths(&sweep(&format!("clip c {{ {notes} }}\npad.play(c)"), 2.0));
    let swept = eighths(&sweep(&format!("clip c {{ {notes}\ncut: env[300hz 1bar 8khz] }}\npad.play(c)"), 2.0));
    assert!((steady[0] - steady[7]).abs() < steady[0] * 0.05, "{steady:?}");
    assert!(swept[0] < swept[1] && swept[1] < swept[2], "{swept:?}");
    assert!(swept[7] > steady[7] * 2.0, "{swept:?} against {steady:?}");
}

#[test]
fn the_shape_of_an_envelope_lane_is_the_envelopes_shape() {
    let mid = |curve: &str| {
        let x = sweep(&format!("clip c {{ notes: [c5 c5 c5 c5 c5 c5 c5 c5]\ncut: env[300hz 1bar{curve} 8khz] }}\npad.play(c)"), 2.0);
        eighths(&x)[3]
    };
    // an exponential curve stays low for longer, a logarithmic one gets there early
    assert!(mid(".exp") < mid("") && mid("") < mid(".log"), "{} {} {}", mid(".exp"), mid(""), mid(".log"));
}

#[test]
fn an_envelope_lane_needs_no_notes_and_holds_its_last_level_or_loops() {
    // two bars of notes under a one-bar envelope that has finished by the second
    let held = eighths(&sweep("clip c { notes: [c5 c5 c5 c5 c5 c5 c5 c5]\ncut: env[300hz 1/4bar 8khz] }\npad.play(c)", 2.0));
    let steady = eighths(&sweep("clip c { notes: [c5 c5 c5 c5 c5 c5 c5 c5] }\npad.play(c)", 2.0));
    assert!(held[7] > steady[7] * 2.0 && (held[7] - held[3]).abs() < held[7] * 0.1, "{held:?}");
    let looped = eighths(&sweep("clip c { notes: [c5 c5 c5 c5 c5 c5 c5 c5]\ncut: env[300hz 1/4bar 700hz 0sec 300hz].loop }\npad.play(c)", 2.0));
    // the lane starts over, so it keeps dipping back to the dull start
    assert!(looped[2] < looped[1] * 0.95 && looped[4] < looped[3] * 0.95, "{looped:?} against {held:?}");
    // a clip with only an envelope lane is allowed: it moves the parameter for notes played elsewhere
    let p = compile(&format!("{SWEEP_STAGE}clip c {{ cut: env[300hz 1bar 8khz] }}\npad.play(c)"));
    assert!(p.is_ok(), "{:?}", p.err());
}

#[test]
fn envelope_lane_mistakes_are_reported() {
    let error = |body: &str| compile(&format!("{SWEEP_STAGE}{body}")).err().expect("an error").remove(0).message;
    assert!(error("clip c { notes: [c5]\nnope: env[1 1bar 2] }\npad.play(c)").contains("no such parameter"));
    assert!(error("clip c { notes: [c5]\ncut: env[0.5 1bar 1] }\npad.play(c)").contains("cut"), "a number is not a frequency");
    assert!(error("clip c { notes: [c5]\ncut: env[300hz 1bar 1khz | 1bar 300hz] }\npad.play(c)").contains("sustain"));
    assert!(error("clip c { notes: env[300hz 1bar 1khz] }").contains("holds notes"));
}

#[test]
fn a_looping_envelope_in_an_instrument_repeats() {
    let render = |tail: &str| {
        let p = program(&format!("tempo 120bpm\ninstr b(freq: hz) {{ osc(sine, freq) * env[0 10ms 1 10ms 0]{tail} }}\nb(freq: a4) for 200ms"));
        render_with(&p, &[], &RenderOptions { limiter: false, ..Default::default() }).map(mono).unwrap()
    };
    let (once, looped) = (render(""), render(".loop"));
    let after = |x: &[f32]| x.get(2400..9000.min(x.len())).map_or(0.0, rms);
    assert!(after(&once) < 0.001, "a one-shot is over after 20ms");
    assert!(after(&looped) > 0.1, "a loop keeps going");
}

#[test]
fn a_track_applies_its_instrument_defaults_to_played_notes() {
    let played = raw("bass.play([c2])", 1.0);
    let written = raw("bass(freq: c2) for 4beats", 0.0);
    assert_eq!(played, written);
    let explicit = raw("lead(freq: c2, bright: 0.1) for 4beats", 0.0);
    assert_eq!(played, explicit, "the bass track's bright: 0.1 is the same as writing it");
}

#[test]
fn an_open_ended_piece_needs_a_length() {
    let err = render_with_bars("lead.play([c4])", None).unwrap_err();
    assert!(err.0.contains("plays until it is stopped") && err.0.contains("--bars"), "{err}");
    // but one that ends itself doesn't
    let out = render_with_bars("lead.play([c4])\nat 2bars hush", None).unwrap();
    assert!(out.len() > (3.9 * SR) as usize && out.len() < (4.5 * SR) as usize, "{}", out.len() as f64 / SR);
}

#[test]
fn the_notes_after_the_length_are_not_started_but_ringing_ones_finish() {
    let out = raw("lead.play([c4])", 1.0);
    // one note starts at 0; the one at 2s is past the end of the render
    let sound = |from: f64, to: f64| rms(&out[(from * SR) as usize..(to * SR) as usize]);
    assert!(sound(0.2, 1.5) > 0.05);
    assert!(out.len() < (2.5 * SR) as usize, "{}", out.len() as f64 / SR);
}

#[test]
fn muting_silences_a_track_for_a_while() {
    let out = raw("lead.play([c4])\nat 1bar lead.mute\nat 3bars lead.unmute", 4.0);
    let loud = |s: f64| rms(&out[(s * SR) as usize..((s + 1.0) * SR) as usize]) > 0.05;
    assert!(loud(0.2), "before the mute");
    assert!(!loud(2.4), "muted (the note from 0s has rung out by 2.4s)");
    assert!(!loud(4.4), "still muted");
    assert!(loud(6.2), "the clip kept its place and sounds again");
}

#[test]
fn solo_isolates_a_track() {
    let both = raw("lead.play([c4])\nbass.play([c2])", 1.0);
    let solo = raw("lead.play([c4])\nbass.play([c2])\nbass.solo", 1.0);
    let bass_only = raw("bass.play([c2])", 1.0);
    assert!(rms(&both) > rms(&bass_only));
    assert_eq!(solo, bass_only);
}

#[test]
fn panic_cuts_everything_including_effect_tails() {
    let base = "
bus space
instr ping(freq: hz) { space += osc(sine, freq) * env[1 5ms 0] }
track echo { out = space.delay(time: 100ms, feedback: 0.9) }
track src { instrument = ping }
src.play([c4*2])
";
    let ringing = raw(base, 2.0);
    let cut = raw(&format!("{base}at 1bar panic"), 2.0);
    assert!(ringing.len() > (4.0 * SR) as usize, "the echoes run on: {}", ringing.len() as f64 / SR);
    // panic at 2s: nothing is heard after it (the echo of the last ping is gone)
    assert!(cut.len() < (2.05 * SR) as usize, "{}", cut.len() as f64 / SR);
    assert!(cut.len() > (1.9 * SR) as usize, "sound up to the panic");
}

#[test]
fn scenes_play_in_sequence() {
    let out = raw("
clip a { notes: [c4] }
clip b { notes: [c2] }
scene one { lead: a }
scene two { lead: b }
one.start
at 1bar two.start", 2.0);
    let written = raw("lead(freq: c4) for 4beats\nat 4beats lead(freq: c2) for 4beats", 0.0);
    assert_eq!(out, written);
}

#[test]
fn the_groove_example_renders_in_stereo() {
    let p = program(include_str!("../../../../examples/groove.nml"));
    let out = render_with(&p, &[], &RenderOptions::default()).unwrap();
    assert_eq!(out.channels.len(), 2);
    assert!(out.len() > (20.0 * SR) as usize, "{}", out.len() as f64 / SR);
    assert!(out.peak() > 0.1 && out.peak() <= 0.9661);
    assert_eq!(out.silenced_voices, 0);
    assert_ne!(out.channels[0], out.channels[1]);
}
