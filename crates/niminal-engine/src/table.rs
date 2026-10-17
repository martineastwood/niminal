//! Immutable tables and shared writable buffers.
//!
//! A table is one channel of f32 samples, built in f64 then stored as f32.
//! Multichannel files are arrays of tables at the language level. Buffers are
//! the same shape, but written from instruments and tracks with atomics so the
//! audio thread never locks.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::array::{Array, wrapped_index};
use crate::ops::Interp;

/// One channel of sample data, shared by every voice that reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    data: Box<[f32]>,
    /// Source rate when loaded from a file; synthetic tables leave this unset.
    sample_rate: Option<f32>,
}

impl Table {
    /// Build from f64 values (constructors accumulate in f64, then store f32).
    pub fn from_f64(values: impl IntoIterator<Item = f64>, sample_rate: Option<f32>) -> Self {
        let data: Box<[f32]> = values.into_iter().map(|v| v as f32).collect();
        assert!(!data.is_empty(), "a table has at least one entry");
        Table { data, sample_rate }
    }

    pub fn from_f64_slice(values: &[f64], sample_rate: Option<f32>) -> Self {
        Self::from_f64(values.iter().copied(), sample_rate)
    }

    pub fn from_f32(values: impl Into<Box<[f32]>>, sample_rate: Option<f32>) -> Self {
        let data = values.into();
        assert!(!data.is_empty(), "a table has at least one entry");
        Table { data, sample_rate }
    }

    /// An empty table is only useful as a zero-length array placeholder; most
    /// reads reject it. Prefer [`Table::from_f32`] / [`Table::from_f64`].
    pub fn empty(sample_rate: Option<f32>) -> Self {
        Table {
            data: Box::new([]),
            sample_rate,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn sample_rate(&self) -> Option<f32> {
        self.sample_rate
    }

    /// Sample rate used for playback when the table has none of its own.
    pub fn sample_rate_or(&self, fallback: f32) -> f32 {
        self.sample_rate.unwrap_or(fallback)
    }

    pub fn get(&self, i: usize) -> f32 {
        self.data[i]
    }

    /// Widen to f64 for band-limited wavetable analysis and kernel math.
    pub fn as_f64(&self) -> Vec<f64> {
        self.data.iter().map(|&v| f64::from(v)).collect()
    }

    pub fn as_array_f64(&self) -> Array<f64> {
        self.as_f64().into()
    }

    /// Read at a fractional index, wrapping. Empty tables return 0.
    pub fn read_at(&self, pos: f64, interp: Interp) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        crate::ops::read_array(self.data.len(), pos, interp, |i| f64::from(self.data[i]))
    }

    /// Read at a fractional index with an edge mode.
    pub fn read_at_edge(&self, pos: f64, interp: Interp, edge: Edge) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        let len = self.data.len();
        let pos = match edge {
            Edge::Wrap => pos,
            Edge::Clamp => {
                if !pos.is_finite() {
                    return f64::from(self.data[0]);
                }
                pos.clamp(0.0, (len - 1) as f64)
            }
        };
        match edge {
            Edge::Wrap => self.read_at(pos, interp),
            Edge::Clamp => read_clamped(&self.data, pos, interp),
        }
    }

    pub fn normalize(&self) -> Self {
        let peak = self.data.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if peak <= 0.0 {
            return self.clone();
        }
        Table::from_f32(
            self.data.iter().map(|v| v / peak).collect::<Box<[f32]>>(),
            self.sample_rate,
        )
    }

    pub fn reverse(&self) -> Self {
        let mut data = self.data.to_vec();
        data.reverse();
        Table::from_f32(data.into_boxed_slice(), self.sample_rate)
    }

    pub fn rotate(&self, amount: isize) -> Self {
        let n = self.data.len() as isize;
        if n == 0 {
            return self.clone();
        }
        let k = amount.rem_euclid(n) as usize;
        let mut out = Vec::with_capacity(self.data.len());
        out.extend_from_slice(&self.data[k..]);
        out.extend_from_slice(&self.data[..k]);
        Table::from_f32(out.into_boxed_slice(), self.sample_rate)
    }

    pub fn resample(&self, size: usize) -> Self {
        assert!(size > 0, "a table has at least one entry");
        if size == self.data.len() {
            return self.clone();
        }
        // Map onto 0..len-1 so the first and last entries are preserved.
        let last = (self.data.len() - 1) as f64;
        let data: Box<[f32]> = (0..size)
            .map(|i| {
                let pos = if size == 1 {
                    0.0
                } else {
                    i as f64 / (size - 1) as f64 * last
                };
                self.read_at_edge(pos, Interp::Linear, Edge::Clamp) as f32
            })
            .collect();
        Table::from_f32(data, self.sample_rate)
    }

    pub fn concat(&self, other: &Table) -> Self {
        let mut data = Vec::with_capacity(self.data.len() + other.data.len());
        data.extend_from_slice(&self.data);
        data.extend_from_slice(&other.data);
        Table::from_f32(data.into_boxed_slice(), self.sample_rate.or(other.sample_rate))
    }

