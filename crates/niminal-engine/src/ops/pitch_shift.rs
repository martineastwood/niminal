//! Real-time pitch shifter using two overlapping grains.

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Grain-based pitch shifter. `shift` is in semitones (positive = up,
/// negative = down). `window` controls the grain size: longer is smoother
/// but adds latency. Uses two overlapping grains with a Hann crossfade
/// to minimise artefacts.
#[derive(Clone)]
pub struct PitchShift {
    buf: Vec<f32>,
    write: usize,
    /// Two grain read positions.
    read: [f64; 2],
    /// Two grain envelope positions (0..1).
    grain_phase: [f64; 2],
    sample_rate: f32,
    window_samples: f64,
}

impl Default for PitchShift {
    fn default() -> Self {
        let mut p = PitchShift {
            buf: Vec::new(),
            write: 0,
            read: [0.0; 2],
            grain_phase: [0.0, 0.5],
            sample_rate: 48_000.0,
            window_samples: 0.0,
        };
        p.prepare(48_000.0);
        p
    }
}

const MAX_WINDOW: f64 = 0.1; // 100ms

impl Opcode for PitchShift {
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.read = [0.0; 2];
        self.grain_phase = [0.0, 0.5];
    }

    fn name(&self) -> &str { "pitch_shift" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("shift", 0.0),
            Port::optional("window", 0.02),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let max = (MAX_WINDOW * 2.0 * f64::from(sample_rate)).ceil() as usize + 4;
        self.buf = vec![0.0; max.max(8)];
        self.write = 0;
        self.window_samples = 0.02 * f64::from(sample_rate);
        self.read = [0.0; 2];
        self.grain_phase = [0.0, 0.5];
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.buf.len() == old.buf.len()
            && self.sample_rate == old.sample_rate
        {
            std::mem::swap(&mut self.buf, &mut old.buf);
            self.write = old.write;
            self.read = old.read;
            self.grain_phase = old.grain_phase;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let len = self.buf.len();
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let shift_semitones = f64::from(inputs[1][i]).clamp(-24.0, 24.0);
            let ratio = (2.0_f64).powf(shift_semitones / 12.0);
            let window = f64::from(inputs[2][i]).clamp(0.005, MAX_WINDOW);
            self.window_samples = window * sr;

            self.buf[self.write] = inputs[0][i];
            self.write = (self.write + 1) % len;

            let grain_inc = 1.0 / self.window_samples;
            let read_speed = ratio;
            let mut result = 0.0;

            for g in 0..2 {
                // Hann window envelope
                let w = (std::f64::consts::PI * self.grain_phase[g]).sin();
                let envelope = w * w;

                // read from delay buffer with interpolation
                let read_int = self.read[g].floor() as isize;
                let frac = (self.read[g] - self.read[g].floor()) as f32;
                let idx0 = ((self.write as isize - 1 - read_int).rem_euclid(len as isize)) as usize;
                let idx1 = ((self.write as isize - 2 - read_int).rem_euclid(len as isize)) as usize;
                let sample = self.buf[idx0] + (self.buf[idx1] - self.buf[idx0]) * frac;

                result += sample as f64 * envelope;

                // advance grain
                self.read[g] += read_speed;
                self.grain_phase[g] += grain_inc;

                // reset grain when its window completes
                if self.grain_phase[g] >= 1.0 {
                    self.grain_phase[g] -= 1.0;
                    self.read[g] = 0.0;
                }
            }

            *o = result as f32;
        }
    }
}
