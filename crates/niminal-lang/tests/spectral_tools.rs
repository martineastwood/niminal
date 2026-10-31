//! End-to-end frequency, selection, capture and latency contracts for spectral tools.
use niminal_engine::Voice;
use niminal_lang::compile;

fn render(body: &str, chunk: usize) -> Vec<f32> {
    let source = format!("instr a() {{ {body} }}");
    let program = compile(&source).unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    let mut voice = Voice::new(program.instruments[0].graph.clone(), 48000.0);
    let mut out = vec![0.0; 24000];
    for block in out.chunks_mut(chunk) {
        voice.process(block);
    }
    assert!(out.iter().all(|x| x.is_finite()));
    out
}
fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}
fn frequency(x: &[f32]) -> f64 {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 * 48000.0 / x.len() as f64
}
fn amplitude(x: &[f32], hz: f64) -> f64 {
    let (re, im) = x.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| {
        let angle = std::f64::consts::TAU * hz * i as f64 / 48000.0;
        (re + v as f64 * angle.cos(), im + v as f64 * angle.sin())
    });
    2.0 * re.hypot(im) / x.len() as f64
}

#[test]
fn neutral_edits_and_both_morph_endpoints_cancel_the_aligned_source() {
    for expression in [
        "spectral_thin(x, count: 257, size: 512)",
        "spectral_arp(x, depth: 0, gain: 1, size: 512)",
        "spectral_hold(x, size: 512)",
        "spectral_shift(x, shift: 0hz, size: 512)",
        "spectral_morph(x, noise(seed: 12), magnitude: 0, frequency: 0, size: 512)",
        "spectral_morph(noise(seed: 12), x, magnitude: 1, frequency: 1, size: 512)",
    ] {
        let out = render(&format!("x=noise(seed: 9)*0.2\n {expression}-x"), 64);
        assert!(
            rms(&out[2048..]) < 1e-5,
            "{expression}: {}",
            rms(&out[2048..])
        );
    }
}
#[test]
fn thinning_retains_the_strongest_bins_and_zero_count_is_silent() {
    let out = render(
        "x=osc(sine, 750hz)*0.4+osc(sine, 3000hz)*0.1\n spectral_thin(x,count: 3,size: 1024)",
        64,
    );
    assert!(amplitude(&out[8192..], 750.0) > 0.35);
    assert!(amplitude(&out[8192..], 3000.0) < 0.005);
    let silent = render("spectral_thin(noise(),count: 0,size: 512)", 64);
    assert!(silent.iter().all(|x| *x == 0.0));
}
#[test]
fn arpeggiator_selects_the_requested_part_of_the_spectrum() {
    let out = render(
        "x=osc(sine, 750hz)*0.2+osc(sine, 3000hz)*0.2\n spectral_arp(x,position: 0.125,width: 2,size: 1024)",
        64,
    );
    assert!(amplitude(&out[8192..], 3000.0) > 0.1);
    assert!(amplitude(&out[8192..], 750.0) < 0.005);
}
#[test]
fn hz_shift_adds_frequency_and_respects_the_lowest_frequency() {
    for (shift, lowest, expected) in [(173, 0, 1170.0), (-173, 0, 824.0), (173, 2000, 997.0)] {
        let out = render(
            &format!(
                "spectral_shift(osc(sine,997hz)*0.2,shift: {shift}hz,lowest: {lowest}hz,size: 1024)"
            ),
            64,
        );
        assert!(
            (frequency(&out[8192..]) - expected).abs() < 6.0,
            "shift {shift}, cutoff {lowest}: {}",
            frequency(&out[8192..])
        );
        assert!(rms(&out[8192..]) > 0.07);
    }
}
#[test]
fn magnitude_hold_retains_level_and_frequency_hold_retains_tuning() {
    let mag = render(
        "x=osc(sine,997hz)*env[0.2 200ms 0.2 20ms 0.02]\n spectral_hold(x,magnitude: env[0 150ms 0 1ms 1],size: 1024)",
        64,
    );
    assert!(rms(&mag[16000..]) > 0.1, "{}", rms(&mag[16000..]));
    let freq = render(
        "x=osc(sine,env[997hz 200ms 997hz 100ms 1027hz])*0.2\n spectral_hold(x,frequency: env[0 150ms 0 1ms 1],size: 1024)",
        64,
    );
    assert!(
        (frequency(&freq[16000..]) - 997.0).abs() < 8.0,
        "{}",
        frequency(&freq[16000..])
    );
}
#[test]
fn morph_has_independent_magnitude_and_frequency_controls() {
    let out = render(
        "spectral_morph(osc(sine,997hz)*0.1,osc(sine,1037hz)*0.3,magnitude: 1,frequency: 0.5,size: 1024)",
        64,
    );
    assert!(rms(&out[8192..]) > 0.08);
    assert!(
        (frequency(&out[8192..]) - 1017.0).abs() < 10.0,
        "{}",
        frequency(&out[8192..])
    );
}
#[test]
fn partial_resynthesis_retains_off_bin_tuning_level_and_freezes_after_silence() {
    let out = render(
        "x=osc(sine,997hz)*0.2*env[1 180ms 1 20ms 0]\n spectral_partials(x,count: 1,freeze: env[0 150ms 0 1ms 1],size: 1024)",
        64,
    );
    let tail = &out[16000..];
    assert!((frequency(tail) - 997.0).abs() < 8.0, "{}", frequency(tail));
    assert!(
        (rms(tail) - 0.2 / 2.0_f64.sqrt()).abs() < 0.02,
        "{}",
        rms(tail)
    );
    let shifted = render(
        "spectral_partials(osc(sine,997hz)*0.2,count: 1,shift: 12,size: 1024)",
        64,
    );
    assert!((frequency(&shifted[8192..]) - 1994.0).abs() < 8.0);
}
#[test]
fn every_new_processor_is_independent_of_host_block_boundaries() {
    for expression in [
        "spectral_thin(x,count: 7,size: 512)",
        "spectral_arp(x,position: phasor(2hz)*0.2,width: 3,size: 512)",
        "spectral_hold(x,magnitude: phasor(5hz)>0.5,frequency: phasor(3hz)>0.5,size: 512)",
        "spectral_shift(x,shift: 173hz,size: 512)",
        "spectral_morph(x,osc(saw,330hz)*0.1,magnitude: phasor(2hz),size: 512)",
        "spectral_partials(x,freeze: phasor(5hz)>0.5,count: 8,size: 512)",
    ] {
        let body = format!("x=osc(saw,220hz)*0.1\n {expression}");
        assert_eq!(render(&body, 64), render(&body, 37), "{expression}");
    }
}
#[test]
fn invalid_sizes_and_tracking_capacities_are_rejected() {
    for args in [
        "max_tracks: 0",
        "max_tracks: 1.5",
        "max_tracks: 257",
        "size: 100",
        "overlap: 3",
    ] {
        assert!(
            compile(&format!(
                "instr a() {{ spectral_partials(noise(),{args}) }}"
            ))
            .is_err(),
            "{args}"
        );
    }
}

