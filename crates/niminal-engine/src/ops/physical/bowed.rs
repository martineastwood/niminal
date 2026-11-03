use super::{Dc, Waveguide, control, loss_delay, smooth};
use crate::denormals::flush;
use crate::opcode::{Opcode, Port, ProcessCtx};

/// Two string sections joined by a velocity-dependent bow friction junction.
#[derive(Clone)]
pub struct Bowed {
    sr: f32,
    bridge: Waveguide,
    neck: Waveguide,
    loss: f64,
    dc: Dc,
    dc_pole: f64,
    smoothing: f64,
    controls: [f64; 5],
    initialized: bool,
    cached_tuning: (f64, f64),
    pole: f64,
    delay: f64,
}

impl Default for Bowed {
    fn default() -> Self {
        Self::new(48000.0)
    }
}
impl Bowed {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            bridge: Waveguide::new(sr),
            neck: Waveguide::new(sr),
            loss: 0.0,
            dc: Dc::default(),
            dc_pole: (-std::f64::consts::TAU * 10.0 / f64::from(sr)).exp(),
            smoothing: 1.0 - (-1.0 / (0.002 * f64::from(sr))).exp(),
            controls: [0.0; 5],
            initialized: false,
            cached_tuning: (f64::NAN, f64::NAN),
            pole: 0.0,
            delay: 0.0,
        }
    }

    fn step(&mut self, x: &[f32]) -> f32 {
        if !x[0].is_finite() || x[0] <= 0.0 {
            return 0.0;
        }
        let sr = f64::from(self.sr);
        let target = [
            f64::from(x[0]).clamp(20.0, (sr / 12.0).min(3000.0)).ln(),
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
        let [log_freq, pressure, position, velocity, brightness] = self.controls;
        if (log_freq, brightness) != self.cached_tuning {
            let freq = log_freq.exp();
            let omega = std::f64::consts::TAU * freq / sr;
            let cutoff = (1200.0 + brightness * 10000.0).min(sr * 0.4);
            self.pole = (-std::f64::consts::TAU * cutoff / sr).exp();
            self.delay = sr / freq - loss_delay(self.pole, omega);
            self.cached_tuning = (log_freq, brightness);
        }
        let pole = self.pole;
        let period = self.delay;
        // At high notes, keep each section at least one sample long while
        // preserving the total period (rather than detuning the string).
        let position = (0.05 + 0.4 * position).max(1.0 / period);
        let bridge = self.bridge.read(period * position);
        self.loss = flush((1.0 - pole) * bridge + pole * self.loss);
        let from_bridge = -0.995 * self.loss;
        let from_neck = -0.998 * self.neck.read(period * (1.0 - position));
        let relative = 0.25 * velocity - (from_bridge + from_neck);
        // The bow sticks at low relative velocity and slips as speed grows.
        // Pressure widens the sticking region; zero pressure lifts the bow.
        // Once the bow contacts the string, near-unit coupling permits
        // sticking. Scaling its maximum by pressure instead causes persistent
        // slip and high-register jumps even under ordinary playing controls.
        let contact = (pressure * 8.0).min(1.0);
        let friction = contact
            * (0.75 + relative.abs() * (5.0 - 4.0 * pressure))
                .powi(-4)
                .min(0.98);
        let force = relative * friction;
        self.bridge.push(from_neck + force);
        self.neck.push(from_bridge + force);
        self.dc.step(bridge, self.dc_pole)
    }
}

impl Opcode for Bowed {
    fn name(&self) -> &str {
        "bowed"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("freq"),
            Port::optional("pressure", 0.7),
            Port::optional("position", 0.25),
            Port::optional("velocity", 0.6),
            Port::optional("brightness", 0.5),
        ];
        PORTS
    }
    fn prepare(&mut self, sr: f32) {
        *self = Self::new(sr.max(1000.0));
    }
    fn reset(&mut self) {
        self.bridge.reset();
        self.neck.reset();
        self.loss = 0.0;
        self.dc = Dc::default();
        self.initialized = false;
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
