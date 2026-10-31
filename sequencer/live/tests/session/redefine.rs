//! Evaluating again, live: redefining, replacing and renaming things while clips play must neither
//! refuse the newest code nor silence what is running.

use crate::common::*;

const TONE: &str = "tempo 120bpm\ninstr tone(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\n";

fn clips(s: &mut Offline) -> Vec<String> {
    s.clips()
        .iter()
        .map(|c| {
            format!(
                "{}[{}]{}",
                c.name,
                c.instrument,
                if c.running { "*" } else { "" }
            )
        })
        .collect()
}

/// The pitch of what plays over half a bar, after running two bars so that a clip has come round again.
fn pitch_after_two_bars(s: &mut Offline) -> f32 {
    run(s, 2 * BAR);
    let out = run(s, BAR / 2);
    assert!(rms(&out[2000..]) > 0.05, "silent");
    hz(&out[2000..])
}

fn ok(s: &mut Offline, src: &str) {
    s.eval(src, Some("now"))
        .unwrap_or_else(|p| panic!("refused: {p:?}\n{src}"));
}

#[test]
fn a_redefined_instrument_reaches_the_clip_that_is_playing_it() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip c {{ instrument: tone\n notes: [a3] }}\nc.start"),
    );
    assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0);
    ok(
        &mut s,
        "instr tone(freq: hz) { osc(sine, freq * 2) * env[1 | 10ms 0] }",
    );
    assert!((pitch_after_two_bars(&mut s) - 440.0).abs() < 12.0);
}

#[test]
fn a_redefined_clip_replaces_the_one_that_is_playing() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip c {{ instrument: tone\n notes: [a3] }}\nc.start"),
    );
    run(&mut s, 4000);
    ok(&mut s, "clip c { instrument: tone\n notes: [a4] }");
    assert!((pitch_after_two_bars(&mut s) - 440.0).abs() < 12.0);
    assert_eq!(clips(&mut s), ["c[tone]*"]);
}

#[test]
fn a_clip_and_an_instrument_of_one_name_work_together_and_each_can_be_redefined() {
    let mut s = session();
    ok(
        &mut s,
        "tempo 120bpm\ninstr bass(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip bass { instrument: bass\n notes: [a3] }\nbass.start",
    );
    assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0);
    ok(
        &mut s,
        "instr bass(freq: hz) { osc(sine, freq * 2) * env[1 | 10ms 0] }",
    );
    assert!((pitch_after_two_bars(&mut s) - 440.0).abs() < 12.0);
    ok(&mut s, "clip bass { instrument: bass\n notes: [a4] }");
    assert!((pitch_after_two_bars(&mut s) - 880.0).abs() < 20.0);
    assert_eq!(clips(&mut s), ["bass[bass]*"]);
}

#[test]
fn it_does_not_matter_which_of_a_clip_and_an_instrument_of_one_name_comes_first() {
    for src in [
        "tempo 120bpm\nclip bass { instrument: bass\n notes: [a3] }\ninstr bass(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nbass.start",
        "tempo 120bpm\ninstr bass(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip bass { instrument: bass\n notes: [a3] }\nbass.start",
    ] {
        let mut s = session();
        ok(&mut s, src);
        assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0, "{src}");
    }
}

#[test]
fn an_instrument_takes_the_name_of_a_clip_that_does_not_play_it_and_says_so() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip pad {{ instrument: tone\n notes: [a3] }}\npad.start"),
    );
    run(&mut s, 4000);
    ok(&mut s, "instr pad(freq: hz) { osc(saw, freq) * 0.1 }");
    assert!(
        clips(&mut s).is_empty(),
        "the newest definition has the name"
    );
    ok(&mut s, "pad(freq: a3) for 1beat");
    let notices = s.take_notices();
    assert!(
        notices.iter().any(|n| n.contains("clip `pad`")),
        "{notices:?}"
    );
}

