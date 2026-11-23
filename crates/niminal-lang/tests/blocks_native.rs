//! Native modular DSP: edge cases and cost.
#![allow(clippy::unwrap_used)]
use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};
use std::path::Path;

fn render(prefix: &str, body: &str, rate: f32, stateful: bool) -> Vec<f32> {
    let body = if stateful {
        format!(
            "fn patch(m) {{ state dummy = 0\n dummy = dummy + 1\n {body} }}\ninstr a(m: num) {{ patch(m) }}"
        )
    } else {
        format!("instr a(m: num) {{ {body} }}")
    };
    let source = format!("{prefix}\n{body}");
    let expanded = expand(&source, Path::new(".")).unwrap();
    let p = compile(&expanded.text).unwrap_or_else(|e| {
        panic!(
            "{}",
            e.iter()
                .map(|e| e.render("native", &expanded.text))
                .collect::<String>()
        )
    });
    let mut v = Voice::new(p.instruments[0].graph.clone(), rate);
    let mut out = vec![0.0; block_size() * 600];
    for (i, chunk) in out.chunks_mut(block_size()).enumerate() {
        v.set_param_by_name("m", if (200..400).contains(&i) { 1.0 } else { 0.0 });
        v.process(chunk);
    }
    out
}

#[test]
fn zero_time_linear_slew_follows_large_values_immediately() {
    let out = render(
        "use std/sources\nuse std/modulation\nuse std/gates\nuse std/random\nuse std/sequencing\nuse std/filters\nuse std/shaping\nuse std/granular\nuse std/delays\nuse std/physical\nuse std/spectral",
        "slew(100, rise: 0sec, shape: linear)",
        48000.0,
        false,
    );
    assert!(out.iter().all(|&v| v == 100.0));
}

#[test]
fn packed_arrays_validate_lengths_and_units_in_both_compilers() {
    for body in [
        "tap_delay_dsp(0, 10ms, taps: 2, shifts: [0], levels: [1, 1], cutoffs: [1khz, 2khz])",
        "tap_delay_dsp(0, 10ms, taps: 2, shifts: [0, 0], levels: [1, 1], cutoffs: [1, 2])",
    ] {
        for stateful in [false, true] {
            let source = if stateful {
                format!("fn f() {{ state s = 0\n {body} }}\ninstr a() {{ f() }}")
            } else {
                format!("instr a() {{ {body} }}")
            };
            assert!(compile(&source).is_err(), "{source}");
        }
    }
}

/// splice_reel must be cheap enough to play live: its worst case (six overlapping, pitched, tiny grains at an odd speed)
/// renders in under a tenth of real time. The .nml kernel it replaced took 25 to 85% of real time; the native one about 2%.
#[test]
fn splice_reel_worst_case_runs_well_within_real_time() {
    let source = "use std/sources\nuse std/modulation\nuse std/gates\nuse std/random\nuse std/sequencing\nuse std/filters\nuse std/shaping\nuse std/granular\nuse std/delays\nuse std/physical\nuse std/spectral\nconfig { channels: stereo }\ninstr a() { m = splice_reel(osc(saw, 110hz) * 0.4, record: if note.time < 1sec { 1 } else { 0 }, grain_size: 0.7, morph: 0.95, speed: 1.37, slide: osc(sine, 0.3hz) * 0.5 + 0.5)\n [m.left, m.right] }";
    let expanded = expand(source, Path::new(".")).unwrap();
    let p = compile(&expanded.text).unwrap_or_else(|_| panic!("compiles"));
    let mut v = Voice::new(p.instruments[0].graph.clone(), 48_000.0);
    let mut buses = niminal_engine::Buses::new(&[]);
    let mut block = vec![vec![0.0; block_size()]; 2];
    // the first second records; time ten seconds of playback after it
    for _ in 0..48_000 / block_size() {
        v.process_blocks(&mut block, block_size(), &mut buses);
    }
    let blocks = 10 * 48_000 / block_size();
    let started = std::time::Instant::now();
    let mut loudest = 0.0f32;
    for _ in 0..blocks {
        v.process_blocks(&mut block, block_size(), &mut buses);
        loudest = block.iter().flatten().fold(loudest, |m, x| m.max(x.abs()));
    }
    let share = started.elapsed().as_secs_f64() / 10.0;
    assert!(loudest > 0.05, "{loudest}");
    eprintln!("splice_reel worst case: {:.1}% of real time", share * 100.0);
    assert!(share < 0.1, "splice_reel took {:.1}% of real time", share * 100.0);
}
