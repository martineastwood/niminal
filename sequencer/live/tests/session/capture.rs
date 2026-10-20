use crate::common::*;

fn work_dir() -> (std::path::PathBuf, std::path::PathBuf) {
    let prev = std::env::current_dir().unwrap();
    let dir = std::env::temp_dir().join(format!("niminal-capture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_current_dir(&dir).unwrap();
    (prev, dir)
}

#[test]
fn capture_writes_the_last_bars_as_a_score() {
    let (prev, dir) = work_dir();

    let mut s = ready();
    s.eval(
        "clip riff { instrument: pluck\n  notes: [c4 e4 g4 b4] }\nriff.start",
        Some("now"),
    )
    .unwrap();
    run(&mut s, 2 * BAR);
    s.eval("capture last 2bars as jam", Some("now")).unwrap();
    // capture lands on the next quantum; advance so it runs
    run(&mut s, BAR);

    let landed = s.take_landed();
    let notice = landed
        .iter()
        .flat_map(|l| &l.changes)
        .find(|c| c.contains("wrote score/jam.nms"))
        .cloned();
    assert!(
        notice.as_ref().is_some_and(|n| n.contains("events")),
        "landed changes: {:?}",
        landed.iter().map(|l| &l.changes).collect::<Vec<_>>()
    );

    let json = std::fs::read_to_string(dir.join("score/jam.nms")).unwrap();
    let section = niminal_compose::Section::from_json(&json).unwrap();
    assert_eq!(section.name, "jam");
    assert_eq!(section.length, Time::Bars(2.0));
    assert!(
        section.events.len() >= 4,
        "expected notes from the looping clip, got {}",
        section.events.len()
    );
    let first = section.events[0].at.to_seconds(niminal_score::Tempo::new(120.0));
    assert!((0.0..4.0).contains(&first), "first note at {first}s into the section");

    std::env::set_current_dir(prev).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn capture_compiles_as_a_side_effect_action() {
    let src = "
tempo 120bpm
instr p(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track t { instrument = p }
t.play([c4])
capture last 1bar as unused
at +1bars hush
";
    let program = niminal_compose::compile(src).unwrap();
    assert!(
        program
            .performance
            .iter()
            .any(|s| matches!(s.action, niminal_compose::Action::Capture { bars, .. } if (bars - 1.0).abs() < 1e-9))
    );
}
