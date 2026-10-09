use crate::opcode::{Opcode, Port, ProcessCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Saw,
    /// Pulse with a fixed 50% duty cycle.
    Square,
    /// Pulse whose duty cycle comes from the `width` port.
    Pulse,
    Tri,
}

/// Oscillator. Saw and pulse are band-limited with PolyBLEP; the triangle is
/// computed directly (its harmonics fall off fast enough that aliasing is mild).
///
/// `phase_mod` is added to the phase in cycles, so a sine modulated by another
/// sine gives phase-modulation FM. All waves start at phase 0: sine and
/// triangle rise from zero, saw starts at -1.
#[derive(Clone)]
pub struct Osc {
    wave: Wave,
    phase: f64,
    sample_rate: f32,
}

impl Osc {
    pub fn new(wave: Wave) -> Self {
        Osc { wave, phase: 0.0, sample_rate: 48_000.0 }
    }
}

fn poly_blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

fn wrap(x: f64) -> f64 {
    x - x.floor()
}

impl Opcode for Osc {
    fn name(&self) -> &'static str {
        "osc"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::required("freq"),
            Port::optional("phase_mod", 0.0),
            Port::optional("width", 0.5),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Osc::new(self.wave))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.phase = old.phase;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let inc = f64::from(inputs[0][i]) / sr;
            let dt = inc.abs().min(0.5);
            let t = wrap(self.phase + f64::from(inputs[1][i]));

            let v = match self.wave {
                Wave::Sine => (t * std::f64::consts::TAU).sin(),
                Wave::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
                Wave::Square | Wave::Pulse => {
                    let w = if self.wave == Wave::Square {
                        0.5
                    } else {
                        f64::from(inputs[2][i]).clamp(0.01, 0.99)
                    };
                    let naive = if t < w { 1.0 } else { -1.0 };
                    naive + poly_blep(t, dt) - poly_blep(wrap(t - w), dt)
                }
                Wave::Tri => {
                    if t < 0.25 {
                        4.0 * t
                    } else if t < 0.75 {
                        2.0 - 4.0 * t
                    } else {
                        4.0 * t - 4.0
                    }
                }
            };

            *o = v as f32;
            self.phase = wrap(self.phase + inc);
        }
    }
}
