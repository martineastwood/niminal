use super::*;
use crate::compile;

const STAGE: &str = "\ntempo 120bpm\ninstr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: freq * (2 + bright * 6)) }\nclip lead { instrument: pluck }\nclip bass { instrument: pluck }\n";

#[test]
fn musical_durations_follow_the_tempo_of_the_sections_they_cross() {
    let tempo = Tempo::new(120.0);
    let quick = Tempo::new(240.0);
    let waltz = Tempo {
        beats_per_bar: 3.0,
        ..Tempo::new(120.0)
    };
    let s = Schedule::new(tempo, vec![], &[], &[(4.0, quick), (5.0, waltz)], &[]);
    // two bars at 120bpm are 4s, the next bar at 240bpm is 1s
    assert!((s.seconds_from(0.0, Time::Bars(3.0)) - 5.0).abs() < 1e-9);
    // starting in the quick section, four beats take a second
    assert!((s.seconds_from(4.0, Time::Beats(4.0)) - 1.0).abs() < 1e-9);
    // a bar of 3/4 at 120bpm is 1.5s, so 2.5bars from the start are 4s + 1s (one quick bar) + 0.5bar of 3/4
    assert!((s.seconds_from(0.0, Time::Bars(3.5)) - 5.75).abs() < 1e-9);
    assert_eq!(s.seconds_from(1.0, Time::Seconds(2.5)), 2.5);
}

fn schedule(body: &str) -> Schedule {
    let src = format!("{STAGE}{body}");
    compile(&src)
        .unwrap_or_else(|e| {
            panic!(
                "{}",
                e.iter().map(|d| d.render("t", &src)).collect::<String>()
            )
        })
        .schedule()
}

/// (seconds, node, freq as MIDI) of each note starting in the window.
fn notes(s: &Schedule, from: f64, to: f64) -> Vec<(f64, String, i32)> {
    s.events(from, to)
        .unwrap()
        .into_iter()
        .map(|e| {
            let Time::Seconds(at) = e.at else {
                panic!("scheduled notes are in seconds")
            };
            let Value::Note(m) = e.args["freq"] else {
                panic!("pitch is a note")
            };
            (at, e.target, m)
        })
        .collect()
}

fn times(s: &Schedule, from: f64, to: f64) -> Vec<f64> {
    s.events(from, to)
        .unwrap()
        .into_iter()
        .map(|e| e.at.to_seconds(Tempo::new(120.0)))
        .collect()
}

fn song(body: &str) -> Schedule {
    let src = format!(
        "{STAGE}clip lead4 {{ instrument: pluck, notes: [c4 c4 c4 c4] }}\nscene s {{ lead4 }}\n{body}"
    );
    compile(&src)
        .unwrap_or_else(|e| {
            panic!(
                "{}",
                e.iter().map(|d| d.render("t", &src)).collect::<String>()
            )
        })
        .arrangement_schedule("song")
        .unwrap()
}

#[test]
fn lines_can_be_placed_by_the_start_and_end_of_a_section() {
    // 4/4 at 120bpm: a bar is 2s, so the sections begin at 0, 8 and 16s
    let s = song(
        "clip riff { instrument: pluck, notes: [g4] }\n\
         scene b { lead4 }\n\
         arrangement song {\n\
           sections: [s.over(4bars) b.over(4bars) s.over(4bars) as again]\n\
           at again.start - 1bar riff.start\n\
           at b.end + 1beat riff.start\n\
           at b.start riff.start\n\
         }",
    );
    let riff: Vec<f64> = notes(&s, 0.0, 40.0)
        .into_iter()
        .filter(|n| n.1 == "riff")
        .map(|n| n.0)
        .collect();
    // `riff` is already playing from 8s, so only the first start of it lands; the rest carry on
    assert_eq!(riff.first().copied(), Some(8.0), "{riff:?}");
}

