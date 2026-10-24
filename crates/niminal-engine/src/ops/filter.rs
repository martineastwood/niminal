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
    }
}

impl Svf {
    #[inline(always)]
    fn step(&mut self, x: f32, cutoff: f32, res: f32) -> f32 {
        self.update(cutoff, res);
        let x = f64::from(x);

        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);

        let y = match self.mode {
            FilterMode::Low => v2,
            FilterMode::Band => v1,
            FilterMode::High => x - self.k * v1 - v2,
            FilterMode::Notch => x - self.k * v1,
        };
        y as f32
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
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.step(inputs[0][i], inputs[1][i], inputs[2][i]);
        }
    }

    fn has_process_one(&self) -> bool {
        true
    }

    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(inputs[0], inputs[1], inputs[2])
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
