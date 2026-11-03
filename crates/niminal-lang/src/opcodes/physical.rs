//! Sustained physical-model sources and their musical controls.
use super::*;
use niminal_engine::ops::{Bowed, Reed};

pub(super) fn specs() -> [OpSpec; 2] {
    [
        OpSpec::new(
            "bowed",
            "A sustained digital-waveguide bowed string. `pressure`, `position`, `velocity` and `brightness` run 0..1. Position runs from near the bridge to near the string's middle; high notes limit the nearest bridge position to one sample. Pressure controls sticking and slipping; zero lifts the bow. Velocity sets bow speed; zero stops driving. Extreme combinations can whistle or stop speaking, as on a string. Controls ease over 2ms, preserving the resonator during modulation. Frequency clamps to 20hz..min(3000hz, sample_rate/12); nonpositive or non-finite frequency is silent. Use an envelope on velocity to start/stop excitation, and an output envelope to end the note. Runs at session rate; oversample(2) or oversample(4) can reduce nonlinear aliasing. See std/instruments.bowed_string for a playable preset.",
            vec![
                req("freq", Kind::Signal(Unit::Hz)),
                opt("pressure", SIGNAL),
                opt("position", SIGNAL),
                opt("velocity", SIGNAL),
                opt("brightness", SIGNAL),
            ],
            Build::Builtin(|_| Ok(Box::new(Bowed::default()))),
        ),
        OpSpec::new(
            "reed",
            "A sustained single-reed cylindrical-bore model, with a clarinet-like odd-harmonic tone. `pressure`, `stiffness`, `breath` and `brightness` run 0..1. Pressure drives the bore; zero stops blowing. Stiffness changes the reed's response; some pressure/stiffness combinations choke or overblow. Breath adds deterministic air noise. Controls ease over 2ms without resetting the bore. Frequency clamps to 20hz..min(4000hz, sample_rate/16); nonpositive or non-finite frequency is silent. Use an envelope on pressure and on the output to end a note. Runs at session rate; explicit oversampling can reduce nonlinear aliasing. See std/instruments.reed_wind for a playable preset.",
            vec![
                req("freq", Kind::Signal(Unit::Hz)),
                opt("pressure", SIGNAL),
                opt("stiffness", SIGNAL),
                opt("breath", SIGNAL),
                opt("brightness", SIGNAL),
            ],
            Build::Builtin(|_| Ok(Box::new(Reed::default()))),
        ),
    ]
}