#[test]
fn a_clip_started_ahead_of_its_section_carries_on_into_it() {
    // `riff` belongs to the second scene but comes in 1 bar (2s) early; the section start must not restart it
    let s = song(
        "clip riff { instrument: pluck, notes: [g4 g4] }\n\
         scene b { lead4, riff }\n\
         arrangement song {\n\
           sections: [s.over(4bars) b.over(4bars)]\n\
           at b.start - 1bar riff.start\n\
         }",
    );
    let riff: Vec<f64> = notes(&s, 0.0, 8.5)
        .into_iter()
        .filter(|n| n.1 == "riff")
        .map(|n| n.0)
        .collect();
    // started at 6s: two notes a bar, so 6, 7, 8, 9... and 8s is one of them, not a second start
    assert_eq!(riff.first().copied(), Some(6.0), "{riff:?}");
    assert_eq!(riff.iter().filter(|t| (**t - 8.0).abs() < 1e-9).count(), 1, "{riff:?}");
}

#[test]
fn a_lane_can_start_at_a_section_marker() {
    // sections start at 0, 8 and 16s; the lane starts a bar (2s) before the third and takes a bar
    let s = song(
        "scene b { lead4 }\n\
         bus src\n\
         fx g(open: 0..1 = 0) { out = src * open }\n\
         arrangement song {\n\
           sections: [s.over(4bars) b.over(4bars) s.over(4bars) as again]\n\
           at again.start - 1bar g.open: env[0.2 1bar 0.9]\n\
           at again.start g.open: env[0.9 1bar 0.1]\n\
         }",
    );
    let c = s.controls(0.0, 40.0).unwrap();
    let at: Vec<(f64, f64)> = c
        .iter()
        .filter(|c| c.name.ends_with("::open"))
        .map(|c| (c.at, c.target))
        .collect();
    assert_eq!(at.first().map(|c| c.0), Some(14.0), "{at:?}");
    assert!(at.iter().any(|c| (c.0 - 16.0).abs() < 1e-9), "{at:?}");
    assert!(at.iter().all(|c| c.0 >= 14.0), "nothing before the lane starts: {at:?}");
}

#[test]
fn a_marker_offset_is_counted_in_the_tempo_of_its_section() {
    let s = song(
        "clip riff { instrument: pluck, notes: [g4] }\n\
         scene b { lead4 }\n\
         arrangement song {\n\
           sections: [s.over(4bars) b.over(4bars)]\n\
           tempo: [120bpm 60bpm]\n\
           at b.start + 1bar riff.start\n\
         }",
    );
    // b starts at 8s and its bars are 4s long
    let first = notes(&s, 0.0, 40.0)
        .into_iter()
        .find(|n| n.1 == "riff")
        .map(|n| n.0);
    assert_eq!(first, Some(12.0));
}

#[test]
fn a_marker_names_a_section_and_a_repeat_needs_its_own_name() {
    let src = |body: &str| {
        format!(
            "{STAGE}clip lead4 {{ instrument: pluck, notes: [c4] }}\nscene s {{ lead4 }}\narrangement song {{\n{body}\n}}"
        )
    };
    let error = |body: &str| {
        let e = compile(&src(body)).err().expect("an error");
        e.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n")
    };
    assert!(error("sections: [s.over(2bars)]\nat verse.start lead.start").contains("no section called `verse`"));
    assert!(error("sections: [s.over(2bars) s.over(2bars) as s]\n").contains("two sections are called `s`"));
    assert!(error("sections: [s.over(2bars)]\nat s.start - 1bar lead.start").contains("before the start"));
    assert!(compile(&src("sections: [s.over(2bars) s.over(2bars)]\nat s.end lead.start")).is_ok());
}

#[test]
fn a_gradual_tempo_change_speeds_up_the_notes_of_the_clip_playing_through_it() {
    // 4 notes a bar, for 8bars, the tempo rising evenly from 60bpm to 240bpm
    let s =
        song("arrangement song { sections: [s.over(8bars)]\n tempo: env[60bpm 8bars 240bpm] }");
    let at: Vec<f64> = s
        .events(0.0, 60.0)
        .unwrap()
        .iter()
        .map(|e| e.at.to_seconds(Tempo::new(120.0)))
        .collect();
    assert_eq!(at.len(), 32, "4 notes in each of 8bars");
    let gaps: Vec<f64> = at.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        gaps.windows(2).all(|g| g[1] <= g[0] + 1e-9),
        "each gap is no longer than the one before: {gaps:?}"
    );
    assert!(
        (gaps[0] - 1.0).abs() < 0.05,
        "a beat at 60bpm is a second: {}",
        gaps[0]
    );
    assert!(
        *gaps.last().unwrap() < 0.3,
        "and at the end it is near a quarter of that: {}",
        gaps.last().unwrap()
    );
    // 240 / (60 + 22.5bar) summed over the bars: about 14.8 seconds in all
    let last = *at.last().unwrap();
    assert!(last > 13.0 && last < 14.8, "{last}");
    assert!(
        s.end().unwrap() > 14.0 && s.end().unwrap() < 15.5,
        "{:?}",
        s.end()
    );
}

