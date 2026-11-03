use super::{Dc, Waveguide, control, loss_delay, smooth};
use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// A cylindrical, inverting bore terminated by a pressure-dependent reed.
#[derive(Clone)]
pub struct Reed {
    sr: f32,
    bore: Waveguide,
    loss: f64,
    dc: Dc,
    dc_pole: f64,
    smoothing: f64,
    controls: [f64; 5],
    initialized: bool,
    cached_tuning: (f64, f64),
    pole: f64,
    delay: f64,
    noise: u32,
}
impl Default for Reed {
    fn default() -> Self {
        Self::new(48000.0)
    }
}
impl Reed {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            bore: Waveguide::new(sr),
            loss: 0.0,
            dc: Dc::default(),
            dc_pole: (-std::f64::consts::TAU * 10.0 / f64::from(sr)).exp(),
            smoothing: 1.0 - (-1.0 / (0.002 * f64::from(sr))).exp(),
            controls: [0.0; 5],
            initialized: false,
            cached_tuning: (f64::NAN, f64::NAN),
            pole: 0.0,
            delay: 0.0,
            noise: 0x71e4_8a93,
        }
    }
    fn step(&mut self, x: &[f32]) -> f32 {
        if !x[0].is_finite() || x[0] <= 0.0 {
            return 0.0;
        }
        let sr = f64::from(self.sr);
        let target = [
            f64::from(x[0]).clamp(20.0, (sr / 16.0).min(4000.0)).ln(),
            control(x[1]),
            control(x[2]),
            control(x[3]),
            control(x[4]),
        ];
        if !self.initialized {
            self.controls = target;
            self.initialized = true;
        } else {
            for (state, target) in self.controls.iter_mut().zip(target) {
                smooth(state, target, self.smoothing);
            }
        }
        let [log_freq, pressure, stiffness, breath, brightness] = self.controls;
        if (log_freq, brightness) != self.cached_tuning {
            let freq = log_freq.exp();
            let omega = std::f64::consts::TAU * freq / sr;
            let cutoff = (1500.0 + brightness * 8000.0).min(sr * 0.4);
            self.pole = (-std::f64::consts::TAU * cutoff / sr).exp();
            self.delay = sr / (2.0 * freq) - loss_delay(self.pole, omega);
            self.cached_tuning = (log_freq, brightness);
        }
        let pole = self.pole;
        let delay = self.delay;
        let reflected = self.bore.read(delay);
        self.loss = flush((1.0 - pole) * reflected + pole * self.loss);
        let returning = -0.995 * self.loss;
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        let noise = f64::from(self.noise) / f64::from(u32::MAX) * 2.0 - 1.0;
        let mouth = pressure * 1.1 * (1.0 + noise * breath * 0.15);
        let difference = returning - mouth;
        // A stiffer reed deforms less: it needs more pressure to speak and
        // tolerates a higher blowing pressure before closing completely.
        let reflection = (0.65 - (0.5 - 0.3 * stiffness) * difference).clamp(-1.0, 1.0);
        self.bore.push(mouth + difference * reflection);
        self.dc.step(reflected, self.dc_pole)
    }
}
impl Opcode for Reed {
    fn name(&self) -> &str {
        "reed"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("freq"),
            Port::optional("pressure", 0.65),
            Port::optional("stiffness", 0.5),
            Port::optional("breath", 0.1),
            Port::optional("brightness", 0.5),
        ];
        PORTS
    }
    fn prepare(&mut self, sr: f32) {
        *self = Self::new(sr.max(1000.0));
    }
    fn reset(&mut self) {
        self.bore.reset();
        self.loss = 0.0;
        self.dc = Dc::default();
        self.initialized = false;
        self.noise = 0x71e4_8a93;
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.sr))
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.sr == old.sr
        {
            std::mem::swap(self, old);
        }
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let mut x = [0.0; 5];
        for (i, o) in out.iter_mut().enumerate() {
            for (a, b) in x.iter_mut().zip(inputs) {
                *a = b[i];
            }
            *o = self.step(&x);
        }
    }
    fn has_process_one(&self) -> bool {
        true
    }
    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(inputs)
    }
}