#[test]
fn a_clip_takes_the_name_of_an_instrument_and_the_clips_that_used_it_go_with_a_notice() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip a {{ instrument: tone\n notes: [a3] }}\na.start"),
    );
    run(&mut s, 4000);
    ok(
        &mut s,
        "instr other(freq: hz) { osc(saw, freq) * 0.1 }\nclip tone { instrument: other\n notes: [a3] }",
    );
    assert_eq!(clips(&mut s), ["tone[other]"]);
    let notices = s.take_notices();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("instrument `tone`") && n.contains("clip `a`")),
        "{notices:?}"
    );
}

#[test]
fn a_name_can_go_back_and_forth_between_a_clip_and_an_instrument() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip c {{ instrument: tone\n notes: [a3] }}\nc.start"),
    );
    run(&mut s, 4000);
    ok(&mut s, "instr c(freq: hz) { osc(saw, freq) * 0.1 }");
    ok(&mut s, "clip c { instrument: tone\n notes: [a3] }\nc.start");
    assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0);
    assert_eq!(clips(&mut s), ["c[tone]*"]);
}

#[test]
fn an_evaluation_with_a_mistake_changes_nothing() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip c {{ instrument: tone\n notes: [a3] }}\nc.start"),
    );
    run(&mut s, 4000);
    assert!(
        s.eval(
            "instr extra(freq: hz) { osc(sine, freq) }\nclip d { instrument: nope\n notes: [a3] }",
            Some("now")
        )
        .is_err()
    );
    assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0);
    assert_eq!(clips(&mut s), ["c[tone]*"]);
    assert!(
        s.eval("extra(freq: a3) for 1beat", Some("now")).is_err(),
        "`extra` was not kept"
    );
}

#[test]
fn a_renamed_clip_leaves_the_old_one_running_until_it_is_stopped() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}clip old {{ instrument: tone\n notes: [a3] }}\nold.start"),
    );
    run(&mut s, 4000);
    ok(
        &mut s,
        "clip fresh { instrument: tone\n notes: [a4] }\nfresh.start",
    );
    assert_eq!(clips(&mut s), ["old[tone]*", "fresh[tone]*"]);
    ok(&mut s, "old.stop");
    run(&mut s, 2 * BAR);
    assert_eq!(clips(&mut s), ["old[tone]", "fresh[tone]*"]);
}

#[test]
fn a_lane_for_a_parameter_the_instrument_lost_is_left_out_and_the_clip_keeps_playing() {
    let lane = "clip c { instrument: t\n notes: [a3]\n amp: 0.3 }\nc.start";
    for change in [
        "instr t(freq: hz) { osc(sine, freq) }",
        "instr t(freq: hz, level: 0..1 = 0.5) { osc(sine, freq) * level }",
    ] {
        let mut s = session();
        ok(
            &mut s,
            &format!(
                "tempo 120bpm\ninstr t(freq: hz, amp: 0..1 = 0.5) {{ osc(sine, freq) * amp }}\n{lane}"
            ),
        );
        run(&mut s, 4000);
        ok(&mut s, change);
        assert!(
            (pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0,
            "{change}"
        );
        let notices = s.take_notices();
        assert!(
            notices
                .iter()
                .any(|n| n.contains("`amp`") && n.contains("left out")),
            "{notices:?}"
        );
    }
}

#[test]
fn a_redefined_pattern_reaches_the_clip_that_plays_it() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{TONE}hits = [a3 ~ ~ ~]\nclip c {{ instrument: tone\n notes: hits }}\nc.start"),
    );
    run(&mut s, 4000);
    ok(&mut s, "hits = [a4 a4 a4 a4]");
    assert!((pitch_after_two_bars(&mut s) - 440.0).abs() < 12.0);
}

#[test]
fn a_bus_taken_for_something_else_takes_what_used_it_and_says_so() {
    let mut s = session();
    ok(
        &mut s,
        "tempo 120bpm\nbus fxb\ninstr t(freq: hz) { fxb += osc(sine, freq) * 0.1\n osc(sine, freq) * 0.2 }\nfx r { out = fxb }\nclip c { instrument: t\n notes: [a3] }\nc.start",
    );
    run(&mut s, 4000);
    ok(&mut s, "const fxb = 1");
    assert!(clips(&mut s).is_empty());
    let notices = s.take_notices();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("bus `fxb`") && n.contains("clip `c`")),
        "{notices:?}"
    );
}

