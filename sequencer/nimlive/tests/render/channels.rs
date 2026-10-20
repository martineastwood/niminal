use crate::common::*;

/// Render with the limiter off and return every channel.
fn raw(src: &str) -> Vec<Vec<f32>> {
    render_with(&program(src), &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap().channels
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

fn crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

const TONE: &str = "osc(sine, 220hz) * env[1 | 1ms 0]";

#[test]
fn a_mono_instrument_is_centred_in_a_stereo_master() {
    let out = raw(&format!("config {{ channels: stereo }}\ninstr a() {{ {TONE} }}\na() for 1beat"));
    assert_eq!(out.len(), 2);
    assert_eq!(out[0], out[1]);
    assert!((peak(&out[0]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "-3db each side: {}", peak(&out[0]));
}

#[test]
fn pan_places_the_sound_between_the_speakers() {
    let at = |az: &str| {
        raw(&format!(
            "config {{ channels: stereo }}\ninstr a() {{ ({TONE}).pan(azimuth: {az}) }}\na() for 1beat"
        ))
    };
    let (left, right) = (at("-30deg"), at("30deg"));
    assert!(peak(&left[0]) > 0.99 && peak(&left[1]) < 1e-6, "hard left");
    assert!(peak(&right[1]) > 0.99 && peak(&right[0]) < 1e-6, "hard right");

    let centre = at("0deg");
    assert_eq!(centre[0], centre[1]);
    let power = |c: &[Vec<f32>], i: usize| c[0][i] * c[0][i] + c[1][i] * c[1][i];
    // constant power across the pan
    let mid = at("12deg");
    let i = 5000;
    assert!((power(&mid, i) - power(&centre, i)).abs() < 1e-4);
    assert!(peak(&mid[1]) > peak(&mid[0]), "12 degrees is right of centre");
}

/// A stereo render of a pattern played on a tone.
fn played(pattern: &str) -> Vec<Vec<f32>> {
    let src = format!(
        "tempo 120bpm\nconfig {{ channels: stereo }}\ninstr tone(freq: hz) {{ osc(sine, freq) * env[1 | 1ms 0] }}\ntrack lead {{ instrument = tone }}\nlead.play({pattern})"
    );
    render_with(&program(&src), &[], &RenderOptions { limiter: false, until: Some(1.9), ..Default::default() }).unwrap().channels
}

#[test]
fn a_note_can_have_a_pan_of_its_own() {
    let first = 2000;
    let left = played("[c4].pan(-30deg)");
    assert!(peak(&left[0][..first]) > 0.9 && peak(&left[1][..first]) < 1e-6, "left");
    let right = played("[c4].pan(30deg)");
    assert!(peak(&right[1][..first]) > 0.9 && peak(&right[0][..first]) < 1e-6, "right");
    // no pan of its own: centred as ever
    let centred = played("[c4]");
    assert_eq!(centred[0][..first], centred[1][..first]);
    // the lane form works the same way
    let lane = played("[c4].set(pan: 30deg)");
    assert!(peak(&lane[1][..first]) > 0.9 && peak(&lane[0][..first]) < 1e-6, "right from a lane");
    // each note its own: the first step left, the second right
    let each = played("[c4 c4].pan([-30deg 30deg])");
    let second = (1.0 * SR) as usize + 500; // past the first note's release
    assert!(peak(&each[0][..first]) > 0.9 && peak(&each[1][..first]) < 1e-6);
    assert!(peak(&each[1][second..second + first]) > 0.9 && peak(&each[0][second..second + first]) < 1e-6);
}

/// A stereo render of a track with these extra lines, playing one middle C.
fn mixed(lines: &str) -> Vec<Vec<f32>> {
    let src = format!(
        "tempo 120bpm\nconfig {{ channels: stereo }}\ninstr tone(freq: hz) {{ osc(sine, freq) * env[1 | 1ms 0] }}\ntrack lead {{\n  instrument = tone\n  {lines}\n}}\nlead.play([c4])"
    );
    render_with(&program(&src), &[], &RenderOptions { limiter: false, until: Some(1.9), ..Default::default() }).unwrap().channels
}

#[test]
fn a_tracks_mixer_lines_set_its_level_and_place() {
    let plain = mixed("");
    let quiet = mixed("gain = -6db");
    let ratio = peak(&quiet[0][..2000]) / peak(&plain[0][..2000]);
    assert!((ratio - 0.5012).abs() < 0.01, "-6db: {ratio}");
    let left = mixed("pan = -30deg");
    assert!(peak(&left[0][..2000]) > 0.99 && peak(&left[1][..2000]) < 1e-6, "hard left, and as loud as the centred mono voice would be per channel");
    let centre = mixed("pan = 0deg");
    assert!((peak(&centre[0][..2000]) - peak(&plain[0][..2000])).abs() < 0.01, "a centred pan changes nothing");
    let both = mixed("gain = -6db\n  pan = 30deg");
    assert!(peak(&both[1][..2000]) > 0.49 && peak(&both[0][..2000]) < 1e-6);
}

#[test]
fn left_right_plays_the_original_and_its_change_in_different_speakers() {
    // left plays low then high; right plays the reverse: high then low
    let out = played("[c3 c5].left_right(it.reverse)");
    let (l, r) = (&out[0], &out[1]);
    let (first, second) = (0..(0.5 * SR) as usize, (1.0 * SR) as usize..(1.5 * SR) as usize);
    assert!(crossings(&l[first.clone()]) < crossings(&l[second.clone()]), "left rises");
    assert!(crossings(&r[first]) > crossings(&r[second]), "right falls");
}

#[test]
fn surround_pan_uses_the_right_speakers() {
    let at = |az: &str| {
        raw(&format!(
            "config {{ channels: surround(5.1) }}\ninstr a() {{ ({TONE}).pan(azimuth: {az}) }}\na() for 1beat"
        ))
    };
    let ahead = at("0deg");
    assert_eq!(ahead.len(), 6);
    let loud: Vec<usize> = (0..6).filter(|&c| peak(&ahead[c]) > 0.01).collect();
    assert_eq!(loud, [2], "straight ahead is the centre speaker only");

    let behind = at("180deg");
    assert!(peak(&behind[4]) > 0.5 && peak(&behind[5]) > 0.5);
    assert!(peak(&behind[0]) < 1e-6 && peak(&behind[2]) < 1e-6);
    assert_eq!(peak(&behind[3]), 0.0, "the LFE is never panned to");
}

#[test]
fn a_channel_list_puts_each_entry_on_its_own_channel() {
    let out = raw("
config { channels: stereo }
instr a() { [osc(sine, 220hz), osc(sine, 330hz)] * env[1 | 1ms 0] }
a() for 2beats");
    let (l, r) = (&out[0], &out[1]);
    assert!((crossings(l) as f32 - 220.0).abs() <= 2.0, "left is 220hz: {}", crossings(l));
    assert!((crossings(r) as f32 - 330.0).abs() <= 2.0, "right is 330hz: {}", crossings(r));
}

#[test]
fn destructuring_and_rebuilding_can_swap_channels() {
    let plain = raw("
config { channels: stereo }
instr a() { [osc(sine, 220hz), osc(sine, 330hz)] * env[1 | 1ms 0] }
a() for 1beat");
    let swapped = raw("
config { channels: stereo }
instr a() {
  [l, r] = [osc(sine, 220hz), osc(sine, 330hz)] * env[1 | 1ms 0]
  [r, l]
}
a() for 1beat");
    assert_eq!(plain[0], swapped[1]);
    assert_eq!(plain[1], swapped[0]);
}

#[test]
fn a_per_channel_argument_gives_each_channel_its_own_value() {
    let out = raw("
config { channels: stereo }
instr a() { [osc(saw, 110hz), osc(saw, 110hz)].lpf(cutoff: [300hz, 4khz]) * env[1 | 1ms 0] }
a() for 1beat");
    // the brighter filter keeps the saw's sharp edges: more curvature
    let roughness = |c: &[f32]| {
        let d2: Vec<f32> = c.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
        d2.iter().map(|x| x.abs()).sum::<f32>() / peak(c)
    };
    assert!(roughness(&out[1]) > roughness(&out[0]) * 2.0, "{} vs {}", roughness(&out[1]), roughness(&out[0]));
}

#[test]
fn filters_on_stereo_keep_separate_state_per_channel() {
    // the same filter on a stereo pair, fed silence on the right: nothing leaks across
    let out = raw("
config { channels: stereo }
instr a() { [osc(saw, 110hz), 0 * osc(saw, 110hz)].lpf(cutoff: 500hz) * env[1 | 1ms 0] }
a() for 1beat");
    assert!(peak(&out[0]) > 0.1);
    assert_eq!(peak(&out[1]), 0.0);
}

#[test]
fn surround_folds_down_to_stereo_without_the_lfe() {
    // a bus in 5.1 receives mono (centre) and is read by a stereo-master track
    let out = raw("
config { channels: stereo }
bus hall: surround(5.1)
instr a() { hall += osc(sine, 220hz) * env[1 | 1ms 0] }
track t { out = hall }
a() for 1beat");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0], out[1]);
    assert!((peak(&out[0]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "centre folds in at -3db");

    let lfe_only = raw("
config { channels: stereo }
bus hall: surround(5.1)
instr a() { hall += [0, 0, 0, osc(sine, 80hz), 0, 0] }
track t { out = hall }
a() for 1beat");
    assert!(lfe_only.iter().all(|c| peak(c) < 1e-6), "the LFE doesn't reach stereo");
}

#[test]
fn routing_to_a_mono_bus_then_back_up() {
    let out = raw("
config { channels: stereo }
bus m: mono
instr a() { osc(sine, 220hz).pan(azimuth: -30deg) * env[1 | 1ms 0] }
track t { instrument = a\nout = it.to(m) }
track back { out = m }
t() for 1beat");
    // hard-left stereo averaged to mono (half), then centred in stereo at -3db
    assert_eq!(out[0], out[1]);
    let expected = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((peak(&out[0]) - expected).abs() < 0.01, "{} vs {expected}", peak(&out[0]));
}

#[test]
fn a_stereo_reverb_decorrelates_the_channels() {
    let out = raw("
config { channels: stereo }
bus space: stereo
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
    assert!(out[0].len() > SR as usize);
    assert_ne!(out[0], out[1]);
    let diff: f32 = out[0].iter().zip(&out[1]).map(|(a, b)| (a - b).abs()).sum();
    assert!(diff > 0.01);
}

#[test]
fn notes_start_on_the_same_sample_on_every_channel() {
    let out = raw("
config { channels: surround(5.1) }
instr click() { osc(sine, 1000hz).pan(azimuth: 70deg) * env[1 0.5ms 0] }
at 250ms click() for 1beat");
    let first = |c: &[f32]| c.iter().position(|s| s.abs() > 1e-6);
    let starts: Vec<_> = out.iter().map(|c| first(c)).collect();
    // the sine rises from zero, so the first non-zero sample is one after the note start
    let on = starts.iter().flatten().copied().min().unwrap();
    assert!((on as f64 - 0.25 * SR).abs() <= 1.0);
    for s in starts.iter().flatten() {
        assert_eq!(*s, on);
    }
}

#[test]
fn the_limiter_ducks_all_channels_together() {
    let src = "
config { channels: stereo }
instr a() { [osc(sine, 220hz) * 4, osc(sine, 330hz) * 0.2] * env[1 | 1ms 0] }
a() for 1beat";
    let quiet_right = |limiter| {
        let out = render_with(&program(src), &[], &RenderOptions { limiter, until: None, ..Default::default() }).unwrap().channels;
        let mid = out[1].len() / 2;
        peak(&out[1][mid..mid + 4800])
    };
    let (on, off) = (quiet_right(true), quiet_right(false));
    assert!(on < off * 0.5, "the quiet side is ducked with the loud one: {on} vs {off}");
}

#[test]
fn wav_files_interleave_the_channels() {
    let p = program("config { channels: stereo }\ninstr a() { osc(sine, 220hz).pan(azimuth: 30deg) * env[1 | 1ms 0] }\na() for 1/4beat");
    let out = render_with(&p, &[], &RenderOptions { limiter: false, until: None, ..Default::default() }).unwrap();
    let dir = std::env::temp_dir().join(format!("niminal-wav-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stereo.wav");
    nimlive::render::write_wav(&path, &out.channels).unwrap();

    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().channels, 2);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples.len(), out.len() * 2);
    for i in [0, 100, 1000] {
        assert_eq!(samples[2 * i], out.channels[0][i]);
        assert_eq!(samples[2 * i + 1], out.channels[1][i]);
    }
    std::fs::remove_dir_all(&dir).ok();
}
