//! Native modular DSP stays compatible with the frozen shelf, including inside fused kernels.
#![allow(clippy::unwrap_used)]
use niminal_engine::{Voice, block_size};
use niminal_lang::{compile, expand};
use std::path::Path;
const REFERENCE: &str = include_str!("fixtures/blocks_dsp_reference.nml");

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
fn native_blocks_match_the_reference_in_graphs_and_kernels() {
    let cases = [
        (
            "contour",
            "contour(trigger: impulse(5hz), rise: 10ms, fall: 70ms, curve: 1.3, cycle: m).value",
        ),
        (
            "contour pulses",
            "c = contour(trigger: impulse(5hz), rise: 10ms, fall: 70ms, cycle: m)\n c.eor + c.eoc",
        ),
        ("slew", "slew(osc(square, 5hz), rise: 10ms, fall: 80ms)"),
        (
            "linear slew",
            "slew(osc(square, 5hz), rise: 10ms, fall: 80ms, shape: linear)",
        ),
        (
            "filter",
            "f = osc(saw, 220hz).multimode_filter(cutoff: 1500hz + osc(sine, 3hz) * 1khz, res: 0.7, drive: 0.2)\n f.low + f.band * 0.3 + f.high * 0.2 + f.notch * 0.1",
        ),
        (
            "stereo filter",
            "f = [osc(saw, 220hz), osc(saw, 330hz)].multimode_filter(cutoff: 2khz, res: 0.7)\n f.low.sum + f.band.sum * 0.3",
        ),
        (
            "frequency shifter",
            "f = osc(saw, 220hz).freq_shifter(shift: 123hz)\n f.up + f.down * 0.3",
        ),
        (
            "tape",
            "osc(saw, 220hz).freeze_reverse(length: 100ms, reverse: 1 - m, freeze: m)",
        ),
        (
            "tap delay",
            "t = osc(saw, 220hz).tap_delay(time: 30ms, taps: 4, shifts: [0, 7, -12], levels: [0.5, 1], cutoffs: [2khz, 1khz], step: 0.3, filter: lowpass, groove: swing, wobble: 0.2, feedback: 0.5, feedback_shift: 3, feedback_tone: -0.2, freeze: m)\n t.left + t.right * 0.3",
        ),
    ];
    for rate in [44100.0, 48000.0, 96000.0] {
        for stateful in [false, true] {
            for (name, body) in cases {
                let old = render(REFERENCE, body, rate, stateful);
                let new = render("use std/blocks", body, rate, stateful);
                assert!(
                    new.iter().all(|v| v.is_finite()),
                    "{name} {rate} {stateful}"
                );
                let rms = (old
                    .iter()
                    .zip(&new)
                    .map(|(a, b)| f64::from(a - b).powi(2))
                    .sum::<f64>()
                    / old.len() as f64)
                    .sqrt();
                println!("{name} {rate} kernel={stateful}: rms difference {rms}");
                assert!(
                    rms < 0.0001,
                    "{name} {rate} kernel={stateful}: rms difference {rms}"
                );
            }
        }
    }
}

