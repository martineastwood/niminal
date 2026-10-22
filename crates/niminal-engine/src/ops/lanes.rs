//! Vector operations for kernels: whole arrays at a time, so one instruction does the work of one per
//! element. All of them run in a fixed order, so the same inputs give the same bits on every machine.

use super::custom::{BinOp, Fn1, Interp, read_at};
use crate::wrapped_index;

/// One side of an elementwise operation: a single value, or an array.
#[derive(Clone, Copy)]
pub enum Operand<'a> {
    Scalar(f64),
    Slice(&'a [f64]),
}

/// `d[i] = a[i] op b[i]`, with a single value standing for every element.
pub fn zip(op: BinOp, d: &mut [f64], a: Operand, b: Operand) {
    macro_rules! each {
        ($f:expr) => {
            match (a, b) {
                (Operand::Slice(a), Operand::Slice(b)) => {
                    for ((d, &a), &b) in d.iter_mut().zip(a).zip(b) {
                        *d = $f(a, b);
                    }
                }
                (Operand::Slice(a), Operand::Scalar(b)) => {
                    for (d, &a) in d.iter_mut().zip(a) {
                        *d = $f(a, b);
                    }
                }
                (Operand::Scalar(a), Operand::Slice(b)) => {
                    for (d, &b) in d.iter_mut().zip(b) {
                        *d = $f(a, b);
                    }
                }
                (Operand::Scalar(a), Operand::Scalar(b)) => d.fill($f(a, b)),
            }
        };
    }
    match op {
        BinOp::Add => each!(|a: f64, b: f64| a + b),
        BinOp::Sub => each!(|a: f64, b: f64| a - b),
        BinOp::Mul => each!(|a: f64, b: f64| a * b),
        other => each!(|a, b| other.apply(a, b)),
    }
}

/// `d[i] = f(a[i])`.
pub fn map(f: Fn1, d: &mut [f64], a: Operand) {
    match a {
        Operand::Slice(a) => {
            for (d, &a) in d.iter_mut().zip(a) {
                *d = f.apply(a);
            }
        }
        Operand::Scalar(a) => d.fill(f.apply(a)),
    }
}

/// The sum of `a`, with four partial sums so that it can run in parallel lanes. The order is fixed.
pub fn sum(a: &[f64]) -> f64 {
    let mut parts = [0.0; 4];
    let (chunks, remainder) = a.as_chunks::<4>();
    for c in chunks {
        for k in 0..4 {
            parts[k] += c[k];
        }
    }
    for (k, &x) in remainder.iter().enumerate() {
        parts[k] += x;
    }
    (parts[0] + parts[1]) + (parts[2] + parts[3])
}

/// The smallest (`min`) or largest entry of `a`.
pub fn extreme(a: &[f64], minimum: bool) -> f64 {
    let pick = |x: f64, y: f64| if minimum { x.min(y) } else { x.max(y) };
    a.iter().copied().reduce(pick).unwrap_or(0.0)
}

/// `Σ a[i] * b[i]`, summed in four parts as [`sum`] does.
pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut parts = [0.0; 4];
    let (ca, ra) = a.as_chunks::<4>();
    let (cb, rb) = b.as_chunks::<4>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..4 {
            parts[k] += x[k] * y[k];
        }
    }
    for (k, (&x, &y)) in ra.iter().zip(rb).enumerate() {
        parts[k] += x * y;
    }
    (parts[0] + parts[1]) + (parts[2] + parts[3])
}

/// `Σ taps[i] * ring[from - i]`: a filter's output, from a ring buffer whose newest entry is at `from`,
/// going back in time and wrapping. `taps` is no longer than the ring.
pub fn fir(taps: &[f64], ring: &[f64], from: f64) -> f64 {
    let n = ring.len();
    let newest = wrapped_index(from, n);
    // taps[0..=newest] meet ring[newest], ring[newest - 1] ... ring[0]; the rest meet ring[n - 1] downwards
    let first = taps.len().min(newest + 1);
    let (head, tail) = taps.split_at(first);
    let mut total = dot_reversed(head, &ring[newest + 1 - first..=newest]);
    if !tail.is_empty() {
        total += dot_reversed(tail, &ring[n - tail.len()..]);
    }
    total
}

/// `Σ a[i] * b[b.len() - 1 - i]`, in four parts.
fn dot_reversed(a: &[f64], b: &[f64]) -> f64 {
    let mut parts = [0.0; 4];
    let last = b.len() - 1;
    let (chunks, remainder) = a.as_chunks::<4>();
    let mut base = 0;
    for c in chunks {
        for k in 0..4 {
            parts[k] += c[k] * b[last - (base + k)];
        }
        base += 4;
    }
    for (k, &x) in remainder.iter().enumerate() {
        parts[k] += x * b[last - (base + k)];
    }
    (parts[0] + parts[1]) + (parts[2] + parts[3])
}

