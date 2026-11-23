//! Musical contracts beyond compilation: velocity, release, and controls that do audible work.
#![allow(clippy::unwrap_used)]

use niminal_engine::{Voice, block_size};
use niminal_lang::presets::{PresetKind, all};
use niminal_lang::{Instrument, compile};

fn instruments() -> Vec<(&'static str, Instrument)> {
    all()
        .iter()
        .filter(|p| p.kind == PresetKind::Instrument)
        .map(|p| {
            let mut program = compile(&format!("{}\n{}", p.helpers, p.source)).unwrap();
            (p.shelf, program.instruments.remove(0))
        })
        .collect()
}

fn render(instrument: &Instrument, params: &[(&str, f32)], hold: usize, frames: usize) -> Vec<f32> {
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let freq = instrument
        .params
        .iter()
        .find(|p| p.name == "freq")
        .and_then(|p| p.default)
        .unwrap_or(220.0);
    voice.set_param_by_name("freq", freq);
    for &(name, value) in params {
        assert!(
            voice.set_param_by_name(name, value),
            "{} has no {name}",
            instrument.name
        );
    }
    voice.start_note();
    let mut out = vec![0.0; frames];
    let mut pos = 0;
    while pos < frames {
        if pos == hold {
            voice.release();
        }
        let mut end = (pos + block_size()).min(frames);
        if pos < hold {
            end = end.min(hold);
        }
        voice.process(&mut out[pos..end]);
        pos = end;
    }
    assert!(
        !voice.is_poisoned(),
        "{} produced invalid audio at {params:?}",
        instrument.name
    );
    assert!(
        out.iter().all(|s| s.is_finite()),
        "{} at {params:?}",
        instrument.name
    );
    out
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|s| f64::from(*s).powi(2)).sum()
}

#[test]
fn every_preset_is_silent_at_zero_gain_and_softer_when_played_softly() {
    for (_, instrument) in instruments() {
        let silent = render(&instrument, &[("gain", 0.0)], 24_000, 48_000);
        assert!(
            silent.iter().all(|s| *s == 0.0),
            "{} ignores zero gain",
            instrument.name
        );
        let soft = render(&instrument, &[("gain", 0.25)], 24_000, 48_000);
        let hard = render(&instrument, &[("gain", 1.0)], 24_000, 48_000);
        assert!(
            energy(&soft) < energy(&hard) * 0.25,
            "{} does not respond to softer playing",
            instrument.name
        );
    }
}

#[test]
fn melodic_presets_have_usable_levels_and_release_in_low_and_high_registers() {
    for (shelf, instrument) in instruments() {
        if shelf != "std/instruments" {
            continue;
        }
        for freq in [130.812_79, 523.251_16] {
            let audio = render(&instrument, &[("freq", freq)], 24_000, 288_000);
            let peak = audio.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            let rms = (energy(&audio[..24_000]) / 24_000.0).sqrt();
            assert!(
                (0.01..1.0).contains(&peak) && rms > 0.001,
                "{} at {freq}: peak {peak}, first-half-second RMS {rms}",
                instrument.name
            );
            assert!(
                energy(&audio[264_000..]) < 1e-6,
                "{} does not release at {freq}",
                instrument.name
            );
        }
    }
}

#[test]
fn every_drum_decay_controls_the_audible_tail() {
    for (shelf, instrument) in instruments() {
        if shelf != "std/drums" {
            continue;
        }
        let short = render(&instrument, &[("decay", 0.04)], 48_000, 48_000);
        let long = render(&instrument, &[("decay", 0.6)], 48_000, 48_000);
        assert!(
            energy(&long[9_600..]) > energy(&short[9_600..]) * 2.0,
            "{}: decay does not lengthen the audible tail",
            instrument.name
        );
    }
}

