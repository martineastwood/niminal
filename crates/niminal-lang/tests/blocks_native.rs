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
