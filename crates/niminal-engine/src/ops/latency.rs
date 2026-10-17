use crate::{Opcode, Port, ProcessCtx};

/// An exact, fixed sample delay used to align parallel processing paths.
pub struct SampleDelay {
    samples: Vec<f32>,
    pos: usize,
    warm: bool,
    started: bool,
}
impl SampleDelay {
    pub fn new(samples: usize) -> Self {
        Self {
            samples: vec![0.0; samples],
            pos: 0,
            warm: false,
            started: false,
        }
    }
    pub fn warm(samples: usize) -> Self {
        Self {
            warm: true,
            ..Self::new(samples)
        }
    }
}
impl Opcode for SampleDelay {
    fn name(&self) -> &str {
        "latency_delay"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(if self.warm {
            Self::warm(self.samples.len())
        } else {
            Self::new(self.samples.len())
        })
    }
    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.pos = 0;
        self.started = false;
    }
    fn latency(&self) -> usize {
        self.samples.len()
    }
    fn is_active(&self) -> bool {
        !self.warm
            && self
                .samples
                .iter()
                .any(|x| x.abs() > crate::opcode::TAIL_LEVEL)
    }
    fn process(&mut self, _: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if self.samples.is_empty() {
            out.copy_from_slice(inputs[0]);
            return;
        }
        if self.warm
            && !self.started
            && let Some(&first) = inputs[0].first()
        {
            self.samples.fill(first);
            self.started = true;
        }
        for (out, input) in out.iter_mut().zip(inputs[0]) {
            *out = self.samples[self.pos];
            self.samples[self.pos] = *input;
            self.pos = (self.pos + 1) % self.samples.len();
        }
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.samples.len() == old.samples.len()
        {
            self.samples.copy_from_slice(&old.samples);
            self.pos = old.pos;
            self.started = old.started;
        }
    }
}
