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
        self.cached_for = (cutoff, res);

        let sr = f64::from(self.sample_rate);
        let fc = f64::from(cutoff).clamp(5.0, sr * 0.45);
        let g = (std::f64::consts::PI * fc / sr).tan();

        const Q_MIN: f64 = std::f64::consts::FRAC_1_SQRT_2;
        const Q_MAX: f64 = 25.0;
        let q = Q_MIN * (Q_MAX / Q_MIN).powf(f64::from(res).clamp(0.0, 1.0));
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
}

impl Opcode for Svf {
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

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            self.update(inputs[1][i], inputs[2][i]);
            let x = f64::from(inputs[0][i]);

            let v3 = x - self.ic2;
            let v1 = self.a1 * self.ic1 + self.a2 * v3;
            let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
            self.ic1 = 2.0 * v1 - self.ic1;
            self.ic2 = 2.0 * v2 - self.ic2;

            let y = match self.mode {
                FilterMode::Low => v2,
                FilterMode::Band => v1,
                FilterMode::High => x - self.k * v1 - v2,
                FilterMode::Notch => x - self.k * v1,
            };
            *o = y as f32;
        }
    }
}
