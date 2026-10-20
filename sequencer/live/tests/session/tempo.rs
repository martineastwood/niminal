use crate::common::*;

#[test]
fn a_new_tempo_keeps_the_clip_in_place_in_its_bar() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // at 120bpm a note lasts half a second: 0.75s in, the second note is half done
    run(&mut s, 36_000);
    s.eval("tempo 240bpm", Some("now")).unwrap();
    assert_eq!(s.transport().bpm, 240.0);
    let after = run(&mut s, 18_000);
    // the bar is now a second long and was 3/8 through: the third note starts an eighth of a bar later
    assert!(
        (hz(&after[7_000..11_000]) - 392.0).abs() < 15.0,
        "g4 comes next, not c4 again: {}",
        hz(&after[7_000..11_000])
    );
}

#[test]
fn a_new_meter_keeps_the_clip_in_place_in_its_bar_and_the_bar_count_going() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // at 120bpm a bar is 2s: 0.75s in, the second note is half done
    run(&mut s, 36_000);
    s.eval("meter 2/4", Some("now")).unwrap();
    let t = s.transport();
    assert_eq!(
        (t.bar, (t.beat * 1000.0).round()),
        (1, 1750.0),
        "3/8 through a bar of two beats: 0.75beats in {t:?}"
    );
    let after = run(&mut s, 18_000);
    // the bar is now a second long and was 3/8 through: the third note starts an eighth of a bar later
    assert!(
        (hz(&after[1_000..5_000]) - 330.0).abs() < 15.0,
        "still e4: {}",
        hz(&after[1_000..5_000])
    );
    assert!(
        (hz(&after[7_000..11_000]) - 392.0).abs() < 15.0,
        "g4 comes next, not c4 again: {}",
        hz(&after[7_000..11_000])
    );
}

#[test]
fn a_meter_change_late_in_a_piece_keeps_the_bar_number_and_bar_lines_in_step_with_the_clips() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // three and a half bars of 4/4
    run(&mut s, 7 * BAR / 2);
    assert_eq!(s.transport().bar, 4);
    s.eval("meter 3/4", Some("now")).unwrap();
    let t = s.transport();
    assert_eq!(t.bar, 4, "{t:?}");
    assert!(
        (t.beat - 2.5).abs() < 0.01,
        "halfway through the bar, which is now 3beats: {t:?}"
    );
    // the next bar line is half a 3/4bar away, and the clip starts over there too
    s.eval("lead.mute", Some("next bar")).unwrap();
    let all = run(&mut s, 40_000);
    assert!(rms(&all[30_000..35_900]) > 0.5);
    assert!(
        rms(&all[38_000..]) < 1e-6,
        "muted at the bar line, half a 3/4bar on, once its release is over: {}",
        rms(&all[38_000..])
    );
}

#[test]
fn a_tempo_change_can_name_when_it_lands_and_ramp_over_a_stretch() {
    let mut s = ready();
    s.eval("tempo 120bpm @ next bar", None).unwrap();
    assert_eq!(s.pending().len(), 1);
    let accepted = s
        .eval("tempo 180bpm @ now over 1bar", Some("now"))
        .unwrap();
    assert!(
        s.pending().len() >= 2,
        "a ramp is a run of changes: {}",
        s.pending().len()
    );
    assert_eq!(
        accepted.changes,
        ["tempo 123.750bpm"],
        "the first step: 16 steps from 120 to 180"
    );
    let mut seen = vec![s.transport().bpm];
    for _ in 0..40 {
        run(&mut s, 6_000);
        seen.push(s.transport().bpm);
    }
    assert!(
        seen.windows(2).all(|w| w[1] >= w[0]),
        "it only speeds up: {seen:?}"
    );
    assert!(
        seen.iter().any(|&b| b > 130.0 && b < 175.0),
        "and passes through the tempos between: {seen:?}"
    );
    assert_eq!(
        *seen.last().unwrap(),
        180.0,
        "and ends at the tempo asked for"
    );
    assert!(s.pending().is_empty());
}

#[test]
fn a_tempo_ramp_replays_to_the_same_audio() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    let mut original = run(&mut s, 20_000);
    s.eval("tempo 200bpm @ now over 2beats", Some("now"))
        .unwrap();
    original.extend(run(&mut s, 120_000));
    let log = from_lines(&to_lines(&s.log())).unwrap();
    let replayed = niminal_daemon::Session::replay(
        SR,
        Layout::Mono,
        &log,
        original.len() as u64,
        false,
    );
    assert_eq!(original, replayed[0]);
    assert_eq!(s.transport().bpm, 200.0);
}

#[test]
fn a_ramp_that_is_too_long_is_refused() {
    let mut s = ready();
    let problems = s
        .eval("tempo 200bpm over 1000bars", Some("now"))
        .unwrap_err();
    assert!(
        problems[0].message.contains("a tempo ramp takes"),
        "{}",
        problems[0].message
    );
}

#[test]
fn a_change_waiting_for_a_bar_line_stays_on_it_when_the_meter_changes_first() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // the meter lands at the next bar (2s), the mute is meant for the one after (4s at 4/4)
    s.eval("lead.mute", Some("next 2bars")).unwrap();
    s.eval("meter 2/4", Some("next bar")).unwrap();
    assert_eq!(
        s.pending().iter().map(|p| p.lands_at).collect::<Vec<_>>(),
        [96_000, 192_000]
    );
    run(&mut s, 97_000);
    // the meter landed at 2s, and the mute was a bar and a half after that: now bars are a second, so half that
    let waiting: Vec<_> = s.pending().iter().map(|p| p.lands_at).collect();
    assert_eq!(
        waiting,
        [96_000 + 48_000],
        "the bar line it was waiting for, now a bar closer"
    );
}

#[test]
fn cancelling_a_ramp_by_its_id_cancels_every_step_of_it() {
    let mut s = ready();
    let ramp = s
        .eval("tempo 180bpm @ next bar over 2bars", Some("now"))
        .unwrap();
    s.eval("lead.mute", Some("next 4bars")).unwrap();
    assert_eq!(s.pending().len(), 33, "32 steps and the mute");
    assert_eq!(s.cancel(ramp.id), 32);
    assert_eq!(s.pending().len(), 1);
    // one that is already under way goes the same way, from any of its steps
    let again = s
        .eval("tempo 180bpm @ next bar over 2bars", Some("now"))
        .unwrap();
    run(&mut s, 2 * BAR);
    let left = s.pending().len();
    assert!(left > 1 && left < 33, "{left}");
    assert_eq!(s.cancel(again.id), left - 1);
    assert_eq!(s.pending().len(), 1);
}
