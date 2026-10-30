use crate::opcode::{KernelStats, Opcode, Port, ProcessCtx, blocks, control_step};

/// A DSP body that runs once every [`control_step`] samples: `control { ... }`. Its inputs are read
/// when it runs, its state advances by one step's time, and each result is held until the next run.
/// A step can start anywhere in a block, and a block can be shorter than a step.
pub struct ControlRate {
    op: Box<dyn Opcode>,
    inputs: Vec<Vec<f32>>,
    indices: Vec<usize>,
    scratch: Vec<Vec<f32>>,
    step: usize,
    /// Samples since the last run. Starts due, so the first call runs.
    since: usize,
    held: Vec<f32>,
}

impl ControlRate {
    pub fn new(op: Box<dyn Opcode>) -> Self {
        let inputs = op.ports().len();
        let outputs = op.outputs();
        let step = control_step();
        Self {
            op,
            inputs: blocks(inputs),
            indices: (0..inputs).collect(),
            scratch: blocks(outputs),
            step,
            since: step,
            held: vec![0.0; outputs],
        }
    }
}

impl Opcode for ControlRate {
    fn buffer_channels(&self, key: u64) -> usize {
        self.op.buffer_channels(key)
    }
    fn buffer_bindings(&self, key: u64) -> usize {
        self.op.buffer_bindings(key)
    }
    fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank], cursor: &mut usize) {
        self.op.swap_buffers(key, banks, cursor);
    }
    fn reports(&self, visit: &mut dyn FnMut(&std::sync::Arc<crate::ops::Report>)) {
        self.op.reports(visit);
    }
    fn name(&self) -> &str {
        "control"
    }
    fn ports(&self) -> &[Port] {
        self.op.ports()
    }
    fn outputs(&self) -> usize {
        self.op.outputs()
    }
    fn max_inputs(&self) -> usize {
        self.op.max_inputs()
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.op.box_clone()))
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.op.prepare(sample_rate / self.step as f32);
    }
    fn reset(&mut self) {
        self.op.reset();
        self.since = self.step;
        self.held.fill(0.0);
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.op.carry_state(&mut *old.op);
            self.since = old.since;
            self.held.copy_from_slice(&old.held);
        }
    }
    fn is_envelope(&self) -> bool {
        self.op.is_envelope()
    }
    fn has_active_envelope(&self) -> bool {
        self.op.has_active_envelope()
    }
    fn is_active(&self) -> bool {
        self.op.is_active()
    }
    fn has_side_effects(&self) -> bool {
        self.op.has_side_effects()
    }
    fn input_is_init(&self, index: usize) -> bool {
        self.op.input_is_init(index)
    }
    fn latency(&self) -> usize {
        self.op.latency()
    }
    fn kernel_stats(&self) -> Option<KernelStats> {
        self.op.kernel_stats().map(|stats| KernelStats {
            control_scope: true,
            ..stats
        })
    }
    fn control_warnings(&self, sample_rate: f32) -> Vec<String> {
        self.op.step_warnings(self.step as f32 / sample_rate)
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], _: &mut [f32]) {
        unreachable!("a control scope uses graph slots")
    }
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[Vec<f32>],
        indices: &[usize],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        if frames == 0 {
            return;
        }
        let mut at = 0;
        while at < frames {
            if self.since >= self.step {
                self.since = 0;
                // the body counts in steps, so its clock does too: a time read there is where this step is
                let sub = ProcessCtx {
                    sample_rate: ctx.sample_rate / self.step as f32,
                    clock: crate::opcode::Clock {
                        sample: (ctx.clock.sample + at as u64) / self.step as u64,
                    },
                    ..*ctx
                };
                for (input, &index) in self.inputs.iter_mut().zip(indices) {
                    input[0] = slots[index][at];
                }
                self.op
                    .process_slots(&sub, &self.inputs, &self.indices, &mut self.scratch, 1);
                for (held, new) in self.held.iter_mut().zip(&self.scratch) {
                    *held = new[0];
                }
            }
            let run = (frames - at).min(self.step - self.since);
            for (out, held) in outs.iter_mut().zip(&self.held) {
                out[at..at + run].fill(*held);
            }
            self.since += run;
            at += run;
        }
    }
}

/// `x.smooth`: a control value `x`, held between steps, ramped linearly to each new value over one
/// control step so a stepped control doesn't zipper.
#[derive(Clone)]
pub struct Ramp {
    step: usize,
    from: f32,
    to: f32,
    /// Samples into the ramp to `to`.
    pos: usize,
    level: f32,
    started: bool,
}

impl Default for Ramp {
    fn default() -> Self {
        Ramp {
            step: control_step(),
            from: 0.0,
            to: 0.0,
            pos: 0,
            level: 0.0,
            started: false,
        }
    }
}

impl Opcode for Ramp {
    fn reset(&mut self) {
        self.started = false;
    }

    fn name(&self) -> &'static str {
        "smooth"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Ramp::default())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.from = old.from;
            self.to = old.to;
            self.pos = old.pos;
            self.level = old.level;
            self.started = old.started;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (&target, o) in inputs[0].iter().zip(out.iter_mut()) {
            if !self.started {
                (self.started, self.from, self.to, self.level) = (true, target, target, target);
                self.pos = self.step;
            } else if target != self.to {
                // a new value: ramp to it from wherever the output is now
                (self.from, self.to, self.pos) = (self.level, target, 0);
            }
            self.pos = (self.pos + 1).min(self.step);
            self.level = self.from + (self.to - self.from) * (self.pos as f32 / self.step as f32);
            *o = self.level;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(op: &mut impl Opcode, x: f32, n: usize) -> Vec<f32> {
        let input = vec![x; n];
        let mut out = vec![0.0; n];
        op.process(&ProcessCtx::new(48_000.0, true), &[&input], &mut out);
        out
    }

    #[test]
    fn a_ramp_reaches_each_new_value_after_one_control_step() {
        let mut ramp = Ramp::default();
        assert!(run(&mut ramp, 1.0, 8).iter().all(|&v| v == 1.0));
        // blocks smaller than a step ramp across several calls
        let mut out = Vec::new();
        for _ in 0..control_step() / 8 {
            out.extend(run(&mut ramp, 5.0, 8));
        }
        assert!(out.windows(2).all(|w| w[1] > w[0]));
        assert!(out[0] > 1.0 && out[0] < 1.2);
        assert_eq!(*out.last().unwrap(), 5.0);
        assert_eq!(run(&mut ramp, 5.0, 8), [5.0; 8]);
    }
}
