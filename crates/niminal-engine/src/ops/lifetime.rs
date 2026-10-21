use crate::opcode::{Opcode, Port, ProcessCtx};

/// A custom effect's completion participates in the same lifetime rules as an envelope.
#[derive(Default)]
pub struct FinishWhen {
    done: bool,
}

impl Opcode for FinishWhen {
    fn name(&self) -> &str {
        "finish_when"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::required("done")];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::default())
    }
    fn reset(&mut self) {
        self.done = false;
    }
    fn is_envelope(&self) -> bool {
        true
    }
    fn is_active(&self) -> bool {
        !self.done
    }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            self.done |= inputs[1][i] > 0.0;
            *o = if self.done { 0.0 } else { inputs[0][i] };
        }
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.done = old.done;
        }
    }
}
