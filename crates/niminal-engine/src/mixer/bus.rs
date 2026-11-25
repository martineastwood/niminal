//! Meters, multichannel buses, and per-lane bus sends.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::opcode::{block_size, blocks};

/// The most channels any signal path may carry (7.1.4 needs 12).
pub const MAX_CHANNELS: usize = 16;

/// Peaks shared with the control thread; rendering never allocates or locks.
#[derive(Debug, Default)]
pub struct Meter {
    peaks: [AtomicU32; MAX_CHANNELS],
    channels: AtomicU32,
    waveform: std::sync::OnceLock<crate::Waveform>,
}

impl Meter {
    /// Records the block and returns this block's peak for each channel.
    pub fn observe(&self, blocks: &[Vec<f32>], frames: usize, sample: u64) -> [f32; MAX_CHANNELS] {
        if let Some(waveform) = self.waveform.get() {
            waveform.observe(blocks, frames, sample);
        }
        self.channels.store(blocks.len() as u32, Ordering::Relaxed);
        let mut values = [0.0_f32; MAX_CHANNELS];
        for ((peak, block), out) in self.peaks.iter().zip(blocks).zip(&mut values) {
            *out = block[..frames].iter().fold(0.0_f32, |a, b| a.max(b.abs()));
            peak.fetch_max(out.to_bits(), Ordering::Relaxed);
        }
        values
    }
    pub fn waveform(&self) -> &crate::Waveform {
        self.waveform.get_or_init(crate::Waveform::default)
    }
    pub fn read(&self, take: bool) -> Vec<f32> {
        self.peaks[..self.channels.load(Ordering::Relaxed) as usize]
            .iter()
            .map(|p| {
                f32::from_bits(if take {
                    p.swap(0, Ordering::Relaxed)
                } else {
                    p.load(Ordering::Relaxed)
                })
            })
            .collect()
    }
}

/// Block-sized, possibly multichannel buffers that are summed into and cleared
/// every block.
pub struct Buses {
    pub(super) data: Vec<Vec<Vec<f32>>>,
    /// What delayed sends put in over the last `BLOCK` samples, as a ring: a sample comes out
    /// where it went in, `BLOCK` samples later.
    late: Vec<Vec<Vec<f32>>>,
    /// What delayed sends have put in during the current block.
    staged: Vec<Vec<Vec<f32>>>,
    late_pos: usize,
    history: Vec<Vec<Vec<f32>>>,
    /// Where the session's clock is, for the block being mixed.
    pub clock: crate::Clock,
}

impl Buses {
    /// One bus per entry, with that many channels.
    pub fn new(channels: &[usize]) -> Self {
        Buses {
            data: channels.iter().map(|&n| blocks(n)).collect(),
            late: channels.iter().map(|&n| blocks(n)).collect(),
            staged: channels.iter().map(|&n| blocks(n)).collect(),
            late_pos: 0,
            history: channels.iter().map(|&n| blocks(n)).collect(),
            clock: crate::Clock::default(),
        }
    }

    /// Empty every bus, including what is on its way.
    pub fn clear(&mut self) {
        for bus in self
            .data
            .iter_mut()
            .chain(&mut self.late)
            .chain(&mut self.staged)
            .chain(&mut self.history)
        {
            bus.iter_mut().for_each(|c| c.fill(0.0));
        }
        self.late_pos = 0;
    }

    /// Start a block of `frames` samples: the buses hold what delayed sends put in a block ago
    /// (exactly `BLOCK` samples ago, whatever the length of the blocks in between).
    pub fn begin_block(&mut self, frames: usize) {
        let (first, rest) = self.ring_split(frames);
        let pos = self.late_pos;
        for ((data, late), staged) in self
            .data
            .iter_mut()
            .zip(&mut self.late)
            .zip(&mut self.staged)
        {
            for ((d, l), s) in data.iter_mut().zip(late.iter_mut()).zip(staged.iter_mut()) {
                s.fill(0.0);
                d.fill(0.0);
                d[..first].copy_from_slice(&l[pos..pos + first]);
                l[pos..pos + first].fill(0.0);
                d[first..frames].copy_from_slice(&l[..rest]);
                l[..rest].fill(0.0);
            }
        }
    }

