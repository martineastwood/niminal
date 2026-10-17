//! Named table constructors and transforms (the modern GEN routines).
//!
//! All constructors are pure, deterministic, and return a [`Table`]. They
//! accumulate in f64 and store f32.

use niminal_engine::Table;

/// One cycle of a wave from harmonic amplitudes (GEN10).
/// `amps[0]` is the fundamental; optional `phases` are in cycles (0..1).
pub fn harmonics(amps: &[f64], phases: Option<&[f64]>, size: usize) -> Table {
    assert!(size > 0);
    let mut table: Vec<f64> = (0..size)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / size as f64;
            amps.iter()
                .enumerate()
                .map(|(h, w)| {
                    let phase = phases.and_then(|p| p.get(h)).copied().unwrap_or(0.0) * std::f64::consts::TAU;
                    w * (t * (h + 1) as f64 + phase).sin()
                })
                .sum()
        })
        .collect();
    normalize_peak(&mut table);
    Table::from_f64(table, None)
}

/// Inharmonic partials: each entry is `[ratio, amp]` or `[ratio, amp, phase]` (GEN09/19).
pub fn partials(partials: &[[f64; 3]], size: usize) -> Table {
    assert!(size > 0);
    let mut table: Vec<f64> = (0..size)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / size as f64;
            partials
                .iter()
                .map(|[ratio, amp, phase]| amp * (t * ratio + phase * std::f64::consts::TAU).sin())
                .sum()
        })
        .collect();
    normalize_peak(&mut table);
    Table::from_f64(table, None)
}

/// Cosine series (GEN11): `amps` weight cosines of successive harmonics.
pub fn cosines(amps: &[f64], size: usize) -> Table {
    assert!(size > 0);
    let mut table: Vec<f64> = (0..size)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / size as f64;
            amps.iter()
                .enumerate()
                .map(|(h, w)| w * (t * (h + 1) as f64).cos())
                .sum()
        })
        .collect();
    normalize_peak(&mut table);
    Table::from_f64(table, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveShape {
    Linear,
    Exp,
    Cubic,
    Spline,
}

/// Breakpoint curve through `[x, y]` points spanning 0..1 on x (GEN05/07/08).
pub fn curve(points: &[[f64; 2]], size: usize, shape: CurveShape) -> Result<Table, String> {
    assert!(size > 0);
    if points.len() < 2 {
        return Err("a curve needs at least two points".into());
    }
    let mut pts: Vec<[f64; 2]> = points.to_vec();
    pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    for w in pts.windows(2) {
        if w[0][0] == w[1][0] {
            return Err("curve points must have distinct x positions".into());
        }
    }
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let x = i as f64 / (size - 1).max(1) as f64;
            interpolate_curve(&pts, x, shape)
        })
        .collect();
    Ok(Table::from_f64(data, None))
}

fn interpolate_curve(pts: &[[f64; 2]], x: f64, shape: CurveShape) -> f64 {
    if x <= pts[0][0] {
        return pts[0][1];
    }
    if x >= pts[pts.len() - 1][0] {
        return pts[pts.len() - 1][1];
    }
    let i = pts.iter().position(|p| p[0] > x).unwrap_or(1).saturating_sub(1);
    let (x0, y0) = (pts[i][0], pts[i][1]);
    let (x1, y1) = (pts[i + 1][0], pts[i + 1][1]);
    let t = (x - x0) / (x1 - x0);
    match shape {
        CurveShape::Linear => y0 + (y1 - y0) * t,
        CurveShape::Exp => {
            if (y0 <= 0.0 && y1 <= 0.0) || (y0 >= 0.0 && y1 >= 0.0 && y0 != 0.0 && y1 != 0.0) {
                let a = y0.abs().max(1e-30);
                let b = y1.abs().max(1e-30);
                let sign = if y1 < 0.0 || y0 < 0.0 { -1.0 } else { 1.0 };
                sign * a * (b / a).powf(t)
            } else {
                y0 + (y1 - y0) * t
            }
        }
        CurveShape::Cubic => {
            let t2 = t * t;
            let t3 = t2 * t;
            y0 * (2.0 * t3 - 3.0 * t2 + 1.0) + y1 * (-2.0 * t3 + 3.0 * t2)
        }
        CurveShape::Spline => {
            let ym1 = if i > 0 { pts[i - 1][1] } else { y0 };
            let y2 = if i + 2 < pts.len() { pts[i + 2][1] } else { y1 };
            let a = -0.5 * ym1 + 1.5 * y0 - 1.5 * y1 + 0.5 * y2;
            let b = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
            let c = -0.5 * ym1 + 0.5 * y1;
            ((a * t + b) * t + c) * t + y0
        }
    }
}

