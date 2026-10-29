use crate::common::*;

fn rig_dir(rig: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("niminal-use-{}-{}", std::process::id(), rig.len()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rig.nml"), rig).unwrap();
    dir
}

#[test]
fn used_modules_play_and_replay_without_their_files() {
    let dir = rig_dir(
        "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip lead { instrument: pluck }\n",
    );
    let mut s = session();
    s.eval_parsed(niminal_live::ParsedEval::from_dir(
        "use rig\nlead.__update([c4])",
        &dir,
        Some("now"),
    ))
    .unwrap();
    let original = run(&mut s, BAR);
    assert!(peak(&original) > 0.1);
    std::fs::remove_dir_all(&dir).unwrap();
    let replayed = niminal_server::Session::replay(SR, Layout::Mono, &s.log(), BAR as u64, false);
    assert_eq!(original, replayed[0]);
}

#[test]
fn a_problem_in_a_used_module_points_at_the_use_line() {
    let dir = rig_dir("instr broken() { nonsense(1) }\n");
    let mut s = session();
    let problems = s
        .eval_parsed(niminal_live::ParsedEval::from_dir(
            "use rig\n",
            &dir,
            Some("now"),
        ))
        .unwrap_err();
    assert!(
        problems[0].message.starts_with("in rig.nml line 1:"),
        "{}",
        problems[0].message
    );
    assert_eq!(problems[0].line, Some(1));
}

#[test]
fn at_plus_counts_from_where_the_code_lands() {
    let mut s = ready();
    run(&mut s, 3 * BAR + 1000);
    s.eval("at +2bars lead.__update([c4])", Some("now"))
        .unwrap();
    let before = run(&mut s, 2 * BAR - 1000);
    let after = run(&mut s, BAR);
    assert!(peak(&before) < 1e-6, "nothing before bar 6");
    assert!(peak(&after) > 0.1, "the note starts two bars after landing");
}

#[test]
fn reset_makes_the_next_bar_line_bar_one() {
    let mut s = ready();
    run(&mut s, 3 * BAR + 1000);
    let accepted = s.eval("reset", None).unwrap();
    assert_eq!(accepted.lands_at, 4 * BAR as u64, "on the next bar line");
    run(&mut s, BAR - 900);
    // bar 5 on the old grid is bar 1 of the new one
    assert_eq!(s.transport().bar, 1);
    let accepted = s.eval("at 2bars lead.__update([c4])", Some("now")).unwrap();
    assert_eq!(accepted.position.0, 1);
    let before = run(&mut s, 2 * BAR - 100);
    let after = run(&mut s, BAR);
    assert!(
        peak(&before) < 1e-6 && peak(&after) > 0.1,
        "two bars after the reset, not after the start"
    );
}

/// `lead` and `bass` play the same four notes, and a scene has both.
const RIFF: &str = "riff = [c4 e4 g4 b4]\nclip lead { instrument: pluck\n notes: riff }\nclip bass { instrument: pluck\n notes: riff }\nscene verse { lead, bass }\n";

#[test]
fn a_clip_started_by_a_scene_can_be_changed_by_name() {
    let mut s = ready();
    s.eval(&format!("{RIFF}verse.start"), Some("now")).unwrap();
    let first = run(&mut s, BAR);
    assert!(
        (hz(&first[2000..22_000]) - 262.0).abs() < 10.0,
        "starts on c4"
    );
    s.eval("lead.reverse", Some("now")).unwrap();
    let second = run(&mut s, BAR);
    assert!(
        (hz(&second[2000..22_000]) - 494.0).abs() < 15.0,
        "now starts on b4: {}",
        hz(&second[2000..22_000])
    );
    // and again: changes build on each other
    s.eval("lead.reverse", Some("now")).unwrap();
    let third = run(&mut s, BAR);
    assert!(
        (hz(&third[2000..22_000]) - 262.0).abs() < 10.0,
        "back to c4"
    );
}

#[test]
fn a_clip_that_a_scene_plays_follows_a_redefinition() {
    let mut s = ready();
    s.eval(
        "riff = [a3]\nclip lead { instrument: pluck\n notes: riff }\nscene verse { lead }\nverse.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, BAR);
    s.eval("riff = [a4]", Some("now")).unwrap();
    let after = run(&mut s, BAR);
    assert!(
        (hz(&after[2000..BAR - 2000]) - 440.0).abs() < 12.0,
        "{}",
        hz(&after[2000..BAR - 2000])
    );
}

#[test]
fn stopping_a_scene_stops_its_clips_and_a_clip_with_nothing_playing_cannot_be_changed() {
    let mut s = ready();
    s.eval(&format!("{RIFF}verse.start"), Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("verse.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    assert!(
        peak(&run(&mut s, BAR)[10_000..]) < 1e-3,
        "silent once the notes have rung out"
    );
    let problems = s.eval("lead.reverse", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("isn't playing anything"),
        "{}",
        problems[0].message
    );
}

#[test]
fn changes_stack_and_can_be_undone_or_cleared() {
    let first_beat = |s: &mut Offline| hz(&run(s, BAR)[2000..22_000]);
    let mut s = ready();
    s.eval(&format!("{RIFF}verse.start"), Some("now")).unwrap();
    assert!((first_beat(&mut s) - 262.0).abs() < 10.0);

    s.eval("lead.reverse", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 494.0).abs() < 15.0, "reversed");
    s.eval("lead.transpose(+12st)", Some("now")).unwrap();
    assert!(
        (first_beat(&mut s) - 988.0).abs() < 30.0,
        "reversed, then up an octave"
    );

    s.eval("lead.undo", Some("now")).unwrap();
    assert!(
        (first_beat(&mut s) - 494.0).abs() < 15.0,
        "the octave is off, the reverse stays"
    );
    s.eval("lead.transpose(+12st)\nlead.clear", Some("now"))
        .unwrap();
    assert!(
        (first_beat(&mut s) - 262.0).abs() < 10.0,
        "everything off, in one go"
    );

    let problems = s.eval("lead.undo", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("nothing to undo"),
        "{}",
        problems[0].message
    );
    s.eval("lead.reverse", Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    assert!(
        (first_beat(&mut s) - 494.0).abs() < 15.0,
        "starting a scene whose clips are running leaves them as they are"
    );
    s.eval("verse.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    s.eval("verse.start", Some("now")).unwrap();
    assert!(
        (first_beat(&mut s) - 262.0).abs() < 10.0,
        "stopped and started, it begins again from the clip as written"
    );
}

#[test]
fn a_redefined_clip_keeps_the_changes_made_to_a_node() {
    let mut s = ready();
    s.eval(&format!("{RIFF}verse.start\nlead.reverse"), Some("now"))
        .unwrap();
    run(&mut s, BAR);
    s.eval("riff = [a3 a4 a4 a4]", Some("now")).unwrap();
    let out = run(&mut s, BAR);
    assert!(
        (hz(&out[2000..22_000]) - 440.0).abs() < 15.0,
        "reversed: a4 comes first, a3 last: {}",
        hz(&out[2000..22_000])
    );
}

#[test]
fn thin_drops_the_same_notes_wherever_it_is_written() {
    let thinned = |code: &str| {
        let mut s = ready();
        s.eval(code, Some("now")).unwrap();
        run(&mut s, 4 * BAR)
    };
    let a = thinned("lead.__update([c4 e4 g4 b4 c5 e5 g5 b5].drop(50%))");
    let b = thinned("riff = [c3]\nlead.__update([c4 e4 g4 b4 c5 e5 g5 b5].drop(50%))");
    assert_eq!(a, b);
}

#[test]
fn changes_work_on_a_project_that_has_timed_commands_in_it() {
    let mut s = ready();
    s.eval(
        &format!(
            "{RIFF}scene chorus {{ lead }}\nverse.start\nat +4bars chorus.start\nat +8bars hush"
        ),
        Some("now"),
    )
    .unwrap();
    run(&mut s, BAR);
    s.eval("lead.reverse", Some("now")).unwrap();
    assert!(
        (hz(&run(&mut s, BAR)[2000..22_000]) - 494.0).abs() < 15.0,
        "the later commands didn't change what is playing now"
    );
    s.eval("lead.clear", Some("now")).unwrap();
}

const PARTS: &str = "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip lead { instrument: pluck\n  notes: [c4 e4 g4 b4] }\nclip bass { instrument: pluck\n  notes: [c2] }\nscene verse { lead, bass }\nscene bridge { lead }\n";

#[test]
fn clips_with_instruments_are_started_changed_muted_and_stopped_by_name() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    let on = run(&mut s, BAR);
    assert!(peak(&on) > 0.1, "the scene plays");

    s.eval("lead.reverse", Some("now")).unwrap();
    s.eval("bass.mute", Some("now")).unwrap();
    s.eval("lead.mute", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)[2000..]) < 1e-3, "both muted");
    s.eval("lead.unmute", Some("now")).unwrap();
    let out = run(&mut s, BAR);
    assert!(peak(&out) > 0.1, "lead is back, reversed");
    s.eval("lead.undo", Some("now")).unwrap();
    s.eval("verse.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    assert!(
        peak(&run(&mut s, BAR)[10_000..]) < 1e-3,
        "the scene's parts have stopped"
    );

    s.eval("lead.start", Some("now")).unwrap();
    let again = run(&mut s, BAR);
    assert!(
        peak(&again) > 0.1 && (hz(&again[2000..22_000]) - 262.0).abs() < 12.0,
        "one clip started alone, without the reverse: {}",
        hz(&again[2000..22_000])
    );
}

#[test]
fn starting_a_scene_stops_the_clips_of_other_scenes_that_it_does_not_use() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    // `bass` is part of `verse`, whoever started it
    s.eval("bass.start", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("bridge.start", Some("now")).unwrap();
    run(&mut s, 24_000);
    let status = s.clips();
    let names: Vec<_> = status
        .iter()
        .map(|t| (t.name.as_str(), t.running))
        .collect();
    assert_eq!(
        names,
        [("lead", true), ("bass", false)],
        "bridge has no bass"
    );
    let scenes: Vec<_> = s
        .scenes()
        .iter()
        .map(|c| (c.name.clone(), c.running, c.active()))
        .collect();
    assert_eq!(
        scenes,
        [("verse".into(), 1, false), ("bridge".into(), 1, true)],
        "only bridge is playing, though its clip is in verse too"
    );
    s.eval("bridge.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    let scenes: Vec<_> = s.scenes().iter().map(|c| (c.running, c.active())).collect();
    assert_eq!(
        scenes,
        [(0, false), (0, false)],
        "stopping the scene stops its clips"
    );
}

#[test]
fn swapping_one_scene_for_another_keeps_a_clip_they_share_running() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("lead.reverse", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("verse.stop\nbridge.start", Some("now")).unwrap();
    run(&mut s, 24_000);
    let status = s.clips();
    assert_eq!(
        status.iter().map(|t| t.running).collect::<Vec<_>>(),
        [true, false],
        "bass stopped, lead carried on"
    );
    assert_eq!(
        status[0].edits,
        ["reverse"],
        "and kept what had been done to it"
    );
}

#[test]
fn a_scene_can_be_muted_and_soloed_as_a_whole() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start\nverse.mute", Some("now")).unwrap();
    run(&mut s, 24_000);
    assert!(s.clips().iter().all(|c| c.muted));
    s.eval("verse.unmute\nbridge.solo", Some("now")).unwrap();
    run(&mut s, 24_000);
    let status = s.clips();
    assert_eq!(
        status
            .iter()
            .map(|c| (c.muted, c.soloed))
            .collect::<Vec<_>>(),
        [(false, true), (false, false)]
    );
}

#[test]
fn a_scene_cannot_be_changed_like_a_clip() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    let problems = s.eval("verse.reverse", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("is a scene"),
        "{}",
        problems[0].message
    );
}

