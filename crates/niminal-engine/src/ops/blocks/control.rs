use super::*;

#[derive(Clone)]
pub struct Contour {
    sample_rate: f64,
    level: f64,
    stage: u8,
    previous: bool,
}
impl Default for Contour {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            level: 0.0,
            stage: 0,
            previous: false,
        }
    }
}
impl Contour {
    fn input_ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::optional("trigger", 0.0),
            Port::optional("rise", 0.01),
            Port::optional("fall", 0.3),
            Port::optional("curve", 1.0),
            Port::optional("cycle", 0.0),
        ];
        PORTS
    }
    fn fresh(&self) -> Self {
        Self::default()
    }
    fn clear(&mut self) {
        self.level = 0.0;
        self.stage = 0;
        self.previous = false;
    }
    fn carry(&mut self, old: &mut Self) {
        self.level = old.level;
        self.stage = old.stage;
        self.previous = old.previous;
    }
    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 3] {
        let high = finite(x[0]) > 0.0;
        if (high && !self.previous) || (self.stage == 0 && finite(x[4]) > 0.0) {
            self.stage = 1;
        }
        self.previous = high;
        let mut eor = 0.0;
        let mut eoc = 0.0;
        if self.stage == 1 {
            self.level = (self.level
                + 1.0
                    / f64::from(bounded(x[1], 0.0, 60.0) as f32 * self.sample_rate as f32)
                        .max(1.0))
            .min(1.0);
            if self.level >= 1.0 {
                self.stage = 2;
                eor = 1.0;
            }
        } else if self.stage == 2 {
            self.level = (self.level
                - 1.0
                    / f64::from(bounded(x[2], 0.0, 60.0) as f32 * self.sample_rate as f32)
                        .max(1.0))
            .max(0.0);
            if self.level <= 0.0 {
                self.stage = 0;
                eoc = 1.0;
            }
        }
        [self.level.powf(bounded(x[3], 0.1, 10.0)) as f32, eor, eoc]
    }
}
block_opcode!(Contour, "contour_dsp", 3, 5);

#[derive(Clone)]
pub struct Slew {
    sample_rate: f64,
    value: f64,
    cached_time: f64,
    coefficient: f64,
}
impl Default for Slew {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            value: 0.0,
            cached_time: f64::NAN,
            coefficient: 1.0,
        }
    }
}
impl Slew {
    fn input_ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("rise", 0.01),
            Port::optional("fall", 0.1),
            Port::optional("linear", 0.0),
        ];
        PORTS
    }
    fn fresh(&self) -> Self {
        Self::default()
    }
    fn clear(&mut self) {
        self.value = 0.0;
        self.cached_time = f64::NAN;
    }
    fn carry(&mut self, old: &mut Self) {
        self.value = old.value;
    }
    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 1] {
        let target = bounded(x[0], -1e6, 1e6);
        let diff = target - self.value;
        let rising = (diff * 1e9).clamp(-1.0, 1.0) * 0.5 + 0.5;
        let time = mix(bounded(x[2], 0.0, 60.0), bounded(x[1], 0.0, 60.0), rising);
        let samples = time * self.sample_rate;
        if samples <= 0.0 {
            self.value = target;
        } else if finite(x[3]) > 0.5 {
            let step = 1.0 / samples.max(1.0);
            self.value += diff.clamp(-step, step);
        } else {
            if time != self.cached_time {
                self.cached_time = time;
                self.coefficient = 1.0 - (-1.0 / samples.max(0.0001)).exp();
            }
            self.value += diff * self.coefficient;
        }
        [self.value as f32]
    }
}
block_opcode!(Slew, "slew_dsp", 1, 4);
