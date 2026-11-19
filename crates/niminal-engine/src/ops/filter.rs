use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Low,
    High,
    Band,
    Notch,
}

/// State-variable filter (zero-delay-feedback topology), stable under fast
/// cutoff modulation. `res` runs 0..1, from no resonance (Q 0.71) to
/// self-oscillation-adjacent (Q 25).
#[derive(Clone)]
pub struct Svf {
    mode: FilterMode,
    sample_rate: f32,
    ic1: f64,
    ic2: f64,
    // Coefficients are cached until cutoff or res change.
    cached_for: (f32, f32),
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    // The same update written as one multiply-add chain per state, so that the next sample waits on less.
    // m11, m12, n1 give the next `ic1` from `ic1`, `ic2` and the input; m21, m22, n2 the next `ic2`.
    m11: f64,
    m12: f64,
    n1: f64,
    m21: f64,
    m22: f64,
    n2: f64,
}

impl Svf {
    pub fn new(mode: FilterMode) -> Self {
        let mut f = Svf {
            mode,
            sample_rate: 48_000.0,
            ic1: 0.0,
            ic2: 0.0,
            cached_for: (f32::NAN, f32::NAN),
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            m11: 0.0,
            m12: 0.0,
            n1: 0.0,
            m21: 0.0,
            m22: 0.0,
            n2: 0.0,
        };
        f.update(1000.0, 0.0);
        f
    }

    fn update(&mut self, cutoff: f32, res: f32) {
        if (cutoff, res) == self.cached_for {
            return;
        }
        // the resonance rarely moves with the cutoff, and its power is the dearer half
        if res != self.cached_for.1 {
            const Q_MIN: f64 = std::f64::consts::FRAC_1_SQRT_2;
            const Q_MAX: f64 = 25.0;
            let q = Q_MIN * (Q_MAX / Q_MIN).powf(f64::from(res).clamp(0.0, 1.0));
            self.k = 1.0 / q;
        }
        self.cached_for = (cutoff, res);

        let sr = f64::from(self.sample_rate);
        let fc = f64::from(cutoff).clamp(5.0, sr * 0.45);
        let g = (std::f64::consts::PI * fc / sr).tan();
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        // ic1' = 2 v1 - ic1 and ic2' = 2 v2 - ic2, with v1 and v2 written out in terms of the state and input.
        self.m11 = 2.0 * self.a1 - 1.0;
        self.m12 = -2.0 * self.a2;
        self.n1 = 2.0 * self.a2;
        self.m21 = 2.0 * self.a2;
        self.m22 = 1.0 - 2.0 * self.a3;
        self.n2 = 2.0 * self.a3;
    }
}

const LOW: u8 = 0;
const HIGH: u8 = 1;
const BAND: u8 = 2;
const NOTCH: u8 = 3;

impl Svf {
    #[inline(always)]
    fn step<const MODE: u8>(&mut self, x: f32, cutoff: f32, res: f32) -> f32 {
        self.update(cutoff, res);
        let x = f64::from(x);
        let (ic1, ic2) = (self.ic1, self.ic2);

        // The output, from the state before this sample.
        let v3 = x - ic2;
        let y = match MODE {
            LOW => ic2 + self.a2 * ic1 + self.a3 * v3,
            // Constant peak: resonance narrows the band without raising its level.
            BAND => self.k * (self.a1 * ic1 + self.a2 * v3),
            HIGH => {
                let v1 = self.a1 * ic1 + self.a2 * v3;
                let v2 = ic2 + self.a2 * ic1 + self.a3 * v3;
                x - self.k * v1 - v2
            }
            _ => x - self.k * (self.a1 * ic1 + self.a2 * v3),
        };
        // The next state: one short chain from the old state, with the input's share already worked out.
        self.ic1 = flush((self.m11 * ic1 + self.m12 * ic2) + self.n1 * x);
        self.ic2 = flush((self.m21 * ic1 + self.m22 * ic2) + self.n2 * x);
        y as f32
    }

    fn run<const MODE: u8>(&mut self, inputs: &[&[f32]], out: &mut [f32]) {
        let n = out.len();
        let (x, cutoff, res) = (&inputs[0][..n], &inputs[1][..n], &inputs[2][..n]);
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.step::<MODE>(x[i], cutoff[i], res[i]);
        }
    }
}

impl Opcode for Svf {
    fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
    fn name(&self) -> &'static str {
        match self.mode {
            FilterMode::Low => "lpf",
            FilterMode::High => "hpf",
            FilterMode::Band => "bpf",
            FilterMode::Notch => "notch",
        }
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::required("cutoff"),
            Port::optional("res", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Svf::new(self.mode))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cached_for = (f32::NAN, f32::NAN);
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.ic1 = old.ic1;
            self.ic2 = old.ic2;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        match self.mode {
            FilterMode::Low => self.run::<LOW>(inputs, out),
            FilterMode::High => self.run::<HIGH>(inputs, out),
            FilterMode::Band => self.run::<BAND>(inputs, out),
            FilterMode::Notch => self.run::<NOTCH>(inputs, out),
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        let (x, cutoff, res) = (inputs[0], inputs[1], inputs[2]);
        match self.mode {
            FilterMode::Low => self.step::<LOW>(x, cutoff, res),
            FilterMode::High => self.step::<HIGH>(x, cutoff, res),
            FilterMode::Band => self.step::<BAND>(x, cutoff, res),
            FilterMode::Notch => self.step::<NOTCH>(x, cutoff, res),
        }
    }
}

