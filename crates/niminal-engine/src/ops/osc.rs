use std::sync::{Arc, LazyLock};

use crate::opcode::{Opcode, Port, ProcessCtx};

static SINE_TABLE: LazyLock<Wavetable> = LazyLock::new(|| {
    let cycle: Vec<f64> = (0..LEVEL_SIZE)
        .map(|i| (i as f64 / LEVEL_SIZE as f64 * std::f64::consts::TAU).sin())
        .collect();
    Wavetable::from_cycle(&cycle)
});

fn fast_log2(x: f64) -> f64 {
    if x <= 0.0 {
        return f64::MIN;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let mantissa_bits = bits & 0x000F_FFFF_FFFF_FFFF;
    exponent as f64 + mantissa_bits as f64 / (1u64 << 52) as f64
}

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
        let (cos_n, sin_n): (Vec<f64>, Vec<f64>) = (0..n)
            .map(|k| angle(k, n).sin_cos())
            .map(|(s, c)| (c, s))
            .unzip();
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
        let (cos_l, sin_l): (Vec<f64>, Vec<f64>) = (0..LEVEL_SIZE)
            .map(|k| angle(k, LEVEL_SIZE).sin_cos())
            .map(|(s, c)| (c, s))
            .unzip();
        let count = top.ilog2() as usize + 1;
        let levels = (0..count)
            .map(|k| {
                let keep = top >> k;
                let mut level: Vec<f32> = (0..LEVEL_SIZE)
                    .map(|i| {
                        let sum: f64 = parts[..keep]
                            .iter()
                            .enumerate()
                            .map(|(j, (a, b))| {
                                a * cos_l[(j + 1) * i % LEVEL_SIZE]
                                    + b * sin_l[(j + 1) * i % LEVEL_SIZE]
                            })
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
        let octaves = fast_log2(self.top as f64 * 2.0 * inc.abs()) + 1.0;
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
/// sine gives phase-modulation FM. The default starting phase is zero; `phase`
/// captures another starting/reset phase on the first sample.
#[derive(Clone)]
pub struct Osc {
    wave: Wave,
    /// Plays this instead of `wave` when set.
    table: Option<Arc<Wavetable>>,
    /// Morphing bank of tables; `position` selects between them.
    bank: Option<Arc<[Arc<Wavetable>]>>,
    phase: f64,
    initial_phase: Option<f64>,
    sync_phase: f64,
    correction: f64,
    sample_rate: f32,
}

impl Osc {
    pub fn new(wave: Wave) -> Self {
        Osc {
            wave,
            table: None,
            bank: None,
            phase: 0.0,
            initial_phase: None,
            sync_phase: 0.0,
            correction: 0.0,
            sample_rate: 48_000.0,
        }
    }

    /// An oscillator that plays one cycle of a table, band-limited at any pitch.
    pub fn with_table(table: Arc<Wavetable>) -> Self {
        Osc {
            table: Some(table),
            ..Osc::new(Wave::Sine)
        }
    }

    /// Morph between several band-limited tables with the `position` port (0..1).
    pub fn with_bank(bank: Arc<[Arc<Wavetable>]>) -> Self {
        assert!(bank.len() >= 2, "a bank needs at least two tables");
        Osc {
            bank: Some(bank),
            ..Osc::new(Wave::Sine)
        }
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
    fn reset(&mut self) {
        self.phase = 0.0;
        self.initial_phase = None;
        self.sync_phase = 0.0;
        self.correction = 0.0;
    }
    fn name(&self) -> &'static str {
        "osc"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::required("freq"),
            Port::optional("phase_mod", 0.0),
            Port::optional("width", 0.5),
            Port::optional("position", 0.0),
            Port::optional("phase", 0.0),
            Port::optional("sync", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Osc {
            phase: 0.0,
            initial_phase: None,
            sync_phase: 0.0,
            correction: 0.0,
            ..self.clone()
        })
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        if self.wave == Wave::Sine && self.table.is_none() && self.bank.is_none() {
            LazyLock::force(&SINE_TABLE);
        }
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.phase = old.phase;
            self.initial_phase = old.initial_phase;
            self.sync_phase = old.sync_phase;
            self.correction = old.correction;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let position = inputs.get(3);
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.step(
                inputs[0][i],
                inputs[1][i],
                inputs.get(2).map_or(0.5, |w| w[i]),
                position.map_or(0.0, |p| p[i]),
                inputs.get(4).map_or(0.0, |p| p[i]),
                inputs.get(5).map_or(0.0, |p| p[i]),
            );
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(
            inputs[0],
            inputs[1],
            inputs.get(2).copied().unwrap_or(0.5),
            inputs.get(3).copied().unwrap_or(0.0),
            inputs.get(4).copied().unwrap_or(0.0),
            inputs.get(5).copied().unwrap_or(0.0),
        )
    }
}

impl Osc {
    /// One sample from `freq`, `phase_mod`, the pulse `width` and the bank `position`.
    #[inline(always)]
    fn step(
        &mut self,
        freq: f32,
        phase_mod: f32,
        width: f32,
        position: f32,
        phase: f32,
        sync: f32,
    ) -> f32 {
        if self.initial_phase.is_none() {
            let initial = wrap(f64::from(phase));
            self.initial_phase = Some(initial);
            self.phase = initial;
        }
        if sync > 0.0 || self.correction != 0.0 {
            return self.synced_step(freq, phase_mod, width, position, sync.max(0.0));
        }
        self.sync_phase = 0.0;
        self.correction = 0.0;
        let sr = f64::from(self.sample_rate);
        let inc = f64::from(freq) / sr;
        let dt = inc.abs().min(0.5);
        let t = wrap(self.phase + f64::from(phase_mod));

        if let Some(bank) = &self.bank {
            let pos = f64::from(position).clamp(0.0, 1.0);
            let slots = (bank.len() - 1) as f64;
            let p = pos * slots;
            let a = p as usize;
            let b = (a + 1).min(bank.len() - 1);
            let w = (p - a as f64) as f32;
            let low = bank[a].read(t, inc);
            let high = bank[b].read(t, inc);
            self.phase = wrap(self.phase + inc);
            return low + (high - low) * w;
        }

        if let Some(table) = &self.table {
            self.phase = wrap(self.phase + inc);
            return table.read(t, inc);
        }

        let v = match self.wave {
            Wave::Sine => {
                self.phase = wrap(self.phase + inc);
                return SINE_TABLE.read(t, inc);
            }
            Wave::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Wave::Square | Wave::Pulse => {
                let w = if self.wave == Wave::Square {
                    0.5
                } else {
                    f64::from(width).clamp(0.01, 0.99)
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

        self.phase = wrap(self.phase + inc);
        v as f32
    }
}

/// Accumulate two-sample polynomial corrections around an event at `time` within this sample.
/// `jump` is the change of value; `slope` is the change of slope in units per sample.
#[derive(Default)]
struct Corrections {
    now: f64,
    next: f64,
}

impl Corrections {
    fn add(&mut self, time: f64, jump: f64, slope: f64) {
        let before = 1.0 - time;
        self.now += jump * before * before * 0.5 + slope * before.powi(3) / 6.0;
        self.next += -jump * time * time * 0.5 + slope * time.powi(3) / 6.0;
    }
}

/// Natural waveform edges occurring during a fraction of the sample interval.
fn edges(
    wave: Wave,
    phase: f64,
    inc: f64,
    width: f64,
    interval: (f64, f64),
    reset_at_end: bool,
    corrections: &mut Corrections,
) {
    let (offset, length) = interval;
    if inc == 0.0 {
        return;
    }
    let breaks: &[(f64, f64, f64)] = match wave {
        Wave::Saw => &[(0.0, -2.0, 0.0)],
        Wave::Square | Wave::Pulse => &[(0.0, 2.0, 0.0), (width, -2.0, 0.0)],
        Wave::Tri => &[(0.25, 0.0, -8.0), (0.75, 0.0, 8.0)],
        Wave::Sine => &[],
    };
    for &(edge, jump, slope) in breaks {
        for cycle in -1..=1 {
            let time = (edge + f64::from(cycle) - phase) / inc;
            if time > 0.0 && (time < length || (time == length && !reset_at_end)) {
                corrections.add(offset + time, jump * inc.signum(), slope * inc.abs());
            }
        }
    }
}

impl Osc {
    fn value(&self, phase: f64, inc: f64, width: f64, position: f32) -> f64 {
        let t = wrap(phase);
        if let Some(bank) = &self.bank {
            let p = f64::from(position).clamp(0.0, 1.0) * (bank.len() - 1) as f64;
            let a = p as usize;
            let b = (a + 1).min(bank.len() - 1);
            let low = bank[a].read(t, inc);
            return f64::from(low + (bank[b].read(t, inc) - low) * (p - a as f64) as f32);
        }
        if let Some(table) = &self.table {
            return f64::from(table.read(t, inc));
        }
        match self.wave {
            Wave::Sine => f64::from(SINE_TABLE.read(t, inc)),
            Wave::Saw => 2.0 * t - 1.0,
            Wave::Square | Wave::Pulse => {
                if t < width {
                    1.0
                } else {
                    -1.0
                }
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
        }
    }

    fn slope(&self, phase: f64, inc: f64, width: f64, position: f32) -> f64 {
        if self.table.is_some() || self.bank.is_some() {
            let delta = 0.00001;
            return (self.value(phase + delta, inc, width, position)
                - self.value(phase - delta, inc, width, position))
                / (2.0 * delta);
        }
        match self.wave {
            Wave::Sine => std::f64::consts::TAU * (phase * std::f64::consts::TAU).cos(),
            Wave::Saw => 2.0,
            Wave::Square | Wave::Pulse => 0.0,
            Wave::Tri => {
                if (0.25..0.75).contains(&wrap(phase)) {
                    -4.0
                } else {
                    4.0
                }
            }
        }
    }

    /// Track the master crossing between samples, preserving the slave's fractional advance.
    /// Correct both value and slope discontinuities, including natural edges on either side.
    fn synced_step(
        &mut self,
        freq: f32,
        phase_mod: f32,
        width: f32,
        position: f32,
        sync: f32,
    ) -> f32 {
        let sr = f64::from(self.sample_rate);
        let inc = (f64::from(freq) / sr).clamp(-0.5, 0.5);
        let master_inc = (f64::from(sync) / sr).min(0.5);
        let width = if self.wave == Wave::Square {
            0.5
        } else {
            f64::from(width).clamp(0.01, 0.99)
        };
        let pm = f64::from(phase_mod);
        let phase = self.phase;
        // At an exact edge, choose the waveform's right-hand time limit, in either direction.
        let side = if inc == 0.0 {
            0.0
        } else {
            inc.signum() * 1e-10
        };
        let mut corrections = Corrections {
            now: self.value(phase + pm + side, inc, width, position) + self.correction,
            next: 0.0,
        };
        if sync <= 0.0 {
            self.sync_phase = 0.0;
        }
        let crossing = if self.sync_phase + master_inc >= 1.0 {
            Some((1.0 - self.sync_phase) / master_inc)
        } else {
            None
        };
        let length = crossing.unwrap_or(1.0);
        let builtin = self.table.is_none() && self.bank.is_none();
        if builtin {
            edges(
                self.wave,
                wrap(phase + pm),
                inc,
                width,
                (0.0, length),
                crossing.is_some(),
                &mut corrections,
            );
        }
        self.phase = wrap(phase + inc);
        if let Some(time) = crossing {
            let reset = self.initial_phase.unwrap_or(0.0);
            let before = phase + inc * time + pm - side;
            let after = reset + pm + side;
            let jump =
                self.value(after, inc, width, position) - self.value(before, inc, width, position);
            let slope = (self.slope(after, inc, width, position)
                - self.slope(before, inc, width, position))
                * inc;
            corrections.add(time, jump, slope);
            if builtin {
                edges(
                    self.wave,
                    wrap(reset + pm),
                    inc,
                    width,
                    (time, 1.0 - time),
                    false,
                    &mut corrections,
                );
            }
            self.phase = wrap(reset + inc * (1.0 - time));
        }
        self.sync_phase = wrap(self.sync_phase + master_inc);
        self.correction = corrections.next;
        corrections.now as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Phasor;

    fn cycle(n: usize, f: impl Fn(f64) -> f64) -> Vec<f64> {
        (0..n).map(|i| f(i as f64 / n as f64)).collect()
    }

    fn render(osc: &mut Osc, freq: f32, len: usize) -> Vec<f32> {
        let ctx = ProcessCtx::new(48_000.0, true);
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
        let worst = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
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
    fn controlled(
        osc: &mut Osc,
        freq: f32,
        sync: f32,
        phase: f32,
        width: f32,
        frames: usize,
        block: usize,
    ) -> Vec<f32> {
        osc.prepare(48_000.0);
        let ctx = ProcessCtx::new(48_000.0, true);
        let mut output = Vec::new();
        for start in (0..frames).step_by(block) {
            let n = block.min(frames - start);
            let f = vec![freq; n];
            let zero = vec![0.0; n];
            let w = vec![width; n];
            let p = vec![phase; n];
            let master = vec![sync; n];
            let mut out = vec![0.0; n];
            osc.process(&ctx, &[&f, &zero, &w, &zero, &p, &master], &mut out);
            output.extend(out);
        }
        output
    }

    #[test]
    fn pulse_width_sets_duty_cycle_and_square_stays_fixed() {
        let quarter = controlled(&mut Osc::new(Wave::Pulse), 100.0, 0.0, 0.0, 0.25, 4800, 64);
        let mean = quarter.iter().sum::<f32>() / quarter.len() as f32;
        assert!((mean + 0.5).abs() < 0.001, "{mean}");
        let square = controlled(&mut Osc::new(Wave::Square), 100.0, 0.0, 0.0, 0.25, 4800, 64);
        assert!(square.iter().sum::<f32>().abs() < 0.01);
        let half = controlled(&mut Osc::new(Wave::Pulse), 100.0, 0.0, 0.0, 0.5, 4800, 64);
        assert_eq!(square, half);
    }

    #[test]
    fn phase_is_captured_once_and_reset_rearms_it() {
        let mut osc = Osc::new(Wave::Sine);
        osc.prepare(48_000.0);
        let ctx = ProcessCtx::new(48_000.0, true);
        let sample =
            |osc: &mut Osc, phase| osc.process_one(&ctx, &[0.0, 0.0, 0.5, 0.0, phase, 0.0]);
        assert!((sample(&mut osc, 0.25) - 1.0).abs() < 1e-5);
        assert!((sample(&mut osc, 0.75) - 1.0).abs() < 1e-5);
        osc.reset();
        assert!((sample(&mut osc, -0.25) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn phasor_preserves_fractional_sync_timing_and_runs_backwards() {
        let ctx = ProcessCtx::new(48_000.0, true);
        let mut phase = Phasor::default();
        phase.prepare(48_000.0);
        // A master period of 3.2 samples, slave advance of .1 cycles/sample.
        let out: Vec<_> = (0..6)
            .map(|_| phase.process_one(&ctx, &[4800.0, 0.25, 15000.0]))
            .collect();
        for (got, want) in out.into_iter().zip([0.25, 0.35, 0.45, 0.55, 0.33, 0.43]) {
            assert!((got - want).abs() < 1e-6, "{got} expected {want}");
        }
        phase.reset();
        assert!((phase.process_one(&ctx, &[-4800.0, 0.25]) - 0.25).abs() < 1e-6);
        assert!((phase.process_one(&ctx, &[-4800.0, 0.75]) - 0.15).abs() < 1e-6);
    }

    #[test]
    fn sync_and_pwm_match_across_block_sizes_scalar_processing_and_state_carry() {
        let want = controlled(
            &mut Osc::new(Wave::Pulse),
            -2317.0,
            937.0,
            0.13,
            0.27,
            4000,
            64,
        );
        let got = controlled(
            &mut Osc::new(Wave::Pulse),
            -2317.0,
            937.0,
            0.13,
            0.27,
            4000,
            7,
        );
        assert_eq!(got, want);
        let ctx = ProcessCtx::new(48_000.0, true);
        let mut osc = Osc::new(Wave::Pulse);
        osc.prepare(48_000.0);
        let inputs = [-2317.0, 0.0, 0.27, 0.0, 0.13, 937.0];
        let mut scalar = Vec::new();
        for _ in 0..31 {
            scalar.push(osc.process_one(&ctx, &inputs));
        }
        let mut replacement = Osc::new(Wave::Pulse);
        replacement.prepare(48_000.0);
        replacement.carry_state(&mut osc);
        for _ in 31..4000 {
            scalar.push(replacement.process_one(&ctx, &inputs));
        }
        assert_eq!(scalar, want);
    }

    fn naive(wave: Wave, phase: f64, width: f64) -> f64 {
        let t = wrap(phase);
        match wave {
            Wave::Sine => (std::f64::consts::TAU * t).sin(),
            Wave::Saw => 2.0 * t - 1.0,
            Wave::Square | Wave::Pulse => {
                if t < width {
                    1.0
                } else {
                    -1.0
                }
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
        }
    }

    #[test]
    fn sync_corrections_reduce_error_against_a_bandlimited_reference() {
        // Exactly 64 samples per master period; retain only harmonics strictly below Nyquist.
        // Integrate the continuous waveform on a 256x finer grid using midpoint quadrature.
        let period = 64;
        let resolution = period * 256;
        let ratio = 3.37;
        for wave in [Wave::Saw, Wave::Pulse, Wave::Tri, Wave::Sine] {
            let mut reference = vec![0.0; period];
            for harmonic in 0..period / 2 {
                let (mut re, mut im) = (0.0, 0.0);
                for j in 0..resolution {
                    let t = (j as f64 + 0.5) / resolution as f64;
                    let x = naive(wave, t * ratio, 0.3);
                    let angle = std::f64::consts::TAU * harmonic as f64 * t;
                    re += x * angle.cos() / resolution as f64;
                    im += x * angle.sin() / resolution as f64;
                }
                for (i, value) in reference.iter_mut().enumerate() {
                    let angle = std::f64::consts::TAU * harmonic as f64 * i as f64 / period as f64;
                    *value += (if harmonic == 0 { 1.0 } else { 2.0 })
                        * (re * angle.cos() + im * angle.sin());
                }
            }
            let output = controlled(
                &mut Osc::new(wave),
                (750.0 * ratio) as f32,
                750.0,
                0.0,
                0.3,
                period * 3,
                13,
            );
            let mut corrected_error = 0.0;
            let mut naive_error = 0.0;
            for (i, &target) in reference.iter().enumerate() {
                corrected_error += (f64::from(output[period + i]) - target).powi(2);
                naive_error +=
                    (naive(wave, i as f64 / period as f64 * ratio, 0.3) - target).powi(2);
            }
            assert!(
                corrected_error < naive_error * 0.65,
                "{wave:?}: corrected error {corrected_error}, naive {naive_error}"
            );
        }
    }
    #[test]
    fn coincident_sync_and_natural_edges_are_corrected_once_in_both_directions() {
        for wave in [Wave::Saw, Wave::Square, Wave::Tri, Wave::Sine] {
            let forward = controlled(&mut Osc::new(wave), 2250.0, 750.0, 0.0, 0.5, 256, 13);
            let reverse = controlled(&mut Osc::new(wave), -2250.0, 750.0, 0.0, 0.5, 256, 13);
            let worst = forward[64..]
                .iter()
                .zip(&reverse[64..])
                .map(|(a, b)| (a + b).abs())
                .fold(0.0, f32::max);
            assert!(worst < 1e-4, "{wave:?}: forward/reverse asymmetry {worst}");
        }
    }
}
