use super::*;
use crate::ops::custom::{Interp, read_at};

const RANDOM: [f64; 16] = [
    0.37, -0.22, 0.05, 0.41, -0.38, 0.18, -0.07, 0.33, -0.44, 0.09, 0.28, -0.15, 0.4, -0.3, 0.02,
    -0.26,
];
#[derive(Clone, Default)]
struct Tap {
    s1: f64,
    s2: f64,
    shift: f64,
    ratio: f64,
    cutoff: f64,
    resonance: f64,
    g: f64,
    a1: f64,
}
impl Tap {
    fn new() -> Self {
        Self {
            shift: f64::NAN,
            cutoff: f64::NAN,
            resonance: f64::NAN,
            ..Self::default()
        }
    }
    fn ratio(&mut self, shift: f64) -> f64 {
        if shift != self.shift {
            self.shift = shift;
            self.ratio = 2.0f64.powf(shift / 12.0);
        }
        self.ratio
    }
}
#[derive(Clone)]
pub struct TapDelay {
    sample_rate: f64,
    ports: Vec<Port>,
    buffer: Vec<f64>,
    tape: FreezeReverse,
    taps: Vec<Tap>,
    write: usize,
    grain_phase: f64,
    swirl: f64,
    feedback: f64,
    tone_low: f64,
    tone_high: f64,
    feedback_pitch: Tap,
    tone: f64,
    lp: f64,
    hp: f64,
}
impl TapDelay {
    pub fn new(size: usize, taps: usize) -> Self {
        let mut ports = vec![
            Port::required("x"),
            Port::required("time"),
            Port::optional("step", 0.0),
            Port::optional("decay", 0.8),
            Port::optional("curve", 1.0),
            Port::optional("groove", 0.0),
            Port::optional("groove_amount", 1.0),
            Port::optional("wobble", 0.0),
            Port::optional("wobble_rate", 0.5),
            Port::optional("spread", 1.0),
            Port::optional("filter", 0.0),
            Port::optional("resonance", 0.3),
            Port::optional("feedback", 0.0),
            Port::optional("feedback_tap", 0.0),
            Port::optional("feedback_shift", 0.0),
            Port::optional("feedback_tone", 0.0),
            Port::optional("reverse", 0.0),
            Port::optional("freeze", 0.0),
            Port::optional("window", 0.04),
        ];
        for (name, default) in [("shifts", 0.0), ("levels", 1.0), ("cutoffs", 2000.0)] {
            for i in 0..taps {
                ports.push(Port::named(format!("{name}_{i}"), Some(default)));
            }
        }
        Self {
            sample_rate: 48000.0,
            ports,
            buffer: vec![0.0; size],
            tape: FreezeReverse::new(size),
            taps: (0..taps).map(|_| Tap::new()).collect(),
            write: 0,
            grain_phase: 0.0,
            swirl: 0.0,
            feedback: 0.0,
            tone_low: 0.0,
            tone_high: 0.0,
            feedback_pitch: Tap::new(),
            tone: f64::NAN,
            lp: 0.0,
            hp: 0.0,
        }
    }
    fn input_ports(&self) -> &[Port] {
        &self.ports
    }
    fn fresh(&self) -> Self {
        Self::new(self.buffer.len(), self.taps.len())
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.tape.sample_rate = self.sample_rate;
        self.tape.clear();
        self.taps.fill(Tap::new());
        self.write = 0;
        self.grain_phase = 0.0;
        self.swirl = 0.0;
        self.feedback = 0.0;
        self.tone_low = 0.0;
        self.tone_high = 0.0;
        self.feedback_pitch = Tap::new();
        self.tone = f64::NAN;
    }
    fn carry(&mut self, old: &mut Self) {
        if self.buffer.len() == old.buffer.len() && self.taps.len() == old.taps.len() {
            std::mem::swap(&mut self.buffer, &mut old.buffer);
            self.tape.carry(&mut old.tape);
            for (tap, previous) in self.taps.iter_mut().zip(&old.taps) {
                tap.s1 = previous.s1;
                tap.s2 = previous.s2;
                // Coefficients belong to the new sample rate and controls.
                tap.cutoff = f64::NAN;
                tap.resonance = f64::NAN;
            }
            self.write = old.write;
            self.grain_phase = old.grain_phase;
            self.swirl = old.swirl;
            self.feedback = old.feedback;
            self.tone_low = old.tone_low;
            self.tone_high = old.tone_high;
        }
    }
    fn time(i: usize, base: f64, curve: f64, amount: f64, mode: u8, wobble: f64, cap: f64) -> f64 {
        let odd = (i % 2) as f64;
        let offset = match mode {
            1 => odd / 3.0,
            2 => odd / 2.0,
            3 => (odd - 1.0) / 3.0,
            4 => RANDOM[i],
            _ => 0.0,
        };
        (base * ((i + 1) as f64).powf(curve) + base * amount * offset + wobble)
            .max(1.0)
            .min(cap)
    }
    fn read(&self, delay: f64, ratio: f64, grain: f64, phase: f64) -> f64 {
        let hold = (ratio - 1.0).max(0.0) * grain;
        let other = if phase + grain / 2.0 >= grain {
            phase - grain / 2.0
        } else {
            phase + grain / 2.0
        };
        let weight = 1.0 - (2.0 * phase / grain - 1.0).abs();
        let from = self.write as f64 - 1.0 - delay - hold;
        read_at(&self.buffer, from - (1.0 - ratio) * phase, Interp::Linear) * weight
            + read_at(&self.buffer, from - (1.0 - ratio) * other, Interp::Linear) * (1.0 - weight)
    }
    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 2] {
        let sr = self.sample_rate;
        let grain = f64::from((bounded(x[18], 0.01, 0.1) as f32) * sr as f32)
            .floor()
            .max(1.0);
        self.grain_phase = if self.grain_phase + 1.0 >= grain {
            0.0
        } else {
            self.grain_phase + 1.0
        };
        self.swirl = (self.swirl + bounded(x[8], 0.05, 10.0) / sr).fract();
        let wobble =
            (std::f64::consts::TAU * self.swirl).sin() * bounded(x[7], 0.0, 1.0) * 0.01 * sr;
        let base = (finite(x[1]).max(0.001) * sr).min(self.buffer.len() as f64);
        // Tiny user buffers cannot fit a pitch window: use the minimum one-sample delay.
        let cap = (self.buffer.len() as f64 - 8.0 - 3.0 * grain).max(1.0);
        let curve = bounded(x[4], 0.5, 2.0);
        let amount = bounded(x[6], 0.0, 1.0);
        let mode = bounded(x[5], 0.0, 4.0) as u8;
        let count = self.taps.len();
        let fbi = (if finite(x[13]) > 0.5 {
            finite(x[13])
        } else {
            count as f64
        })
        .clamp(1.0, count as f64)
        .floor() as usize
            - 1;
        let feedback_time = Self::time(fbi, base, curve, amount, mode, wobble, cap);
        let input = bounded(x[0], -16.0, 16.0) + self.feedback * bounded(x[12], 0.0, 0.95);
        let input = self.tape.tick(
            input,
            feedback_time / sr,
            finite(x[16]) > 0.5,
            finite(x[17]) > 0.5,
        );
        self.buffer[self.write] = input;
        self.write = (self.write + 1) % self.buffer.len();
        let step = bounded(x[2], -12.0, 12.0);
        let decay = bounded(x[3], 0.0, 1.0);
        let spread = bounded(x[9], 0.0, 1.0);
        let resonance = bounded(x[11], 0.0, 1.0);
        let k = 1.0 / (0.5 + resonance * resonance * 15.0);
        let filter = bounded(x[10], 0.0, 3.0) as u8;
        let mut left = 0.0;
        let mut right = 0.0;
        let mut gain = 1.0;
        for i in 0..count {
            gain *= decay;
            let delay = Self::time(i, base, curve, amount, mode, wobble, cap);
            let ratio =
                self.taps[i].ratio((finite(x[19 + i]) + (i + 1) as f64 * step).clamp(-24.0, 24.0));
            let shifted = self.grain_phase + (i as f64 * grain / count as f64).floor();
            let phase = if shifted >= grain {
                shifted - grain
            } else {
                shifted
            };
            let y = self.read(delay, ratio, grain, phase);
            let cutoff = bounded(x[19 + 2 * count + i], 20.0, sr * 0.45);
            let tap = &mut self.taps[i];
            if tap.cutoff != cutoff || tap.resonance != resonance {
                tap.cutoff = cutoff;
                tap.resonance = resonance;
                tap.g = (std::f64::consts::PI * cutoff / sr).tan();
                tap.a1 = 1.0 / (1.0 + tap.g * (tap.g + k));
            }
            let a2 = tap.g * tap.a1;
            let v3 = y - tap.s2;
            let v1 = tap.a1 * tap.s1 + a2 * v3;
            let v2 = tap.s2 + a2 * tap.s1 + tap.g * a2 * v3;
            tap.s1 = (2.0 * v1 - tap.s1).clamp(-100.0, 100.0);
            tap.s2 = (2.0 * v2 - tap.s2).clamp(-100.0, 100.0);
            let output = match filter {
                1 => v2,
                2 => v1 * k,
                3 => y - k * v1 - v2,
                _ => y,
            };
            let heard = output * gain * bounded(x[19 + count + i], 0.0, 4.0);
            let side = (i % 2) as f64;
            left += heard * (1.0 - side * spread);
            right += heard * (1.0 - (1.0 - side) * spread);
        }
        let ratio = self.feedback_pitch.ratio(bounded(x[14], -12.0, 12.0));
        let phase = if self.grain_phase + 1.0 >= grain {
            0.0
        } else {
            self.grain_phase
        };
        let feedback = self.read(feedback_time, ratio, grain, phase);
        let tone = bounded(x[15], -1.0, 1.0);
        if self.tone != tone {
            self.tone = tone;
            self.lp =
                (-std::f64::consts::TAU * 20000.0 * 0.01f64.powf((-tone).max(0.0)) / sr).exp();
            self.hp = (-std::f64::consts::TAU * 20.0 * 500.0f64.powf(tone.max(0.0)) / sr).exp();
        }
        self.tone_low = feedback * (1.0 - self.lp) + self.tone_low * self.lp;
        self.tone_high = feedback * (1.0 - self.hp) + self.tone_high * self.hp;
        self.feedback = ((if tone > 0.0 {
            feedback - self.tone_high
        } else {
            self.tone_low
        }) * 0.25)
            .tanh()
            * 4.0;
        [
            (left * 0.25).tanh() as f32 * 4.0,
            (right * 0.25).tanh() as f32 * 4.0,
        ]
    }
}
block_opcode!(TapDelay, "tap_delay_dsp", 2, 67);
