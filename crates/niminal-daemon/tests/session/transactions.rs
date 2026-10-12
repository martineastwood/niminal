use crate::common::*;

// ---- determinism ---------------------------------------------------------------

/// Drive a session through a performance, dividing time into blocks of `block`
/// samples and sending each input at the first block boundary at or after the
/// time it was meant for.
fn performance(block: usize) -> Session {
    let inputs: [(usize, &str, Option<&str>); 7] = [
        (0, SETUP, Some("now")),
        (0, "riff = [a3 e4 a4 e4]\nlead.play(riff)", None),
        (30_000, "bass.play([a2*2])", Some("next beat")),
        (70_000, "riff = [a3 c4 e4 g4]", None),
        (150_000, "bass.mute @ next bar", None),
        (
            210_000,
            "instr pluck(freq: hz) { osc(saw, freq) * env[1 | 30ms 0] }",
            Some("next 2 beats"),
        ),
        (330_000, "hush", Some("next bar")),
    ];
    let mut s = Session::new(SR, Layout::Mono);
    let mut i = 0;
    while s.clock() < 600_000 {
        while i < inputs.len() && inputs[i].0 as u64 <= s.clock() {
            let (_, src, q) = inputs[i];
            s.eval(src, q).unwrap();
            i += 1;
        }
        s.process(block);
    }
    s
}

#[test]
fn a_replay_reproduces_the_original_audio_exactly() {
    for block in [64, 480, 777, 1024] {
        let mut s = Session::new(SR, Layout::Mono);
        let inputs: [(usize, &str, Option<&str>); 4] = [
            (0, SETUP, Some("now")),
            (0, "riff = [a3 e4 a4 e4]\nlead.play(riff)", None),
            (30_000, "bass.play([a2*2])", Some("next beat")),
            (70_000, "riff = [a3 c4 e4 g4]", None),
        ];
        let mut original: Vec<f32> = Vec::new();
        let mut i = 0;
        while s.clock() < 300_000 {
            while i < inputs.len() && inputs[i].0 as u64 <= s.clock() {
                s.eval(inputs[i].1, inputs[i].2).unwrap();
                i += 1;
            }
            original.extend(s.process(block).remove(0));
        }
        let total = s.clock();
        let replayed = Session::replay(
            SR,
            Layout::Mono,
            QuantizeDefaults::default(),
            s.log(),
            total,
            true,
        )
        .remove(0);
        assert_eq!(original.len(), replayed.len(), "block {block}");
        assert_eq!(
            original, replayed,
            "block {block}: replaying the log gives back the audio"
        );
    }
}

#[test]
fn logs_survive_being_written_down() {
    let s = performance(480);
    let text = to_lines(s.log());
    let back = from_lines(&text).unwrap();
    assert_eq!(back, s.log());
    let a = Session::replay(
        SR,
        Layout::Mono,
        QuantizeDefaults::default(),
        s.log(),
        100_000,
        true,
    );
    let b = Session::replay(
        SR,
        Layout::Mono,
        QuantizeDefaults::default(),
        &back,
        100_000,
        true,
    );
    assert_eq!(a, b);
}

