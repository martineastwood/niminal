//! Sustained digital-waveguide instruments with nonlinear bow/reed excitation.
//! Scattering model background: Julius O. Smith, Physical Audio Signal Processing,
//! "Bowed Strings" and "Single-Reed Instruments" (CCRMA).

mod bowed;
mod reed;
pub use bowed::Bowed;
pub use reed::Reed;

use crate::denormals::flush;

/// Convex fractional reads keep a moving, high-feedback waveguide bounded.
#[derive(Clone)]
struct Waveguide {
    samples: Vec<f64>,
    write: usize,
}

impl Waveguide {
    fn new(sr: f32) -> Self {
        Self {
            samples: vec![0.0; (sr / 20.0).ceil() as usize + 8],
            write: 0,
        }
    }

    fn read(&self, delay: f64) -> f64 {
        let delay = delay.clamp(1.0, (self.samples.len() - 2) as f64);
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f64;
        let len = self.samples.len();
        let newer = (self.write + len - whole) % len;
        let older = (newer + len - 1) % len;
        self.samples[newer] * (1.0 - fraction) + self.samples[older] * fraction
    }

    fn push(&mut self, x: f64) {
        // A last-resort physical displacement/pressure bound inside the loop,
        // independent of the live master limiter. Normal playing stays below it.
        self.samples[self.write] = flush(x.clamp(-2.0, 2.0));
        self.write = (self.write + 1) % self.samples.len();
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.write = 0;
    }
}

fn control(x: f32) -> f64 {
    if x.is_finite() {
        f64::from(x).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn smooth(state: &mut f64, target: f64, coefficient: f64) {
    let delta = target - *state;
    if delta.abs() < 1e-10 {
        *state = target;
    } else {
        *state += coefficient * delta;
    }
}

/// One-pole loss phase expressed as samples at the requested fundamental.
fn loss_delay(pole: f64, omega: f64) -> f64 {
    (pole * omega.sin()).atan2(1.0 - pole * omega.cos()) / omega
}

#[derive(Clone, Default)]
struct Dc {
    x: f64,
    y: f64,
}
impl Dc {
    fn step(&mut self, x: f64, pole: f64) -> f32 {
        self.y = flush(x - self.x + pole * self.y);
        self.x = x;
        (self.y * 0.4) as f32
    }
}
