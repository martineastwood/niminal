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
    for module in ["filters", "physical", "delays", "spectral"] {
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

/// The delay is shortened to make up for the damping in the loop, so the string sounds at the pitch asked for.
#[test]
fn a_string_is_in_tune() {
    for (freq, damp) in [(220, "0.5"), (440, "0.5"), (220, "0.2")] {
        let out = render(
            &format!("use std/physical\ninstr a() {{ (noise() * env[1 4ms 0]).string({freq}hz, decay: 0.998, damp: {damp}) }}"),
            1200,
        );
        let heard = &out[4800..4800 + 24_000];
        let (mut best, mut loudest) = (0.0, 0.0f32);
        let mut hz = freq as f32 - 6.0;
        while hz <= freq as f32 + 6.0 {
            let level = at_hz(heard, hz);
            if level > loudest {
                (best, loudest) = (hz, level);
            }
            hz += 0.25;
        }
        assert!((best - freq as f32).abs() < 1.0, "{freq}hz with damp {damp} sounds at {best}");
    }
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
        "use std/physical\ninstr a() { impulse(0.5hz).membrane(150hz, decay: 300ms) }",
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

/// Echoes at times and gains of your own are delays added by hand, one line each; the `echoes` function does a
/// regular train of them.
#[test]
fn taps_are_delays_and_echoes_is_a_train_of_them() {
    let peak = |out: &[f32], range: std::ops::Range<usize>| out[range].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // the click at the start, and echoes 4800 and 12000 samples after it
    let out = render(
        "instr a() { x = impulse(0.5hz) * 0.5\n x + x.delay(100ms) * 0.5 + x.delay(250ms) * 0.25 }",
        400,
    );
    assert!(peak(&out, 4700..4900) > 0.2 && peak(&out, 4700..4900) < 0.3, "{}", peak(&out, 4700..4900));
    assert!(peak(&out, 11_900..12_100) > 0.1 && peak(&out, 11_900..12_100) < 0.15, "{}", peak(&out, 11_900..12_100));
    assert!(peak(&out, 7000..9000) < 0.01, "silence between the echoes");
    // three echoes 100 ms apart, each half the level of the one before
    let out = render(
        "use std/delays\ninstr a() { (impulse(0.5hz) * 0.5).echoes(count: 3, spacing: 100ms, fade: 0.5) }",
        400,
    );
    for (at, level) in [(4800, 0.25), (9600, 0.125), (14_400, 0.0625)] {
        let heard = peak(&out, at - 100..at + 100);
        assert!((heard - level).abs() < level * 0.1, "echo at {at}: {heard}");
    }
    assert!(peak(&out, 17_000..25_000) < 0.01, "and no more");
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
        "instr a() { impulse(0.2hz).fdn_reverb(time: 1sec) }",
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
            &format!("instr a() {{ impulse(0.2hz).fdn_reverb(time: {time}) }}"),
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
        "instr a() {\n  freq = 110hz\n  out = osc(saw, freq).ladder(cutoff: 900hz, res: 0.4)\n  out += out.fdn_reverb(time: 2.5sec) * 0.3\n}",
        "use std/physical\ninstr a(freq: hz = 110hz) { (impulse(1hz) * 0.5).membrane(freq) }",
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
    let expanded = expand(&take_all("std/instruments"), Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("std", &expanded.text))
                .collect::<String>()
        )
    });
    let names: Vec<_> = program
        .instruments
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "kick", "snare", "hat", "bass", "pluck", "pad", "lead", "bell"
        ]
    );
    for instrument in &program.instruments {
        let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
        voice.set_param_by_name("freq", 220.0);
        voice.start_note();
        let mut out = vec![0.0; 100 * block_size()];
        for chunk in out.chunks_mut(block_size()) {
            voice.process(chunk);
        }
        assert!(
            out.iter().all(|v| v.is_finite() && v.abs() < 2.0),
            "{} is not bounded",
            instrument.name
        );
        assert!(
            rms(&out) > 1e-4,
            "{} is silent: {}",
            instrument.name,
            rms(&out)
        );
    }
}

/// The effect library compiles, and every effect's parameters have a range and a default.
#[test]
fn the_effect_library_compiles() {
    let expanded = expand(&take_all("std/fx"), Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("std", &expanded.text))
                .collect::<String>()
        )
    });
    assert!(program.instruments.is_empty());
    assert!(expanded.text.matches("\nfx ").count() >= 15);
}

