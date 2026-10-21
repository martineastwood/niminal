//! Bounded disk-player cache. Only the host's decoder worker publishes pages.
use crate::{Opcode, Port, ProcessCtx};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
pub const STREAM_PAGE: usize = 4096;
const PAGES: usize = 8;
struct Page {
    tag: AtomicU64,
    values: Box<[AtomicU32]>,
}
pub struct StreamCache {
    pub frames: u64,
    pub channels: usize,
    pub sample_rate: u32,
    pub requested: AtomicU64,
    pub loop_start: AtomicU64,
    pub failed: AtomicBool,
    pub underruns: AtomicU64,
    pages: Box<[Page]>,
    victim: AtomicU64,
}
impl StreamCache {
    pub fn new(frames: u64, channels: usize, sample_rate: u32) -> Arc<Self> {
        Arc::new(Self {
            victim: AtomicU64::new(0),
            frames,
            channels,
            sample_rate,
            requested: AtomicU64::new(0),
            loop_start: AtomicU64::new(u64::MAX),
            failed: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            pages: (0..PAGES)
                .map(|_| Page {
                    tag: AtomicU64::new(u64::MAX),
                    values: (0..STREAM_PAGE * channels)
                        .map(|_| AtomicU32::new(0))
                        .collect(),
                })
                .collect(),
        })
    }
    pub fn cached(&self, page: u64) -> bool {
        self.pages
            .iter()
            .any(|p| p.tag.load(Ordering::Acquire) == page * 2)
    }
    pub fn publish(&self, page: u64, values: &[f32]) {
        let slot = self
            .pages
            .iter()
            .find(|p| p.tag.load(Ordering::Acquire) == page * 2)
            .unwrap_or_else(|| {
                &self.pages[self.victim.fetch_add(1, Ordering::Relaxed) as usize % PAGES]
            });
        slot.tag.store(page * 2 + 1, Ordering::Release);
        for (i, out) in slot.values.iter().enumerate() {
            out.store(
                values.get(i).copied().unwrap_or(0.).to_bits(),
                Ordering::Relaxed,
            );
        }
        slot.tag.store(page * 2, Ordering::Release);
    }
    fn read(&self, frame: u64, channel: usize) -> Option<f32> {
        if frame >= self.frames {
            return Some(0.);
        }
        let page = frame / STREAM_PAGE as u64;
        let tag = page * 2;
        let slot = self
            .pages
            .iter()
            .find(|p| p.tag.load(Ordering::Acquire) == tag)?;
        if slot.tag.load(Ordering::Acquire) != tag {
            return None;
        }
        let value = f32::from_bits(
            slot.values[(frame as usize % STREAM_PAGE) * self.channels + channel]
                .load(Ordering::Relaxed),
        );
        std::sync::atomic::fence(Ordering::Acquire);
        (slot.tag.load(Ordering::Relaxed) == tag).then_some(value)
    }
}
pub struct StreamOut {
    cache: Arc<StreamCache>,
    position: f64,
    trigger: f32,
}
impl StreamOut {
    pub fn new(cache: Arc<StreamCache>) -> Self {
        Self {
            cache,
            position: 0.,
            trigger: f32::NAN,
        }
    }
}
impl Opcode for StreamOut {
    fn stream_cache(&self) -> Option<&Arc<StreamCache>> {
        Some(&self.cache)
    }
    fn name(&self) -> &str {
        "stream_player"
    }
    fn ports(&self) -> &[Port] {
        static PORTS: [Port; 7] = [
            Port::required("rate"),
            Port::required("playing"),
            Port::required("seek"),
            Port::required("seek_trigger"),
            Port::required("loop"),
            Port::required("loop_start"),
            Port::required("loop_end"),
        ];
        &PORTS
    }
    fn outputs(&self) -> usize {
        self.cache.channels
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Self::new(self.cache.clone()))
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && Arc::ptr_eq(&self.cache, &old.cache)
        {
            self.position = old.position;
            self.trigger = old.trigger;
        }
    }
    fn process(&mut self, _: &ProcessCtx, _: &[&[f32]], _: &mut [f32]) {
        unreachable!("multichannel player")
    }
    fn process_outputs(
        &mut self,
        ctx: &ProcessCtx,
        inputs: &[&[f32]],
        out: &mut [Vec<f32>],
        frames: usize,
    ) {
        for i in 0..frames {
            if inputs[3][i] != self.trigger {
                self.position = f64::from(inputs[2][i]) * f64::from(self.cache.sample_rate);
                self.trigger = inputs[3][i];
            }
            let looped = inputs[4][i] > 0.;
            let start = (f64::from(inputs[5][i]) * f64::from(self.cache.sample_rate)).max(0.);
            let end = (f64::from(inputs[6][i]) * f64::from(self.cache.sample_rate))
                .min(self.cache.frames as f64);
            if looped && end > start && self.position >= end {
                self.position = start + (self.position - start) % (end - start);
            }
            self.cache.loop_start.store(
                if looped { start as u64 } else { u64::MAX },
                Ordering::Relaxed,
            );
            let at = self.position.floor().max(0.) as u64;
            self.cache.requested.store(at, Ordering::Release);
            let playing = inputs[1][i] > 0.
                && at < self.cache.frames
                && !self.cache.failed.load(Ordering::Acquire);
            let mut missing = false;
            for (channel, output) in out.iter_mut().enumerate() {
                if !playing {
                    output[i] = 0.;
                    continue;
                }
                let value = loop {
                    if let Some(a) = self.cache.read(at, channel)
                        && let Some(b) = self.cache.read(at + 1, channel)
                    {
                        break Some(a + (b - a) * (self.position.fract() as f32));
                    }
                    if ctx.realtime || self.cache.failed.load(Ordering::Acquire) {
                        break None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                };
                missing |= value.is_none();
                output[i] = value.unwrap_or(0.);
            }
            if missing {
                self.cache.underruns.fetch_add(1, Ordering::Relaxed);
            }
            if playing {
                self.position += f64::from(inputs[0][i].clamp(0.25, 4.))
                    * f64::from(self.cache.sample_rate)
                    / f64::from(ctx.sample_rate);
            }
        }
    }
}
