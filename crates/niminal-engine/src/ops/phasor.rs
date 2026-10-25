use crate::opcode::{Opcode, Port, ProcessCtx};

fn wrap(x: f64) -> f64 {
    x - x.floor()
}

/// A normalized phase ramp for table readers and custom oscillators. It is not an audio waveform.
#[derive(Clone)]
pub struct Phasor {
    phase: f64,
    initial_phase: Option<f64>,
    sync_phase: f64,
    sample_rate: f32,
}

impl Default for Phasor {
    fn default() -> Self {
        Self {
            phase: 0.0,
            initial_phase: None,
            sync_phase: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

impl Phasor {
    fn step(&mut self, freq: f32, phase: f32, sync: f32) -> f32 {
        if self.initial_phase.is_none() {
            let initial = wrap(f64::from(phase));
            self.initial_phase = Some(initial);
            self.phase = initial;
        }
        let initial = self.initial_phase.unwrap_or(0.0);
        let out = (self.phase as f32).min(1.0_f32.next_down());
        let inc = f64::from(freq) / f64::from(self.sample_rate);
        if sync > 0.0 {
            let master_inc = (f64::from(sync) / f64::from(self.sample_rate)).min(0.5);
            self.phase = if self.sync_phase + master_inc >= 1.0 {
                let time = (1.0 - self.sync_phase) / master_inc;
                wrap(initial + inc * (1.0 - time))
            } else {
                wrap(self.phase + inc)
            };
            self.sync_phase = wrap(self.sync_phase + master_inc);
        } else {
            self.sync_phase = 0.0;
            self.phase = wrap(self.phase + inc);
        }
        out
    }
}

impl Opcode for Phasor {
    fn name(&self) -> &'static str {
        "phasor"
    }
    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::required("freq"),
            Port::optional("phase", 0.0),
            Port::optional("sync", 0.0),
        ];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }
    fn reset(&mut self) {
        self.phase = 0.0;
        self.initial_phase = None;
        self.sync_phase = 0.0;
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.phase = old.phase;
            self.initial_phase = old.initial_phase;
            self.sync_phase = old.sync_phase;
        }
    }
    fn has_process_one(&self) -> bool {
        true
    }
    fn process_one(&mut self, _ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.step(
            inputs[0],
            inputs.get(1).copied().unwrap_or(0.0),
            inputs.get(2).copied().unwrap_or(0.0),
        )
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, sample) in out.iter_mut().enumerate() {
            *sample = self.step(
                inputs[0][i],
                inputs.get(1).map_or(0.0, |p| p[i]),
                inputs.get(2).map_or(0.0, |p| p[i]),
            );
        }
    }
}
