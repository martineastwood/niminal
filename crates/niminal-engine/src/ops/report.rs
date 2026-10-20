//! Fixed-size DSP reports shared with independent control-thread readers.
use crate::{Opcode, Port, ProcessCtx};
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};
const CAPACITY: usize = 256;
struct Event {
    sequence: AtomicU64,
    sample: AtomicU64,
    token: AtomicU64,
    instance: AtomicU64,
    values: Box<[AtomicU32]>,
}
/// (sample, token, instance, values)
pub type ReportEvent = (u64, u64, u64, Vec<f32>);

pub struct Report {
    pub name: String,
    pub width: usize,
    cursor: AtomicU64,
    events: Box<[Event]>,
}
impl Report {
    pub fn new(name: String, width: usize) -> Arc<Self> {
        Arc::new(Self {
            name,
            width,
            cursor: AtomicU64::new(0),
            events: (0..CAPACITY)
                .map(|_| Event {
                    sequence: AtomicU64::new(0),
                    sample: AtomicU64::new(0),
                    token: AtomicU64::new(0),
                    instance: AtomicU64::new(0),
                    values: (0..width).map(|_| AtomicU32::new(0)).collect(),
                })
                .collect(),
        })
    }
    fn write(&self, sample: u64, token: u64, instance: u64, inputs: &[&[f32]], index: usize) {
        if inputs[..self.width]
            .iter()
            .any(|input| !input[index].is_finite())
        {
            return;
        }
        let cursor = self.cursor.load(Ordering::Relaxed);
        let event = &self.events[cursor as usize % CAPACITY];
        event.sequence.store(cursor * 2 + 1, Ordering::Release);
        event.sample.store(sample, Ordering::Relaxed);
        event.token.store(token, Ordering::Relaxed);
        event.instance.store(instance, Ordering::Relaxed);
        for (out, input) in event.values.iter().zip(inputs) {
            out.store(input[index].to_bits(), Ordering::Relaxed);
        }
        event.sequence.store(cursor * 2 + 2, Ordering::Release);
        self.cursor.store(cursor + 1, Ordering::Release);
    }
    pub fn read(&self, cursor: &mut u64) -> (u64, Vec<ReportEvent>) {
        let end = self.cursor.load(Ordering::Acquire);
        let dropped = end.saturating_sub(*cursor).saturating_sub(CAPACITY as u64);
        *cursor = (*cursor).max(end.saturating_sub(CAPACITY as u64));
        let mut result = Vec::new();
        while *cursor < end {
            let event = &self.events[*cursor as usize % CAPACITY];
            let expected = *cursor * 2 + 2;
            if event.sequence.load(Ordering::Acquire) != expected {
                break;
            }
            let sample = event.sample.load(Ordering::Relaxed);
            let token = event.token.load(Ordering::Relaxed);
            let instance = event.instance.load(Ordering::Relaxed);
            let values = event
                .values
                .iter()
                .map(|v| f32::from_bits(v.load(Ordering::Relaxed)))
                .collect();
            std::sync::atomic::fence(Ordering::Acquire);
            if event.sequence.load(Ordering::Relaxed) != expected {
                break;
            }
            result.push((sample, token, instance, values));
            *cursor += 1;
        }
        (dropped, result)
    }
}
pub struct ReportOut {
    report: Arc<Report>,
    ports: Vec<Port>,
    triggered: bool,
    rate: f64,
    positive: bool,
    next: u64,
}
impl ReportOut {
    pub fn new(report: Arc<Report>, triggered: bool, rate: f64) -> Self {
        let mut ports = (0..report.width)
            .map(|i| Port::named(format!("x{i}"), None))
            .collect::<Vec<_>>();
        if triggered {
            ports.push(Port::required("on"));
        }
        Self {
            report,
            ports,
            triggered,
            rate,
            positive: false,
            next: 0,
        }
    }
}
impl Opcode for ReportOut {
    fn name(&self) -> &str {
        "report"
    }
    fn ports(&self) -> &[Port] {
        &self.ports
    }
    fn max_inputs(&self) -> usize {
        65
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.report.clone(), self.triggered, self.rate))
    }
    fn has_side_effects(&self) -> bool {
        true
    }
    fn reports(&self, visit: &mut dyn FnMut(&Arc<Report>)) {
        visit(&self.report);
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        out.copy_from_slice(inputs[0]);
        for i in 0..out.len() {
            let sample = ctx.clock.sample + i as u64;
            let fire = if self.triggered {
                let positive = inputs[self.report.width][i] > 0.;
                let fire = positive && !self.positive;
                self.positive = positive;
                fire
            } else {
                sample >= self.next
            };
            if fire {
                self.report
                    .write(sample, ctx.note_token, ctx.instance, inputs, i);
                self.next =
                    sample + (f64::from(ctx.sample_rate) / self.rate).round().max(1.) as u64;
            }
        }
    }
    fn process_slots(
        &mut self,
        ctx: &ProcessCtx,
        slots: &[[f32; crate::BLOCK]],
        indices: &[usize],
        outs: &mut [[f32; crate::BLOCK]],
        frames: usize,
    ) {
        let mut inputs: [&[f32]; 65] = [&[]; 65];
        for (out, &index) in inputs.iter_mut().zip(indices) {
            *out = &slots[index][..frames];
        }
        self.process(ctx, &inputs[..indices.len()], &mut outs[0][..frames]);
    }
}
