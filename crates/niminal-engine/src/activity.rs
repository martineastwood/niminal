//! Renderer-written note receipts. The control thread reads atomic transitions.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
pub const WAITING_SAMPLE: u64 = u64::MAX;
#[derive(Debug)]
pub struct Activity {
    pub token: u64,
    pub owner: u64,
    pub started: AtomicU64,
    pub released: AtomicU64,
    pub ended: AtomicU64,
    pub dropped: AtomicU64,
    pub serial: AtomicU64,
    /// 0 natural, 1 stolen/choked, 2 panic, 3 invalid DSP, 4 graph removal, 5 capacity.
    pub reason: AtomicU32,
}
impl Activity {
    pub fn new(token: u64, owner: u64) -> Self {
        Self {
            token,
            owner,
            started: AtomicU64::new(WAITING_SAMPLE),
            released: AtomicU64::new(WAITING_SAMPLE),
            ended: AtomicU64::new(WAITING_SAMPLE),
            dropped: AtomicU64::new(WAITING_SAMPLE),
            serial: AtomicU64::new(0),
            reason: AtomicU32::new(0),
        }
    }
    pub fn start(&self, sample: u64, serial: u64) {
        self.serial.store(serial, Ordering::Relaxed);
        let _ = self.started.compare_exchange(
            WAITING_SAMPLE,
            sample,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
    pub fn release(&self, sample: u64) {
        let _ = self.released.compare_exchange(
            WAITING_SAMPLE,
            sample,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
    pub fn finish(&self, sample: u64, reason: u32) {
        if self.started.load(Ordering::Acquire) != WAITING_SAMPLE
            && self.ended.load(Ordering::Relaxed) == WAITING_SAMPLE
        {
            self.reason.store(reason, Ordering::Relaxed);
            self.ended.store(sample, Ordering::Release);
        }
    }
    pub fn drop_at(&self, sample: u64) {
        self.reason.store(5, Ordering::Relaxed);
        self.dropped.store(sample, Ordering::Release);
    }
}
