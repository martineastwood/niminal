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