#[test]
fn a_local_may_have_the_name_of_an_instrument_from_another_evaluation() {
    let mut s = session();
    ok(
        &mut s,
        "instr voice() { osc(sine, 220hz) * 0.1 }\nvoice() for 1beat\n",
    );
    ok(
        &mut s,
        "instr pad(freq: hz) {\n  voice = osc(saw, freq) * 0.2\n  voice\n}\npad(freq: a3) for 1beat\n",
    );
    ok(&mut s, "voice() for 1beat\n");
}

#[test]
fn a_local_may_not_take_the_name_of_a_bus() {
    let mut s = session();
    s.eval("bus space\n", Some("now")).unwrap();
    let problems = s
        .eval(
            "instr pad(freq: hz) {\n  space = osc(saw, freq) * 0.2\n  space\n}\n",
            Some("now"),
        )
        .expect_err("a bus can be read as a plain name, so a local may not hide it");
    assert!(
        problems[0]
            .message
            .contains("`space` is already the name of a bus"),
        "{problems:?}"
    );
}

#[test]
fn a_keyboard_clip_with_a_stale_lane_still_plays() {
    let mut s = session();
    ok(
        &mut s,
        "tempo 120bpm\ninstr organ(freq: hz, amp: 0..1 = 0.5) { osc(sine, freq) * amp }\nclip pad { instrument: organ\n  keys: midi.keys(channel: 1)\n  amp: 0.3 }\npad.start",
    );
    ok(&mut s, "instr organ(freq: hz) { osc(sine, freq) * 0.5 }");
    s.key_input(1, 69, 127);
    let out = run(&mut s, 9600);
    assert!(rms(&out[2000..]) > 0.05, "silent");
    assert!((hz(&out[2000..]) - 440.0).abs() < 12.0);
    let notices = s.take_notices();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("`amp`") && n.contains("left out")),
        "{notices:?}"
    );
}

#[test]
fn a_new_required_parameter_is_said_aloud_with_how_to_fix_it_and_the_clip_stays_defined() {
    let mut s = session();
    ok(
        &mut s,
        "tempo 120bpm\ninstr bass(freq: hz) { osc(saw, freq) * 0.3 }\nclip groove { instrument: bass\n notes: [a2] }\ngroove.start",
    );
    run(&mut s, 4000);
    ok(
        &mut s,
        "instr bass(freq: hz, cutoff: hz) { osc(saw, freq).lpf(cutoff: cutoff) * 0.3 }",
    );
    run(&mut s, 2 * BAR);
    let notices = s.take_notices();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("`bass` needs an argument `cutoff`")
                && n.contains("give it a default")),
        "{notices:?}"
    );
    assert_eq!(clips(&mut s), ["groove[bass]*"]);
    // giving the clip the lane brings it back
    ok(
        &mut s,
        "clip groove { instrument: bass\n notes: [a2]\n cutoff: 1khz }",
    );
    assert!(pitch_after_two_bars(&mut s) > 40.0);
}

const DRUMS: &str = "tempo 120bpm
instr lo(freq: hz = 100hz) { osc(sine, freq) * env[1 | 10ms 0] }
instr hi(freq: hz = 1khz) { osc(sine, freq) * env[1 | 10ms 0] }
kit_a = kit { kick: lo\n hat: hi }
";

#[test]
fn a_redefined_grid_reaches_the_clip_that_plays_it() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{DRUMS}hits = grid {{\n kick: x...\n}}\nclip d {{ instrument: kit_a\n notes: hits }}\nd.start"),
    );
    run(&mut s, 2 * BAR);
    let before = run(&mut s, BAR);
    assert!(rms(&before) > 0.02, "silent");
    ok(&mut s, "hits = grid {\n hat: x...\n}");
    run(&mut s, 2 * BAR);
    let after = run(&mut s, BAR);
    assert!(rms(&after) > 0.02, "silent after redefining the grid");
    let f = hz(&after[2000..BAR / 4]);
    assert!(f > 600.0, "still the old drum: {f}");
}

