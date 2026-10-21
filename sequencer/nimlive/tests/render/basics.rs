use crate::common::*;

#[test]
fn a_note_sounds_for_its_length_then_rings_out_its_release() {
    let out = render(&program(ONE_NOTE), &[]).unwrap();
    // 1beat at 120bpm is 0.5s; the 300ms release follows.
    let secs = out.len() as f64 / SR;
    assert!((0.79..0.81).contains(&secs), "{secs}");
    assert!(rms(&out[(0.3 * SR) as usize..(0.5 * SR) as usize]) > 0.05);
    // the release is linear, so it is still fading in its last 10ms, and ends at zero
    assert!(rms(&out[out.len() - 480..]) < 0.01);
    assert!(out[out.len() - 1].abs() < 1e-3);
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn tempo_changes_when_notes_land() {
    let slow = render(&program(&ONE_NOTE.replace("120bpm", "60bpm")), &[]).unwrap();
    let fast = render(&program(ONE_NOTE), &[]).unwrap();
    assert!(slow.len() > fast.len() + (0.4 * SR) as usize);
}

#[test]
fn rendering_is_deterministic() {
    let p = program(SAW_LEAD);
    assert_eq!(render(&p, &[]).unwrap(), render(&p, &[]).unwrap());
}

#[test]
fn notes_are_placed_sample_accurately_and_overlap() {
    let src = "\ntempo 120bpm\ninstr click() { osc(sine, 1000hz) * env[1 0.5ms 0] }\nat 250ms click() for 1beat\nat 251ms click() for 1beat\n";
    let out = render(&program(src), &[]).unwrap();
    let first = (0.250 * SR) as usize;
    assert!(
        out[..first].iter().all(|s| *s == 0.0),
        "silent before the first note"
    );
    assert!(out[first].abs() < 1e-10, "sine starts at zero");
    assert!(out[first + 10] != 0.0);
}

#[test]
fn the_same_music_from_source_or_from_a_score_file() {
    let instruments = "\ninstr lead(freq: hz, amp: db = -6db) {\n  level = env[0 5ms 1 200ms 0.6 | 300ms 0]\n  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level\n}\n";
    let in_source = format!(
        "{instruments}\nlead(freq: e5, amp: -12db) for 1/2beat\nat 1/2beat lead(freq: g5, amp: -12db) for 1/2beat\nat 1beat lead(freq: b5, amp: -12db) for 1beat\n"
    );
    let score = PHI.replace("saw_lead", "lead");
    let events = Section::from_json(&score).unwrap().events;

    let a = render(&program(&in_source), &[]).unwrap();
    let b = render(&program(instruments), &events).unwrap();
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn the_examples_render() {
    let p = program(SAW_LEAD);
    let events = Section::from_json(PHI).unwrap().events;
    let out = render(&p, &events).unwrap();
    assert!(out.len() > (3.0 * SR) as usize);

    let stereo = render_with(
        &program(include_str!("../../../../examples/stereo.nml")),
        &[],
        &RenderOptions::default(),
    )
    .unwrap();
    assert_eq!(stereo.channels.len(), 2);
    assert!(stereo.peak() > 0.1 && stereo.peak() <= 0.9661);
    assert!(stereo.len() > (3.0 * SR) as usize);
    assert_ne!(stereo.channels[0], stereo.channels[1]);

    let room = render(&program(include_str!("../../../../examples/room.nml")), &[]).unwrap();
    assert!(room.len() > SR as usize);
    assert!(rms(&room) > 0.01);
}

#[test]
fn score_events_are_checked_against_the_instrument() {
    let p = program(ONE_NOTE);
    let render_score = |json: &str| {
        let events = Section::from_json(json).unwrap().events;
        render(&p, &events).unwrap_err().to_string()
    };
    let score = |target: &str, args: &str| {
        format!(
            r#"{{"version": 1, "section": "s", "length": "1beat",
                "events": [{{"at": "0beat", "dur": "1beat", "target": "{target}", "args": {args}}}]}}"#
        )
    };

    assert_eq!(
        render_score(&score("lede", r#"{"freq": "a3"}"#)),
        "note for `lede`: no instrument or clip named `lede`"
    );
    assert_eq!(
        render_score(&score("lead", r#"{"freq": "440"}"#)),
        "note for `lead`: argument `freq`: expected a frequency (hz or a note name), got `440` (did you mean `440hz`?)"
    );
    assert_eq!(
        render_score(&score("lead", r#"{"freq": "6db"}"#)),
        "note for `lead`: argument `freq`: expected a frequency (hz or a note name), got `6db`"
    );
    assert_eq!(
        render_score(&score("lead", "{}")),
        "note for `lead`: `lead` needs an argument `freq`"
    );
    assert_eq!(
        render_score(&score("lead", r#"{"freq": "a3", "amq": "-6db"}"#)),
        "note for `lead`: `lead` has no parameter `amq` (did you mean `amp`?)"
    );
}

#[test]
fn a_program_with_no_notes_renders_nothing() {
    let p = program("instr a() { osc(sine, 100hz) }");
    assert!(render(&p, &[]).unwrap().is_empty());
}

#[test]
fn the_limiter_keeps_loud_output_under_the_ceiling_and_can_be_turned_off() {
    let p = program("instr loud() { osc(saw, 220hz) * 4 }\nloud() for 1beat");
    let limited = mono(
        render_with(
            &p,
            &[],
            &RenderOptions {
                limiter: true,
                until: None,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let raw = mono(
        render_with(
            &p,
            &[],
            &RenderOptions {
                limiter: false,
                until: None,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&raw) > 3.0);
    assert!(peak(&limited) <= 0.9661, "{}", peak(&limited));
    assert_eq!(limited.len(), raw.len(), "latency is compensated");
}

#[test]
fn the_limiter_does_not_move_notes() {
    let src = "instr click() { osc(sine, 1000hz) * env[1 0.5ms 0] }\nat 250ms click() for 1beat";
    let p = program(src);
    let out = render(&p, &[]).unwrap();
    let first = (0.250 * SR) as usize;
    assert!(
        out[..first].iter().all(|s| s.abs() < 1e-6),
        "silent before the note"
    );
    assert!(
        out[first..].iter().any(|s| s.abs() > 0.1),
        "sounds right at the note start"
    );
    assert!(
        out.len() < first + 200,
        "and the silence after the click is trimmed"
    );
}

#[test]
fn an_arrangement_plays_its_scenes_in_order_and_then_stops() {
    let p = program(SONG);
    let out = mono(render_with(&p, &[], &arrangement("song")).unwrap());
    // a bar at 120bpm is 2s: scene one for 4s, scene two for 2s
    let freq_in = |from: f64, to: f64| {
        let x = &out[(from * SR) as usize..(to * SR) as usize];
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / (to - from)
    };
    assert!(
        (freq_in(0.2, 3.9) - 261.6).abs() < 6.0,
        "scene one plays c4"
    );
    assert!(
        freq_in(4.2, 5.9) > 280.0,
        "scene two plays e4 and g4 together"
    );
    let secs = out.len() as f64 / SR;
    assert!(
        secs < 6.2,
        "and nothing starts after the last section ends: {secs}"
    );
}

#[test]
fn an_arrangement_can_be_lines_placed_from_its_start() {
    let src = format!(
        "{SONG}arrangement lines {{\n  at +0bars one.start\n  at +1bar one.stop\n  at +1bar two.start\n  at +2bars two.stop\n}}"
    );
    let p = program(&src);
    let out = mono(render_with(&p, &[], &arrangement("lines")).unwrap());
    let freq_in = |from: f64, to: f64| {
        let x = &out[(from * SR) as usize..(to * SR) as usize];
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / (to - from)
    };
    assert!(
        (freq_in(0.2, 1.9) - 261.6).abs() < 6.0,
        "one plays c4 for a bar"
    );
    assert!(freq_in(2.2, 3.9) > 280.0, "then two plays e4 and g4");
    assert!(
        (out.len() as f64 / SR) < 4.5,
        "and it ends at its last line: {}",
        out.len() as f64 / SR
    );
}

#[test]
fn sections_can_have_their_own_tempo_and_meter() {
    let song = |lanes: &str| {
        format!(
            "{SONG}\narrangement fast {{ sections: [one.over(2bars) two.over(1bar)]\n{lanes} }}"
        )
    };
    let secs = |lanes: &str| {
        mono(render_with(&program(&song(lanes)), &[], &arrangement("fast")).unwrap()).len() as f64
            / SR
    };
    let plain = secs("");
    assert!((plain - 6.0).abs() < 0.2, "{plain}");
    // the second section at twice the speed lasts half as long
    let faster = secs("tempo: [120bpm 240bpm]");
    assert!((faster - 5.0).abs() < 0.2, "4s and 1s: {faster}");
    // and in 3/4 a bar is three beats: two bars of 3/4 at 120bpm are 3s, one bar of 7/8 (3.5beats) is 1.75s
    let meters = secs("meter: [3/4 7/8]");
    assert!((meters - 4.75).abs() < 0.2, "{meters}");
    // a section's clip cycles with its bar, so the notes of a faster section come faster
    let out = mono(
        render_with(
            &program(&song("tempo: [120bpm 120bpm]")),
            &[],
            &arrangement("fast"),
        )
        .unwrap(),
    );
    let quick = mono(
        render_with(
            &program(&song("tempo: [240bpm 240bpm]")),
            &[],
            &arrangement("fast"),
        )
        .unwrap(),
    );
    assert!(
        quick.len() * 2 < out.len() + 4800,
        "{} vs {}",
        quick.len(),
        out.len()
    );
}

#[test]
fn an_arrangement_tempo_can_change_gradually_through_a_section() {
    let src = format!(
        "{SONG}\narrangement ramp {{ sections: [two.over(8bars)]\n tempo: env[60bpm 8bars 240bpm] }}"
    );
    let out = mono(render_with(&program(&src), &[], &arrangement("ramp")).unwrap());
    // 240 / (60 + 22.5bar) summed over 8bars: about 14.8 seconds, between the 32s at 60bpm and 8s at 240bpm
    let secs = out.len() as f64 / SR;
    assert!(secs > 14.0 && secs < 15.6, "{secs}");
}

#[test]
fn render_with_an_arrangement_ignores_the_files_own_timeline() {
    let src = format!("{SONG}\nclip z {{ instrument: tone, notes: [c6] }}\nz.start");
    let p = program(&src);
    // c6 is 1046hz; the arrangement alone never plays it
    let out = mono(render_with(&p, &[], &arrangement("song")).unwrap());
    let x = &out[(0.2 * SR) as usize..(3.9 * SR) as usize];
    let freq = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / 3.7;
    assert!((freq - 261.6).abs() < 6.0, "{freq}");
}

#[test]
fn arrangement_mistakes_are_reported() {
    let err = |src: &str| {
        compile(src)
            .err()
            .map(|e| {
                e.iter()
                    .map(|d| d.message.clone() + &d.help.clone().unwrap_or_default())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    let base = "tempo 120bpm\ninstr t(freq: hz) { osc(sine, freq) }\nclip a { instrument: t, notes: [c4] }\nscene one { a }\n";
    assert!(
        err(&format!(
            "{base}arrangement s {{ sections: [on.over(1bar)] }}"
        ))
        .contains("no scene or clip named `on`did you mean `one`?")
    );
    assert!(
        err(&format!("{base}arrangement s {{ sections: [one] }}"))
            .contains("`one` needs a lengthfor example `one.over(8bars)`")
    );
    assert!(
        err(&format!("{base}arrangement s {{ volume: [1] }}"))
            .contains("`volume` isn't supported in an arrangement yet")
    );
    let two = "sections: [one.over(1bar) one.over(1bar)]";
    assert!(
        err(&format!(
            "{base}arrangement s {{ {two}\n tempo: [100bpm] }}"
        ))
        .contains("`tempo` has 1 where there are 2 sectionsgive `tempo` one for each section")
    );
    assert!(
        err(&format!("{base}arrangement s {{ {two}\n meter: [4/4 4] }}"))
            .contains("meter is a fraction of whole numbers")
    );
    assert!(
        err(&format!(
            "{base}arrangement s {{ {two}\n tempo: [100bpm 0bpm] }}"
        ))
        .contains("positive number of bpm")
    );
    assert!(
        err(&format!(
            "{base}arrangement s {{ sections: [one.over(0bars)] }}"
        ))
        .contains("above zero")
    );
    assert!(err(&format!("{base}arrangement s {{ sections: [one.over(1bar)] }}\narrangement s {{ sections: [one.over(1bar)] }}")).contains("defined twice"));

    let p = program(&format!(
        "{base}arrangement song {{ sections: [one.over(1bar)] }}"
    ));
    let Err(e) = render_with(&p, &[], &arrangement("sogn")) else {
        panic!("expected an error")
    };
    assert!(
        e.0.contains("no arrangement named `sogn`") && e.0.contains("did you mean `song`?"),
        "{e}"
    );
}

#[test]
fn an_arrangement_lane_moves_a_node_parameter_over_the_whole_piece() {
    let eighths = |x: &[f32]| x.chunks(x.len() / 8).take(8).map(rms).collect::<Vec<_>>();
    let steady = eighths(&lane_song(""));
    let swept = eighths(&lane_song("pad_fx.cut: env[300hz 2bars 8khz]"));
    assert!(swept[0] < swept[1] && swept[1] < swept[2], "{swept:?}");
    assert!(swept[6] > steady[6] * 2.0, "{swept:?} against {steady:?}");
}

#[test]
fn arrangement_lane_mistakes_are_reported() {
    let err = |lane: &str| {
        let src = format!("{LANE_STAGE}arrangement song {{ sections: [s.over(1bar)]\n{lane} }}");
        compile(&src)
            .err()
            .map(|e| {
                e.iter()
                    .map(|d| d.message.clone() + &d.help.clone().unwrap_or_default())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    assert!(err("pd.cut: env[300hz 1bar 1khz]").contains("no clip named `pd`did you mean `pad`?"));
    assert!(
        err("pad_fx.cutt: env[300hz 1bar 1khz]")
            .contains("has no such parameterdid you mean `cut`?"),
        "{}",
        err("pad_fx.cutt: env[300hz 1bar 1khz]")
    );
    assert!(err("pad_fx.cut: 300hz").contains("an arrangement lane is an envelope"));
    assert!(err("pad_fx.cut: env[0.5 1bar 1]").contains("cut"));
    assert!(err("pad_fx.cut: env[300hz 1bar 1khz | 1bar 300hz]").contains("sustain"));
    assert!(
        err("pad_fx.cut: env[300hz 1bar 1khz]\npad_fx.cut: env[300hz 1bar 2khz]")
            .contains("given twice")
    );
}

#[test]
fn render_command_takes_an_arrangement_and_a_passage() {
    let (whole, wav) = render_command(&["song"]);
    assert!(
        whole.status.success(),
        "{}",
        String::from_utf8_lossy(&whole.stderr)
    );
    let full = seconds_of(&wav);
    assert!((6.0..6.5).contains(&full), "{full}");

    // bar 2 onwards drops the first bar (2s)
    let (from, wav) = render_command(&["song", "--from", "bar 2"]);
    assert!(from.status.success());
    assert!((seconds_of(&wav) - (full - 2.0)).abs() < 0.01);

    // stopping at bar 3 stops starting notes there, though scene one's tone rings out its release
    let (to, wav) = render_command(&["song", "--to", "bar 2"]);
    assert!(to.status.success());
    assert!(seconds_of(&wav) < 2.5, "{}", seconds_of(&wav));

    let (bad, _) = render_command(&["song", "--from", "sometime"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("isn't a position"));

    let (missing, _) = render_command(&["nope"]);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no arrangement named `nope`"));
}
