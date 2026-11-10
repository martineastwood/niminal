//! Native DSP implementations of the modular shelf; wrappers retain its enums and named outputs.
use super::*;
use niminal_engine::ops::{Contour, FreezeReverse, FreqShifter, MultimodeFilter, Slew, TapDelay};

fn size(a: &BuildArgs, default: usize) -> Result<usize, String> {
    let n = a.consts.get("size").copied().unwrap_or(default as f64);
    if !n.is_finite() || n.fract() != 0.0 || !(8.0..=1_536_000.0).contains(&n) {
        return Err("size must be a whole number from 8 to 1536000 samples".into());
    }
    Ok(n as usize)
}

pub(super) fn specs() -> [OpSpec; 6] {
    let time = Kind::Signal(Unit::Time);
    let hz = Kind::Signal(Unit::Hz);
    [
        OpSpec::new(
            "contour_dsp",
            "Rise/fall contour DSP for std/blocks.contour. Outputs [value, eor, eoc]; times are full-scale travel times, zero takes one sample. Positive trigger edges retrigger from the current level; cycle repeats.",
            vec![
                opt("trigger", SIGNAL),
                opt("rise", time),
                opt("fall", time),
                opt("curve", SIGNAL),
                opt("cycle", SIGNAL),
            ],
            Build::Builtin(|_| Ok(Box::new(Contour::default()))),
        ),
        OpSpec::new(
            "slew_dsp",
            "Asymmetric slew DSP for std/blocks.slew. Rise/fall are seconds; linear above 0.5 selects full-scale linear travel, otherwise exponential smoothing. Zero time follows immediately.",
            vec![
                req("x", SIGNAL),
                opt("rise", time),
                opt("fall", time),
                opt("linear", SIGNAL),
            ],
            Build::Builtin(|_| Ok(Box::new(Slew::default()))),
        ),
        OpSpec::new(
            "multimode_filter_dsp",
            "Nonlinear TPT state-variable filter for std/blocks.multimode_filter. Outputs [low, band, high, notch] from shared integrators. Res and drive run 0..1.",
            vec![
                req("x", SIGNAL),
                opt("cutoff", hz),
                opt("res", SIGNAL),
                opt("drive", SIGNAL),
            ],
            Build::Builtin(|_| Ok(Box::new(MultimodeFilter::default()))),
        ),
        OpSpec::new(
            "freq_shifter_dsp",
            "Quadrature frequency shifter for std/blocks.freq_shifter. Outputs [up, down]. Shift is in hertz, bounded to +/-2000hz.",
            vec![req("x", SIGNAL), opt("shift", hz)],
            Build::Builtin(|_| Ok(Box::new(FreqShifter::default()))),
        ),
        OpSpec::new(
            "freeze_reverse_dsp",
            "Record/reverse/freeze ring for std/blocks.freeze_reverse. Reverse and freeze activate above 0.5; freeze wins. Size is fixed in samples. Changes fade over 5ms.",
            vec![
                req("x", SIGNAL),
                opt("length", time),
                opt("reverse", SIGNAL),
                opt("freeze", SIGNAL),
                setting("size", SIGNAL),
            ],
            Build::Builtin(|a| Ok(Box::new(FreezeReverse::new(size(a, 384000)?)))),
        ),
        OpSpec::new(
            "tap_delay_dsp",
            "Multi-tap delay DSP for std/blocks.tap_delay. Outputs [left, right]. Shifts, levels and cutoffs are live arrays of exactly taps entries. Groove: 0 straight, 1 swing, 2 hard swing, 3 reverse swing, 4 random. Filter: 0 off, 1 lowpass, 2 bandpass, 3 highpass. Taps is 1..16; size is fixed in samples.",
            vec![
                req("x", SIGNAL),
                req("time", time),
                opt("step", SIGNAL),
                opt("decay", SIGNAL),
                opt("curve", SIGNAL),
                opt("groove", SIGNAL),
                opt("groove_amount", SIGNAL),
                opt("wobble", SIGNAL),
                opt("wobble_rate", hz),
                opt("spread", SIGNAL),
                opt("filter", SIGNAL),
                opt("resonance", SIGNAL),
                opt("feedback", SIGNAL),
                opt("feedback_tap", SIGNAL),
                opt("feedback_shift", SIGNAL),
                opt("feedback_tone", SIGNAL),
                opt("reverse", SIGNAL),
                opt("freeze", SIGNAL),
                opt("window", time),
                req("shifts", Kind::Array(Unit::Num)),
                req("levels", Kind::Array(Unit::Num)),
                req("cutoffs", Kind::Array(Unit::Hz)),
                setting("taps", SIGNAL),
                setting("size", SIGNAL),
            ],
            Build::Builtin(|a| {
                let taps = a.consts.get("taps").copied().unwrap_or(8.0);
                if !taps.is_finite() || taps.fract() != 0.0 || !(1.0..=16.0).contains(&taps) {
                    return Err("taps must be a whole number from 1 to 16".into());
                }
                Ok(Box::new(TapDelay::new(size(a, 192000)?, taps as usize)))
            }),
        ),
    ]
}