#[test]
fn a_redefined_kit_reaches_the_clip_that_plays_it() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{DRUMS}clip d {{ instrument: kit_a\n notes: grid {{\n kick: x...\n}} }}\nd.start"),
    );
    run(&mut s, 2 * BAR);
    ok(&mut s, "kit_a = kit { kick: hi\n hat: hi }");
    run(&mut s, 2 * BAR);
    let out = run(&mut s, BAR);
    assert!(rms(&out) > 0.02, "silent");
    let f = hz(&out[2000..BAR / 4]);
    assert!(f > 600.0, "still the old drum: {f}");
}

#[test]
fn a_drum_the_kit_lost_is_left_out_of_the_clip_and_the_other_rows_keep_playing() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{DRUMS}clip d {{ instrument: kit_a\n notes: grid {{\n kick: x...\n hat: ..x.\n}} }}\nd.start"),
    );
    run(&mut s, BAR);
    s.take_notices();
    ok(&mut s, "kit_a = kit { hat: hi }");
    assert_eq!(clips(&mut s), ["d[kit_a]*"]);
    let notices = s.take_notices();
    assert!(
        notices.iter().any(|n| n.contains("`kick` in clip `d`")),
        "{notices:?}"
    );
    run(&mut s, 2 * BAR);
    let out = run(&mut s, BAR);
    assert!(rms(&out) > 0.02, "the remaining row went silent");
}

#[test]
fn a_drum_the_kit_never_had_is_still_refused_in_a_clip_just_sent() {
    let mut s = session();
    let refused = s.eval(
        &format!("{DRUMS}clip d {{ instrument: kit_a\n notes: grid {{\n snare: x...\n}} }}\nd.start"),
        Some("now"),
    );
    assert!(refused.is_err());
}

const GLOBALS: &str = "tempo 120bpm
const pitch_mult = 1
const wave = harmonics([1.0], size: 512)
fn double(x) { x * 2 }
instr g(freq: hz) { osc(sine, freq * pitch_mult) * env[1 | 10ms 0] }
instr t(freq: hz) { osc(wave, freq) * env[1 | 10ms 0] }
instr f(freq: hz) { osc(sine, double(freq)) * env[1 | 10ms 0] }
clip a { instrument: g\n notes: [a3] }
clip b { instrument: t\n notes: [a3] }
clip c { instrument: f\n notes: [a2] }
";

fn only(s: &mut Offline, name: &str) -> f32 {
    ok(s, &format!("{name}.start"));
    pitch_after_two_bars(s)
}

#[test]
fn a_redefined_constant_reaches_the_instruments_that_use_it() {
    let mut s = session();
    ok(&mut s, GLOBALS);
    ok(&mut s, "a.start");
    assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0);
    ok(&mut s, "const pitch_mult = 2");
    assert!((pitch_after_two_bars(&mut s) - 440.0).abs() < 12.0);
    assert!(s.take_notices().iter().all(|n| !n.contains("took out")));
}

#[test]
fn a_redefined_function_reaches_the_instruments_that_call_it() {
    let mut s = session();
    ok(&mut s, GLOBALS);
    assert!((only(&mut s, "c") - 220.0).abs() < 8.0);
    ok(&mut s, "fn double(x) { x * 4 }");
    assert!((pitch_after_two_bars(&mut s) - 440.0).abs() < 12.0);
}

#[test]
fn a_redefined_table_reaches_the_instruments_that_play_it() {
    let mut s = session();
    ok(&mut s, GLOBALS);
    let before = only(&mut s, "b");
    assert!((before - 220.0).abs() < 8.0, "{before}");
    ok(&mut s, "const wave = harmonics([0.0, 1.0], size: 512)");
    let after = pitch_after_two_bars(&mut s);
    assert!((after - 440.0).abs() < 12.0, "{after}");
}

#[test]
fn a_constant_taken_for_something_else_takes_what_used_it_and_says_so() {
    let mut s = session();
    ok(&mut s, GLOBALS);
    ok(&mut s, "a.start");
    run(&mut s, BAR);
    s.take_notices();
    ok(&mut s, "bus pitch_mult: stereo");
    let notices = s.take_notices();
    assert!(notices.iter().any(|n| n.contains("took out")), "{notices:?}");
}