#[test]
fn a_bus_with_a_body_processes_what_instruments_send_to_it() {
    let heard = |gain: &str| {
        let mut s = Offline::new(SR, Layout::Stereo).without_limiter();
        let code = format!(
            "tempo 120bpm\nbus space: stereo\nfx space_return {{ out = space.gain({gain}) }}\ninstr ping(freq: hz) {{ out = osc(sine, freq) * env[1 | 10ms 0]\n space += out }}\nclip p {{ instrument: ping\n notes: [c4] }}\np.start"
        );
        s.eval(&code, Some("now")).unwrap();
        let out = s.process(BAR);
        assert!(
            s.clips().iter().all(|t| t.name != "space.fx"),
            "the bus's processing is not a part to play"
        );
        peak(&out[0])
    };
    // the instrument also plays directly; what the bus adds is the rest
    let direct = heard("0");
    let (loud, quiet) = (heard("0.5") - direct, heard("0.25") - direct);
    assert!(
        quiet > 0.05 && (loud / quiet - 2.0).abs() < 0.1,
        "the bus's gain shapes the sound: {loud} vs {quiet}"
    );
}

#[test]
fn set_changes_a_lane_live_and_undo_puts_it_back() {
    // the first bit of each bar is the last bar's note dying away
    let level = |s: &mut Offline| peak(&run(s, BAR)[2000..]);
    let mut s = session();
    s.eval("tempo 120bpm\ninstr loud(freq: hz, amp: 0..1 = 1) { osc(sine, freq) * amp * env[1 | 10ms 0] }\nclip lead { instrument: loud\n notes: [c4] }\nlead.start", Some("now")).unwrap();
    let full = level(&mut s);
    assert!(full > 0.9, "{full}");
    s.eval("lead.set(amp: 0.25)", Some("now")).unwrap();
    let quiet = level(&mut s);
    assert!((quiet - 0.25).abs() < 0.03, "{quiet}");
    assert_eq!(s.clips()[0].edits, ["set(amp: 0.25)"]);
    s.eval("lead.set(amp: 0.5)", Some("now")).unwrap();
    assert!((level(&mut s) - 0.5).abs() < 0.03, "the newer set wins");
    s.eval("lead.undo\nlead.undo", Some("now")).unwrap();
    assert!(level(&mut s) > 0.9, "back to the clip as written");
    let problems = s.eval("lead.set(wobble: 1)", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("wobble"),
        "{}",
        problems[0].message
    );
}