#[test]
fn a_tempo_that_holds_and_then_changes_only_changes_where_it_is_written() {
    // steady for 4bars at 120bpm (2s a bar), then a rise to 240bpm over the next 4
    let s = song(
        "arrangement song { sections: [s.over(8bars)]\n tempo: env[120bpm 4bars 120bpm 4bars 240bpm] }",
    );
    let at: Vec<f64> = s
        .events(0.0, 60.0)
        .unwrap()
        .iter()
        .map(|e| e.at.to_seconds(Tempo::new(120.0)))
        .collect();
    assert_eq!(at[0], 0.0);
    assert!(
        (at[16] - 8.0).abs() < 1e-6,
        "four bars of 2 seconds: {}",
        at[16]
    );
    assert!(
        (at[1] - at[0] - 0.5).abs() < 1e-9 && (at[15] - at[14] - 0.5).abs() < 1e-9,
        "a steady half second"
    );
    assert!(at[31] - at[30] < 0.4);
}

#[test]
fn a_tempo_envelope_is_checked() {
    let error = |body: &str| {
        let src = format!(
            "{STAGE}clip lead1 {{ instrument: pluck, notes: [c4] }}\nscene s {{ lead1 }}\n{body}"
        );
        compile(&src)
            .err()
            .map(|e| e.iter().map(|d| d.message.clone()).collect::<String>())
            .unwrap_or_default()
    };
    assert!(
        error("arrangement song { sections: [s.over(8bars)]\n tempo: env[60 8bars 240bpm] }")
            .contains("positive number of bpm")
    );
    assert!(
        error("arrangement song { sections: [s.over(8bars)]\n tempo: env[60bpm 8sec 240bpm] }")
            .contains("lengths in bars")
    );
    assert!(error("arrangement song { sections: [s.over(8bars)]\n tempo: env[60bpm 8bars 240bpm].loop }").contains("runs once"));
    assert!(error("arrangement song { sections: [s.over(8bars)]\n tempo: [60bpm]\n tempo: env[60bpm 8bars 240bpm] }").contains("given twice"));
}

#[test]
fn timed_lines_follow_one_start_with_another() {
    let s = schedule(
        r#"
        clip a { instrument: pluck, notes: [c4] }
        clip b { instrument: pluck, notes: [e4] }
        a.start
        at +1bar a.stop
        at +1bar b.start
        "#,
    );
    let early = notes(&s, 0.0, 1.9);
    let later = notes(&s, 2.0, 3.9);
    assert_eq!(early[0].2, 60);
    assert_eq!(later[0].2, 64);
}

#[test]
fn events_blocks_schedule_exact_notes() {
    let s = schedule(
        r#"
        clip fill {
          length: 1bar
          events {
            0beat c4 for 1/2beat gain: -6db
            1/2beat e4 for 1/2beat
          }
        }
        lead.__update(fill)
        "#,
    );
    let found = notes(&s, 0.0, 2.0);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].2, 60);
    assert_eq!(found[1].2, 64);
}

#[test]
fn a_looping_pattern_produces_notes_on_the_beat() {
    // 120bpm in 4/4: a bar is 2 seconds, so two steps are 1 second apart
    let s = schedule("lead.__update([c4 e4])");
    let n = notes(&s, 0.0, 4.0);
    assert_eq!(
        n,
        [
            (0.0, "lead".into(), 60),
            (1.0, "lead".into(), 64),
            (2.0, "lead".into(), 60),
            (3.0, "lead".into(), 64)
        ]
    );
    let e = s.events(0.0, 1.0).unwrap();
    assert_eq!(e[0].dur, Time::Seconds(1.0));
    assert_eq!(s.end(), None, "it plays until stopped");
}

