#[cfg(test)]
mod tests {
    use crate::compile;
    use crate::schedule::Schedule;
    use niminal_score::{Tempo, Time, Value};

    const STAGE: &str = "\ntempo 120bpm\ninstr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: freq * (2 + bright * 6)) }\nclip lead { instrument: pluck }\n";

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

    fn midis(s: &Schedule, from: f64, to: f64) -> Vec<i32> {
        s.events(from, to)
            .unwrap()
            .into_iter()
            .map(|e| {
                let Value::Note(m) = e.args["freq"] else {
                    panic!("pitch is a note")
                };
                m
            })
            .collect()
    }

    #[test]
    fn def_fills_pattern_atoms_from_params() {
        let s = schedule(
            "\ndef motif(root) {\n  [root ~]\n}\nclip riff {\n  notes: motif(c4)\n}\nlead.play(riff)\n",
        );
        // `[root ~]` is one note then a rest in each bar
        assert_eq!(midis(&s, 0.0, 2.0), vec![60]);
        assert_eq!(midis(&s, 0.0, 4.0), vec![60, 60]);
    }

    #[test]
    fn def_with_if_picks_a_branch() {
        let s = schedule(
            "\ndef pick(n) {\n  if n > 0 { [c5] } else { [c3] }\n}\nclip riff { notes: pick(1) }\nlead.play(riff)\n",
        );
        assert_eq!(midis(&s, 0.0, 1.0)[0], 72);
    }

    #[test]
    fn top_level_for_expands_notes() {
        let src =
            format!("{STAGE}\nfor i in 0..<4 {{\n  at i * 1beat pluck(freq: c4) for 1/2beat\n}}\n");
        let p = compile(&src).unwrap_or_else(|e| {
            panic!(
                "{}",
                e.iter().map(|d| d.render("t", &src)).collect::<String>()
            )
        });
        assert_eq!(p.notes.len(), 4);
        assert_eq!(p.notes[2].at, Time::Beats(2.0));
    }

    #[test]
    fn for_in_def_builds_a_list_pattern() {
        let s = schedule(
            "\ndef stairs(n) {\n  for i in 0..<n {\n    c4 + i * 1st\n  }\n}\nclip riff { notes: stairs(4) }\nlead.play(riff)\n",
        );
        // one bar at 120bpm is 2 seconds
        assert_eq!(midis(&s, 0.0, 2.0), vec![60, 61, 62, 63]);
    }

    #[test]
    fn anonymous_clip_from_def() {
        let s =
            schedule("\ndef make() {\n  clip {\n    notes: [c4 e4]\n  }\n}\nlead.play(make())\n");
        assert_eq!(midis(&s, 0.0, 2.0), vec![60, 64]);
    }

    #[test]
    fn stack_and_cat_combine_patterns() {
        let stacked = schedule("lead.play(stack([c4], [e4]))");
        let notes = midis(&stacked, 0.0, 2.0);
        assert!(notes.contains(&60) && notes.contains(&64));

        let cat = schedule("lead.play(cat([c4], [d4]))");
        assert_eq!(midis(&cat, 0.0, 2.0), vec![60, 62]);
    }

    #[test]
    fn compile_accepts_folded_note_args() {
        let _ = Tempo::new(120.0);
        let src = format!("{STAGE}at 0beats pluck(freq: c4 + 2st) for 1beat");
        let p = compile(&src).unwrap();
        assert_eq!(p.notes[0].args.get("freq"), Some(&Value::Note(62)));
    }

    #[test]
    fn top_level_if_picks_a_branch() {
        let src = format!(
            "{STAGE}\nif 1 > 0 {{\n  at 0beats pluck(freq: c5) for 1beat\n}} else {{\n  at 0beats pluck(freq: c3) for 1beat\n}}\n"
        );
        let p = compile(&src).unwrap_or_else(|e| {
            panic!(
                "{}",
                e.iter().map(|d| d.render("t", &src)).collect::<String>()
            )
        });
        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].args.get("freq"), Some(&Value::Note(72)));
    }

    #[test]
    fn deep_arrays_are_rejected() {
        let brackets = "[".repeat(100) + "0" + &"]".repeat(100);
        let src = format!("instr a(freq: hz) {{ osc(saw, freq).lpf({brackets}) }}");
        let err = match compile(&src) {
            Err(e) => e,
            Ok(_) => panic!("expected nest depth error"),
        };
        assert!(
            err.iter().any(|d| d.message.contains("nested")),
            "{:?}",
            err.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}
