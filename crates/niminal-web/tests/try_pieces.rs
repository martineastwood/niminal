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