/// `d[k] = ring[from - offsets[k]]`, read between entries as `interp` says: many taps of a delay line.
pub fn taps(d: &mut [f64], ring: &[f64], offsets: Operand, from: f64, interp: Interp) {
    for (k, d) in d.iter_mut().enumerate() {
        let offset = match offsets {
            Operand::Scalar(o) => o,
            Operand::Slice(o) => o[k],
        };
        *d = read_at(ring, from - offset, interp);
    }
}

/// `d = m * v` for a matrix `m` stored row by row.
pub fn matvec(d: &mut [f64], m: &[f64], cols: usize, v: &[f64]) {
    for (d, row) in d.iter_mut().zip(m.chunks_exact(cols)) {
        *d = dot(row, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_fir(taps: &[f64], ring: &[f64], from: usize) -> f64 {
        let n = ring.len();
        taps.iter()
            .enumerate()
            .map(|(i, t)| t * ring[(from + n * 4 - i) % n])
            .sum()
    }

    #[test]
    fn fir_walks_back_from_the_newest_entry_and_wraps() {
        let ring: Vec<f64> = (0..16).map(|i| (i * i) as f64 * 0.01 + 0.3).collect();
        let taps: Vec<f64> = (0..11).map(|i| 1.0 / (i + 1) as f64).collect();
        for from in 0..16 {
            let got = fir(&taps, &ring, from as f64);
            let want = naive_fir(&taps, &ring, from);
            assert!((got - want).abs() < 1e-12, "from {from}: {got} {want}");
        }
        // a position outside the ring wraps in both directions
        assert_eq!(fir(&taps, &ring, 18.0), fir(&taps, &ring, 2.0));
        assert_eq!(fir(&taps, &ring, -14.0), fir(&taps, &ring, 2.0));
    }

    #[test]
    fn fir_over_the_whole_ring_and_a_single_tap() {
        let ring = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(fir(&[1.0; 4], &ring, 1.0), 10.0);
        assert_eq!(fir(&[2.0], &ring, 3.0), 8.0);
    }

    #[test]
    fn sums_and_dots_do_not_depend_on_the_length() {
        for n in 0..20 {
            let a: Vec<f64> = (0..n).map(|i| i as f64 + 0.5).collect();
            let b: Vec<f64> = (0..n).map(|i| 2.0 - i as f64 * 0.25).collect();
            let want_sum: f64 = a.iter().sum();
            let want_dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!((sum(&a) - want_sum).abs() < 1e-9);
            assert!((dot(&a, &b) - want_dot).abs() < 1e-9);
        }
    }

    #[test]
    fn zip_broadcasts_a_single_value() {
        let a = [1.0, 2.0, 3.0];
        let mut d = [0.0; 3];
        zip(BinOp::Mul, &mut d, Operand::Slice(&a), Operand::Scalar(2.0));
        assert_eq!(d, [2.0, 4.0, 6.0]);
        zip(
            BinOp::Sub,
            &mut d,
            Operand::Scalar(10.0),
            Operand::Slice(&a),
        );
        assert_eq!(d, [9.0, 8.0, 7.0]);
        zip(BinOp::Div, &mut d, Operand::Slice(&a), Operand::Scalar(0.0));
        assert_eq!(d, [0.0; 3], "division by zero gives zero, as everywhere");
    }

    #[test]
    fn taps_read_a_ring_at_offsets() {
        let ring = [10.0, 11.0, 12.0, 13.0];
        let mut d = [0.0; 3];
        taps(
            &mut d,
            &ring,
            Operand::Slice(&[0.0, 1.0, 5.0]),
            2.0,
            Interp::None,
        );
        assert_eq!(d, [12.0, 11.0, 11.0]);
        taps(&mut d, &ring, Operand::Scalar(0.5), 2.0, Interp::Linear);
        assert_eq!(d, [11.5; 3]);
    }

    #[test]
    fn matvec_multiplies_row_by_row() {
        let m = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut d = [0.0; 2];
        matvec(&mut d, &m, 3, &[1.0, 0.0, -1.0]);
        assert_eq!(d, [-2.0, -2.0]);
    }

    #[test]
    fn math_maps_over_an_array() {
        let a = [0.0, 1.0];
        let mut d = [9.0; 2];
        map(Fn1::Exp, &mut d, Operand::Slice(&a));
        assert!((d[1] - std::f64::consts::E).abs() < 1e-12);
    }
}