/// Every drum preset plays one note, bounded and audible.
#[test]
fn the_drum_library_compiles_and_every_preset_sounds() {
    let expanded = expand(&take_all("std/drums"), Path::new(".")).unwrap();
    let program = compile(&expanded.text).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("std", &expanded.text))
                .collect::<String>()
        )
    });
    assert_eq!(program.instruments.len(), 21);
    for instrument in &program.instruments {
        let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
        voice.start_note();
        let mut out = vec![0.0; 150 * block_size()];
        for chunk in out.chunks_mut(block_size()) {
            voice.process(chunk);
        }
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(
            out.iter().all(|v| v.is_finite()) && peak <= 1.0,
            "{} is not bounded: {peak}",
            instrument.name
        );
        assert!(rms(&out) > 1e-4, "{} is silent", instrument.name);
    }
}

/// A choice word (`wobble` in `mod_mode: (drop | wobble | random)`) can't also name a preset: a file
/// that uses both quietly plays nothing.
#[test]
fn no_drum_preset_is_named_like_a_choice_of_the_drum_voice() {
    let text = include_str!("../std/drums.nml");
    let helpers = &niminal_lang::presets::find("kick_808").unwrap().helpers;
    let voice = &helpers[helpers.find("fn drum(").unwrap()..];
    let signature = &voice[..voice.find(") {").unwrap()];
    let choices: Vec<&str> = signature
        .lines()
        .filter_map(|l| l.split_once('(').and_then(|(_, rest)| rest.split_once(')')))
        .flat_map(|(inside, _)| inside.split('|').map(str::trim))
        .collect();
    assert!(
        choices.contains(&"wobble"),
        "found the choices: {choices:?}"
    );
    for name in text
        .lines()
        .filter_map(|l| l.strip_prefix("instr "))
        .filter_map(|l| l.split('(').next())
    {
        assert!(
            !choices.contains(&name),
            "`{name}` is also a choice of `drum`"
        );
    }
}

/// Renders `instr a() { drum_call }` from `std/drums` through the engine and returns its samples.
fn render_drum(call: &str, blocks: usize) -> Vec<f32> {
    render(&format!("use std/drums.{{kick_808}}\ninstr a() {{ {call} }}"), blocks)
}

#[test]
fn a_softer_note_is_quieter_by_as_much_as_the_sensitivity_says() {
    let peak = |vel: f32, osc_vel: f32| {
        let out = render_drum(
            &format!("drum(freq: 100hz, decay: 300ms, vel: {vel}, osc_vel: {osc_vel})"),
            60,
        );
        out.iter().fold(0.0f32, |a, b| a.max(b.abs()))
    };
    let (hard, soft, blind) = (peak(1.0, 1.0), peak(0.25, 1.0), peak(0.25, 0.0));
    assert!(
        soft < hard * 0.4,
        "a quarter-velocity note with full sensitivity: {soft} against {hard}"
    );
    assert!(
        (blind - hard).abs() < hard * 0.02,
        "no sensitivity ignores velocity: {blind} against {hard}"
    );
}

#[test]
fn the_drum_waves_have_their_own_harmonics() {
    // a square has odd harmonics only, so at 3x the pitch it holds energy a sine doesn't
    let at = |wave: &str| {
        let out = render_drum(
            &format!("drum(freq: 500hz, wave: {wave}, decay: 2sec, drive: 0)"),
            200,
        );
        let tail = &out[out.len() / 4..];
        let bin = |hz: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in tail.iter().enumerate() {
                let phase = std::f32::consts::TAU * hz * i as f32 / 48_000.0;
                re += v * phase.cos();
                im += v * phase.sin();
            }
            (re * re + im * im).sqrt() / tail.len() as f32
        };
        (bin(500.0), bin(1500.0))
    };
    // the voice always soft-clips a little, so a sine is not perfectly pure: compare the two waves
    let (sine, square) = (at("sine"), at("square"));
    let (sine_third, square_third) = (sine.1 / sine.0, square.1 / square.0);
    assert!(
        square_third > 0.2,
        "a square has a strong third harmonic: {square:?}"
    );
    assert!(
        sine_third < square_third * 0.5,
        "a sine has much less: {sine:?} against {square:?}"
    );
}

