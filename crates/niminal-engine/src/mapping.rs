use crate::{Buses, Opcode, ProcessCtx, ops::Control};

/// Prepared modulation connection. Its storage moves between commands and voices without freeing on audio.
pub struct ParamMapping {
    source: Source,
    scale: f32,
    offset: f32,
    range: Option<(f32, f32)>,
}
enum Source {
    Control(Control),
    Bus { bus: Option<usize>, channel: usize },
}
impl ParamMapping {
    pub fn control(key: String, initial: f32, scale: f32, offset: f32) -> Self {
        Self {
            source: Source::Control(Control::new(key, initial)),
            scale,
            offset,
            range: None,
        }
    }
    pub fn bus(bus: usize, channel: usize, scale: f32, offset: f32) -> Self {
        Self {
            source: Source::Bus {
                bus: Some(bus),
                channel,
            },
            scale,
            offset,
            range: None,
        }
    }
    pub fn with_range(mut self, range: Option<(f32, f32)>) -> Self {
        self.range = range;
        self
    }
    pub(crate) fn remap(&mut self, mapping: &mut impl FnMut(usize) -> Option<usize>) -> bool {
        if let Source::Bus { bus, .. } = &mut self.source {
            *bus = bus.and_then(mapping);
            return bus.is_none();
        }
        false
    }
    pub(crate) fn set_control(
        &mut self,
        key: &str,
        start: f32,
        target: f32,
        frames: u64,
        elapsed: u64,
    ) {
        if let Source::Control(control) = &mut self.source {
            control.set_control(key, start, target, frames, elapsed);
        }
    }
    pub(crate) fn process(&mut self, ctx: &ProcessCtx, buses: &Buses, output: &mut [f32]) {
        match &mut self.source {
            Source::Control(control) => control.process(ctx, &[], output),
            Source::Bus {
                bus: Some(bus),
                channel,
            } => {
                for (i, out) in output.iter_mut().enumerate() {
                    *out = buses.previous(*bus, *channel, i);
                }
            }
            Source::Bus { bus: None, .. } => return,
        }
        for out in output {
            *out = *out * self.scale + self.offset;
            if let Some((lo, hi)) = self.range {
                *out = out.clamp(lo, hi);
            }
        }
    }
}