#[test]
fn windows_select_the_notes_that_start_in_them() {
    let s = schedule("lead.__update([c4 e4 g4 b4])");
    assert_eq!(times(&s, 0.5, 2.0), [0.5, 1.0, 1.5]);
    assert_eq!(times(&s, 2.0, 2.0), Vec::<f64>::new());
    assert_eq!(times(&s, 0.0, 0.5), [0.0]);
    // adjacent windows tile exactly: nothing doubled, nothing dropped
    let whole = times(&s, 0.0, 8.0);
    let mut pieces = times(&s, 0.0, 3.3);
    pieces.extend(times(&s, 3.3, 5.7));
    pieces.extend(times(&s, 5.7, 8.0));
    assert_eq!(whole, pieces);
    assert_eq!(whole.len(), 16);
}

#[test]
fn a_clip_loops_every_length_and_restarts_its_alternations() {
    // the lane spans the three bars, so it is one cycle of the clip: it restarts every 6s
    let s = schedule("clip c { length: 3bars\nnotes: [<c4 e4>] }\nlead.__update(c)");
    let n: Vec<i32> = notes(&s, 0.0, 12.0).into_iter().map(|e| e.2).collect();
    assert_eq!(n, [60, 60]);
    // a short pattern in a long clip is spread over it
    let s = schedule("clip c { length: 2bars\nnotes: [c4 e4] }\nlead.__update(c)");
    assert_eq!(times(&s, 0.0, 8.0), [0.0, 2.0, 4.0, 6.0]);
}