#[test]
fn closed_hats_share_a_choke_group_with_open_ones() {
    let text = include_str!("../std/drums.nml");
    for name in ["hat_closed", "hat_open", "hat_tick"] {
        let start = text.find(&format!("instr {name}(")).unwrap();
        let body = &text[start
            ..text[start..]
                .find("\n}\n")
                .map_or(text.len(), |e| start + e)];
        assert!(
            body.contains("choke = hats"),
            "{name} is in the hats choke group"
        );
    }
    assert_eq!(text.matches("choke = ").count(), 3, "only the hats choke");
}

/// Whatever the settings, the drum voice stays finite and no louder than full scale: random settings
/// across every choice and the whole range of every control, with the extremes over-represented.
#[test]
fn the_drum_voice_is_bounded_at_any_setting() {
    let mut state = 7u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    for case in 0..150 {
        let mut pick = |options: &[&'static str]| {
            options[(next() * options.len() as f64) as usize % options.len()]
        };
        let (wave, mode, filter, shape) = (
            pick(&["sine", "tri", "square", "saw"]),
            pick(&["drop", "wobble", "random"]),
            pick(&["low", "band", "high"]),
            pick(&["soft", "linear", "burst"]),
        );
        let mut value = |lo: f64, hi: f64, log: bool| {
            let r = next();
            let t = if r < 0.1 {
                0.0
            } else if r < 0.2 {
                1.0
            } else {
                next()
            };
            if log {
                (lo.ln() + t * (hi.ln() - lo.ln())).exp()
            } else {
                lo + t * (hi - lo)
            }
        };
        let call = format!(
            "drum(freq: {:.2}hz, wave: {wave}, attack: {:.3}ms, decay: {:.2}ms, mod_mode: {mode}, mod_amount: {:.1}, \
             mod_rate: {:.2}hz, noise_filter: {filter}, noise_freq: {:.1}hz, noise_q: {:.3}, noise_shape: {shape}, \
             noise_attack: {:.3}ms, noise_decay: {:.2}ms, mix: {:.3}, drive: {:.3}, eq_freq: {:.1}hz, eq_gain: {:.1}db, \
             amp: 0db, vel: {:.3}, osc_vel: {:.3}, noise_vel: {:.3}, mod_vel: {:.3})",
            value(20.0, 8000.0, true),
            value(0.1, 50.0, true),
            value(5.0, 10_000.0, true),
            value(-48.0, 48.0, false),
            value(1.0, 4000.0, true),
            value(20.0, 20_000.0, true),
            value(0.0, 1.0, false),
            value(0.1, 200.0, true),
            value(5.0, 10_000.0, true),
            value(0.0, 1.0, false),
            value(0.0, 1.0, false),
            value(20.0, 20_000.0, true),
            value(-24.0, 24.0, false),
            value(0.0, 1.0, false),
            value(0.0, 1.0, false),
            value(0.0, 1.0, false),
            value(0.0, 1.0, false),
        );
        let out = render_drum(&call, 150);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(
            out.iter().all(|v| v.is_finite()) && peak <= 1.0 + 1e-3,
            "case {case}: peak {peak}\n{call}"
        );
    }
}

#[test]
fn repeat_last_repeats_the_last_piece_while_held_and_passes_the_sound_after() {
    // a ramp that rises every second, held from 0.5 s to 0.7 s, with a 47 ms piece
    let out = render(
        "instr a() { osc(sine, 220hz).repeat_last(hold: (phasor(1hz) > 0.5) * (phasor(1hz) < 0.7), length: 47ms) }",
        1000,
    );
    assert!(finite(&out));
    let piece = 2256; // 47 ms: not a whole number of cycles, so only a real repeat matches
    let at = 24_000 + 300; // past the 3 ms blend that fades the repeat in
    let (first, second) = (&out[at..at + 500], &out[at + piece..at + piece + 500]);
    let differ = first.iter().zip(second).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(differ < 0.01, "the second repeat matches the first: {differ}");
    assert!(rms(&out[40_000..46_000]) > 0.3, "the sound returns after the hold");
}

#[test]
fn rewind_last_plays_the_last_piece_backwards_while_held() {
    // a slow rising ramp, held from 0.5 s: read backwards, it falls
    let out = render(
        "instr a() { phasor(0.5hz).rewind_last(hold: (phasor(1hz) > 0.5) * (phasor(1hz) < 0.7), length: 50ms) }",
        1000,
    );
    assert!(finite(&out));
    let at = 24_000 + 200;
    assert!(out[at + 400] < out[at + 100], "the ramp falls while rewinding: {} then {}", out[at + 100], out[at + 400]);
    assert!(out[40_000] > out[30_000], "the live ramp is back after the hold");
}