    /// How `frames` samples written at `late_pos` of a ring one block long divide: this many up to
    /// the end of the ring, then this many from its start.
    fn ring_split(&self, frames: usize) -> (usize, usize) {
        let first = frames.min(block_size() - self.late_pos);
        (first, frames - first)
    }

    /// Finish the block: what delayed sends staged waits to arrive in `BLOCK` samples.
    pub fn end_block(&mut self, frames: usize) {
        let (first, rest) = self.ring_split(frames);
        let pos = self.late_pos;
        for (history, data) in self.history.iter_mut().zip(&self.data) {
            for (h, d) in history.iter_mut().zip(data) {
                h[pos..pos + first].copy_from_slice(&d[..first]);
                h[..rest].copy_from_slice(&d[first..frames]);
            }
        }
        for (late, staged) in self.late.iter_mut().zip(&self.staged) {
            for (l, s) in late.iter_mut().zip(staged) {
                for (l, s) in l[pos..pos + first].iter_mut().zip(&s[..first]) {
                    *l += s;
                }
                for (l, s) in l[..rest].iter_mut().zip(&s[first..frames]) {
                    *l += s;
                }
            }
        }
        self.late_pos = (self.late_pos + frames) % block_size();
    }

    /// Add `samples` to the start of one channel of a bus. A bus or channel that
    /// doesn't exist is ignored.
    pub fn add(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.data.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            for (d, s) in c.iter_mut().zip(samples) {
                *d += s;
            }
        }
    }

    /// Like [`Buses::add`], but the samples arrive `BLOCK` samples later.
    pub fn add_delayed(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.staged.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            for (d, s) in c.iter_mut().zip(samples) {
                *d += s;
            }
        }
    }

    pub fn get(&self, bus: usize, channel: usize, frames: usize) -> &[f32] {
        &self.data[bus][channel][..frames]
    }

    /// Runtime bus mappings intentionally read exactly one block behind, avoiding dynamic cycles.
    pub fn previous(&self, bus: usize, channel: usize, i: usize) -> f32 {
        self.history
            .get(bus)
            .and_then(|b| b.get(channel))
            .map_or(0.0, |c| c[(self.late_pos + i) % block_size()])
    }

    pub fn channels(&self, bus: usize) -> usize {
        self.data[bus].len()
    }
}

/// What a few voices send to the buses over one block, kept apart from the buses until it is added
/// to them in a fixed order, so the result does not depend on which voices ran when.
pub(crate) struct Sends {
    data: Vec<Vec<Vec<f32>>>,
    staged: Vec<Vec<Vec<f32>>>,
    /// Per bus: whether anything arrived this block, in `data` and in `staged`.
    touched: Vec<(bool, bool)>,
}

impl Sends {
    pub(super) fn new(channels: &[usize]) -> Self {
        let shape = || channels.iter().map(|&n| blocks(n)).collect();
        Sends {
            data: shape(),
            staged: shape(),
            touched: vec![(false, false); channels.len()],
        }
    }

    pub(crate) fn add(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.data.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            self.touched[bus].0 = true;
            c.iter_mut().zip(samples).for_each(|(d, s)| *d += s);
        }
    }

    pub(crate) fn add_delayed(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.staged.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            self.touched[bus].1 = true;
            c.iter_mut().zip(samples).for_each(|(d, s)| *d += s);
        }
    }

    /// Add everything to `buses` and start over.
    pub(super) fn drain_into(&mut self, buses: &mut Buses, frames: usize) {
        for (bus, touched) in self.touched.iter_mut().enumerate() {
            let (now, later) = std::mem::take(touched);
            for (c, (data, staged)) in self.data[bus]
                .iter_mut()
                .zip(&mut self.staged[bus])
                .enumerate()
            {
                if now {
                    buses.add(bus, c, &data[..frames]);
                    data.fill(0.0);
                }
                if later {
                    buses.add_delayed(bus, c, &staged[..frames]);
                    staged.fill(0.0);
                }
            }
        }
    }
}
