use super::*;

/// A bounded record/reverse/freeze ring. The extra endpoint sample preserves shelf timing.
#[derive(Clone)]
pub struct FreezeReverse {
    pub(super) sample_rate: f64,
    buffer: Vec<f64>,
    write: usize,
    phase: f64,
    anchor: usize,
    mode: u8,
    fade: f64,
}
impl FreezeReverse {
    pub fn new(size: usize) -> Self {
        Self {
            sample_rate: 48000.0,
            buffer: vec![0.0; size],
            write: 0,
            phase: 0.0,
            anchor: 0,
            mode: 0,
            fade: 0.0,
        }
    }
    fn input_ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("length", 0.25),
            Port::optional("reverse", 0.0),
            Port::optional("freeze", 0.0),
        ];
        PORTS
    }
    fn fresh(&self) -> Self {
        Self::new(self.buffer.len())
    }
    pub(super) fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
        self.phase = 0.0;
        self.anchor = 0;
        self.mode = 0;
        self.fade = 0.0;
    }
    pub(super) fn carry(&mut self, old: &mut Self) {
        if self.buffer.len() == old.buffer.len() {
            std::mem::swap(&mut self.buffer, &mut old.buffer);
            self.write = old.write;
            self.phase = old.phase;
            self.anchor = old.anchor;
            self.mode = old.mode;
            self.fade = old.fade;
        }
    }
    fn read(&self, position: f64) -> f64 {
        self.buffer[crate::wrapped_index(position, self.buffer.len())]
    }
    pub(super) fn tick(&mut self, input: f64, length: f64, reverse: bool, freeze: bool) -> f64 {
        let input = input.clamp(-16.0, 16.0);
        let len = (length.max(0.001) * self.sample_rate)
            .floor()
            .max(2.0)
            .min(self.buffer.len() as f64 / 2.0 - 2.0);
        let fade_samples = (0.005 * self.sample_rate).floor().max(2.0);
        let want = if freeze {
            2
        } else if reverse {
            1
        } else {
            0
        };
        let switch = want != self.mode && self.fade <= 0.0;
        if switch {
            self.mode = want;
        }
        if switch || self.phase >= len {
            self.anchor = self.write;
            self.phase = 0.0;
        } else {
            self.phase += 1.0;
        }
        self.fade = if want == self.mode && self.mode > 0 {
            (self.fade + 1.0 / fade_samples).min(1.0)
        } else {
            (self.fade - 1.0 / fade_samples).max(0.0)
        };
        if self.mode != 2 {
            self.buffer[self.write] = input;
            self.write = (self.write + 1) % self.buffer.len();
        }
        if self.fade <= 0.0 {
            return input;
        }
        let anchor = self.anchor as f64;
        let wet = if self.mode == 2 {
            let join = ((self.phase - (len - fade_samples)) / fade_samples).clamp(0.0, 1.0);
            mix(
                self.read(anchor - len + self.phase),
                self.read(anchor - 2.0 * len + self.phase),
                join,
            )
        } else {
            let edge = (self.phase.min(len - 1.0 - self.phase) / fade_samples).clamp(0.0, 1.0);
            self.read(anchor - 1.0 - self.phase) * edge
        };
        mix(input, wet, self.fade)
    }
    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 1] {
        [self.tick(
            finite(x[0]),
            finite(x[1]),
            finite(x[2]) > 0.5,
            finite(x[3]) > 0.5,
        ) as f32]
    }
}
block_opcode!(FreezeReverse, "freeze_reverse_dsp", 1, 4);