#[test]
fn zero_time_linear_slew_follows_large_values_immediately() {
    let out = render(
        "use std/blocks",
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

/// Sets a voice's parameters before each block.
type Setter<'a> = &'a dyn Fn(usize, &mut Voice);

/// The native splice_reel against the .nml kernel it replaced, over the things it does: recording and looping, grains and
/// overlap, speed and direction, the clock, markers and choosing, overdubbing, retriggers.
#[test]
fn native_splice_reel_matches_the_kernel_it_replaced() {
    let reference = include_str!("fixtures/splice_reel_reference.nml");
    let render = |call: &str, set: &dyn Fn(usize, &mut Voice)| -> Vec<f32> {
        let source = format!(
            "use std/blocks\nconfig {{ channels: stereo }}\n{reference}\ninstr a(r: num, nw: num, ch: num, mk: num, um: num, pl: num = 1, sp: num = 1, gs: num, sl: num, mo: num = 0.33, ck: num, so: num) {{ m = {call}(osc(saw, 110hz) * 0.4 + osc(sine, 3hz) * 0.1, record: r, new_splice: nw, choose: ch, mark: mk, unmark: um, play: pl, speed: sp, grain_size: gs, slide: sl, morph: mo, clock: ck, sos: so, size: 96000, splices: 8)\n [m.left + m.grain_end * 0.001 + m.splice * 0.01 + m.count * 0.1, m.right + m.level * 0.01] }}"
        );
        let expanded = expand(&source, Path::new(".")).unwrap();
        let p = compile(&expanded.text).unwrap_or_else(|e| {
            panic!("{}", e.iter().map(|e| e.render("native", &expanded.text)).collect::<String>())
        });
        let mut v = Voice::new(p.instruments[0].graph.clone(), 48_000.0);
        let mut buses = niminal_engine::Buses::new(&[]);
        let mut block = vec![vec![0.0; block_size()]; 2];
        let mut out = Vec::new();
        for n in 0..1500 {
            set(n, &mut v);
            v.process_blocks(&mut block, block_size(), &mut buses);
            out.extend_from_slice(&block[0]);
            out.extend_from_slice(&block[1]);
        }
        out
    };
    let scenarios: [(&str, Setter); 6] = [
        ("loop", &|n, v| {
            v.set_param_by_name("r", if n < 100 { 1.0 } else { 0.0 });
        }),
        ("grains, overlap and speed", &|n, v| {
            v.set_param_by_name("r", if n < 100 { 1.0 } else { 0.0 });
            v.set_param_by_name("gs", 0.5);
            v.set_param_by_name("mo", if n < 700 { 0.6 } else { 0.95 });
            v.set_param_by_name("sl", (n as f32 / 1500.0).fract());
            v.set_param_by_name("sp", if n < 500 { 1.37 } else if n < 1000 { -0.7 } else { 2.2 });
        }),
        ("clock step and stretch", &|n, v| {
            v.set_param_by_name("r", if n < 100 { 1.0 } else { 0.0 });
            v.set_param_by_name("gs", 0.4);
            v.set_param_by_name("mo", if n < 800 { 0.3 } else { 0.7 });
            v.set_param_by_name("ck", if (n / 15) % 2 == 0 { 1.0 } else { 0.0 });
        }),
        ("markers, choosing, new splices", &|n, v| {
            v.set_param_by_name("r", if n < 100 || (300..360).contains(&n) { 1.0 } else { 0.0 });
            v.set_param_by_name("nw", if n >= 200 { 1.0 } else { 0.0 });
            v.set_param_by_name("mk", if (150..152).contains(&n) || (450..452).contains(&n) { 1.0 } else { 0.0 });
            v.set_param_by_name("um", if (900..902).contains(&n) { 1.0 } else { 0.0 });
            v.set_param_by_name("ch", if n < 500 { 0.1 } else { (n as f32 / 200.0).fract() });
        }),
        ("overdub", &|n, v| {
            v.set_param_by_name("r", if n < 100 || (400..700).contains(&n) { 1.0 } else { 0.0 });
            v.set_param_by_name("so", 0.6);
        }),
        ("retriggers", &|n, v| {
            v.set_param_by_name("r", if n < 100 { 1.0 } else { 0.0 });
            v.set_param_by_name("pl", if n % 37 == 0 { 0.0 } else { 1.0 });
            v.set_param_by_name("gs", 0.3);
        }),
    ];
    for (name, set) in scenarios {
        let native = render("splice_reel", set);
        let kernel = render("splice_reel_reference", set);
        let worst = native.iter().zip(&kernel).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "{name}: native and kernel differ by {worst}");
        assert!(native.iter().map(|v| v.abs()).fold(0.0f32, f32::max) > 0.05, "{name}: silent");
    }
}

/// splice_reel must be cheap enough to play live: its worst case (six overlapping, pitched, tiny grains at an odd speed)
/// renders in under a tenth of real time. The .nml kernel it replaced took 25 to 85% of real time; the native one about 2%.
#[test]
fn splice_reel_worst_case_runs_well_within_real_time() {
    let source = "use std/blocks\nconfig { channels: stereo }\ninstr a() { m = splice_reel(osc(saw, 110hz) * 0.4, record: if note.time < 1sec { 1 } else { 0 }, grain_size: 0.7, morph: 0.95, speed: 1.37, slide: osc(sine, 0.3hz) * 0.5 + 0.5)\n [m.left, m.right] }";
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
