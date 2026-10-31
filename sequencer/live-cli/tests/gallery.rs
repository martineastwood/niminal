use std::path::Path;

use niminal_compose::compile_with;
use niminal_lang::{CompileOptions, Samples};
use niminal_live_cli::gallery::{Amount, families, generate};

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/break.wav");

#[test]
fn every_version_of_every_recipe_compiles() {
    let options = CompileOptions {
        samples: Samples::new(Path::new(SAMPLE).parent().unwrap()),
        ..Default::default()
    };
    for seed in [1, 2, 3] {
        for version in generate(SAMPLE, &[], Amount::Lots, seed) {
            let unfilled = version.source.split('{').skip(1).any(|rest| {
                rest.split_once('}').is_some_and(|(word, _)| {
                    !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                })
            });
            assert!(
                !unfilled,
                "{} has an unfilled setting:\n{}",
                version.name, version.source
            );
            let expanded =
                niminal_compose::expand(&version.source, Path::new(SAMPLE).parent().unwrap())
                    .unwrap_or_else(|d| {
                        panic!(
                            "{}: {}",
                            version.name,
                            d.render(&version.name, &version.source)
                        )
                    });
            if let Err(errors) = compile_with(&expanded.text, &options) {
                let shown: Vec<_> = errors
                    .iter()
                    .map(|d| d.render(&version.name, &version.source))
                    .collect();
                panic!(
                    "{} (seed {seed}) does not compile:\n{}\n{}",
                    version.name,
                    shown.join("\n"),
                    version.source
                );
            }
        }
    }
}

#[test]
fn the_same_seed_makes_the_same_versions_and_a_family_filter_keeps_only_that_family() {
    let a = generate(SAMPLE, &[], Amount::Enough, 7);
    let b = generate(SAMPLE, &[], Amount::Enough, 7);
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(&b).all(|(x, y)| x.source == y.source));
    let other = generate(SAMPLE, &[], Amount::Enough, 8);
    assert!(a.iter().zip(&other).any(|(x, y)| x.source != y.source));

    assert!(families().contains(&"manglers".to_string()));
    let only = generate(SAMPLE, &["manglers".into()], Amount::Few, 1);
    assert!(!only.is_empty() && only.iter().all(|v| v.family == "manglers"));
    assert!(only.len() < generate(SAMPLE, &[], Amount::Few, 1).len());
}

#[test]
fn whole_number_settings_are_whole_numbers_in_range() {
    for version in generate(SAMPLE, &[], Amount::Lots, 5) {
        if version.recipe == "echoes" {
            let count = version.values.iter().find(|(n, _)| n == "count").unwrap().1;
            assert_eq!(count, count.round(), "{}", version.name);
            assert!((2.0..=8.0).contains(&count));
        }
    }
}

#[test]
fn every_version_can_be_evaluated_live_the_way_the_stage_sends_it() {
    use niminal_lang::Layout;
    use niminal_live::{Offline, ParsedEval};
    let dir = Path::new(SAMPLE).parent().unwrap();
    for version in generate(SAMPLE, &[], Amount::Lots, 2) {
        let mut live = Offline::new(48_000.0, Layout::Stereo).without_limiter();
        let sent = ParsedEval::from_dir(&version.source, dir, Some("now"));
        if let Err(problems) = live.eval_parsed(sent) {
            panic!(
                "{} is refused live: {problems:?}\n{}",
                version.name, version.source
            );
        }
    }
}

#[test]
fn a_gallery_version_leaves_the_names_other_code_uses_alone() {
    use niminal_lang::Layout;
    use niminal_live::{Offline, ParsedEval};
    // a pad that calls a local `voice` and a bus `src`, as people's own code does
    let own = "bus src: stereo\ninstr pad(freq: hz) {\n  voice = osc(saw, freq) * 0.2\n  src += voice.pan\n}\npad(freq: a3) for 1beat\n";
    let dir = Path::new(SAMPLE).parent().unwrap();
    for version in generate(SAMPLE, &[], Amount::Few, 1) {
        let mut live = Offline::new(48_000.0, Layout::Stereo).without_limiter();
        live.eval_parsed(ParsedEval::from_dir(&version.source, dir, Some("now")))
            .unwrap_or_else(|p| panic!("{}: {p:?}", version.name));
        live.eval_parsed(ParsedEval::from_dir(own, dir, Some("now")))
            .unwrap_or_else(|p| panic!("after {}, other code is refused: {p:?}", version.name));
    }
}
