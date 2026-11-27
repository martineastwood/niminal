//! The pieces the "Try it" page offers (docs/try): each starts, makes sound, and every slider moves a real control.
#![allow(clippy::unwrap_used)]

use niminal_web::live::{LiveFailure, LiveSession};

#[test]
fn every_try_piece_plays_and_its_sliders_work() {
    let dir = format!("{}/../../docs/try", env!("CARGO_MANIFEST_DIR"));
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/manifest.json")).unwrap()).unwrap();
    for entry in manifest.as_array().unwrap() {
        let file = entry["file"].as_str().unwrap();
        let path = if std::path::Path::new(&format!("{dir}/{file}.nml")).exists() {
            format!("{dir}/{file}.nml")
        } else {
            format!("{}/../../examples/{file}.nml", env!("CARGO_MANIFEST_DIR"))
        };
        let source = std::fs::read_to_string(&path).unwrap();
        let mut session = match LiveSession::start(&source, 48_000.0) {
            Ok((session, _)) => session,
            Err(LiveFailure::Source(d)) => panic!("{file}: {:?}", d.first().map(|d| (d.message.clone(), d.span))),
            Err(_) => panic!("{file} did not start"),
        };
        let mut peak = 0.0f32;
        for _ in 0..40 {
            session.process(4800);
            peak = session.out.iter().flatten().fold(peak, |p, s| p.max(s.abs()));
        }
        assert!(peak > 0.01, "{file} made no sound");
        assert!(peak <= 1.0, "{file} is over the ceiling: {peak}");
        for slider in entry["sliders"].as_array().unwrap() {
            let name = slider["name"].as_str().unwrap();
            let unit = slider["unit"].as_str().filter(|u| !u.is_empty());
            for key in ["min", "max", "value"] {
                let value = slider[key].as_f64().unwrap();
                session.set_control(name, value, unit).unwrap_or_else(|e| panic!("{file}: {name} = {value}: {e}"));
                for _ in 0..4 {
                    session.process(4800);
                    let finite = session.out.iter().flatten().all(|s| s.is_finite() && s.abs() <= 1.0);
                    assert!(finite, "{file}: {name} = {value} gave a bad sample");
                }
            }
        }
    }
}

/// A slider is for playing with, so it must be heard soon after it moves, not when the next note starts.
#[test]
fn every_slider_is_heard_within_a_moment() {
    let dir = format!("{}/../../docs/try", env!("CARGO_MANIFEST_DIR"));
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/manifest.json")).unwrap()).unwrap();
    for entry in manifest.as_array().unwrap() {
        let file = entry["file"].as_str().unwrap();
        let Ok(source) = std::fs::read_to_string(format!("{dir}/{file}.nml")) else { continue };
        for slider in entry["sliders"].as_array().unwrap() {
            let name = slider["name"].as_str().unwrap();
            let unit = slider["unit"].as_str().filter(|u| !u.is_empty());
            // the same piece twice, the slider sent to each end at the same moment
            let heard: Vec<f32> = ["min", "max"]
                .iter()
                .map(|end| {
                    let (mut s, _) = LiveSession::start(&source, 48_000.0).ok().unwrap();
                    for _ in 0..20 {
                        s.process(4800); // two seconds in
                    }
                    s.set_control(name, slider[end].as_f64().unwrap(), unit).unwrap();
                    let mut energy = 0.0f32;
                    for _ in 0..5 {
                        s.process(4800); // the half second after
                        // the change from one sample to the next: filters and echoes show in it, however low the notes
                        energy += s.out.iter().map(|c| c.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>()).sum::<f32>();
                    }
                    energy
                })
                .collect();
            let (low, high) = (heard[0].min(heard[1]), heard[0].max(heard[1]));
            assert!(high > low * 1.05, "{file}: {name} is not heard within half a second ({:?})", heard);
        }
    }
}

/// A `ctl` line may say how far its slider goes: `// range 300hz..12khz`. Returns (min, max, unit) in the units the
/// engine takes (hz, db or none).
fn annotated_range(source: &str, name: &str) -> Option<(f64, f64, Option<&'static str>)> {
    let line = source.lines().find(|l| l.trim_start().starts_with(&format!("ctl {name} ")))?;
    let range = line.split("// range").nth(1)?.trim();
    let (lo, hi) = range.split_once("..")?;
    let number = |text: &str| -> (f64, Option<&'static str>) {
        let text = text.trim();
        let at = text.find(|c: char| c.is_alphabetic()).unwrap_or(text.len());
        let value: f64 = text[..at].parse().unwrap();
        match &text[at..] {
            "khz" => (value * 1000.0, Some("hz")),
            "hz" => (value, Some("hz")),
            "db" => (value, Some("db")),
            _ => (value, None),
        }
    };
    let (min, unit) = number(lo);
    let (max, _) = number(hi);
    Some((min, max, unit))
}

/// The pieces of the Studio are made to be played live: they have clips for the Clips panel and annotated controls for the
/// sliders, they start, they are not too loud, and every slider goes its whole way without a bad sample.
#[test]
fn every_studio_piece_is_ready_to_play_live() {
    let dir = format!("{}/../../docs/studio", env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "nml") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path).unwrap();
        let (mut session, _) = match LiveSession::start(&source, 48_000.0) {
            Ok(started) => started,
            Err(LiveFailure::Source(d)) => panic!("{file}: {:?}", d.first().map(|d| (d.message.clone(), d.span))),
            Err(_) => panic!("{file} does not start"),
        };
        let status: serde_json::Value = serde_json::from_str(&session.status()).unwrap();
        assert!(!status["clips"].as_array().unwrap().is_empty(), "{file} has no clips for the Clips panel");
        let mut peak = 0.0f32;
        for _ in 0..40 {
            session.process(4800);
            peak = session.out.iter().flatten().fold(peak, |p, s| p.max(s.abs()));
        }
        assert!(peak > 0.02, "{file} is silent or nearly");
        assert!(peak <= 1.0, "{file} is over the ceiling: {peak}");
        for control in status["controls"].as_array().unwrap() {
            let name = control["name"].as_str().unwrap();
            let (min, max, unit) = annotated_range(&source, name).unwrap_or_else(|| panic!("{file}: ctl {name} has no `// range`"));
            for value in [min, max, (min + max) / 2.0] {
                session.set_control(name, value, unit).unwrap_or_else(|e| panic!("{file}: {name} = {value}: {e}"));
                for _ in 0..4 {
                    session.process(4800);
                    assert!(session.out.iter().flatten().all(|s| s.is_finite() && s.abs() <= 1.0), "{file}: {name} = {value} gave a bad sample");
                }
            }
        }
        // every clip can be started and stopped from the Clips panel
        for clip in status["clips"].as_array().unwrap() {
            let name = clip["name"].as_str().unwrap();
            for verb in ["start", "stop"] {
                session.eval(&format!("{name}.{verb}\n"), Some("now")).ok().unwrap_or_else(|| panic!("{file}: {name}.{verb} failed"));
            }
        }
    }
}