#[test]
fn undo_can_name_the_change_to_take_off() {
    let mut s = ready();
    s.eval(&format!("{RIFF}verse.start"), Some("now")).unwrap();
    s.eval(
        "lead.reverse\nlead.transpose(+12st)\nlead.drop(10%)",
        Some("now"),
    )
    .unwrap();
    assert_eq!(
        s.clips()[0].edits,
        ["reverse", "transpose(+12st)", "drop(10%)"]
    );
    s.eval("lead.undo(reverse)", Some("now")).unwrap();
    assert_eq!(
        s.clips()[0].edits,
        ["transpose(+12st)", "drop(10%)"],
        "the middle one went, order kept"
    );
    let problems = s.eval("lead.undo(reverse)", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("no `reverse` change to undo"),
        "{}",
        problems[0].message
    );
}

#[test]
fn a_clip_keeps_running_across_control_changes() {
    let mut s = session();
    s.eval("tempo 120bpm\nctl level = 0.2\ninstr tone(freq: hz) { osc(sine, freq) * level * env[1 | 1ms 0] }\nclip t { instrument: tone\n notes: [c4] }\nt.start", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)) > 0.1);
    s.eval("ctl level = 0.4", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)) > 0.3);
    assert!(s.take_notices().is_empty());
}

#[test]
fn a_scene_lands_at_its_own_quantum_over_its_clips_and_the_default() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval(
        "scene slow { lead, bass, quantize: 2bars }\nscene quick { lead, quantize: 1beat }\nclip fill { instrument: pluck\n notes: [c4]\n quantize: 1beat }",
        Some("now"),
    )
    .unwrap();
    run(&mut s, 1000);
    let beat = BAR as u64 / 4;
    assert_eq!(
        s.eval("verse.start", None).unwrap().lands_at,
        BAR as u64,
        "the default is the bar"
    );
    assert_eq!(
        s.eval("slow.start", None).unwrap().lands_at,
        2 * BAR as u64,
        "the scene's own"
    );
    assert_eq!(s.eval("quick.start", None).unwrap().lands_at, beat);
    assert_eq!(
        s.eval("fill.start", None).unwrap().lands_at,
        beat,
        "a clip's own, when started alone"
    );
    assert_eq!(
        s.eval("slow.start @ now", None).unwrap().lands_at,
        1000,
        "an `@` on the statement beats it"
    );
    assert_eq!(
        s.eval("slow.start", Some("beat")).unwrap().lands_at,
        beat,
        "so does an override for the whole evaluation"
    );
}

