use std::sync::Arc;

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

/// Samples in each band-limited copy of a table.
const LEVEL_SIZE: usize = 2048;
/// The most harmonics a table keeps.
const MAX_HARMONICS: usize = 1024;

/// One cycle of a wave, kept as copies with fewer and fewer harmonics so it can be played at any
/// pitch without aliasing. Level `k` holds the first `top >> k` harmonics.
pub struct Wavetable {
    levels: Vec<Box<[f32]>>,
    top: usize,
}

impl Wavetable {
    /// Build every level from one cycle of a wave. The cycle must not be empty.
    pub fn from_cycle(cycle: &[f64]) -> Self {
        let n = cycle.len();
        assert!(n > 0, "a table has at least one entry");
        let top = (n / 2).clamp(1, MAX_HARMONICS);
        let angle = |k: usize, size: usize| std::f64::consts::TAU * (k % size) as f64 / size as f64;
        // harmonic h: cos and sin parts, found by a direct transform
        let (cos_n, sin_n): (Vec<f64>, Vec<f64>) = (0..n).map(|k| angle(k, n).sin_cos()).map(|(s, c)| (c, s)).unzip();
        let dc = cycle.iter().sum::<f64>() / n as f64;
        let parts: Vec<(f64, f64)> = (1..=top)
            .map(|h| {
                let (mut a, mut b) = (0.0, 0.0);
                for (i, x) in cycle.iter().enumerate() {
                    a += x * cos_n[h * i % n];
                    b += x * sin_n[h * i % n];
                }
                (2.0 * a / n as f64, 2.0 * b / n as f64)
            })
            .collect();
        let (cos_l, sin_l): (Vec<f64>, Vec<f64>) =
            (0..LEVEL_SIZE).map(|k| angle(k, LEVEL_SIZE).sin_cos()).map(|(s, c)| (c, s)).unzip();
        let count = top.ilog2() as usize + 1;
        let levels = (0..count)
            .map(|k| {
                let keep = top >> k;
                let mut level: Vec<f32> = (0..LEVEL_SIZE)
                    .map(|i| {
                        let sum: f64 = parts[..keep]
                            .iter()
                            .enumerate()
                            .map(|(j, (a, b))| a * cos_l[(j + 1) * i % LEVEL_SIZE] + b * sin_l[(j + 1) * i % LEVEL_SIZE])
                            .sum();
                        (dc + sum) as f32
                    })
                    .collect();
                level.push(level[0]);
                level.into_boxed_slice()
            })
            .collect();
        Wavetable { levels, top }
    }

    /// The wave at phase `t` (in cycles, 0 to 1) for a note that advances `inc` cycles a sample.
    fn read(&self, t: f64, inc: f64) -> f32 {
        // the first level whose harmonics all fall below Nyquist, and the one after it
        let octaves = (self.top as f64 * 2.0 * inc.abs()).max(f64::MIN_POSITIVE).log2() + 1.0;
        let p = octaves.clamp(0.0, (self.levels.len() - 1) as f64);
        let a = p as usize;
        let b = (a + 1).min(self.levels.len() - 1);
        let w = (p - a as f64) as f32;
        let pos = t * LEVEL_SIZE as f64;
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        let at = |level: &[f32]| level[i] + (level[i + 1] - level[i]) * f;
        let low = at(&self.levels[a]);
        low + (at(&self.levels[b]) - low) * w
    }
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
    /// Plays this instead of `wave` when set.
    table: Option<Arc<Wavetable>>,
    phase: f64,
    sample_rate: f32,
}

impl Osc {
    pub fn new(wave: Wave) -> Self {
        Osc { wave, table: None, phase: 0.0, sample_rate: 48_000.0 }
    }

    /// An oscillator that plays one cycle of a table, band-limited at any pitch.
    pub fn with_table(table: Arc<Wavetable>) -> Self {
        Osc { table: Some(table), ..Osc::new(Wave::Sine) }
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
    fn reset(&mut self) { self.phase = 0.0; }
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
        Box::new(Osc { phase: 0.0, ..self.clone() })
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

            if let Some(table) = &self.table {
                *o = table.read(t, inc);
                self.phase = wrap(self.phase + inc);
                continue;
            }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(n: usize, f: impl Fn(f64) -> f64) -> Vec<f64> {
        (0..n).map(|i| f(i as f64 / n as f64)).collect()
    }

    fn render(osc: &mut Osc, freq: f32, len: usize) -> Vec<f32> {
        let ctx = ProcessCtx { sample_rate: 48_000.0, gate: true };
        let f = vec![freq; len];
        let zero = vec![0.0; len];
        let half = vec![0.5; len];
        let mut out = vec![0.0; len];
        osc.prepare(48_000.0);
        osc.process(&ctx, &[&f, &zero, &half], &mut out);
        out
    }

    #[test]
    fn a_one_harmonic_table_is_a_sine() {
        let table = Wavetable::from_cycle(&cycle(512, |t| (t * std::f64::consts::TAU).sin()));
        let got = render(&mut Osc::with_table(Arc::new(table)), 440.0, 2000);
        let want = render(&mut Osc::new(Wave::Sine), 440.0, 2000);
        let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "{worst}");
    }

    /// Energy at one frequency, by a single-bin transform.
    fn bin(x: &[f32], freq: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * freq * i as f64 / 48_000.0;
            re += f64::from(*v) * a.cos();
            im += f64::from(*v) * a.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn a_bright_table_does_not_alias_at_high_pitch() {
        // a saw with every harmonic up to 1000
        let saw = Wavetable::from_cycle(&cycle(2048, |t| 2.0 * t - 1.0));
        // 5kHz: harmonics 1..4 fit under 24kHz; the 5th would fold back to 23kHz->1kHz and so on
        let x = render(&mut Osc::with_table(Arc::new(saw)), 5000.0, 48_000);
        let wanted = bin(&x, 5000.0);
        // 5kHz*5 = 25kHz folds to 23kHz: it must be silent
        let alias = bin(&x, 23_000.0);
        assert!(alias < wanted * 0.01, "alias {alias} against {wanted}");
    }

    #[test]
    fn the_sound_changes_smoothly_through_a_pitch_sweep() {
        let saw = Arc::new(Wavetable::from_cycle(&cycle(2048, |t| 2.0 * t - 1.0)));
        let a = saw.read(0.3, 0.0099);
        let b = saw.read(0.3, 0.0101);
        assert!((a - b).abs() < 0.01, "{a} {b}");
    }
}