#[test]
fn a_lane_the_instrument_does_not_have_yet_is_left_out_whichever_comes_first() {
    let mut s = session();
    ok(&mut s, &format!("{TONE}clip c {{ instrument: tone\n notes: [a3]\n k: [0.2 0.8] }}\nc.start"));
    let notices = s.take_notices();
    assert!(notices.iter().any(|n| n.contains("lane `k` of clip `c`")), "{notices:?}");
    assert!((pitch_after_two_bars(&mut s) - 220.0).abs() < 8.0);
    // sent again with nothing changed, it is not said again
    ok(&mut s, "pat = [a3]");
    assert!(s.take_notices().iter().all(|n| !n.contains("lane `k`")));
    // the instrument gets the parameter later, and the lane is used
    ok(&mut s, "instr tone(freq: hz, k: 0..1 = 0.5) { osc(sine, freq * (1 + k)) * env[1 | 10ms 0] }");
    run(&mut s, 2 * BAR);
    assert!(s.take_notices().iter().all(|n| !n.contains("lane `k`")));
}

#[test]
fn a_kit_given_its_drums_in_another_order_still_plays_each_drum() {
    let mut s = session();
    ok(
        &mut s,
        &format!("{DRUMS}clip d {{ instrument: kit_a\n notes: grid {{\n kick: x...\n}} }}\nd.start"),
    );
    run(&mut s, BAR);
    let before = run(&mut s, BAR);
    assert!(hz(&before[2000..BAR / 4]) < 300.0, "the kick is the low drum");
    ok(&mut s, "kit_a = kit { hat: hi\n kick: lo }");
    run(&mut s, BAR);
    let after = run(&mut s, BAR);
    assert!(rms(&after) > 0.02, "silent");
    let f = hz(&after[2000..BAR / 4]);
    assert!(f < 300.0, "the kick turned into another drum: {f}");
}

#[test]
fn a_scene_redefined_while_it_plays_says_which_clips_it_started_and_stopped() {
    let mut s = session();
    ok(&mut s, &format!("{TONE}clip c1 {{ instrument: tone\n notes: [a3] }}\nclip c2 {{ instrument: tone\n notes: [a4] }}\nclip c3 {{ instrument: tone\n notes: [a2] }}\nscene s1 {{ c1, c2 }}\ns1.start"));
    s.take_notices();
    ok(&mut s, "scene s1 { c3, c1 }");
    let notices = s.take_notices();
    assert!(
        notices.iter().any(|n| n.contains("scene `s1`") && n.contains("`c3`") && n.contains("`c2`")),
        "{notices:?}"
    );
    assert_eq!(clips(&mut s), ["c1[tone]*", "c2[tone]", "c3[tone]*"]);
}

/// The code the Stage's Drums tab sends to let you hear a grid (`auditionDrums` in stage/src/drums.ts).
fn lab_drums(density: &str) -> String {
    format!(
        "use std/drums.{{kick_deep, snare_tight, clap, hat_closed, hat_open, rim}}
__lab_kit = kit {{ kick: kick_deep, snare: snare_tight, clap: clap, hat: hat_closed, open: hat_open, perc: rim }}
clip __lab_drums {{
  instrument: __lab_kit
  notes: drum_grid(theme: techno, density: {density}, seed: 1)
}}
__lab_drums.start"
    )
}

#[test]
fn the_labs_drum_audition_plays_changes_while_it_sounds_and_stops() {
    let mut s = session();
    let send = |s: &mut Offline, src: &str| {
        s.eval_parsed(niminal_live::ParsedEval::from_dir(src, &std::env::temp_dir(), Some("now")))
            .unwrap_or_else(|p| panic!("refused: {:?}", p.iter().map(|p| &p.message).collect::<Vec<_>>()));
    };
    send(&mut s, &lab_drums("0.7"));
    run(&mut s, BAR);
    assert!(rms(&run(&mut s, BAR)) > 0.01, "silent");
    send(&mut s, &lab_drums("0.3"));
    run(&mut s, BAR);
    assert!(rms(&run(&mut s, BAR)) > 0.01, "silent after a slider moved");
    send(&mut s, "__lab_drums.stop");
    run(&mut s, 2 * BAR);
    assert!(rms(&run(&mut s, BAR)) < 0.001, "still sounding after stop");
}
