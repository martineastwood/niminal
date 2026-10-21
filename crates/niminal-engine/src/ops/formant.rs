//! Formant filter: parallel bandpass filters tuned to vowel resonances.

use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

struct Vowel {
    freq: [f64; 3],
    bw: [f64; 3],
    amp: [f64; 3],
}

const VOWELS: [Vowel; 5] = [
    Vowel { freq: [800.0, 1150.0, 2800.0], bw: [80.0, 90.0, 120.0], amp: [1.0, 0.5, 0.25] },
    Vowel { freq: [400.0, 1600.0, 2700.0], bw: [60.0, 80.0, 120.0], amp: [1.0, 0.5, 0.2] },
    Vowel { freq: [350.0, 2300.0, 3000.0], bw: [50.0, 100.0, 120.0], amp: [1.0, 0.4, 0.15] },
    Vowel { freq: [450.0, 800.0, 2830.0], bw: [70.0, 80.0, 100.0], amp: [1.0, 0.35, 0.15] },
    Vowel { freq: [325.0, 700.0, 2530.0], bw: [50.0, 60.0, 100.0], amp: [1.0, 0.25, 0.1] },
];

/// Three parallel biquad bandpass filters for vowel formant synthesis.
/// `vowel` morphs through A(0)–E(0.25)–I(0.5)–O(0.75)–U(1.0).
#[derive(Clone)]
pub struct Formant {
    sample_rate: f32,
    // 3 biquads, each with (x1, x2, y1, y2)
    state: [(f64, f64, f64, f64); 3],
}

impl Default for Formant {
    fn default() -> Self {
        Formant {
            sample_rate: 48_000.0,
            state: [(0.0, 0.0, 0.0, 0.0); 3],
        }
    }
}

fn bpf_coeffs(freq: f64, bw: f64, sr: f64) -> (f64, f64, f64, f64, f64) {
    let w0 = std::f64::consts::TAU * freq.clamp(20.0, sr * 0.45) / sr;
    let alpha = w0.sin() * (w0 / (std::f64::consts::TAU * bw.max(1.0))).sinh();
    let b0 = alpha;
    let b1 = 0.0;
    let b2 = -alpha;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * w0.cos();
    let a2 = 1.0 - alpha;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

impl Opcode for Formant {
    fn reset(&mut self) { self.state = [(0.0, 0.0, 0.0, 0.0); 3]; }

    fn name(&self) -> &str { "formant" }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("vowel", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.state = old.state;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let sr = f64::from(self.sample_rate);
        for (i, o) in out.iter_mut().enumerate() {
            let x = f64::from(inputs[0][i]);
            let vowel = f64::from(inputs[1][i]).clamp(0.0, 1.0);

            let pos = vowel * 4.0;
            let idx = (pos.floor() as usize).min(3);
            let frac = pos - pos.floor();
            let v0 = &VOWELS[idx];
            let v1 = &VOWELS[idx + 1];

            let freqs: [f64; 3] = std::array::from_fn(|f| v0.freq[f] + (v1.freq[f] - v0.freq[f]) * frac);
            let bws: [f64; 3] = std::array::from_fn(|f| v0.bw[f] + (v1.bw[f] - v0.bw[f]) * frac);
            let amps: [f64; 3] = std::array::from_fn(|f| v0.amp[f] + (v1.amp[f] - v0.amp[f]) * frac);

            let mut result = 0.0;
            for f in 0..3 {
                let (b0, b1, b2, a1, a2) = bpf_coeffs(freqs[f], bws[f], sr);
                let s = &mut self.state[f];
                let y = b0 * x + b1 * s.0 + b2 * s.1 - a1 * s.2 - a2 * s.3;
                s.1 = s.0;
                s.0 = x;
                s.3 = flush(s.2);
                s.2 = flush(y);
                result += y * amps[f];
            }
            *o = result as f32;
        }
    }
}
