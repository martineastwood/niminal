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
        "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\ntrack lead { instrument = pluck }\n",
    );
    let mut s = session();
    s.eval_parsed(niminal_daemon::ParsedEval::from_dir(
        "use rig\nlead.play([c4])",
        &dir,
        Some("now"),
    ))
    .unwrap();
    let original = run(&mut s, BAR);
    assert!(peak(&original) > 0.1);
    std::fs::remove_dir_all(&dir).unwrap();
    let replayed = Session::replay(
        SR,
        Layout::Mono,
        QuantizeDefaults::default(),
        s.log(),
        BAR as u64,
        false,
    );
    assert_eq!(original, replayed[0]);
}

#[test]
fn a_problem_in_a_used_module_points_at_the_use_line() {
    let dir = rig_dir("instr broken() { nonsense(1) }\n");
    let mut s = session();
    let problems = s
        .eval_parsed(niminal_daemon::ParsedEval::from_dir(
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
    s.eval("at +2bars lead.play([c4])", Some("now")).unwrap();
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
    let accepted = s.eval("at bar 3 lead.play([c4])", Some("now")).unwrap();
    assert_eq!(accepted.position.0, 1);
    let before = run(&mut s, 2 * BAR - 100);
    let after = run(&mut s, BAR);
    assert!(
        peak(&before) < 1e-6 && peak(&after) > 0.1,
        "bar 3 is two bars after the reset, not 3bars after the start"
    );
}

#[test]
fn a_track_playing_a_launched_scene_can_be_changed_by_name() {
    let mut s = ready();
    s.eval(
        "riff = [c4 e4 g4 b4]\nscene verse { lead: riff, bass: riff }\nverse.start",
        Some("now"),
    )
    .unwrap();
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
        "riff = [a3]\nscene verse { lead: riff }\nverse.start",
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
fn stopping_a_scene_stops_its_tracks_and_a_track_with_nothing_playing_cannot_be_changed() {
    let mut s = ready();
    s.eval(
        "riff = [c4]\nscene verse { lead: riff, bass: riff }\nverse.start",
        Some("now"),
    )
    .unwrap();
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
    let first_beat = |s: &mut Session| hz(&run(s, BAR)[2000..22_000]);
    let mut s = ready();
    s.eval(
        "riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nverse.start",
        Some("now"),
    )
    .unwrap();
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
        (first_beat(&mut s) - 262.0).abs() < 10.0,
        "launching the scene again starts from its clips"
    );
}

#[test]
fn a_redefined_clip_keeps_the_changes_made_to_a_track() {
    let mut s = ready();
    s.eval(
        "riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nverse.start\nlead.reverse",
        Some("now"),
    )
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
    let a = thinned("lead.play([c4 e4 g4 b4 c5 e5 g5 b5].thin(50%))");
    let b = thinned("riff = [c3]\nlead.play([c4 e4 g4 b4 c5 e5 g5 b5].thin(50%))");
    assert_eq!(a, b);
}

#[test]
fn changes_work_on_a_project_that_has_timed_commands_in_it() {
    let mut s = ready();
    s.eval("riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nscene chorus { lead: riff }\nverse.start\nat +4bars chorus.start\nat +8bars hush", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("lead.reverse", Some("now")).unwrap();
    assert!(
        (hz(&run(&mut s, BAR)[2000..22_000]) - 494.0).abs() < 15.0,
        "the later commands didn't change what is playing now"
    );
    s.eval("lead.clear", Some("now")).unwrap();
}

const PARTS: &str = "
tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
clip lead { instrument: pluck
  notes: [c4 e4 g4 b4] }
clip bass { instrument: pluck
  notes: [c2] }
scene verse { lead, bass }
scene bridge { lead }
";

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
fn launching_a_scene_replaces_the_parts_it_does_not_list() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("bridge.start", Some("now")).unwrap();
    run(&mut s, 24_000);
    let status = s.tracks();
    let names: Vec<_> = status
        .iter()
        .map(|t| (t.name.as_str(), t.clip.is_some()))
        .collect();
    assert_eq!(names, [("lead", true), ("bass", false)]);
    let problems = s.eval("bass.reverse", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("isn't playing"),
        "{}",
        problems[0].message
    );
}

#[test]
fn a_bus_with_a_body_processes_what_instruments_send_to_it() {
    let heard = |gain: &str| {
        let mut s = Session::new(SR, Layout::Stereo).without_limiter();
        let code = format!(
            "tempo 120bpm\nbus space: stereo {{ out = it.gain({gain}) }}\ninstr ping(freq: hz) {{ out = osc(sine, freq) * env[1 | 10ms 0]\n space += out }}\nclip p {{ instrument: ping\n notes: [c4] }}\np.start"
        );
        s.eval(&code, Some("now")).unwrap();
        let out = s.process(BAR);
        assert!(
            s.tracks().iter().all(|t| t.name != "space.fx"),
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
    let level = |s: &mut Session| peak(&run(s, BAR)[2000..]);
    let mut s = session();
    s.eval("tempo 120bpm\ninstr loud(freq: hz, amp: 0..1 = 1) { osc(sine, freq) * amp * env[1 | 10ms 0] }\nclip lead { instrument: loud\n notes: [c4] }\nlead.start", Some("now")).unwrap();
    let full = level(&mut s);
    assert!(full > 0.9, "{full}");
    s.eval("lead.set(amp: 0.25)", Some("now")).unwrap();
    let quiet = level(&mut s);
    assert!((quiet - 0.25).abs() < 0.03, "{quiet}");
    assert_eq!(s.tracks()[0].edits, ["set(amp: 0.25)"]);
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
    s.eval(
        "riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nverse.start",
        Some("now"),
    )
    .unwrap();
    s.eval(
        "lead.reverse\nlead.transpose(+12st)\nlead.thin(10%)",
        Some("now"),
    )
    .unwrap();
    assert_eq!(
        s.tracks()[0].edits,
        ["reverse", "transpose(+12st)", "thin(10%)"]
    );
    s.eval("lead.undo(reverse)", Some("now")).unwrap();
    assert_eq!(
        s.tracks()[0].edits,
        ["transpose(+12st)", "thin(10%)"],
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
fn a_change_named_for_a_clip_goes_to_every_part_playing_it() {
    let mut s = ready();
    s.eval("answer = [c4 e4 g4 b4]\nother = [c3]\nscene chorus { lead: answer, bass: answer.fast(2) }\nchorus.start", Some("now")).unwrap();
    s.eval("answer.reverse", Some("now")).unwrap();
    let edits: Vec<_> = s.tracks().iter().map(|t| t.edits.clone()).collect();
    assert_eq!(
        edits,
        [vec!["reverse"], vec!["reverse"]],
        "both parts play `answer`"
    );
    s.eval("answer.clear", Some("now")).unwrap();
    assert!(s.tracks().iter().all(|t| t.edits.is_empty()));
    let problems = s.eval("other.reverse", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("isn't playing anything"),
        "{}",
        problems[0].message
    );
}
