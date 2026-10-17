use crate::opcode::{BLOCK, Opcode, Port, ProcessCtx};

struct Layer {
    op: Box<dyn Opcode>,
    inputs: Vec<[f32; BLOCK]>,
    age: u64,
    duration: u64,
    serial: u64,
    active: bool,
}

/// A fixed pool of independently reset processors. Inputs are captured on a rising edge.
pub struct Spawn {
    template: Box<dyn Opcode>,
    layers: Vec<Layer>,
    ports: Vec<Port>,
    indices: Vec<usize>,
    scratch: Vec<[f32; BLOCK]>,
    positive: bool,
    serial: u64,
}

impl Spawn {
    pub fn new(template: Box<dyn Opcode>, max: usize) -> Self {
        assert!((1..=64).contains(&max));
        let mut ports = template.ports().to_vec();
        let inputs = ports.len();
        ports.extend([Port::required("__trigger"), Port::required("__duration")]);
        let layers = (0..max)
            .map(|_| Layer {
                op: template.box_clone(),
                inputs: vec![[0.0; BLOCK]; inputs],
                age: 0,
                duration: 0,
                serial: 0,
                active: false,
            })
            .collect();
        Self {
            scratch: vec![[0.0; BLOCK]; template.outputs()],
            template,
            layers,
            ports,
            indices: (0..inputs).collect(),
            positive: false,
            serial: 0,
        }
    }
}

impl Opcode for Spawn {
    fn name(&self) -> &str {
        "spawn"
    }
    fn ports(&self) -> &[Port] {
        &self.ports
    }
    fn latency(&self) -> usize { self.template.latency() }
    fn outputs(&self) -> usize {
        self.template.outputs()
    }
    fn max_inputs(&self) -> usize {
        self.ports.len()
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.template.box_clone(), self.layers.len()))
    }
    fn prepare(&mut self, sample_rate: f32) {
        for layer in &mut self.layers {
            layer.op.prepare(sample_rate);
        }
    }
    fn reset(&mut self) {
        self.positive = false;
        self.serial = 0;
        for layer in &mut self.layers {
            layer.active = false;
            layer.op.reset();
        }
    }
    fn is_envelope(&self) -> bool {
        true
    }
    fn is_active(&self) -> bool {
        self.layers.iter().any(|l| l.active)
    }
    fn has_active_envelope(&self) -> bool {
        self.is_active()
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], _: &mut [f32]) {
        unreachable!("spawn uses graph slots")
    }
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[[f32; BLOCK]],
        indices: &[usize],
        outs: &mut [[f32; BLOCK]],
        frames: usize,
    ) {
        for out in outs.iter_mut() {
            out[..frames].fill(0.0);
        }
        let n = self.indices.len();
        for i in 0..frames {
            let positive = slots[indices[n]][i] > 0.0;
            if ctx.gate
                && positive
                && !self.positive
                && let Some(free) = self.layers.iter().position(|l| !l.active)
            {
                let layer = &mut self.layers[free];
                layer.op.reset();
                layer.age = 0;
                layer.serial = self.serial;
                self.serial = self.serial.wrapping_add(1);
                let duration = slots[indices[n + 1]][i];
                layer.duration = if duration.is_finite() && duration > 0.0 {
                    (f64::from(duration.min(600.0)) * f64::from(ctx.sample_rate))
                        .round()
                        .max(1.0) as u64
                } else {
                    1
                };
                layer.active = true;
                for (input, &index) in layer.inputs.iter_mut().zip(indices) {
                    input[0] = slots[index][i];
                }
            }
            self.positive = positive;
            for layer in &mut self.layers {
                if !layer.active {
                    continue;
                }
                let gate = ctx.gate && layer.age < layer.duration;
                let sub = ProcessCtx {
                    gate,
                    dur: layer.duration as f32 / ctx.sample_rate,
                    index: layer.serial as u32,
                    ..*ctx
                };
                layer
                    .op
                    .process_slots(&sub, &layer.inputs, &self.indices, &mut self.scratch, 1);
                for (out, scratch) in outs.iter_mut().zip(&self.scratch) {
                    out[i] += scratch[0];
                }
                layer.age += 1;
                if !gate && !layer.op.is_active() {
                    layer.active = false;
                }
            }
        }
    }
}