#[test]
fn a_live_eval_plays_a_kit_from_the_sample_folder_and_picks_up_a_changed_file() {
    let dir = std::env::temp_dir().join(format!("niminal-live-kit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("kit")).unwrap();
    let write = |name: &str, level: f32| {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(dir.join("kit").join(name), spec).unwrap();
        (0..4800).for_each(|_| w.write_sample((level * 32767.0) as i16).unwrap());
        w.finalize().unwrap();
    };
    write("kick.wav", 0.5);
    write("snare.wav", 0.25);

    let mut s = session();
    s.set_sample_dir(&dir);
    s.eval(
        "tempo 120bpm\nkit k = \"kit\"\ntrack t { instrument = k }",
        Some("now"),
    )
    .unwrap();
    s.eval("t.play([snare ~ ~ ~])", Some("now")).unwrap();
    let out = run(&mut s, 2000);
    assert!((out[500] - 0.25).abs() < 0.01, "{}", out[500]);

    // code sent from another folder doesn't lose the files the project already named
    let elsewhere = std::env::temp_dir();
    s.set_sample_dir(&elsewhere);
    s.eval("t.play([snare ~ ~ ~])", Some("now")).unwrap();
    s.set_sample_dir(&dir);

    // a bad name is rejected without disturbing what plays
    let problems = s.eval("t.play([snar])", Some("now")).unwrap_err();
    assert!(
        problems[0].message.contains("did you mean `snare`?")
            || problems[0].help.as_deref() == Some("did you mean `snare`?")
    );

    // replacing a file is seen the next time the project is evaluated
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write("snare.wav", -0.25);
    s.eval("kit k = \"kit\"", Some("now")).unwrap();
    s.eval("t.play([~ snare ~ ~])", Some("now")).unwrap();
    let out = run(&mut s, 26_000);
    // the clip starts where it lands, so its second step is a quarter bar later
    assert!(out[24_500] < 0.0, "{}", out[24_500]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compiling_can_happen_away_from_the_session() {
    use niminal_daemon::Begin;
    let mut s = ready();
    let Begin::Job(job) = s.begin_eval("lead.play([c4])", Some("now")) else {
        panic!("expected something to compile")
    };
    // the session is free to run while the code compiles elsewhere
    run(&mut s, 1000);
    let done = job.compile();
    let accepted = s.finish_eval(done).unwrap();
    assert_eq!(
        accepted.lands_at, 1000,
        "it lands now, which is when the session finished it"
    );
    assert!(rms(&run(&mut s, 12_000)[2000..]) > 0.1);
    // and what the session logged replays to the same sound
    let replayed = Session::replay(
        SR,
        Layout::Mono,
        QuantizeDefaults::default(),
        s.log(),
        13_000,
        false,
    );
    assert!(rms(&replayed[0][3000..]) > 0.1);
}

#[test]
fn an_evaluation_compiled_before_another_landed_is_compiled_again() {
    use niminal_daemon::Begin;
    let mut s = ready();
    let Begin::Job(first) = s.begin_eval("clip riff { notes: [c4 e4] }", Some("next bar")) else {
        panic!()
    };
    let Begin::Job(second) = s.begin_eval("lead.play(riff)", Some("next bar")) else {
        panic!()
    };
    // `second` was prepared before `riff` existed; finishing the first moves the project on
    let first = first.compile();
    let second = second.compile();
    s.finish_eval(first).unwrap();
    s.finish_eval(second)
        .expect("it is compiled again against the project that now has `riff`");
    assert_eq!(s.pending().len(), 2);
}

#[test]
fn a_rejected_evaluation_through_the_two_step_path_changes_nothing() {
    use niminal_daemon::Begin;
    let mut s = ready();
    let Begin::Job(job) = s.begin_eval("lead.play([c4 zz])", Some("now")) else {
        panic!()
    };
    let problems = s.finish_eval(job.compile()).unwrap_err();
    assert!(problems[0].message.contains("zz"));
    assert!(s.pending().is_empty());
}

#[test]
fn a_new_play_replaces_a_pending_play_for_the_same_track() {
    let mut s = ready();
    run(&mut s, 1000);
    let old = s.eval("lead.play([c4]) @ next 4 bars", None).unwrap();
    let new = s.eval("lead.play([a4]) @ next beat", None).unwrap();
    assert!(new.lands_at < old.lands_at);
    assert_eq!(s.pending().len(), 1);
    assert_eq!(s.pending()[0].id, new.id.unwrap());
    let mut reference = ready();
    run(&mut reference, 1000);
    reference.eval("lead.play([a4]) @ next beat", None).unwrap();
    assert_eq!(run(&mut s, 5 * BAR), run(&mut reference, 5 * BAR));
    let landed = s.take_landed();
    assert!(!landed.iter().any(|l| Some(l.id) == old.id));
}

#[test]
fn replacing_one_write_preserves_the_rest_of_an_older_transaction() {
    let mut s = ready();
    run(&mut s, 1000);
    let old = s
        .eval(
            "lead.play([c4])
bass.play([c2])",
            Some("next 4 bars"),
        )
        .unwrap();
    s.eval("lead.play([a4])", Some("now")).unwrap();
    assert_eq!(s.pending().len(), 1);
    assert_eq!(s.pending()[0].id, old.id.unwrap());
    assert_eq!(s.pending()[0].changes, ["bass.play([c2])"]);
    let mut reference = ready();
    run(&mut reference, 1000);
    reference.eval("lead.play([a4])", Some("now")).unwrap();
    reference
        .eval("bass.play([c2])", Some("next 4 bars"))
        .unwrap();
    assert_eq!(run(&mut s, 5 * BAR), run(&mut reference, 5 * BAR));
}

#[test]
fn a_later_definition_snapshot_cannot_revert_a_newer_write() {
    let mut s = ready();
    run(&mut s, 1000);
    s.eval(
        "instr pluck(freq: hz) { osc(sine, freq) * 0.1 }
clip extra { notes: [c4] }",
        Some("next 4 bars"),
    )
    .unwrap();
    s.eval(
        "instr pluck(freq: hz) { osc(sine, freq) * 0.8 }",
        Some("now"),
    )
    .unwrap();
    run(&mut s, 5 * BAR);
    s.eval("pluck(freq: a4) for 1beat", Some("now")).unwrap();
    assert!(peak(&run(&mut s, 5000)) > 0.7);
}

#[test]
fn rejected_replacements_leave_the_pending_transaction_intact() {
    let mut s = ready();
    run(&mut s, 1000);
    let old = s.eval("lead.play([c4])", Some("next bar")).unwrap();
    assert!(s.eval("lead.play(nonexistent)", Some("now")).is_err());
    assert_eq!(s.pending()[0].id, old.id.unwrap());
    run(&mut s, BAR);
    assert!(s.take_landed().iter().any(|l| Some(l.id) == old.id));
}

#[test]
fn an_effect_parameter_edit_keeps_its_tail_and_uses_the_new_setting() {
    let mut live = with_tail();
    run(&mut live, 5000);
    live.eval(
        "track hall { out = space.reverb(room: 0.95, damp: 0.2) }",
        Some("now"),
    )
    .unwrap();
    assert!(peak(&run(&mut live, BAR)) > 1e-4);
}

#[test]
fn panic_clears_the_limiter_lookahead_as_well_as_the_mixer() {
    let mut s = Session::new(SR, Layout::Mono);
    s.eval(SETUP, Some("now")).unwrap();
    s.eval("pluck(freq: a4) for 4beats", Some("now")).unwrap();
    assert!(peak(&run(&mut s, 1000)) > 0.1);
    let epoch = s.panic_epoch();
    s.panic();
    assert_eq!(epoch.load(std::sync::atomic::Ordering::Acquire), 1);
    assert_eq!(peak(&run(&mut s, 1000)), 0.0);
}

#[test]
fn a_track_edit_keeps_memory_but_applies_the_new_output_gain() {
    let mut live = with_tail();
    let mut reference = with_tail();
    run(&mut live, 5000);
    run(&mut reference, 5000);
    live.eval(
        "track hall { out = space.reverb(room: 0.8, damp: 0.5) * 0.5 }",
        Some("now"),
    )
    .unwrap();
    let expected = run(&mut reference, BAR);
    let actual = run(&mut live, BAR);
    assert!(peak(&expected) > 1e-4);
    for (a, b) in actual.iter().zip(expected) {
        assert_eq!(*a, b * 0.5);
    }
}

#[test]
fn instrument_replacements_do_not_supersede_explicitly_timed_notes() {
    let mut s = ready();
    run(&mut s, 1000);
    s.eval("at bar 3 lead.play([c4])", Some("next bar"))
        .unwrap();
    s.eval("lead.play([a4])", Some("next beat")).unwrap();
    assert_eq!(s.pending().len(), 2);
    run(&mut s, 3 * BAR);
    assert_eq!(s.take_landed().len(), 3);
}

#[test]
fn replacing_a_pending_definition_moves_its_dependent_play_to_the_new_boundary() {
    let mut s = ready();
    run(&mut s, 1000);
    s.eval("riff = [c4]", Some("next beat")).unwrap();
    s.eval("lead.play(riff)", Some("next bar")).unwrap();
    let replacement = s.eval("riff = [a4]", Some("next 4 bars")).unwrap();
    assert_eq!(s.pending().len(), 2);
    assert!(
        s.pending()
            .iter()
            .all(|p| p.lands_at == replacement.lands_at)
    );
    let mut reference = ready();
    run(&mut reference, 1000);
    reference
        .eval(
            "riff = [a4]
lead.play(riff)",
            Some("next 4 bars"),
        )
        .unwrap();
    assert_eq!(run(&mut s, 5 * BAR), run(&mut reference, 5 * BAR));
}

#[test]
fn a_slow_compile_moves_to_the_next_boundary_and_reports_it() {
    let mut s = ready();
    run(&mut s, 23_000);
    let niminal_daemon::Begin::Job(job) = s.begin_eval("lead.play([a4])", Some("next beat")) else {
        panic!()
    };
    let done = job.compile();
    run(&mut s, 10_000);
    let accepted = s.finish_eval(done).unwrap();
    assert_eq!(accepted.lands_at, 48_000);
    assert!(
        s.take_notices()
            .iter()
            .any(|n| n.contains("missed its boundary"))
    );
}

#[test]
fn superseded_pending_changes_replay_to_the_same_audio() {
    let mut s = ready();
    let mut original = run(&mut s, 1000);
    s.eval(
        "riff = [c4]
bass.play([c2])",
        Some("next 4 bars"),
    )
    .unwrap();
    s.eval(
        "riff = [a4]
lead.play(riff)",
        Some("now"),
    )
    .unwrap();
    original.extend(run(&mut s, 5 * BAR));
    let replayed = Session::replay(
        SR,
        Layout::Mono,
        QuantizeDefaults::default(),
        s.log(),
        original.len() as u64,
        false,
    );
    assert_eq!(original, replayed[0]);
}

#[test]
fn adding_a_bus_keeps_existing_effect_memory() {
    let mut live = with_tail();
    let mut reference = with_tail();
    run(&mut live, 5000);
    run(&mut reference, 5000);
    live.eval("bus extra", Some("now")).unwrap();
    assert_eq!(run(&mut live, BAR), run(&mut reference, BAR));
}