#[test]
fn a_change_to_a_running_clip_lands_at_its_next_loop_point() {
    let mut s = ready();
    s.eval(
        "clip long { instrument: pluck\n notes: [c4 e4 g4 b4]\n length: 4bars }\nlong.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, 2 * BAR + BAR / 2);
    let at = s.eval("long.reverse", None).unwrap();
    assert_eq!(
        at.lands_at,
        4 * BAR as u64,
        "not at the next bar line, which is bar 3"
    );
    let at = s.eval("long.transpose(+12st) @ next cycle", None).unwrap();
    assert_eq!(at.lands_at, 4 * BAR as u64);
    // a clip that has not got going has no loop points, so it waits for a bar
    s.eval(
        "clip other { instrument: pluck\n notes: [c4]\n length: 4bars }",
        Some("now"),
    )
    .unwrap();
    assert_eq!(
        s.eval("other.start", None).unwrap().lands_at,
        3 * BAR as u64
    );
}

#[test]
fn redefining_a_running_clip_keeps_its_place_in_the_loop() {
    let first_of_bar = |s: &mut Offline| hz(&run(s, BAR)[2000..22_000]);
    let mut s = ready();
    s.eval(
        "clip long { instrument: pluck\n notes: [c4 e4 g4 b4]\n length: 4bars }\nlong.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, 2 * BAR);
    // two bars in, the lane's third step is playing: a restart would begin on c5 instead
    s.eval(
        "clip long { instrument: pluck\n notes: [c5 e5 g5 b5]\n length: 4bars }",
        Some("now"),
    )
    .unwrap();
    let heard = first_of_bar(&mut s);
    assert!(
        (heard - 784.0).abs() < 20.0,
        "the new notes sound from the third step, not from the start: {heard}"
    );
    assert!(s.clips().iter().any(|c| c.name == "long" && c.running));
}

#[test]
fn redefining_a_running_clip_in_a_snippet_that_also_starts_it_still_changes_it() {
    let first_of_bar = |s: &mut Offline| hz(&run(s, BAR)[2000..22_000]);
    let mut s = ready();
    s.eval(
        "clip long { instrument: pluck\n notes: [c4 e4 g4 b4]\n length: 4bars }\nlong.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, 2 * BAR);
    // `start` on a running clip does nothing, so the new definition has to be applied anyway
    s.eval(
        "clip long { instrument: pluck\n notes: [c5 e5 g5 b5]\n length: 4bars }\nlong.start",
        Some("now"),
    )
    .unwrap();
    let heard = first_of_bar(&mut s);
    assert!(
        (heard - 784.0).abs() < 20.0,
        "the new notes sound from the third step: {heard}"
    );
}

#[test]
fn mute_and_solo_survive_stop_and_hush_and_go_with_reset() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start\nlead.mute\nbass.solo", Some("now"))
        .unwrap();
    run(&mut s, 24_000);
    let state = |s: &Offline| {
        s.clips()
            .iter()
            .map(|c| (c.muted, c.soloed))
            .collect::<Vec<_>>()
    };
    assert_eq!(state(&s), [(true, false), (false, true)]);
    s.eval("verse.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    s.eval("hush", Some("now")).unwrap();
    run(&mut s, 24_000);
    assert_eq!(state(&s), [(true, false), (false, true)], "still there");
    s.eval("reset", Some("now")).unwrap();
    run(&mut s, 2 * BAR);
    assert_eq!(
        state(&s),
        [(false, false), (false, false)],
        "reset clears them"
    );
}

#[test]
fn a_clip_that_plays_once_stops_running_when_it_is_over() {
    let mut s = ready();
    s.eval(
        "clip once { instrument: pluck\n notes: [c4]\n length: 1bar\n times: 1 }\nonce.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, BAR / 2);
    s.eval("once.set(gain: -6db)", Some("now")).unwrap();
    let status = |s: &Offline| s.clips().into_iter().find(|c| c.name == "once").unwrap();
    assert!(status(&s).running);
    assert_eq!(status(&s).edits, ["set(gain: -6db)"]);
    run(&mut s, 2 * BAR);
    s.advance();
    assert!(!status(&s).running, "finished");
    assert!(status(&s).edits.is_empty(), "and the changes with it");
    s.eval("once.start", Some("now")).unwrap();
    run(&mut s, 2000);
    assert!(status(&s).running, "it can be started again");
    assert!(status(&s).edits.is_empty(), "from the clip as written");
}

#[test]
fn a_clip_can_play_a_number_of_times_and_then_start_another() {
    let mut s = session();
    s.eval(
        "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip intro { instrument: pluck\n notes: [c4]\n length: 1bar\n times: 2\n then: verse.start }\nclip verse { instrument: pluck\n notes: [e4] }\nintro.start",
        Some("now"),
    )
    .unwrap();
    let running = |s: &Offline| {
        s.clips()
            .into_iter()
            .filter(|c| c.running)
            .map(|c| c.name)
            .collect::<Vec<_>>()
    };
    run(&mut s, BAR / 2);
    assert_eq!(running(&s), ["intro"], "the intro plays first");
    run(&mut s, BAR);
    assert_eq!(running(&s), ["intro"], "and again, its second time");
    run(&mut s, BAR);
    s.advance();
    assert_eq!(running(&s), ["verse"], "the verse follows the last pass");
}

#[test]
fn times_can_be_added_to_a_running_clip() {
    let mut s = ready();
    s.eval("lead.__update([c4])", Some("now")).unwrap();
    run(&mut s, BAR / 2);
    let running = |s: &Offline| {
        s.clips()
            .into_iter()
            .find(|c| c.name == "lead")
            .unwrap()
            .running
    };
    assert!(running(&s));
    s.eval("lead.times(2)", Some("now")).unwrap();
    run(&mut s, 3 * BAR);
    s.advance();
    assert!(!running(&s), "it played its two times and finished");
}

#[test]
fn a_clip_stopped_before_its_last_pass_starts_nothing() {
    let mut s = session();
    s.eval(
        "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip intro { instrument: pluck\n notes: [c4]\n length: 1bar\n times: 2\n then: verse.start }\nclip verse { instrument: pluck\n notes: [e4] }\nintro.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, BAR);
    s.eval("intro.stop", Some("now")).unwrap();
    run(&mut s, 3 * BAR);
    s.advance();
    assert!(s.clips().iter().all(|c| !c.running), "nothing is playing");
}

#[test]
fn clips_can_hand_over_in_a_loop() {
    let mut s = session();
    s.eval(
        "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip a { instrument: pluck\n notes: [c4]\n length: 1bar\n times: 1\n then: b.start }\nclip b { instrument: pluck\n notes: [e4]\n length: 1bar\n times: 1\n then: a.start }\na.start",
        Some("now"),
    )
    .unwrap();
    let running = |s: &Offline| {
        s.clips()
            .into_iter()
            .filter(|c| c.running)
            .map(|c| c.name)
            .collect::<Vec<_>>()
    };
    let mut seen = Vec::new();
    for _ in 0..4 {
        run(&mut s, BAR / 2);
        seen.push(running(&s));
        run(&mut s, BAR / 2);
    }
    assert_eq!(seen, [["a"], ["b"], ["a"], ["b"]]);
}

const SONG_PARTS: &str = "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip a { instrument: pluck\n notes: [c4] }\nclip b { instrument: pluck\n notes: [e4] }\nscene one { a }\nscene two { b }\narrangement song {\n  at +0bars one.start\n  at +1bar one.stop\n  at +1bar two.start\n  at +3bars two.stop\n}\n";

#[test]
fn an_arrangement_runs_its_lines_from_when_it_is_started() {
    let mut s = session();
    s.eval(SONG_PARTS, Some("now")).unwrap();
    run(&mut s, 5 * BAR / 2);
    assert!(
        s.clips().iter().all(|c| !c.running),
        "defining it plays nothing"
    );
    s.eval("song.start", Some("now")).unwrap();
    let first = run(&mut s, BAR / 2);
    assert!(
        (hz(&first[2000..22_000]) - 262.0).abs() < 10.0,
        "scene one: c4"
    );
    assert!(s.clips().iter().find(|c| c.name == "a").unwrap().running);
    run(&mut s, BAR / 2);
    let second = run(&mut s, BAR);
    assert!(
        (hz(&second[2000..22_000]) - 330.0).abs() < 12.0,
        "scene two: e4"
    );
    run(&mut s, 2 * BAR);
    s.advance();
    assert!(
        s.clips().iter().all(|c| !c.running),
        "and it stopped itself"
    );
    assert_eq!(s.defined("arrangement"), ["song"]);
}

#[test]
fn stopping_an_arrangement_drops_what_it_has_not_got_to() {
    let mut s = session();
    s.eval(SONG_PARTS, Some("now")).unwrap();
    s.eval("song.start", Some("now")).unwrap();
    run(&mut s, BAR / 2);
    s.eval("song.stop", Some("now")).unwrap();
    // the clip that had started carries on, and nothing after it happens: not scene two, not the stops
    let rest = run(&mut s, 3 * BAR);
    s.advance();
    let status = s.clips();
    assert!(
        status.iter().find(|c| c.name == "a").unwrap().running,
        "still going"
    );
    assert!(
        !status.iter().find(|c| c.name == "b").unwrap().running,
        "never started"
    );
    assert!(peak(&rest[2 * BAR..]) > 0.05, "it did not stop either");
}

#[test]
fn an_arrangement_can_only_be_started_and_stopped() {
    let mut s = session();
    s.eval(SONG_PARTS, Some("now")).unwrap();
    let problems = s.eval("song.mute", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("is an arrangement"),
        "{}",
        problems[0].message
    );
}

#[test]
fn notes_written_below_a_use_line_point_at_the_sent_text() {
    let dir = rig_dir("instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\n");
    let sent = "use rig\nclip riff {\n  instrument: pluck\n  notes: [c4 e4]\n}\nriff.start\n";
    let mut s = session();
    s.eval_parsed(niminal_live::ParsedEval::from_dir(sent, &dir, Some("now")).with_origin(7))
        .unwrap();
    let written: Vec<_> = s.clips()[0].lanes[0]
        .events
        .iter()
        .map(|e| {
            let (start, end) = e.src.expect("a written note has a source range");
            assert_eq!(e.eval, Some(7));
            sent[start..end].to_string()
        })
        .collect();
    assert_eq!(written, ["c4", "e4"]);
}
