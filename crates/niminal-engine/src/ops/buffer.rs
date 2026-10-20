//! Shared writable buffer opcodes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::opcode::{Opcode, Port, ProcessCtx};
use crate::ops::Interp;
use crate::table::Buffer;

/// Read a shared buffer at a fractional index.
#[derive(Clone)]
pub struct BufferRead {
    channel: usize,
    buffer: Arc<Buffer>,
    interp: Interp,
}

impl BufferRead {
    pub fn channel(mut self, channel: usize) -> Self {
        self.channel = channel;
        self
    }
    pub fn new(buffer: Arc<Buffer>, interp: Interp) -> Self {
        assert!(!buffer.is_empty());
        BufferRead {
            buffer,
            interp,
            channel: 0,
        }
    }
}

impl Opcode for BufferRead {
    fn name(&self) -> &'static str {
        "buffer_read"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("pos")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, p) in out.iter_mut().zip(inputs[0]) {
            *o = self
                .buffer
                .read_at_channel(f64::from(*p), self.interp, self.channel) as f32;
        }
    }
}

/// Write a value into a shared buffer at an index (floored, wrapping).
#[derive(Clone)]
pub struct BufferWrite {
    buffer: Arc<Buffer>,
}

impl BufferWrite {
    pub fn new(buffer: Arc<Buffer>) -> Self {
        assert!(!buffer.is_empty());
        BufferWrite { buffer }
    }
}

impl Opcode for BufferWrite {
    fn name(&self) -> &'static str {
        "buffer_write"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[
            Port::required("idx"),
            Port::required("value"),
            Port::optional("gate", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn has_side_effects(&self) -> bool {
        true
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let idx = f64::from(inputs[0][i]);
            let value = inputs[1][i];
            // a write inside an `if` only happens while the `if` is taken
            if inputs[2][i] != 0.0 {
                self.buffer.write_at(idx, value);
            }
            *o = value;
        }
    }
}

/// Record a signal into a shared buffer, advancing a write head each sample.
#[derive(Clone)]
pub struct BufferRecord {
    buffer: Arc<Buffer>,
    head: Arc<AtomicUsize>,
    looped: bool,
    channel: usize,
}

impl BufferRecord {
    pub fn with_channel(mut self, channel: usize) -> Self {
        self.channel = channel;
        self
    }
    pub fn new(buffer: Arc<Buffer>, looped: bool) -> Self {
        assert!(!buffer.is_empty());
        BufferRecord {
            buffer,
            head: Arc::new(AtomicUsize::new(0)),
            looped,
            channel: 0,
        }
    }
}

impl Opcode for BufferRecord {
    fn name(&self) -> &'static str {
        "buffer_record"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("gate", 1.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(BufferRecord {
            buffer: self.buffer.clone(),
            head: self.head.clone(),
            looped: self.looped,
            channel: self.channel,
        })
    }

    fn has_side_effects(&self) -> bool {
        true
    }

    fn reset(&mut self) {
        self.head.store(0, Ordering::Relaxed);
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let n = self.buffer.len();
        for (i, o) in out.iter_mut().enumerate() {
            let x = inputs[0][i];
            if inputs[1][i] == 0.0 {
                *o = x;
                continue;
            }
            let mut pos = self.head.load(Ordering::Relaxed);
            if pos >= n {
                if self.looped {
                    pos = 0;
                } else {
                    *o = x;
                    continue;
                }
            }
            self.buffer.write_channel(pos, self.channel, x);
            self.head.store(pos + 1, Ordering::Relaxed);
            *o = x;
        }
    }
}

/// A running voice selects from resources prepared with its graph.
#[derive(Clone)]
pub struct BufferReadBank {
    channel: usize,
    key: u64,
    buffers: Arc<Vec<Arc<Buffer>>>,
    interp: Interp,
}
impl BufferReadBank {
    pub fn channel(mut self, channel: usize) -> Self {
        self.channel = channel;
        self
    }
    pub fn new(buffers: Arc<Vec<Arc<Buffer>>>, interp: Interp, key: u64) -> Self {
        assert!(!buffers.is_empty());
        Self {
            buffers,
            interp,
            key,
            channel: 0,
        }
    }
}
impl Opcode for BufferReadBank {
    fn buffer_channels(&self, key: u64) -> usize {
        if key == self.key { self.channel + 1 } else { 0 }
    }
    fn buffer_bindings(&self, key: u64) -> usize {
        usize::from(self.key == key)
    }
    fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank], cursor: &mut usize) {
        if key == self.key {
            if let Some(bank) = banks.get_mut(*cursor) {
                std::mem::swap(bank, &mut self.buffers);
            }
            *cursor += 1;
        }
    }

    fn name(&self) -> &str {
        "buffer_bank_read"
    }
    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("source"), Port::required("pos")];
        PORTS
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }
    fn process(&mut self, _: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, out) in out.iter_mut().enumerate() {
            let source = inputs[0][i].max(0.0).floor() as usize;
            *out = self.buffers[source.min(self.buffers.len() - 1)].read_at_channel(
                f64::from(inputs[1][i]),
                self.interp,
                self.channel,
            ) as f32;
        }
    }
}
