//! Frame scopes: code that runs on a window of samples every half window, with `fft` and `ifft`.

use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};

fn render(source: &str, blocks: usize) -> Vec<f32> {
    render_chunk(source, blocks * block_size(), block_size())
}

fn render_chunk(source: &str, frames: usize, chunk: usize) -> Vec<f32> {
    let expanded = expand(source, std::path::Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut out = vec![0.0; frames];
    for chunk in out.chunks_mut(chunk) {
        voice.process(chunk);
    }
    out
}

fn error(source: &str) -> String {
    match compile(source) {
        Ok(_) => panic!("expected an error: {source}"),
        Err(errors) => errors[0].message.clone(),
    }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

const SIGNAL: &str = "osc(saw, 220hz) * 0.5 + osc(sine, 1333hz) * 0.25";

#[test]
fn a_frame_that_returns_its_window_gives_the_input_back_one_frame_late() {
    let dry = render(&format!("instr a() {{ {SIGNAL} }}"), 60);
    let wet = render(
        &format!("fn same(x) {{ x.frames(256, fn(w) {{ w }}) }}\ninstr a() {{ same({SIGNAL}) }}"),
        60,
    );
    // nothing is aligned for a single path, so the output is simply late by the frame
    let shifted = (512..dry.len())
        .map(|i| (wet[i] - dry[i - 256]).abs())
        .fold(0.0, f32::max);
    assert!(shifted < 1e-4, "{shifted}");
}

#[test]
fn the_latency_of_a_frame_is_declared_so_a_path_beside_it_is_delayed_to_match() {
    let out = render(
        &format!(
            "fn same(x) {{ x.frames(256, fn(w) {{ w }}) }}\ninstr a() {{ {SIGNAL} - same({SIGNAL}) }}"
        ),
        60,
    );
    assert!(rms(&out[1024..]) < 1e-4, "{}", rms(&out[1024..]));
}

#[test]
fn fft_then_ifft_gives_the_window_back() {
    let wet = render(
        &format!(
            "fn through(x) {{ x.frames(512, fn(w) {{ s = fft(w)\n ifft(s[0], s[1]) }}) }}\ninstr a() {{ through({SIGNAL}) }}"
        ),
        80,
    );
    let dry = render(&format!("instr a() {{ {SIGNAL} }}"), 80);
    let worst = (1024..dry.len())
        .map(|i| (wet[i] - dry[i - 512]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn a_function_can_filter_the_spectrum() {
    // keep the first 40 bins of 513: below about 1.9khz, so the 1333hz tone stays and a 5khz tone goes
    let source = |tone: &str| {
        format!(
            "const keep = [{}]\nfn low(x) {{ x.frames(1024, fn(w) {{ s = fft(w)\n ifft(s[0] * keep, s[1] * keep) }}) }}\ninstr a() {{ low(osc(sine, {tone})) }}",
            (0..513)
                .map(|k| if k < 40 { "1" } else { "0" })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let low = render(&source("1333hz"), 120);
    let high = render(&source("5000hz"), 120);
    assert!(rms(&low[4096..]) > 0.5, "{}", rms(&low[4096..]));
    assert!(rms(&high[4096..]) < 0.02, "{}", rms(&high[4096..]));
}

#[test]
fn state_inside_a_frame_lasts_from_frame_to_frame() {
    // a spectrum that is held while a state array keeps the loudest magnitudes
    let out = render(
        "fn peaks(x) {
  x.frames(512, fn(w) {
    state peak = zeros(257)
    s = fft(w)
    mag = sqrt(s[0] * s[0] + s[1] * s[1])
    peak = max(peak, mag)
    ifft(peak, s[1] * 0)
  })
}
instr a() { peaks(osc(saw, 220hz) * env[1 400ms 0]) }",
        200,
    );
    assert!(out.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn atan2_gives_the_angle() {
    let out = render(
        "fn angle(x) { state held = 0\n atan2(x, 1) }\ninstr a() { angle(osc(sine, 100hz)) }",
        20,
    );
    assert!(out.iter().any(|v| v.abs() > 0.1));
    let flat = render("instr a() { atan2(1, 1) }", 2);
    assert!((flat[0] - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
}

#[test]
fn mistakes_are_reported() {
    let body = |b: &str| format!("fn f(x) {{ {b} }}\ninstr a() {{ f(osc(saw, 110hz)) }}");
    assert!(error(&body("x.frames(100, fn(w) { w })")).contains("power of two"));
    assert!(error(&body("x.frames(256, fn(w) { w[0] })")).contains("256 plain samples"));
    assert!(
        error(&body("x.frames(256, fn(w) { w.frames(256, fn(v) { v }) })"))
            .contains("another frame")
    );
    assert!(error(&body("fft([1, 2, 3])")).contains("power of two"));
    assert!(error(&body("ifft([1, 2, 3, 4], [1, 2])")).contains("same length"));
}

#[test]
fn a_mask_can_be_built_with_map() {
    let out = render(
        "const keep = (0..<513).map(it < 40)\nfn low(x) { x.frames(1024, fn(w) { s = fft(w)\n ifft(s[0] * keep, s[1] * keep) }) }\ninstr a() { low(osc(sine, 1333hz)) }",
        120,
    );
    assert!(rms(&out[4096..]) > 0.5);
}

#[test]
fn paired_frames_share_windows_and_align_input_latencies() {
    let out = render(
        "fn same(x) { x.frames(256, fn(w) { w }) }
fn compare(x, y) { x.frames(512, fn(a, b) { a - b }, other: y) }
instr a() { x = osc(saw, 220hz)\n compare(x, same(x)) }",
        80,
    );
    assert!(rms(&out[2048..]) < 1e-5);
}

#[test]
fn paired_frames_deliver_distinct_inputs_without_changing_reconstruction() {
    let dry = render("instr a() { osc(sine, 1333hz) }", 80);
    let wet = render(
        "fn second(x, y) { x.frames(512, fn(a, b) { b }, other: y) }
instr a() { second(osc(saw, 220hz), osc(sine, 1333hz)) }",
        80,
    );
    let worst = (1024..dry.len())
        .map(|i| (wet[i] - dry[i - 512]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-5, "{worst}");
}

#[test]
fn envelope_transfer_preserves_bypass_and_self_and_silences_a_silent_donor() {
    for expression in [
        "envelope_transfer(x, noise(), amount: 0)",
        "envelope_transfer(x, x)",
    ] {
        let out = render(
            &format!(
                "use std/spectral\ninstr a() {{ x = osc(saw, 220hz) * 0.2\n {expression} - x }}"
            ),
            100,
        );
        assert!(
            rms(&out[4096..]) < 1e-5,
            "{expression}: {}",
            rms(&out[4096..])
        );
    }
    let out = render(
        "use std/spectral\ninstr a() { envelope_transfer(osc(saw, 220hz), 0) }",
        100,
    );
    assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1e-6));
}

#[test]
fn envelope_transfer_follows_the_donors_broad_colour() {
    let out = render(
        "use std/spectral\ninstr a() {
  carrier = noise(seed: 11) * 0.1
  donor = noise(seed: 7).lpf(cutoff: 700hz)
  envelope_transfer(carrier, donor)
}",
        240,
    );
    let tail = &out[out.len() - 8192..];
    let amplitude = |hz: f64| {
        let (re, im) = tail
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (i, &v)| {
                let phase = std::f64::consts::TAU * hz * i as f64 / 48_000.0;
                (re + v as f64 * phase.cos(), im + v as f64 * phase.sin())
            });
        2.0 * re.hypot(im) / tail.len() as f64
    };
    let low = amplitude(375.0);
    let high = amplitude(3000.0);
    assert!(out.iter().all(|v| v.is_finite()));
    assert!(low > 0.001 && low > high * 3.0, "low {low}, high {high}");
}

#[test]
fn paired_frames_reject_wrong_callback_arity_and_non_audio_units() {
    assert!(
        error("fn f(x) { x.frames(256, fn(a) { a }, other: x) }\ninstr a() { f(noise()) }")
            .contains("argument")
    );
    assert!(
        error("fn f(x) { x.frames(256, fn(a, b) { a }, other: 440hz) }\ninstr a() { f(noise()) }")
            .contains("plain signal")
    );
}

#[test]
fn denser_frame_overlaps_reconstruct_and_compensate_latency() {
    for overlap in [2, 4, 8, 16] {
        let out = render(
            &format!(
                "fn same(x) {{ x.frames(256, fn(w) {{ w }}, overlap: {overlap}) }}\ninstr a() {{ {SIGNAL} - same({SIGNAL}) }}"
            ),
            60,
        );
        assert!(rms(&out[1024..]) < 1e-5, "overlap {overlap}");
    }
    for overlap in ["0", "3", "32", "noise()"] {
        assert!(error(&format!("fn f(x) {{ x.frames(16, fn(w) {{ w }}, overlap: {overlap}) }}\ninstr a() {{ f(noise()) }}")).contains("frame overlap"));
    }
}

#[test]
fn phase_frequency_and_accumulation_reconstruct_fft_phases() {
    let out = render(
        "use std/spectral
fn through(x) {
  x.frames(512, fn(w) {
    s = fft(w)
    mag = sqrt(s[0] * s[0] + s[1] * s[1])
    frequency = phase_frequency(atan2(s[1], s[0]), size: 512, hop: 128)
    phase = phase_accumulate(frequency, size: 512, hop: 128)
    ifft(mag * cos(phase), mag * sin(phase))
  }, overlap: 4)
}
instr a() { x = osc(saw, 997hz) * 0.2 + noise(seed: 17) * 0.1\n through(x) - x }",
        100,
    );
    assert!(rms(&out[2048..]) < 1e-5, "{}", rms(&out[2048..]));
}

#[test]
fn spectral_pitch_moves_an_off_bin_tone_and_is_independent_of_chunks() {
    for shift in [-12, -7, 0, 7, 12] {
        let source = format!(
            "use std/spectral\ninstr a() {{ spectral_pitch(osc(sine, 997hz) * 0.2, shift: {shift}, size: 512) }}"
        );
        let out = render_chunk(&source, 24_000, block_size());
        let tail = &out[8192..];
        let cycles = tail
            .windows(2)
            .filter(|s| s[0] < 0.0 && s[1] >= 0.0)
            .count();
        let actual = cycles as f64 * 48_000.0 / tail.len() as f64;
        let expected = 997.0 * 2.0_f64.powf(shift as f64 / 12.0);
        assert!(
            (actual - expected).abs() < 6.0,
            "shift {shift}: {actual}, expected {expected}"
        );
        assert!(
            rms(tail) > 0.01 && rms(tail) < 0.5,
            "shift {shift}: rms {}",
            rms(tail)
        );
        assert!(out.iter().all(|v| v.is_finite()));
        if shift == 7 {
            assert_eq!(out, render_chunk(&source, 24_000, 37));
        }
    }
}

#[test]
fn spectral_pitch_zero_is_identity_and_out_of_band_energy_is_removed() {
    let out = render(
        "use std/spectral\ninstr a() { x = noise(seed: 31) * 0.1\n spectral_pitch(x, size: 512) - x }",
        100,
    );
    assert!(rms(&out[2048..]) < 1e-5, "{}", rms(&out[2048..]));
    let high = render(
        "use std/spectral\ninstr a() { spectral_pitch(osc(sine, 18000hz) * 0.2, shift: 12, size: 512) }",
        100,
    );
    assert!(rms(&high[4096..]) < 0.005, "{}", rms(&high[4096..]));
}

#[test]
fn spectral_pitch_follows_a_shift_envelope_without_restarting() {
    let out = render_chunk(
        "use std/spectral\ninstr a() {
  spectral_pitch(osc(sine, 997hz) * 0.2, shift: env[0 100ms 0 100ms 12], size: 512)
}",
        24_000,
        block_size(),
    );
    let frequency = |slice: &[f32]| {
        let cycles = slice
            .windows(2)
            .filter(|s| s[0] < 0.0 && s[1] >= 0.0)
            .count();
        cycles as f64 * 48_000.0 / slice.len() as f64
    };
    assert!((frequency(&out[2048..4096]) - 997.0).abs() < 25.0);
    assert!((frequency(&out[16000..24000]) - 1994.0).abs() < 8.0);
    assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.0));
}

#[test]
fn peak_remapping_retains_narrow_partials_on_large_downward_shifts() {
    for shift in [-24, -36, -48] {
        let out = render_chunk(
            &format!(
                "use std/spectral\ninstr a() {{ spectral_pitch(osc(sine, 997hz) * 0.2, shift: {shift}, size: 512, transients: 0) }}"
            ),
            48_000,
            block_size(),
        );
        let tail = &out[8192..];
        assert!(rms(tail) > 0.04, "shift {shift}: {}", rms(tail));
        let cycles = tail
            .windows(2)
            .filter(|s| s[0] < 0.0 && s[1] >= 0.0)
            .count();
        let actual = cycles as f64 * 48000.0 / tail.len() as f64;
        let expected = 997.0 * 2.0_f64.powf(shift as f64 / 12.0);
        assert!(
            (actual - expected).abs() < 5.0,
            "shift {shift}: {actual}, expected {expected}"
        );
    }
}

#[test]
fn spectral_memory_freezes_chronological_frames_and_transposes_them() {
    for (position, shift, expected) in [(0.15, 0, 750.0), (0.9, 0, 1500.0), (0.15, 12, 1500.0)] {
        let source=format!("instr a() {{
  tone = osc(sine, env[750hz 100ms 750hz 1ms 1500hz]) * 0.2 * env[1 180ms 1 20ms 0]
  spectral_memory(tone, record: env[1 150ms 1 1ms 0], position: {position}, shift: {shift}, frames: 64, size: 512)
}}");
        let out = render_chunk(&source, 24000, block_size());
        let tail = &out[16000..];
        assert!(
            rms(tail) > 0.05,
            "position {position}, shift {shift}: {}",
            rms(tail)
        );
        let cycles = tail
            .windows(2)
            .filter(|s| s[0] < 0.0 && s[1] >= 0.0)
            .count();
        let hz = cycles as f64 * 48000.0 / tail.len() as f64;
        assert!(
            (hz - expected).abs() < 8.0,
            "position {position}, shift {shift}: {hz}, expected {expected}"
        );
        assert_eq!(out, render_chunk(&source, 24000, 37));
    }
}

#[test]
fn empty_spectral_memory_is_silent_and_storage_settings_are_checked() {
    let out = render(
        "instr a() { spectral_memory(noise(), record: 0, frames: 8, size: 512) }",
        50,
    );
    assert!(out.iter().all(|v| *v == 0.0));
    for arguments in [
        "frames: 0",
        "frames: 1.5",
        "frames: 65536, size: 65536",
        "overlap: 3",
        "size: 100",
        "detail: 0",
    ] {
        assert!(
            compile(&format!(
                "instr a() {{ spectral_memory(noise(), {arguments}) }}"
            ))
            .is_err(),
            "{arguments}"
        );
    }
}

#[test]
fn independent_formant_control_changes_colour_at_unchanged_pitch() {
    let render_colour = |formant: i32| {
        render_chunk(
            &format!(
                "use std/spectral\ninstr a() {{
  source=osc(saw, 93.75hz).bpf(cutoff: 1500hz, res: 0.7) * 0.2
  spectral_pitch(source, formant: {formant}, detail: 32, size: 1024)
}}"
            ),
            24000,
            block_size(),
        )
    };
    let roughness = |out: &[f32]| {
        let tail = &out[8192..];
        tail.windows(2)
            .map(|s| (s[1] - s[0]).powi(2) as f64)
            .sum::<f64>()
            / tail.iter().map(|x| (*x as f64).powi(2)).sum::<f64>()
    };
    let low = render_colour(-12);
    let original = render_colour(0);
    let high = render_colour(12);
    let (lo, plain, hi) = (roughness(&low), roughness(&original), roughness(&high));
    assert!(
        lo < plain * 0.8 && hi > plain * 1.5,
        "low {lo}, original {plain}, high {hi}"
    );
    for audio in [&low, &high] {
        assert!(audio.iter().all(|v| v.is_finite()));
        let tail = &audio[8192..];
        let difference: Vec<_> = tail[512..].iter().zip(tail).map(|(a, b)| a - b).collect();
        assert!(
            rms(&difference) < rms(tail) * 0.02,
            "formant shift changed the 512-sample fundamental period"
        );
    }
}

#[test]
fn envelope_preservation_keeps_a_resonance_below_the_shifted_pitch_envelope() {
    let render_preserve = |preserve: i32| {
        render_chunk(
            &format!(
                "use std/spectral\ninstr a() {{
  source=osc(saw, 93.75hz).bpf(cutoff: 1500hz, res: 0.7) * 0.2
  spectral_pitch(source, shift: 12, preserve: {preserve}, detail: 32, size: 1024)
}}"
            ),
            24000,
            block_size(),
        )
    };
    let amplitude = |audio: &[f32], hz: f64| {
        let tail = &audio[audio.len() - 8192..];
        let (re, im) = tail
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (i, &x)| {
                let angle = std::f64::consts::TAU * hz * i as f64 / 48000.0;
                (re + x as f64 * angle.cos(), im + x as f64 * angle.sin())
            });
        re.hypot(im)
    };
    let natural = render_preserve(0);
    let preserved = render_preserve(1);
    let natural_ratio = amplitude(&natural, 1500.0) / amplitude(&natural, 3000.0);
    let preserved_ratio = amplitude(&preserved, 1500.0) / amplitude(&preserved, 3000.0);
    assert!(
        preserved_ratio > natural_ratio * 2.0,
        "natural {natural_ratio}, preserved {preserved_ratio}"
    );
}

#[test]
fn peak_remapping_keeps_off_bin_tones_tuned_on_large_upward_shifts() {
    for shift in [24, 36, 48] {
        let source = format!(
            "use std/spectral\ninstr a() {{ spectral_pitch(osc(sine, 997hz) * 0.1, shift: {shift}, size: 512, transients: 0) }}"
        );
        let out = render_chunk(&source, 24000, block_size());
        let tail = &out[8192..];
        let cycles = tail
            .windows(2)
            .filter(|s| s[0] < 0.0 && s[1] >= 0.0)
            .count();
        let actual = cycles as f64 * 48000.0 / tail.len() as f64;
        let expected = 997.0 * 2.0_f64.powf(shift as f64 / 12.0);
        assert!(
            (actual - expected).abs() < 8.0,
            "shift {shift}: {actual}, expected {expected}"
        );
        assert!(rms(tail) > 0.01);
    }
}