#[test]
fn slow_down_lowers_the_pitch_and_comes_to_a_stop() {
    let out = render(
        "instr a() { osc(sine, 1khz).slow_down(hold: (phasor(1hz) > 0.5) * (phasor(1hz) < 0.9), time: 100ms) }",
        1000,
    );
    assert!(finite(&out));
    let crossings = |range: std::ops::Range<usize>| out[range].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    let (before, early, late) = (crossings(20_000..22_400), crossings(24_000..26_400), crossings(27_000..29_400));
    assert!(before >= 45, "a 1 khz tone: {before}");
    assert!(early < before && late < early, "the pitch falls: {before} {early} {late}");
    assert!(rms(&out[30_000..32_000]) < 0.05, "it has stopped");
    assert!(rms(&out[40_000..46_000]) > 0.3, "the sound returns after the hold");
}

#[test]
fn bitcrush_keeps_silence_silent_and_full_scale_exact() {
    // a quantizer with no zero level would leave a constant offset on silence
    for bits in [2, 4, 8, 16] {
        let out = render(
            &format!("instr a() {{ (osc(sine, 100hz) * 0).bitcrush(rate: 12khz, bits: {bits}) }}"),
            200,
        );
        assert!(out.iter().all(|v| *v == 0.0), "{bits} bits: silence stays at zero");
    }
    // two bits are three levels: -1, 0 and 1
    let out = render("instr a() { osc(sine, 100hz).bitcrush(bits: 2) }", 200);
    assert!(out.iter().all(|v| [-1.0, 0.0, 1.0].contains(v)), "two bits give three levels");
    assert!(out.contains(&1.0) && out.contains(&-1.0), "full scale is reached");
    // eight bits: 255 levels, evenly spaced and including both ends
    let out = render("instr a() { osc(saw, 50hz).bitcrush(bits: 8) }", 2000);
    let mut levels: Vec<i32> = out.iter().map(|v| (v * 127.0).round() as i32).collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(levels.len() > 200 && levels[0] == -127 && levels[levels.len() - 1] == 127, "{} levels", levels.len());
    assert!(out.iter().all(|v| ((v * 127.0) - (v * 127.0).round()).abs() < 1e-3), "levels sit on the 1/127 grid");
}

#[test]
fn slow_down_starts_from_the_sound_as_it_is_now() {
    // a ramp that rises for two seconds: held at 0.5 s, the tape starts from where the ramp is (0.25), not from
    // the oldest sound in its buffer
    let out = render(
        "instr a() { phasor(0.5hz).slow_down(hold: (phasor(1hz) > 0.5) * (phasor(1hz) < 0.9), time: 200ms) }",
        1000,
    );
    assert!(finite(&out));
    let start = out[24_300];
    assert!((start - 0.25).abs() < 0.01, "the tape starts at the current level: {start}");
}

#[test]
fn shrink_makes_each_repeat_shorter() {
    // a ramp, held from 0.5 s with a 50 ms piece that shrinks to half each repeat, down to a floor of 4 ms: every
    // repeat starts silent (the seam), so the gaps between the silent samples are the lengths of the pieces after
    // the first
    let out = render(
        "instr a() { phasor(0.5hz).repeat_last(hold: (phasor(1hz) > 0.5) * (phasor(1hz) < 0.9), length: 50ms, shrink: 0.5) }",
        1000,
    );
    assert!(finite(&out));
    let silent: Vec<usize> = (24_000..43_000).filter(|&i| out[i].abs() < 1e-9).collect();
    let gaps: Vec<usize> = silent.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 100).collect();
    assert_eq!(gaps[..4], [1200, 600, 300, 192], "{gaps:?}");
}

#[test]
fn a_moving_length_is_read_once_per_repeat() {
    // a length that ramps while held still gives clean repeats: no jump bigger than a 220 hz sine can make
    let out = render(
        "instr a() { gate = (phasor(1hz) > 0.5) * (phasor(1hz) < 0.9)\n osc(sine, 220hz).repeat_last(hold: gate, length: 20ms + phasor(1hz) * 30ms) }",
        1000,
    );
    assert!(finite(&out));
    let jump = out[26_000..43_000].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(jump < 0.1, "a jump of {jump}");
}