/// Step function: at each integer index `xs[i]`, hold value `ys[i]` (GEN17-style).
pub fn steps(xs: &[f64], ys: &[f64], size: usize) -> Result<Table, String> {
    assert!(size > 0);
    if xs.len() != ys.len() || xs.is_empty() {
        return Err("`steps` needs matching x and y arrays".into());
    }
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let mut value = ys[0];
            for (x, y) in xs.iter().zip(ys) {
                if *x <= i as f64 {
                    value = *y;
                }
            }
            value
        })
        .collect();
    Ok(Table::from_f64(data, None))
}

/// Polynomial evaluated over -1..1 mapped onto the table (GEN03).
pub fn poly(coeffs: &[f64], size: usize) -> Table {
    assert!(size > 0);
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let x = if size == 1 {
                0.0
            } else {
                2.0 * i as f64 / (size - 1) as f64 - 1.0
            };
            coeffs
                .iter()
                .enumerate()
                .map(|(p, c)| c * x.powi(p as i32))
                .sum()
        })
        .collect();
    Table::from_f64(data, None)
}

/// Linear ramp from `start` to `end`.
pub fn ramp(start: f64, end: f64, size: usize) -> Table {
    assert!(size > 0);
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let t = if size == 1 {
                0.0
            } else {
                i as f64 / (size - 1) as f64
            };
            start + (end - start) * t
        })
        .collect();
    Table::from_f64(data, None)
}

/// Chebyshev transfer function (GEN13): `amps[0]` weights T1(x), etc., over -1..1.
pub fn cheby(amps: &[f64], size: usize) -> Table {
    assert!(size > 0);
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let x = if size == 1 {
                0.0
            } else {
                2.0 * i as f64 / (size - 1) as f64 - 1.0
            };
            // T_0 = 1, T_1 = x, T_{n+1} = 2x T_n - T_{n-1}; amps[k] weights T_{k+1}
            let mut t0 = 1.0;
            let mut t1 = x;
            let mut sum = 0.0;
            for (k, amp) in amps.iter().enumerate() {
                let tn = if k == 0 {
                    t1
                } else {
                    let t = 2.0 * x * t1 - t0;
                    t0 = t1;
                    t1 = t;
                    t
                };
                sum += amp * tn;
            }
            sum
        })
        .collect();
    Table::from_f64(data, None)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowKind {
    Hann,
    Hamming,
    Blackman,
    Kaiser { beta: f64 },
    Gauss { alpha: f64 },
    Tukey { alpha: f64 },
    Sinc,
    Tri,
}

/// Window function (GEN20).
pub fn window(kind: WindowKind, size: usize) -> Table {
    assert!(size > 0);
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let x = if size == 1 {
                0.0
            } else {
                i as f64 / (size - 1) as f64
            };
            match kind {
                WindowKind::Hann => 0.5 - 0.5 * (std::f64::consts::TAU * x).cos(),
                WindowKind::Hamming => 0.54 - 0.46 * (std::f64::consts::TAU * x).cos(),
                WindowKind::Blackman => {
                    0.42 - 0.5 * (std::f64::consts::TAU * x).cos()
                        + 0.08 * (2.0 * std::f64::consts::TAU * x).cos()
                }
                WindowKind::Kaiser { beta } => {
                    let t = 2.0 * x - 1.0;
                    bessel0(beta * (1.0 - t * t).max(0.0).sqrt()) / bessel0(beta)
                }
                WindowKind::Gauss { alpha } => {
                    let t = (x - 0.5) * alpha * 2.0;
                    (-0.5 * t * t).exp()
                }
                WindowKind::Tukey { alpha } => {
                    let a = alpha.clamp(0.0, 1.0);
                    if a < 1e-12 {
                        1.0
                    } else if x < a / 2.0 {
                        0.5 * (1.0 + (std::f64::consts::TAU * x / a - std::f64::consts::PI).cos())
                    } else if x > 1.0 - a / 2.0 {
                        0.5 * (1.0
                            + (std::f64::consts::TAU * (1.0 - x) / a - std::f64::consts::PI).cos())
                    } else {
                        1.0
                    }
                }
                WindowKind::Sinc => {
                    let t = 2.0 * x - 1.0;
                    if t.abs() < 1e-12 {
                        1.0
                    } else {
                        (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
                    }
                }
                WindowKind::Tri => 1.0 - (2.0 * x - 1.0).abs(),
            }
        })
        .collect();
    Table::from_f64(data, None)
}