#[test]
fn partial_morph_moves_a_tone_across_an_octave_without_overlapping_fft_bins() {
    for (blend, expected) in [(0.0, 997.0), (0.5, 997.0 * 2.0_f64.sqrt()), (1.0, 1994.0)] {
        let out = render(
            &format!(
                "partial_morph(osc(sine,997hz)*0.2,osc(sine,1994hz)*0.1,magnitude: 0.5,frequency: {blend},count: 1,size: 1024)"
            ),
            64,
        );
        let actual = frequency(&out[8192..]);
        assert!(
            (actual - expected).abs() < 8.0,
            "blend {blend}: {actual}, expected {expected}"
        );
        assert!((rms(&out[8192..]) - 0.15 / 2.0_f64.sqrt()).abs() < 0.02);
    }
}
#[test]
fn partial_morph_preserves_unmatched_sources_and_handles_silence() {
    for (a, b, blend, expected) in [
        ("osc(sine,997hz)*0.2", "0", 0, 997.0),
        ("0", "osc(sine,1994hz)*0.2", 1, 1994.0),
    ] {
        let out = render(
            &format!("partial_morph({a},{b},magnitude: {blend},count: 1,size: 1024)"),
            64,
        );
        assert!((frequency(&out[8192..]) - expected).abs() < 8.0);
        assert!(rms(&out[8192..]) > 0.1);
    }
    let out = render("partial_morph(0,0,size: 512)", 64);
    assert!(out.iter().all(|x| *x == 0.0));
}
#[test]
fn partial_morph_freezes_both_banks_but_keeps_blends_live() {
    let out = render(
        "gate=env[1 180ms 1 20ms 0]\n left=osc(sine,997hz)*0.2*gate\n right=osc(sine,1994hz)*0.1*gate\n partial_morph(left,right,freeze: env[0 150ms 0 1ms 1],frequency: env[0 250ms 0 100ms 1],magnitude: 0.5,count: 1,size: 1024)",
        64,
    );
    assert!((frequency(&out[18000..]) - 1994.0).abs() < 10.0);
    assert!(rms(&out[18000..]) > 0.08);
}
#[test]
fn partial_morph_is_chunk_independent_and_validates_storage_settings() {
    let body = "left=osc(saw,220hz)*0.1\n right=osc(tri,330hz)*0.1\n partial_morph(left,right,magnitude: phasor(2hz),frequency: phasor(3hz),freeze: phasor(8hz)>0.5,count: 8,size: 512)";
    assert_eq!(render(body, 64), render(body, 37));
    for args in [
        "max_tracks: 0",
        "max_tracks: 1.5",
        "max_tracks: 257",
        "size: 100",
        "overlap: 3",
    ] {
        assert!(
            compile(&format!(
                "instr a() {{ partial_morph(noise(),noise(),{args}) }}"
            ))
            .is_err(),
            "{args}"
        );
    }
}

#[test]
fn partial_morph_matches_a_harmonic_stack_despite_reversed_loudness_order() {
    let out = render(
        "left=osc(sine,997hz)*0.2+osc(sine,1994hz)*0.1+osc(sine,2991hz)*0.05\n right=osc(sine,1495.5hz)*0.1+osc(sine,2991hz)*0.2+osc(sine,4486.5hz)*0.1\n partial_morph(left,right,magnitude: 0.5,frequency: 0.5,count: 3,size: 2048)",
        64,
    );
    let root = 997.0 * 1.5_f64.sqrt();
    for (harmonic, minimum) in [(1.0, 0.1), (2.0, 0.1), (3.0, 0.04)] {
        let actual = amplitude(&out[8192..], root * harmonic);
        assert!(actual > minimum, "harmonic {harmonic}: amplitude {actual}");
    }
}

#[test]
fn partial_morph_can_take_one_sources_pitch_and_the_others_strength() {
    for (magnitude, blend, expected, level) in [(0, 1, 1994.0, 0.2), (1, 0, 997.0, 0.1)] {
        let out = render(
            &format!(
                "partial_morph(osc(sine,997hz)*0.2,osc(sine,1994hz)*0.1,magnitude: {magnitude},frequency: {blend},count: 1,size: 1024)"
            ),
            64,
        );
        assert!((frequency(&out[8192..]) - expected).abs() < 8.0);
        assert!((rms(&out[8192..]) - level / 2.0_f64.sqrt()).abs() < 0.02);
    }
}