#[test]
fn gate_silences_the_end_of_each_repeat_and_reverse_plays_it_backwards() {
    let held = "(phasor(1hz) > 0.5) * (phasor(1hz) < 0.9)";
    let render_with = |args: &str| {
        render(&format!("instr a() {{ phasor(0.5hz).repeat_last(hold: {held}, length: 50ms{args}) }}"), 1000)
    };
    let (open, half) = (render_with(""), render_with(", gate: 0.5"));
    let silent = |out: &[f32]| out[24_300..24_300 + 2400].iter().filter(|v| v.abs() < 1e-9).count();
    assert!(silent(&half) > silent(&open) + 800, "half of each repeat is silent: {} {}", silent(&half), silent(&open));
    let back = render_with(", reverse: 1");
    assert!(finite(&back));
    assert!(back[24_400 + 400] < back[24_400 + 100], "a rising ramp falls when reversed");
}

#[test]
fn shape_and_damp_change_each_repeat() {
    let held = "(phasor(1hz) > 0.5) * (phasor(1hz) < 0.9)";
    let render_with = |args: &str| {
        render(&format!("instr a() {{ osc(saw, 440hz).repeat_last(hold: {held}, length: 50ms{args}) }}"), 1000)
    };
    let window = |out: &[f32], from: usize| rms(&out[from..from + 400]);
    // the start of a 2400-sample repeat against its end
    let (plain, fading, swelling) = (render_with(""), render_with(", shape: 1"), render_with(", shape: -1"));
    let (head, tail) = (24_400, 24_000 + 2000);
    assert!(window(&fading, tail) < window(&fading, head) * 0.5, "a fade-out dies away");
    assert!(window(&swelling, head) < window(&swelling, tail) * 0.5, "a swell builds");
    assert!(window(&plain, tail) > window(&plain, head) * 0.8, "no shape stays level");
    // a damped saw loses its highs, so later repeats jump less from sample to sample
    let damped = render_with(", shrink: 1, damp: 0.6");
    let later = |out: &[f32]| out[24_000 + 2400 * 4..24_000 + 2400 * 4 + 1500].windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>();
    assert!(finite(&damped) && later(&damped) < later(&plain) * 0.8, "later repeats are darker: {} {}", later(&damped), later(&plain));
}

#[test]
fn attack_bend_and_accent_shape_the_repeats() {
    let held = "(phasor(1hz) > 0.5) * (phasor(1hz) < 0.9)";
    let render_with = |args: &str| {
        render(&format!("instr a() {{ osc(saw, 440hz).repeat_last(hold: {held}, length: 50ms{args}) }}"), 1000)
    };
    let window = |out: &[f32], from: usize| rms(&out[from..from + 300]);
    // a slow attack: the start of a repeat is quieter than its middle
    let slow = render_with(", attack: 0.5");
    assert!(window(&slow, 24_400) < window(&slow, 24_000 + 1000) * 0.6, "it rises");
    // a bent fade-out is quieter by the middle than a straight one
    let (straight, bent) = (render_with(", shape: 1"), render_with(", shape: 1, bend: 1"));
    assert!(window(&bent, 24_000 + 1200) < window(&straight, 24_000 + 1200) * 0.5, "a pluck");
    // accent: the second repeat is quieter than the first
    let accented = render_with(", accent: 0.8");
    assert!(finite(&accented));
    assert!(window(&accented, 24_000 + 2400 + 600) < window(&accented, 24_000 + 600) * 0.4, "the second is softer");
}

#[test]
fn a_hold_starts_and_ends_without_a_click() {
    // a 220 hz sine moves at most 0.03 a sample, so a jump of more than 0.1 anywhere is a click
    for call in [
        "repeat_last(hold: gate, length: 47ms)",
        "rewind_last(hold: gate, length: 50ms)",
        "slow_down(hold: gate, time: 100ms)",
    ] {
        let out = render(
            &format!("instr a() {{ gate = (phasor(1hz) > 0.5) * (phasor(1hz) < 0.7)\n osc(sine, 220hz).{call} }}"),
            1000,
        );
        assert!(finite(&out));
        let jump = out[20_000..40_000].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.1, "{call}: a jump of {jump}");
        assert!(rms(&out[40_000..46_000]) > 0.3, "{call}: the sound returns after the hold");
    }
}