#[test]
fn drum_controls_change_the_sound_at_the_default_mix() {
    for (shelf, instrument) in instruments() {
        if shelf != "std/drums" {
            continue;
        }
        for param in &instrument.params {
            let Some((lo, hi)) = param.range else {
                continue;
            };
            let low = render(
                &instrument,
                &[(param.name.as_str(), lo as f32)],
                24_000,
                24_000,
            );
            let high = render(
                &instrument,
                &[(param.name.as_str(), hi as f32)],
                24_000,
                24_000,
            );
            let difference: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a - b).collect();
            assert!(
                energy(&difference) > 1e-5,
                "{}.{} has no audible effect",
                instrument.name,
                param.name
            );
        }
    }
}

#[test]
fn individual_extremes_and_mixed_controls_do_not_hide_behind_all_controls_moving_together() {
    for (_, instrument) in instruments() {
        for param in &instrument.params {
            for value in [-1e30, 1e30] {
                render(&instrument, &[(param.name.as_str(), value)], 2_400, 4_800);
            }
        }
        let mixed: Vec<_> = instrument
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.as_str(), if i % 2 == 0 { 1e30 } else { -1e30 }))
            .collect();
        render(&instrument, &mixed, 2_400, 4_800);
    }
}

fn preset(name: &str) -> Instrument {
    instruments()
        .into_iter()
        .find(|(_, i)| i.name == name)
        .unwrap()
        .1
}

fn roughness(samples: &[f32]) -> f64 {
    samples
        .windows(2)
        .map(|s| f64::from(s[1] - s[0]).powi(2))
        .sum::<f64>()
        / energy(samples).max(1e-20)
}

#[test]
fn the_electric_piano_responds_to_strike_and_key_damping() {
    let piano = preset("electric_piano");
    let soft = render(&piano, &[("gain", 0.25)], 48_000, 48_000);
    let hard = render(&piano, &[("gain", 1.0)], 48_000, 48_000);
    assert!(
        roughness(&hard[480..12_000]) > roughness(&soft[480..12_000]) * 1.1,
        "harder strikes should brighten the piano, independently of output level"
    );
    let damped = render(&piano, &[("release", 0.02)], 4_800, 48_000);
    let ringing = render(&piano, &[("release", 0.6)], 4_800, 48_000);
    assert!(energy(&ringing[7_200..24_000]) > energy(&damped[7_200..24_000]) * 100.0);
}

#[test]
fn fm_index_zero_removes_modulation_and_contour_changes_its_lifetime() {
    let keys = preset("fm_keys");
    let pure_low = render(&keys, &[("index", 0.0), ("ratio", 1.0)], 48_000, 24_000);
    let pure_high = render(&keys, &[("index", 0.0), ("ratio", 7.0)], 48_000, 24_000);
    assert_eq!(
        pure_low, pure_high,
        "ratio cannot change an unmodulated carrier"
    );
    let plucked = render(&keys, &[("index", 4.0), ("contour", 0.0)], 48_000, 48_000);
    let bright = render(&keys, &[("index", 4.0), ("contour", 1.0)], 48_000, 48_000);
    assert!(
        roughness(&bright[9_600..19_200]) > roughness(&plucked[9_600..19_200]) * 1.5,
        "the long contour must retain the bright modulation beyond the attack"
    );
}

#[test]
fn the_808_has_a_full_low_frequency_body_after_its_initial_pitch_drop() {
    let kick = preset("kick_808");
    let audio = render(&kick, &[], 24_000, 24_000);
    let body = &audio[2_400..7_200];
    assert!(
        (energy(body) / body.len() as f64).sqrt() > 0.3,
        "the 808 needs body through the first 150ms, not just an attack spike"
    );
    let crossings = body
        .windows(2)
        .filter(|s| s[0] <= 0.0 && s[1] > 0.0)
        .count();
    assert!(
        (4..=6).contains(&crossings),
        "the body should settle near 52 Hz: {crossings} cycles in 100ms"
    );
}
