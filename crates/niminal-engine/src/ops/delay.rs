use crate::opcode::{Opcode, Port, ProcessCtx};

/// Feedback delay with linear interpolation. Output is the delayed signal
/// only; add the dry signal yourself for a mix.
///
/// The buffer is sized once, from `max_seconds`, so no allocation happens
/// while processing. `time` is clamped to between one sample and that maximum,
/// and `feedback` to 0..0.995 so the loop always decays.
#[derive(Clone)]
pub struct Delay {
    max_seconds: f32,
    max_samples: f64,
    buf: Vec<f32>,
    write: usize,
    sample_rate: f32,
}

impl Delay {
    pub fn new(max_seconds: f32) -> Self {
        let mut d = Delay { max_seconds, max_samples: 0.0, buf: Vec::new(), write: 0, sample_rate: 48_000.0 };
        d.prepare(48_000.0);
        d
    }
}

impl Opcode for Delay {
    fn name(&self) -> &str {
        "delay"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::required("time"), Port::optional("feedback", 0.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Delay::new(self.max_seconds))
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let max = (self.max_seconds * sample_rate).round().max(1.0);
        self.max_samples = f64::from(max);
        // room for the longest delay, one sample of interpolation, and the write head
        self.buf = vec![0.0; max as usize + 3];
        self.write = 0;
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let len = self.buf.len();
        let longest = self.max_samples;
        for (i, o) in out.iter_mut().enumerate() {
            let exact = f64::from(inputs[1][i]) * f64::from(self.sample_rate);
            // A time like 10ms isn't exactly 480 samples in floating point; snap
            // when within a thousandth of a sample so round times land cleanly.
            let snapped = exact.round();
            let samples = if (exact - snapped).abs() < 1e-3 { snapped } else { exact }.clamp(1.0, longest);
            let whole = samples.floor();
            let frac = (samples - whole) as f32;
            let whole = whole as usize;

            let newer = self.buf[(self.write + len - whole) % len];
            let older = self.buf[(self.write + len - whole - 1) % len];
            let delayed = newer + (older - newer) * frac;

            let feedback = inputs[2][i].clamp(0.0, 0.995);
            self.buf[self.write] = inputs[0][i] + feedback * delayed;
            self.write = (self.write + 1) % len;
            *o = delayed;
        }
    }
}
