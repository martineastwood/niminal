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

/// Several state-variable filters of one kind that run side by side: lane `j` has its own `x`, `cutoff`
/// and `res`, and gives exactly what a lone [`Svf`] would. The lanes advance in lockstep, one sample
/// at a time, so their independent recurrences overlap instead of each waiting on its own.
#[derive(Clone)]
pub struct SvfBank {
    mode: FilterMode,
    lanes: usize,
    ports: Vec<Port>,
    /// Each lane's coefficient bookkeeping; its state lives in `ic1` and `ic2`.
    filters: Vec<Svf>,
    /// The cutoff and resonance each lane's coefficients were made for.
    seen_cutoff: Vec<f32>,
    seen_res: Vec<f32>,
    ic1: Vec<f64>,
    ic2: Vec<f64>,
    a1: Vec<f64>,
    a2: Vec<f64>,
    a3: Vec<f64>,
    k: Vec<f64>,
    m11: Vec<f64>,
    m12: Vec<f64>,
    n1: Vec<f64>,
    m21: Vec<f64>,
    m22: Vec<f64>,
    n2: Vec<f64>,
    /// This sample's inputs and results, one entry per lane.
    row_x: Vec<f32>,
    row_cutoff: Vec<f32>,
    row_res: Vec<f32>,
    row_y: Vec<f32>,
}

impl SvfBank {
    pub fn new(mode: FilterMode, lanes: usize) -> Self {
        assert!(lanes > 0, "a bank has at least one filter");
        let mut ports = Vec::with_capacity(3 * lanes);
        for j in 0..lanes {
            ports.push(Port::named(format!("x{j}"), None));
        }
        for j in 0..lanes {
            ports.push(Port::named(format!("cutoff{j}"), None));
        }
        for j in 0..lanes {
            ports.push(Port::named(format!("res{j}"), Some(0.0)));
        }
        let zeros = || vec![0.0f64; lanes];
        let mut bank = SvfBank {
            mode,
            lanes,
            ports,
            filters: (0..lanes).map(|_| Svf::new(mode)).collect(),
            seen_cutoff: vec![0.0; lanes],
            seen_res: vec![0.0; lanes],
            ic1: zeros(),
            ic2: zeros(),
            a1: zeros(),
            a2: zeros(),
            a3: zeros(),
            k: zeros(),
            m11: zeros(),
            m12: zeros(),
            n1: zeros(),
            m21: zeros(),
            m22: zeros(),
            n2: zeros(),
            row_x: vec![0.0; lanes],
            row_cutoff: vec![0.0; lanes],
            row_res: vec![0.0; lanes],
            row_y: vec![0.0; lanes],
        };
        for j in 0..lanes {
            bank.take_coefficients(j);
        }
        bank
    }

    /// Copy lane `j`'s coefficients, and what they were made for, out of its filter.
    fn take_coefficients(&mut self, j: usize) {
        let f = &self.filters[j];
        self.seen_cutoff[j] = f.cached_for.0;
        self.seen_res[j] = f.cached_for.1;
        self.a1[j] = f.a1;
        self.a2[j] = f.a2;
        self.a3[j] = f.a3;
        self.k[j] = f.k;
        self.m11[j] = f.m11;
        self.m12[j] = f.m12;
        self.n1[j] = f.n1;
        self.m21[j] = f.m21;
        self.m22[j] = f.m22;
        self.n2[j] = f.n2;
    }

    fn run<const MODE: u8>(
        &mut self,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        let n = self.lanes;
        for i in 0..frames {
            let mut changed = false;
            for j in 0..n {
                let (c, r) = (slots[indices[n + j]][i], slots[indices[2 * n + j]][i]);
                self.row_x[j] = slots[indices[j]][i];
                self.row_cutoff[j] = c;
                self.row_res[j] = r;
                changed |= (c != self.seen_cutoff[j]) | (r != self.seen_res[j]);
            }
            if changed {
                for j in 0..n {
                    let (c, r) = (self.row_cutoff[j], self.row_res[j]);
                    if c != self.seen_cutoff[j] || r != self.seen_res[j] {
                        self.filters[j].update(c, r);
                        self.take_coefficients(j);
                    }
                }
            }
            // Every lane does the same arithmetic as `Svf::step`, on its own state.
            let (a1, a2, a3, k) = (&self.a1[..n], &self.a2[..n], &self.a3[..n], &self.k[..n]);
            let (m11, m12, n1) = (&self.m11[..n], &self.m12[..n], &self.n1[..n]);
            let (m21, m22, n2) = (&self.m21[..n], &self.m22[..n], &self.n2[..n]);
            let (ic1s, ic2s) = (&mut self.ic1[..n], &mut self.ic2[..n]);
            let (row_x, row_y) = (&self.row_x[..n], &mut self.row_y[..n]);
            for j in 0..n {
                let x = f64::from(row_x[j]);
                let (ic1, ic2) = (ic1s[j], ic2s[j]);
                let v3 = x - ic2;
                let y = match MODE {
                    LOW => ic2 + a2[j] * ic1 + a3[j] * v3,
                    BAND => k[j] * (a1[j] * ic1 + a2[j] * v3),
                    HIGH => {
                        let v1 = a1[j] * ic1 + a2[j] * v3;
                        let v2 = ic2 + a2[j] * ic1 + a3[j] * v3;
                        x - k[j] * v1 - v2
                    }
                    _ => x - k[j] * (a1[j] * ic1 + a2[j] * v3),
                };
                ic1s[j] = flush((m11[j] * ic1 + m12[j] * ic2) + n1[j] * x);
                ic2s[j] = flush((m21[j] * ic1 + m22[j] * ic2) + n2[j] * x);
                row_y[j] = y as f32;
            }
            for (out, &y) in outs.iter_mut().zip(&self.row_y) {
                out[i] = y;
            }
        }
    }
}