#[test]
fn lanes_span_a_longer_clip_and_line_up() {
    let s = schedule(
        "clip c { length: 2bars\nnotes: [c4 e4 g4 b4]\ngain: [0db -6db -12db -18db] }\nlead.__update(c)",
    );
    // four notes spread over the two bars (4s), then the clip starts again
    assert_eq!(
        times(&s, 0.0, 8.0),
        [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
    );
    let gains: Vec<Value> = s
        .events(0.0, 4.0)
        .unwrap()
        .into_iter()
        .map(|e| e.args["gain"])
        .collect();
    assert_eq!(
        gains,
        [
            Value::Db(0.0),
            Value::Db(-6.0),
            Value::Db(-12.0),
            Value::Db(-18.0)
        ]
    );
}

#[test]
fn a_clip_keeps_counting_cycles_until_it_is_given_a_length() {
    let order = |s: &Schedule| {
        notes(s, 0.0, 8.0)
            .into_iter()
            .map(|e| e.2)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        order(&schedule("lead.__update([<c4 e4>])")),
        [60, 64, 60, 64]
    );
    assert_eq!(
        order(&schedule("lead.__update([c4 e4].every(2, reverse))")),
        [64, 60, 60, 64, 64, 60, 60, 64]
    );
    assert_eq!(
        order(&schedule("clip c { notes: [<c4 e4>] }\nlead.__update(c)")),
        [60, 64, 60, 64]
    );
    assert_eq!(
        order(&schedule(
            "clip c { length: 1bar\nnotes: [<c4 e4>] }\nlead.__update(c)"
        )),
        [60, 60, 60, 60]
    );
}

#[test]
fn starting_a_scene_leaves_the_clips_it_does_not_name_alone() {
    let s = schedule(
        "clip l1 { instrument: pluck, notes: [c4] }\nclip b1 { instrument: pluck, notes: [c2] }\nscene a { l1, b1 }\nscene b { l1 }\na.start\nat +2bars b.start",
    );
    let nodes = |from, to| {
        notes(&s, from, to)
            .into_iter()
            .map(|e| e.1)
            .collect::<Vec<_>>()
    };
    assert_eq!(nodes(0.0, 2.0), ["l1", "b1"]);
    assert_eq!(
        nodes(4.0, 6.0),
        ["l1", "b1"],
        "b1 plays on: scene b did not stop it"
    );
}

#[test]
fn starting_a_clip_that_is_running_does_nothing() {
    // [<c4 e4>] alternates each cycle, and a restart would begin on c4 again
    let s =
        schedule("clip c { instrument: pluck, notes: [<c4 e4>] }\nc.start\nat +1bar c.start");
    let n: Vec<i32> = notes(&s, 0.0, 4.0).into_iter().map(|e| e.2).collect();
    assert_eq!(n, [60, 64], "it carried on: c4, then e4");
}

#[test]
fn a_stop_and_a_start_at_the_same_moment_leave_the_clip_running() {
    let s = schedule(
        "clip c { instrument: pluck, notes: [<c4 e4>] }\nc.start\nat +1bar c.stop\nat +1bar c.start",
    );
    let n: Vec<i32> = notes(&s, 0.0, 4.0).into_iter().map(|e| e.2).collect();
    assert_eq!(n, [60, 64]);
    // but a stop on its own stops it, and a later start begins again
    let s = schedule(
        "clip c { instrument: pluck, notes: [<c4 e4>] }\nc.start\nat +1bar c.stop\nat +2bars c.start",
    );
    let n: Vec<i32> = notes(&s, 0.0, 6.0).into_iter().map(|e| e.2).collect();
    assert_eq!(n, [60, 60]);
}

#[test]
fn changing_a_running_clip_keeps_its_place_in_the_loop() {
    // a four-bar clip changed during its third bar goes on with the third bar
    let s = schedule(
        "clip c { instrument: pluck, notes: [c4 d4 e4 f4], length: 4bars }\nc.start\nat +5bars c.__update(c.transpose(+12st))",
    );
    let n: Vec<i32> = notes(&s, 0.0, 16.0).into_iter().map(|e| e.2).collect();
    // bar 1..4: c4 d4 e4 f4; the change at bar 6 (second bar of the second pass) is d4 an octave up
    assert_eq!(n, [60, 62, 64, 65, 60, 74, 76, 77]);
}

#[test]
fn a_clip_that_does_not_loop_plays_once() {
    let s =
        schedule("clip c { instrument: pluck, notes: [c4], length: 1bar, times: 1 }\nc.start");
    assert_eq!(notes(&s, 0.0, 10.0).len(), 1);
    let s = schedule("clip c { instrument: pluck, notes: [c4], times: 1 }\nc.start");
    assert_eq!(notes(&s, 0.0, 10.0).len(), 1);
}

#[test]
fn a_clip_that_has_finished_can_be_started_again() {
    let s = schedule(
        "clip c { instrument: pluck, notes: [c4], length: 1bar, times: 1 }\nc.start\nat +3bars c.start",
    );
    assert_eq!(notes(&s, 0.0, 10.0).len(), 2);
    let c = s.node_names.iter().position(|n| n == "c").unwrap();
    assert!(s.node_state(c, 1.0).clip.is_some());
    assert!(s.node_state(c, 3.0).clip.is_none(), "it is over");
}

#[test]
fn reset_clears_mute_and_solo() {
    let s = schedule(
        "clip c { instrument: pluck, notes: [c4] }\nc.start\nc.mute\nat +1bar reset\nat +1bar c.start",
    );
    let c = s.node_names.iter().position(|n| n == "c").unwrap();
    assert!(s.node_state(c, 1.0).muted);
    assert!(!s.node_state(c, 3.0).muted);
}

#[test]
fn scene_commands_act_on_each_clip() {
    let s = schedule(
        "clip x { instrument: pluck, notes: [c4] }\nclip y { instrument: pluck, notes: [e4] }\nscene z { x, y }\nz.start\nat +1bar z.mute\nat +2bars z.unmute\nat +3bars z.stop",
    );
    let n: Vec<_> = notes(&s, 0.0, 10.0)
        .into_iter()
        .map(|e| (e.0, e.2))
        .collect();
    // bar 1 (0..2s) sounds, bar 2 (2..4s) is muted, bar 3 (4..6s) sounds, then it stops
    assert_eq!(n.iter().filter(|e| e.0 < 2.0).count(), 2);
    assert_eq!(n.iter().filter(|e| (2.0..4.0).contains(&e.0)).count(), 0);
    assert_eq!(n.iter().filter(|e| (4.0..6.0).contains(&e.0)).count(), 2);
    assert_eq!(n.iter().filter(|e| e.0 >= 6.0).count(), 0);
}

#[test]
fn set_gives_a_clip_or_a_pattern_lanes_in_place_of_its_own() {
    let bright = |s: &Schedule| -> Vec<Value> {
        s.events(0.0, 4.0)
            .unwrap()
            .iter()
            .map(|e| e.args["bright"])
            .collect()
    };
    let s =
        schedule("clip c { notes: [c4 e4]\nbright: [0.2] }\nlead.__update(c.set(bright: 0.9))");
    assert_eq!(
        bright(&s)[..2],
        [Value::Num(0.9), Value::Num(0.9)],
        "replaced"
    );
    let s = schedule("lead.__update([c4 e4].set(bright: [0.1 0.7]))");
    assert_eq!(
        bright(&s)[..2],
        [Value::Num(0.1), Value::Num(0.7)],
        "added to a bare pattern"
    );
    let s = schedule("lead.__update([c4].reverse.set(gain: -6db))");
    assert_eq!(s.events(0.0, 1.0).unwrap()[0].args["gain"], Value::Db(-6.0));
}

#[test]
fn other_lanes_are_read_where_each_note_starts() {
    let s = schedule("clip c { notes: [c4 e4 g4 b4]\nbright: [0.2 0.8] }\nlead.__update(c)");
    let e = s.events(0.0, 2.0).unwrap();
    let brights: Vec<_> = e.iter().map(|e| e.args["bright"]).collect();
    assert_eq!(
        brights,
        [
            Value::Num(0.2),
            Value::Num(0.2),
            Value::Num(0.8),
            Value::Num(0.8)
        ]
    );
    assert!(e.iter().all(|e| e.args.contains_key("freq")));
}

#[test]
fn dur_and_nudge_lanes_shape_each_note() {
    let s = schedule(
        "clip c { notes: [c4 e4]\ndur: [1/4beat]\nnudge: [0ms 100ms] }\nlead.__update(c)",
    );
    let e = s.events(0.0, 2.0).unwrap();
    assert_eq!(e[0].dur, Time::Beats(0.25));
    assert_eq!(e[0].at, Time::Seconds(0.0));
    assert_eq!(e[1].at, Time::Seconds(1.1), "100ms late");
    assert!(
        !e[0].args.contains_key("dur") && !e[0].args.contains_key("nudge"),
        "they aren't instrument arguments"
    );
    // a nudge never pushes a note before the start
    let early = schedule("clip c { notes: [c4]\nnudge: [-50ms] }\nlead.__update(c)");
    assert_eq!(early.events(0.0, 1.0).unwrap()[0].at, Time::Seconds(0.0));
}

#[test]
fn commands_start_replace_and_stop_clips() {
    let s = schedule("\nlead.__update([c4])\nat 2bars lead.__update([e4])\nat 3bars lead.stop");
    // bar = 2s: c4 for the first 4s, e4 for 4s..6s, then silence
    let n = notes(&s, 0.0, 20.0);
    assert_eq!(n.iter().map(|e| e.2).collect::<Vec<_>>(), [60, 60, 64]);
    assert_eq!(n.iter().map(|e| e.0).collect::<Vec<_>>(), [0.0, 2.0, 4.0]);
    assert_eq!(s.end(), Some(6.0));
}

#[test]
fn a_clip_started_later_starts_its_own_cycle_there() {
    let s = schedule("at 2bars lead.__update([c4 e4])");
    assert_eq!(times(&s, 0.0, 8.0), [4.0, 5.0, 6.0, 7.0]);
    let mid_beat = schedule("at 1beat lead.__update([c4 e4])");
    assert_eq!(times(&mid_beat, 0.0, 3.0), [0.5, 1.5, 2.5]);
}

#[test]
fn nodes_play_independently_and_hush_ends_them_all() {
    let s = schedule("lead.__update([c4])\nbass.__update([c2 g2])\nat 2bars hush");
    let n = notes(&s, 0.0, 20.0);
    assert_eq!(n.iter().filter(|e| e.1 == "lead").count(), 2);
    assert_eq!(n.iter().filter(|e| e.1 == "bass").count(), 4);
    assert_eq!(s.end(), Some(4.0));
    assert!(!s.is_empty());
}

#[test]
fn mute_and_solo_silence_nodes_without_stopping_them() {
    let s = schedule(
        "lead.__update([c4])\nbass.__update([c2])\nat 1bar lead.mute\nat 3bars lead.unmute",
    );
    let lead: Vec<f64> = notes(&s, 0.0, 10.0)
        .into_iter()
        .filter(|e| e.1 == "lead")
        .map(|e| e.0)
        .collect();
    assert_eq!(
        lead,
        [0.0, 6.0, 8.0],
        "silent between 2s and 6s; the clip kept its place"
    );
    assert_eq!(
        notes(&s, 0.0, 10.0)
            .iter()
            .filter(|e| e.1 == "bass")
            .count(),
        5
    );

    let solo = schedule(
        "lead.__update([c4])\nbass.__update([c2])\nat 1bar bass.solo\nat 2bars unsolo",
    );
    let who = |t0, t1| {
        notes(&solo, t0, t1)
            .into_iter()
            .map(|e| e.1)
            .collect::<Vec<_>>()
    };
    assert_eq!(who(0.0, 2.0), ["lead", "bass"]);
    assert_eq!(who(2.0, 4.0), ["bass"], "only the soloed node");
    assert_eq!(who(4.0, 6.0), ["lead", "bass"]);
    let one = schedule(
        "lead.__update([c4])\nbass.__update([c2])\nlead.solo\nbass.solo\nat 1bar bass.unsolo",
    );
    assert_eq!(
        notes(&one, 2.0, 4.0)
            .iter()
            .map(|e| e.1.as_str())
            .collect::<Vec<_>>(),
        ["lead"]
    );
}

#[test]
fn panic_is_reported_and_ends_every_clip() {
    let s = schedule("lead.__update([c4])\nat 3bars panic");
    assert_eq!(s.panics(), [6.0]);
    assert_eq!(s.end(), Some(6.0));
    assert_eq!(times(&s, 0.0, 20.0), [0.0, 2.0, 4.0]);
}

#[test]
fn scenes_launch_whole_sets_of_clips() {
    let s = schedule(
        "\nclip bass2 { instrument: pluck, notes: [c2 g2] }\nclip lead2 { instrument: pluck, notes: [c4] }\nclip lead3 { instrument: pluck, notes: [e4] }\nscene verse { bass2, lead2 }\nscene bridge { lead3 }\nverse.start\nat 2bars bass2.stop\nat 2bars lead2.stop\nat 2bars bridge.start",
    );
    let n = notes(&s, 0.0, 8.0);
    let bass = n.iter().filter(|e| e.1 == "bass2").count();
    assert_eq!(bass, 4, "two bars of two notes, then stopped");
    let lead: Vec<i32> = n
        .iter()
        .filter(|e| e.1 == "lead2" || e.1 == "lead3")
        .map(|e| e.2)
        .collect();
    assert_eq!(lead, [60, 60, 64, 64]);
}

#[test]
fn thinned_patterns_are_the_same_whichever_way_they_are_asked_for() {
    let s = schedule("lead.__update([c4*8].drop(40%))");
    let whole = times(&s, 0.0, 16.0);
    let mut pieces = Vec::new();
    for i in 0..8 {
        pieces.extend(times(&s, f64::from(i) * 2.0, f64::from(i + 1) * 2.0));
    }
    assert_eq!(whole, pieces);
    assert!(whole.len() < 64 && whole.len() > 20, "{}", whole.len());
}

#[test]
fn notes_that_are_not_pitches_are_errors() {
    let s = schedule("lead.__update([0.5])");
    let e = s.events(0.0, 2.0).unwrap_err();
    assert!(e.0.contains("must be a note name or a frequency"), "{e}");
    let s = schedule("clip c { notes: [c4]\ndur: [440hz] }\nlead.__update(c)");
    assert!(s.events(0.0, 2.0).unwrap_err().0.contains("lane `dur`"));
}

#[test]
fn a_program_with_no_commands_schedules_nothing() {
    let s = schedule("lead(freq: c4) for 1beat");
    assert!(s.is_empty());
    assert!(s.events(0.0, 100.0).unwrap().is_empty());
    assert_eq!(s.end(), Some(0.0));
}

#[test]
fn other_meters_change_the_length_of_a_cycle() {
    let src = "meter 3/4\ntempo 120bpm\ninstr p(freq: hz) { osc(sine, freq) }\nclip t { instrument: p }\nt.__update([c4 e4 g4])";
    let s = compile(src).unwrap().schedule();
    // a 3/4bar at 120bpm is 1.5 seconds, so three steps are half a second apart
    assert_eq!(times(&s, 0.0, 1.6), [0.0, 0.5, 1.0, 1.5]);
}
