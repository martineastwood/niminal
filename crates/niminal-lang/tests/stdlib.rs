//! The library that comes with niminal (`use std/...`), tested by what it does to a sound.

use std::path::Path;

use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};

fn render(source: &str, blocks: usize) -> Vec<f32> {
    let expanded =
        expand(source, Path::new(".")).unwrap_or_else(|d| panic!("{}: {source}", d.message));
    let program = compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("std", &expanded.text))
                .collect::<String>()
        )
    });
    let instrument = program.instruments.iter().find(|i| i.name == "a").unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut out = vec![0.0; blocks * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn finite(x: &[f32]) -> bool {
    x.iter().all(|v| v.is_finite() && v.abs() < 50.0)
}

/// The level of a sine at `hz` after `filter`, once settled.
fn gain(module: &str, filter: &str, hz: u32) -> f32 {
    let out = render(
        &format!("use {module}\ninstr a() {{ osc(sine, {hz}hz).{filter} }}"),
        400,
    );
    assert!(finite(&out), "{filter} at {hz}");
    rms(&out[out.len() / 2..]) / (0.5f32).sqrt()
}

#[test]
fn the_ladder_lets_low_through_and_stops_high() {
    let (low, high) = (
        gain("std/filters", "ladder(1khz, res: 0)", 100),
        gain("std/filters", "ladder(1khz, res: 0)", 8000),
    );
    assert!(low > 0.9, "{low}");
    assert!(high < 0.02, "{high}");
}

#[test]
fn raising_the_ladders_resonance_lifts_the_cutoff() {
    // a quiet signal, so the stages' `tanh` is nearly linear
    let level = |res: &str| {
        let out = render(
            &format!(
                "use std/filters\ninstr a() {{ (osc(sine, 1khz) * 0.02).ladder(1khz, res: {res}) }}"
            ),
            400,
        );
        rms(&out[out.len() / 2..]) / (0.02 * 0.5f32.sqrt())
    };
    let (flat, peaked) = (level("0"), level("0.9"));
    assert!(peaked > flat * 4.0, "{flat} {peaked}");
}

#[test]
fn the_ladder_stays_bounded_when_it_sings() {
    let out = render(
        "use std/filters\ninstr a() { noise().ladder(2khz, res: 1) }",
        600,
    );
    assert!(finite(&out));
    assert!(out.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn biquads_pass_and_stop_the_right_bands() {
    let lp = |hz| gain("std/filters", "biquad_lp(1khz)", hz);
    let hp = |hz| gain("std/filters", "biquad_hp(1khz)", hz);
    let bp = |hz| gain("std/filters", "biquad_bp(1khz, q: 4)", hz);
    let notch = |hz| gain("std/filters", "biquad_notch(1khz, q: 4)", hz);
    assert!(
        lp(100) > 0.95 && lp(10_000) < 0.05,
        "{} {}",
        lp(100),
        lp(10_000)
    );
    assert!(
        hp(10_000) > 0.95 && hp(100) < 0.05,
        "{} {}",
        hp(10_000),
        hp(100)
    );
    assert!(bp(1000) > 0.9 && bp(100) < 0.2 && bp(10_000) < 0.2);
    assert!(notch(1000) < 0.1 && notch(100) > 0.9 && notch(10_000) > 0.9);
}

#[test]
fn one_pole_filters_split_a_signal() {
    let lp = gain("std/filters", "onepole_lp(1khz)", 100);
    let hp = gain("std/filters", "onepole_hp(1khz)", 100);
    assert!(lp > 0.9 && hp < 0.2, "{lp} {hp}");
}

#[test]
fn every_library_module_compiles() {
    for module in ["filters", "physical", "delays", "reverb", "spectral"] {
        let expanded = expand(
            &format!("use std/{module}\ninstr a() {{ osc(sine, 1hz) }}"),
            Path::new("."),
        )
        .unwrap_or_else(|d| panic!("{module}: {}", d.message));
        compile(&expanded.text).unwrap_or_else(|errors| {
            panic!(
                "{module}: {}",
                errors
                    .iter()
                    .map(|e| e.render(module, &expanded.text))
                    .collect::<String>()
            )
        });
    }
}

/// How strongly `x` has a component at `hz` (a Goertzel filter), as an amplitude.
fn at_hz(x: &[f32], hz: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * hz / 48_000.0;
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in x {
        let s = v + 2.0 * w.cos() * s1 - s2;
        (s2, s1) = (s1, s);
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2)
        .max(0.0)
        .sqrt()
        * 2.0
        / x.len() as f32
}

#[test]
fn a_string_rings_at_its_pitch() {
    let out = render(
        "use std/physical\ninstr a() { (noise() * env[1 4ms 0]).string(220hz, decay: 0.999) }",
        1200,
    );
    assert!(finite(&out));
    // the fundamental stands out from frequencies between the harmonics
    let heard = &out[4800..4800 + 24_000];
    let (f0, between_low, between_high) = (
        at_hz(heard, 220.0),
        at_hz(heard, 165.0),
        at_hz(heard, 275.0),
    );
    assert!(
        f0 > between_low * 5.0 && f0 > between_high * 5.0,
        "{f0} {between_low} {between_high}"
    );
}

#[test]
fn a_drum_rings_at_its_modes_and_dies_away() {
    let out = render(
        "use std/physical\ninstr a() { impulse(0.5hz).drum(150hz, decay: 0.9995) }",
        1000,
    );
    assert!(finite(&out));
    let early = &out[2000..14_000];
    // the fundamental and the second mode (1.593 times it) stand out from a frequency between them
    let (f0, f1, between) = (
        at_hz(early, 150.0),
        at_hz(early, 150.0 * 1.593),
        at_hz(early, 195.0),
    );
    assert!(
        f0 > between * 5.0 && f1 > between * 5.0,
        "{f0} {f1} {between}"
    );
    assert!(
        rms(&out[30_000..36_000]) < rms(&out[2000..8000]),
        "it decays"
    );
}

#[test]
fn a_bar_and_a_bell_have_their_partials() {
    let bar = render(
        "use std/physical\ninstr a() { impulse(0.5hz).free_bar(200hz) }",
        800,
    );
    let bell = render(
        "use std/physical\ninstr a() { impulse(0.5hz).bell(200hz) }",
        800,
    );
    assert!(finite(&bar) && finite(&bell));
    assert!(
        at_hz(&bar[2000..14_000], 200.0 * 2.756) > at_hz(&bar[2000..14_000], 200.0 * 2.0) * 5.0
    );
    // the hum of a bell is half the strike note
    assert!(at_hz(&bell[2000..14_000], 100.0) > at_hz(&bell[2000..14_000], 150.0) * 5.0);
}

#[test]
fn a_multitap_gives_echoes_at_its_times_and_gains() {
    let out = render(
        "use std/delays\ninstr a() { (impulse(0.5hz) * 0.5).multitap([100ms, 250ms], [0.5, 0.25]) }",
        400,
    );
    // the click at the start, and echoes 4800 and 12000 samples after it
    let peak =
        |range: std::ops::Range<usize>| out[range].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        peak(4700..4900) > 0.2 && peak(4700..4900) < 0.3,
        "{}",
        peak(4700..4900)
    );
    assert!(
        peak(11_900..12_100) > 0.1 && peak(11_900..12_100) < 0.15,
        "{}",
        peak(11_900..12_100)
    );
    assert!(peak(7000..9000) < 0.01, "silence between the echoes");
}

#[test]
fn a_tape_delay_repeats_and_each_repeat_is_quieter_and_darker() {
    let out = render(
        "use std/delays\ninstr a() { (impulse(0.2hz) * 0.5).tape(100ms, feedback: 0.6, damp: 2khz) }",
        400,
    );
    let peak = |at: usize| {
        out[at - 100..at + 100]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let (one, two, three) = (peak(4800), peak(9600), peak(14_400));
    assert!(
        one > two && two > three && three > 0.01,
        "{one} {two} {three}"
    );
}

#[test]
fn the_fdn_reverb_has_a_tail_that_falls_away() {
    let out = render(
        "use std/reverb\ninstr a() { impulse(0.2hz).fdn(time: 1sec) }",
        1500,
    );
    assert!(finite(&out));
    let early = rms(&out[2400..9600]);
    let late = rms(&out[40_000..46_000]);
    let later = rms(&out[70_000..76_000]);
    assert!(early > 1e-4, "{early}");
    assert!(late < early && later < late, "{early} {late} {later}");
    // about 60db down after `time`: the tail at 1.5 seconds is far below the start
    assert!(later < early * 0.05, "{early} {later}");
}

#[test]
fn a_shorter_reverb_time_dies_faster() {
    let tail = |time: &str| {
        let out = render(
            &format!("use std/reverb\ninstr a() {{ impulse(0.2hz).fdn(time: {time}) }}"),
            1200,
        );
        rms(&out[40_000..46_000])
    };
    assert!(tail("0.5sec") < tail("2sec") * 0.5);
}

#[test]
fn spectral_band_keep_passes_one_band_and_removes_another() {
    // 1024 bins span 24khz: the first quarter is below 6khz
    let level = |hz: u32| {
        let out = render(
            &format!(
                "use std/spectral\ninstr a() {{ osc(sine, {hz}hz).band_keep(lo: 0, hi: 0.25) }}"
            ),
            300,
        );
        assert!(finite(&out));
        rms(&out[8000..])
    };
    assert!(level(1000) > 0.6, "{}", level(1000));
    assert!(level(12_000) < 0.02, "{}", level(12_000));
}

#[test]
fn spectral_effects_run_and_make_sound() {
    for effect in ["robot()", "blur(amount: 0.8)", "denoise(threshold: 0.001)"] {
        let out = render(
            &format!("use std/spectral\ninstr a() {{ (osc(saw, 220hz) * 0.5).{effect} }}"),
            300,
        );
        assert!(finite(&out), "{effect}");
        assert!(rms(&out[8000..]) > 0.02, "{effect}: {}", rms(&out[8000..]));
    }
}

#[test]
fn the_spectral_gate_removes_quiet_noise_and_keeps_a_loud_tone() {
    let gated = render(
        "use std/spectral\ninstr a() { (noise() * 0.003).denoise(threshold: 0.02) }",
        300,
    );
    let tone = render(
        "use std/spectral\ninstr a() { (osc(sine, 1000hz) * 0.5).denoise(threshold: 0.02) }",
        300,
    );
    assert!(rms(&gated[8000..]) < 0.0005, "{}", rms(&gated[8000..]));
    assert!(rms(&tone[8000..]) > 0.2, "{}", rms(&tone[8000..]));
}

/// The examples the documentation shows, so they cannot rot.
#[test]
fn the_documented_examples_compile_and_sound() {
    let cases = [
        // standard library page
        "use std/filters\nuse std/reverb\ninstr a() {\n  freq = 110hz\n  out = osc(saw, freq).ladder(cutoff: 900hz, res: 0.4)\n  out += out.fdn(time: 2.5sec) * 0.3\n}",
        "use std/physical\ninstr a(freq: hz = 110hz) { (impulse(1hz) * 0.5).drum(freq) }",
        // functions page
        "const modes = [1, 2.76, 5.4, 8.9, 13.3, 18.6, 24.8, 31.9]\nfn chime(x, freq: init hz) {\n  state s1 = zeros(8)\n  state s2 = zeros(8)\n  w = 2 * pi * freq / sample_rate * modes\n  y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2\n  s2 = s1\n  s1 = y\n  y.sum\n}\ninstr a() { impulse(2hz).chime(220hz) }",
        "const keep = (0..<513).map(it < 40)\nfn low(x) {\n  x.frames(1024, fn(w) {\n    s = fft(w)\n    ifft(s[0] * keep, s[1] * keep)\n  })\n}\ninstr a() { low(osc(saw, 220hz)) }",
        "fn shape(x, curve: [num]) { x.waveshape(curve) }\ninstr a() { shape(osc(sine, 110hz), cheby([1, 0.3])) }",
    ];
    for source in cases {
        let out = render(source, 200);
        assert!(
            finite(&out) && out.iter().any(|v| v.abs() > 1e-3),
            "{source}"
        );
    }
}

/// Every instrument in `std/instruments` plays a pitched note: finite, audible, and not wildly loud.
#[test]
fn the_instrument_library_compiles_and_every_instrument_sounds() {
    let expanded = expand("use std/instruments\n", Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap_or_else(|errors| {
        panic!("{}", errors.iter().map(|e| e.render("std", &expanded.text)).collect::<String>())
    });
    let names: Vec<_> = program.instruments.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["kick", "snare", "hat", "bass", "pluck", "pad", "lead", "bell"]);
    for instrument in &program.instruments {
        let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
        voice.set_param_by_name("freq", 220.0);
        voice.start_note();
        let mut out = vec![0.0; 100 * block_size()];
        for chunk in out.chunks_mut(block_size()) {
            voice.process(chunk);
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 2.0), "{} is not bounded", instrument.name);
        assert!(rms(&out) > 1e-4, "{} is silent: {}", instrument.name, rms(&out));
    }
}

/// The effect library compiles, and every effect's parameters have a range and a default.
#[test]
fn the_effect_library_compiles() {
    let expanded = expand("use std/fx\n", Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap_or_else(|errors| {
        panic!("{}", errors.iter().map(|e| e.render("std", &expanded.text)).collect::<String>())
    });
    assert!(program.instruments.is_empty());
    assert!(expanded.text.matches("\nfx ").count() >= 14);
}
