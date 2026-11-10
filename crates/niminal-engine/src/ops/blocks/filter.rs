use super::*;
use crate::denormals::flush;

/// The shelf's nonlinear TPT filter, with all responses from the same integrators.
/// Its resonance curve differs from `Svf`'s constant-peak band response.
#[derive(Clone)]
pub struct MultimodeFilter {
    sample_rate: f64,
    s1: f64,
    s2: f64,
    cached: (f64, f64),
    g: f64,
    k: f64,
    a1: f64,
}
impl Default for MultimodeFilter {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            s1: 0.0,
            s2: 0.0,
            cached: (f64::NAN, f64::NAN),
            g: 0.0,
            k: 0.0,
            a1: 0.0,
        }
    }
}
impl MultimodeFilter {
    fn input_ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("cutoff", 1000.0),
            Port::optional("res", 0.2),
            Port::optional("drive", 0.0),
        ];
        PORTS
    }
    fn fresh(&self) -> Self {
        Self::default()
    }
    fn clear(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
        self.cached = (f64::NAN, f64::NAN);
    }
    fn carry(&mut self, old: &mut Self) {
        self.s1 = old.s1;
        self.s2 = old.s2;
    }
    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 4] {
        let drive = bounded(x[3], 0.0, 1.0);
        let res = bounded(x[2], 0.0, 1.0);
        let input = finite(x[0]);
        let input = if drive > 0.0 {
            mix(input, (input * (1.0 + drive * 7.0)).tanh(), drive)
        } else {
            input
        };
        let cutoff = bounded(x[1], 10.0, self.sample_rate * 0.45);
        if self.cached != (cutoff, res) {
            self.cached = (cutoff, res);
            self.g = (std::f64::consts::PI * cutoff / self.sample_rate).tan();
            self.k = 2.0 - 1.95 * res;
            self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        }
        let v1 = self.a1 * self.s1 + self.g * self.a1 * (input - self.s2);
        let v2 = self.s2 + self.g * v1;
        let next = 2.0 * v1 - self.s1;
        let saturation = drive.max((res - 0.6) * 2.5).min(1.0);
        self.s1 = flush(if saturation > 0.0 {
            mix(next, (next / 1.5).tanh() * 1.5, saturation)
        } else {
            next
        });
        self.s2 = flush(2.0 * v2 - self.s2);
        [
            sample(v2),
            sample(v1),
            sample(input - self.k * v1 - v2),
            sample(input - self.k * v1),
        ]
    }
}
block_opcode!(MultimodeFilter, "multimode_filter_dsp", 4, 4);