    pub fn integrate(&self) -> Self {
        let mut data = Vec::with_capacity(self.data.len());
        let mut sum = 0.0f64;
        for &v in &self.data {
            sum += f64::from(v);
            data.push(sum as f32);
        }
        Table::from_f32(data.into_boxed_slice(), self.sample_rate).normalize()
    }

    pub fn mix(&self, other: &Table, amount: f64) -> Self {
        let amount = amount.clamp(0.0, 1.0) as f32;
        let n = self.data.len().max(other.data.len());
        let a = if self.data.len() == n {
            self.clone()
        } else {
            self.resample(n)
        };
        let b = if other.data.len() == n {
            other.clone()
        } else {
            other.resample(n)
        };
        let data: Box<[f32]> = a
            .data
            .iter()
            .zip(b.data.iter())
            .map(|(x, y)| x * (1.0 - amount) + y * amount)
            .collect();
        Table::from_f32(data, self.sample_rate.or(other.sample_rate))
    }

    pub fn slice(&self, start: usize, end: usize) -> Self {
        let end = end.min(self.data.len());
        let start = start.min(end);
        assert!(end > start, "a table slice must not be empty");
        Table::from_f32(self.data[start..end].to_vec().into_boxed_slice(), self.sample_rate)
    }
}

/// How a table read behaves past its ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    #[default]
    Wrap,
    Clamp,
}

fn read_clamped(data: &[f32], pos: f64, interp: Interp) -> f64 {
    let len = data.len();
    let last = (len - 1) as f64;
    let pos = pos.clamp(0.0, last);
    let base = pos.floor();
    let t = pos - base;
    let at = |k: f64| data[(base + k).clamp(0.0, last) as usize];
    match interp {
        Interp::None => f64::from(at(0.0)),
        Interp::Linear => {
            let a = f64::from(at(0.0));
            let b = f64::from(at(1.0));
            a + (b - a) * t
        }
        Interp::Cubic => {
            let (y0, y1, y2, y3) = (
                f64::from(at(-1.0)),
                f64::from(at(0.0)),
                f64::from(at(1.0)),
                f64::from(at(2.0)),
            );
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            ((a * t + b) * t + c) * t + y1
        }
    }
}

/// A writable table shared across voices, instruments and tracks.
///
/// Storage uses atomics so readers and writers on the audio thread never lock.
#[derive(Debug)]
pub struct Buffer {
    data: Box<[AtomicU32]>,
    sample_rate: Option<f32>,
}

impl Buffer {
    pub fn zeros(len: usize, sample_rate: Option<f32>) -> Self {
        assert!(len > 0, "a buffer has at least one entry");
        Buffer {
            data: (0..len).map(|_| AtomicU32::new(0f32.to_bits())).collect(),
            sample_rate,
        }
    }

    pub fn from_table(table: &Table) -> Self {
        Buffer {
            data: table
                .data
                .iter()
                .map(|v| AtomicU32::new(v.to_bits()))
                .collect(),
            sample_rate: table.sample_rate,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn sample_rate(&self) -> Option<f32> {
        self.sample_rate
    }

    pub fn sample_rate_or(&self, fallback: f32) -> f32 {
        self.sample_rate.unwrap_or(fallback)
    }

    pub fn read(&self, i: usize) -> f32 {
        f32::from_bits(self.data[i].load(Ordering::Relaxed))
    }

    pub fn write(&self, i: usize, value: f32) {
        self.data[i].store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn read_at(&self, pos: f64, interp: Interp) -> f64 {
        let n = self.data.len();
        crate::ops::read_array(n, pos, interp, |i| f64::from(self.read(i)))
    }

    pub fn write_at(&self, pos: f64, value: f32) {
        let n = self.data.len();
        self.write(wrapped_index(pos, n), value);
    }

    /// Snapshot as an immutable table (for `osc`, wavetable banks, etc.).
    pub fn snapshot(&self) -> Table {
        let data: Box<[f32]> = (0..self.data.len()).map(|i| self.read(i)).collect();
        Table::from_f32(data, self.sample_rate)
    }

    /// Copy contents into an existing buffer of the same length (live re-eval carry).
    pub fn copy_into(&self, other: &Buffer) {
        assert_eq!(self.len(), other.len());
        for (src, dst) in self.data.iter().zip(other.data.iter()) {
            dst.store(src.load(Ordering::Relaxed), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_peaks_at_one() {
        let t = Table::from_f64([-2.0, 1.0, 0.5], None).normalize();
        assert!((t.get(0).abs() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn buffer_round_trips() {
        let b = Buffer::zeros(4, Some(48_000.0));
        b.write(2, 0.5);
        assert_eq!(b.read(2), 0.5);
        assert!((b.read_at(2.0, Interp::None) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resample_preserves_endpoints() {
        let t = Table::from_f64([0.0, 1.0], None).resample(5);
        assert_eq!(t.len(), 5);
        assert!((t.get(0) - 0.0).abs() < 1e-5);
        assert!((t.get(4) - 1.0).abs() < 1e-5);
    }
}