#[test]
fn apply_runs_a_function_as_much_as_the_gate_says() {
    let level = |when: &str| {
        let out = render(
            &format!("instr a() {{ osc(sine, 220hz).apply(fn(s) {{ s * 0.5 }}, when: {when}) }}"),
            400,
        );
        assert!(finite(&out));
        rms(&out[out.len() / 2..]) / (0.5f32).sqrt()
    };
    assert!((level("0") - 1.0).abs() < 0.01, "off passes the sound");
    assert!((level("1") - 0.5).abs() < 0.01, "on runs the function");
    assert!((level("0.5") - 0.75).abs() < 0.01, "half way mixes the two");
}

#[test]
fn apply_fades_between_dry_and_wet_without_a_click() {
    let out = render(
        "instr a() { osc(sine, 220hz).apply(fn(s) { s * 0 }, when: (phasor(1hz) > 0.5) * (phasor(1hz) < 0.7)) }",
        1000,
    );
    let jump = out[20_000..40_000].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(jump < 0.1, "a jump of {jump}");
    assert!(rms(&out[25_000..30_000]) < 0.01, "the function is in force while gated");
}

/// `use std/fx` alone would add every effect to the piece, so it asks for names; naming some adds only those.
#[test]
fn std_fx_is_taken_by_name() {
    let err = expand("use std/fx\n", Path::new(".")).err().unwrap();
    assert!(err.message.contains("name the ones you want"), "{}", err.message);
    let err = expand("use std/fx.{nope}\n", Path::new(".")).err().unwrap();
    assert!(err.message.contains("no `nope`"), "{}", err.message);
    let expanded = expand("use std/fx.{echo, soft_clip}\n", Path::new(".")).unwrap();
    assert_eq!(expanded.text.matches("\nfx ").count(), 2);
    assert!(compile(&expanded.text).is_ok());
}

/// A file and a module it uses can each take their own effects from `std/fx`; the piece gets all of them, once.
#[test]
fn separate_uses_of_std_fx_add_up() {
    let dir = std::env::temp_dir().join(format!("niminal-use-fx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rig.nml"), "use std/fx.{echo}\n").unwrap();
    let expanded = expand("use rig\nuse std/fx.{soft_clip, echo}\n", &dir).unwrap();
    assert_eq!(expanded.text.matches("\nfx echo(").count(), 1);
    assert_eq!(expanded.text.matches("\nfx soft_clip(").count(), 1);
    assert_eq!(expanded.text.matches("\nfx lofi(").count(), 0);
    assert!(compile(&expanded.text).is_ok());
    std::fs::remove_dir_all(&dir).ok();
}

/// `use` for every preset on a shelf.
fn take_all(shelf: &str) -> String {
    let names: Vec<&str> = niminal_lang::presets::of_shelf(shelf).map(|p| p.id.as_str()).collect();
    format!("use {shelf}.{{{}}}\n", names.join(", "))
}

/// The catalog the tracker, the editor and the command line read: every preset has a folder, a description, its
/// parameters and a source that compiles on its own with the helpers it names; and the JSON file the tracker
/// imports is this catalog as it stands (`BLESS=1` rewrites it).
#[test]
fn the_preset_catalog_is_complete_and_its_json_is_current() {
    use niminal_lang::presets::{PresetKind, all, find, json};
    let presets = all();
    assert_eq!(presets.iter().filter(|p| p.kind == PresetKind::Instrument).count(), 29);
    assert!(presets.iter().filter(|p| p.kind == PresetKind::Effect).count() >= 15);
    for p in presets {
        assert!(!p.folder.is_empty() && !p.description.is_empty(), "{} needs a folder and a description", p.id);
        assert!(!p.params.is_empty(), "{} lists its parameters", p.id);
        let alone = format!("{}\n{}", p.helpers, p.source);
        compile(&alone).unwrap_or_else(|e| panic!("{} on its own: {}", p.id, e[0].message));
    }
    assert_eq!(find("kick_808").unwrap().folder, "drums/kicks");
    assert!(find("kick_808").unwrap().helpers.starts_with("/// A drum voice") || find("kick_808").unwrap().helpers.contains("fn drum("));
    assert!(find("echo").unwrap().helpers.is_empty());
    assert_eq!(find("echo").unwrap().name, "Echo");
    assert_eq!(find("band_cut").unwrap().name, "Band cut");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("std/presets.json");
    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&path, json()).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), json(), "std/presets.json is out of date; run with BLESS=1");
}
