use crate::common::*;

const SONG: &str = "
tempo 120bpm
bus space: mono {
  out = it.gain(0.5)
}
instr tone(freq: hz) { out = osc(sine, freq) * env[1 | 5ms 0]
  space += out * 0.5 }
clip bass {
  instrument: tone
  notes: [c2 ~]
}
clip lead {
  instrument: tone
  notes: [~ c4]
}
scene verse { bass, lead }
verse.start
at +1bars hush
";

fn stems_of(src: &str) -> (Vec<f32>, Vec<niminal_cli::render::Stem>) {
    let out = render_with(&program(src), &[], &RenderOptions { limiter: false, stems: true, ..Default::default() }).unwrap();
    let mix = out.channels.into_iter().next().unwrap();
    (mix, out.stems)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

#[test]
fn every_track_and_bus_has_its_own_stem_as_long_as_the_mix() {
    let (mix, stems) = stems_of(SONG);
    let mut names: Vec<_> = stems.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["bass", "lead", "space", "space.fx"], "the track nobody plays directly is left out");
    assert!(stems.iter().all(|s| s.channels[0].len() == mix.len()), "same length as the mix");
    let stem = |name: &str| &stems.iter().find(|s| s.name == name).unwrap().channels[0];
    // bass plays in the first half of the bar, lead in the second
    let half = SR as usize;
    assert!(peak(&stem("bass")[..half]) > 0.5 && peak(&stem("bass")[half + 4800..]) < 1e-3);
    assert!(peak(&stem("lead")[..half]) < 1e-3 && peak(&stem("lead")[half..]) > 0.5);
    assert!(peak(stem("space")) > 0.3, "the bus carries what was sent to it");
    assert!(peak(stem("space.fx")) > 0.1, "and its processing has its own stem");
}

#[test]
fn the_stems_of_the_tracks_add_up_to_the_mix() {
    let (mix, stems) = stems_of(SONG);
    for (i, m) in mix.iter().enumerate() {
        let sum: f32 = stems.iter().filter(|s| s.name != "space").map(|s| s.channels[0][i]).sum();
        assert!((m - sum).abs() < 1e-5, "sample {i}: mix {m} vs stems {sum}");
    }
}

#[test]
fn no_stems_unless_asked_for() {
    let out = render_with(&program(SONG), &[], &RenderOptions { limiter: false, ..Default::default() }).unwrap();
    assert!(out.stems.is_empty());
}