#[cfg(test)]
mod one_sample_tests {
    use super::*;
    use crate::ops::{Lag, Osc, Wave};

    /// A one-sample path must give exactly what the block path gives.
    fn same(mut block: Box<dyn Opcode>, mut one: Box<dyn Opcode>) {
        let n = 300;
        let ports = block.ports().len();
        let inputs: Vec<Vec<f32>> = (0..ports)
            .map(|p| {
                (0..n)
                    .map(|i| match block.ports()[p].name.as_ref() {
                        "cutoff" | "freq" => 200.0 + 5.0 * i as f32,
                        "res" => 0.1 + 0.002 * i as f32,
                        "time" => 0.001 * (1 + i % 7) as f32,
                        _ => ((i * 7 + p) % 13) as f32 / 13.0 - 0.5,
                    })
                    .collect()
            })
            .collect();
        block.prepare(48_000.0);
        one.prepare(48_000.0);
        let ctx = ProcessCtx::new(48_000.0, true);
        let refs: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
        let mut expected = vec![0.0; n];
        block.process(&ctx, &refs, &mut expected);
        assert!(one.has_process_one());
        for i in 0..n {
            let x: Vec<f32> = inputs.iter().map(|v| v[i]).collect();
            assert_eq!(
                one.process_one(&ctx, &x),
                expected[i],
                "sample {i} of {}",
                one.name()
            );
        }
    }

    #[test]
    fn the_one_sample_paths_match_the_block_paths() {
        same(
            Box::new(Svf::new(FilterMode::Low)),
            Box::new(Svf::new(FilterMode::Low)),
        );
        same(
            Box::new(Svf::new(FilterMode::High)),
            Box::new(Svf::new(FilterMode::High)),
        );
        same(Box::new(Osc::new(Wave::Saw)), Box::new(Osc::new(Wave::Saw)));
        same(
            Box::new(Osc::new(Wave::Pulse)),
            Box::new(Osc::new(Wave::Pulse)),
        );
        same(
            Box::new(Osc::new(Wave::Sine)),
            Box::new(Osc::new(Wave::Sine)),
        );
        same(Box::new(Lag::default()), Box::new(Lag::default()));
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    /// The filter as it was written first: the state taken from `v1` and `v2`.
    fn reference(mode: FilterMode, x: &[f32], cutoff: &[f32], res: &[f32]) -> Vec<f32> {
        let mut f = Svf::new(mode);
        let (mut ic1, mut ic2) = (0.0f64, 0.0f64);
        x.iter()
            .zip(cutoff)
            .zip(res)
            .map(|((&x, &c), &r)| {
                f.update(c, r);
                let x = f64::from(x);
                let v3 = x - ic2;
                let v1 = f.a1 * ic1 + f.a2 * v3;
                let v2 = ic2 + f.a2 * ic1 + f.a3 * v3;
                ic1 = flush(2.0 * v1 - ic1);
                ic2 = flush(2.0 * v2 - ic2);
                (match mode {
                    FilterMode::Low => v2,
                    FilterMode::Band => f.k * v1,
                    FilterMode::High => x - f.k * v1 - v2,
                    FilterMode::Notch => x - f.k * v1,
                }) as f32
            })
            .collect()
    }

    /// The shorter chain rounds differently from the original form, but only at the last few bits of
    /// an f64, so every mode stays within a few f32 steps of it over a long noisy run, with the cutoff
    /// and resonance sweeping fast and the filter close to self-oscillation.
    #[test]
    fn the_shorter_chain_stays_within_rounding_of_the_original_form() {
        let n = 96_000;
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        let x: Vec<f32> = (0..n).map(|_| noise()).collect();
        let cutoff: Vec<f32> = (0..n)
            .map(|i| 200.0 + 8000.0 * (0.5 + 0.5 * (i as f32 * 0.002).sin()))
            .collect();
        let res: Vec<f32> = (0..n).map(|i| 0.9 + 0.1 * (i as f32 * 0.0007).sin()).collect();
        for mode in [FilterMode::Low, FilterMode::High, FilterMode::Band, FilterMode::Notch] {
            let want = reference(mode, &x, &cutoff, &res);
            let mut f = Svf::new(mode);
            f.prepare(48_000.0);
            let mut got = vec![0.0; n];
            f.process(&ProcessCtx::new(48_000.0, true), &[&x, &cutoff, &res], &mut got);
            let (mut worst, mut peak) = (0.0f32, 0.0f32);
            for (g, w) in got.iter().zip(&want) {
                assert!(g.is_finite());
                worst = worst.max((g - w).abs());
                peak = peak.max(w.abs());
            }
            assert!(worst <= 1e-4 * peak.max(1.0), "{mode:?}: worst {worst} against a peak of {peak}");
        }
    }
}