impl Opcode for SvfBank {
    fn reset(&mut self) {
        self.ic1.fill(0.0);
        self.ic2.fill(0.0);
    }

    fn name(&self) -> &str {
        match self.mode {
            FilterMode::Low => "lpf",
            FilterMode::High => "hpf",
            FilterMode::Band => "bpf",
            FilterMode::Notch => "notch",
        }
    }

    fn ports(&self) -> &[Port] {
        &self.ports
    }

    fn outputs(&self) -> usize {
        self.lanes
    }

    fn max_inputs(&self) -> usize {
        self.ports.len()
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(SvfBank::new(self.mode, self.lanes))
    }

    fn prepare(&mut self, sample_rate: f32) {
        for j in 0..self.lanes {
            self.filters[j].prepare(sample_rate);
            self.take_coefficients(j);
        }
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && old.lanes == self.lanes
        {
            self.ic1.copy_from_slice(&old.ic1);
            self.ic2.copy_from_slice(&old.ic2);
        }
    }

    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], _: &mut [f32]) {
        unreachable!("a filter bank uses graph slots")
    }

    fn process_slots(
        &mut self,
        _: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        match self.mode {
            FilterMode::Low => self.run::<LOW>(slots, indices, outs, frames),
            FilterMode::High => self.run::<HIGH>(slots, indices, outs, frames),
            FilterMode::Band => self.run::<BAND>(slots, indices, outs, frames),
            FilterMode::Notch => self.run::<NOTCH>(slots, indices, outs, frames),
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

#[cfg(test)]
mod bank_tests {
    use super::*;

    /// A bank must give, lane by lane and bit for bit, what separate filters give: with steady and
    /// swept cutoffs, a resonance that moves in some lanes only, a lane fed a non-finite cutoff, and a
    /// run split into uneven blocks.
    #[test]
    fn a_bank_matches_separate_filters_bit_for_bit() {
        let lanes = 7;
        let n = 3000;
        let mut seed = 99u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        let x: Vec<Vec<f32>> = (0..lanes).map(|_| (0..n).map(|_| noise()).collect()).collect();
        let cutoff: Vec<Vec<f32>> = (0..lanes)
            .map(|j| {
                (0..n)
                    .map(|i| match j {
                        0 | 1 => 300.0 + 700.0 * j as f32,
                        5 if i > 1200 && i < 1300 => f32::NAN,
                        _ => 200.0 + 6000.0 * (0.5 + 0.5 * (i as f32 * 0.003 * j as f32).sin()),
                    })
                    .collect()
            })
            .collect();
        let res: Vec<Vec<f32>> = (0..lanes)
            .map(|j| {
                (0..n)
                    .map(|i| if j % 2 == 0 { 0.4 } else { 0.5 + 0.4 * (i as f32 * 0.001).sin() })
                    .collect()
            })
            .collect();
        for mode in [FilterMode::Low, FilterMode::High, FilterMode::Band, FilterMode::Notch] {
            let ctx = ProcessCtx::new(48_000.0, true);
            let mut alone: Vec<Svf> = (0..lanes).map(|_| Svf::new(mode)).collect();
            let mut want = vec![vec![0.0f32; n]; lanes];
            for j in 0..lanes {
                alone[j].prepare(48_000.0);
                alone[j].process(&ctx, &[&x[j], &cutoff[j], &res[j]], &mut want[j]);
            }
            let mut bank = SvfBank::new(mode, lanes);
            bank.prepare(48_000.0);
            assert_eq!(bank.ports().len(), 3 * lanes);
            assert_eq!(bank.outputs(), lanes);
            let mut got = vec![vec![0.0f32; n]; lanes];
            let mut start = 0;
            for len in [64, 1, 100, 7, 63].iter().cycle() {
                if start >= n {
                    break;
                }
                let len = (*len).min(n - start);
                let slots: Vec<Vec<f32>> = (0..lanes)
                    .map(|j| x[j][start..start + len].to_vec())
                    .chain((0..lanes).map(|j| cutoff[j][start..start + len].to_vec()))
                    .chain((0..lanes).map(|j| res[j][start..start + len].to_vec()))
                    .collect();
                let indices: Vec<usize> = (0..3 * lanes).collect();
                let mut outs = vec![vec![0.0f32; len]; lanes];
                bank.process_slots(&ctx, &slots, &indices, &mut outs, len);
                for j in 0..lanes {
                    got[j][start..start + len].copy_from_slice(&outs[j]);
                }
                start += len;
            }
            for j in 0..lanes {
                for i in 0..n {
                    assert_eq!(got[j][i].to_bits(), want[j][i].to_bits(), "{mode:?} lane {j} sample {i}");
                }
            }
        }
    }
}