fn bessel0(x: f64) -> f64 {
    // Series expansion of I0, enough for typical Kaiser beta.
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = (x * 0.5).powi(2);
    for k in 1..32 {
        term *= y / (k as f64).powi(2);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dist {
    Uniform,
    Linear,
    Triangle,
    Exponential,
    Bilateral,
    Cauchy,
    Poisson,
}

/// Deterministic pseudo-random table (GEN21). `seed` is required for replay identity.
pub fn random(size: usize, dist: Dist, seed: u64) -> Table {
    assert!(size > 0);
    let mut state = seed | 1;
    let mut next = || {
        // xorshift64*
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state as f64) / (u64::MAX as f64)
    };
    let data: Vec<f64> = (0..size)
        .map(|_| match dist {
            Dist::Uniform => next() * 2.0 - 1.0,
            Dist::Linear => {
                let u = next();
                u * u * 2.0 - 1.0
            }
            Dist::Triangle => next() + next() - 1.0,
            Dist::Exponential => {
                let u = next().max(1e-12);
                -u.ln() * 2.0 - 1.0
            }
            Dist::Bilateral => {
                let u = next().max(1e-12);
                if next() < 0.5 {
                    u.ln()
                } else {
                    -u.ln()
                }
            }
            Dist::Cauchy => {
                let u = (next() - 0.5).clamp(-0.499, 0.499);
                (std::f64::consts::PI * u).tan().clamp(-10.0, 10.0) / 10.0
            }
            Dist::Poisson => {
                // Simple count truncated to -1..1 scale.
                let lambda: f64 = 4.0;
                let mut k = 0;
                let mut p = (-lambda).exp();
                let mut s = p;
                let u = next();
                while u > s && k < 20 {
                    k += 1;
                    p *= lambda / k as f64;
                    s += p;
                }
                k as f64 / 10.0 * 2.0 - 1.0
            }
        })
        .collect();
    Table::from_f64(data, None)
}

fn normalize_peak(table: &mut [f64]) {
    let peak = table.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        for v in table {
            *v /= peak;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonics_sine_is_unit_peak() {
        let t = harmonics(&[1.0], None, 64);
        let peak = t.data().iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 1.0).abs() < 1e-5, "{peak}");
    }

    #[test]
    fn curve_linear_endpoints() {
        let t = curve(&[[0.0, 0.0], [1.0, 1.0]], 5, CurveShape::Linear).unwrap();
        assert!((t.get(0) - 0.0).abs() < 1e-5);
        assert!((t.get(4) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cheby_t1_is_identity() {
        let t = cheby(&[1.0], 5);
        assert!((t.get(0) + 1.0).abs() < 1e-5);
        assert!(t.get(2).abs() < 1e-5);
        assert!((t.get(4) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn random_is_deterministic() {
        let a = random(32, Dist::Uniform, 42);
        let b = random(32, Dist::Uniform, 42);
        assert_eq!(a.data(), b.data());
    }

    #[test]
    fn window_hann_ends_at_zero() {
        let t = window(WindowKind::Hann, 65);
        assert!(t.get(0).abs() < 1e-5);
        assert!(t.get(64).abs() < 1e-5);
        assert!((t.get(32) - 1.0).abs() < 1e-5);
    }
}
