//! Named table constructors and transforms.
//!
//! All constructors are pure, deterministic, and return a [`Table`]. They
//! accumulate in f64 and store f32.

use niminal_engine::Table;

/// One cycle of a wave from harmonic amplitudes.
/// `amps[0]` is the fundamental; optional `phases` are in cycles (0..1). With `normalize` the peak is 1.
pub fn harmonics(amps: &[f64], phases: Option<&[f64]>, size: usize, normalize: bool) -> Table {
    assert!(size > 0);
    let mut table: Vec<f64> = (0..size)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / size as f64;
            amps.iter()
                .enumerate()
                .map(|(h, w)| {
                    let phase = phases.and_then(|p| p.get(h)).copied().unwrap_or(0.0)
                        * std::f64::consts::TAU;
                    w * (t * (h + 1) as f64 + phase).sin()
                })
                .sum()
        })
        .collect();
    if normalize {
        normalize_peak(&mut table);
    }
    Table::from_f64(table, None)
}

/// Inharmonic partials: each entry is `[ratio, amp]` or `[ratio, amp, phase]`.
pub fn partials(partials: &[[f64; 3]], size: usize, normalize: bool) -> Table {
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
    if normalize {
        normalize_peak(&mut table);
    }
    Table::from_f64(table, None)
}

/// Cosine series: `amps` weight cosines of successive harmonics.
pub fn cosines(amps: &[f64], size: usize, normalize: bool) -> Table {
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
    if normalize {
        normalize_peak(&mut table);
    }
    Table::from_f64(table, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveShape {
    Linear,
    Exp,
    Cubic,
    Spline,
}

/// Breakpoint curve through `[x, y]` points spanning 0..1 on x.
/// `bend` curves every linear segment: negative starts fast and settles, positive starts slow and rushes.
pub fn curve(
    points: &[[f64; 2]],
    size: usize,
    shape: CurveShape,
    bend: f64,
) -> Result<Table, String> {
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
    if bend != 0.0 && shape != CurveShape::Linear {
        return Err("`bend` curves a `linear` curve".into());
    }
    if shape == CurveShape::Exp
        && pts
            .windows(2)
            .any(|w| w[0][1] == 0.0 || w[1][1] == 0.0 || (w[0][1] < 0.0) != (w[1][1] < 0.0))
    {
        return Err(
            "an `exp` curve can't start, end or cross at 0 (it grows by a ratio); use `bend:` on a linear curve instead"
                .into(),
        );
    }
    let data: Vec<f64> = (0..size)
        .map(|i| {
            let x = i as f64 / (size - 1).max(1) as f64;
            interpolate_curve(&pts, x, shape, bend)
        })
        .collect();
    Ok(Table::from_f64(data, None))
}

fn interpolate_curve(pts: &[[f64; 2]], x: f64, shape: CurveShape, bend: f64) -> f64 {
    if x <= pts[0][0] {
        return pts[0][1];
    }
    if x >= pts[pts.len() - 1][0] {
        return pts[pts.len() - 1][1];
    }
    let i = pts
        .iter()
        .position(|p| p[0] > x)
        .unwrap_or(1)
        .saturating_sub(1);
    let (x0, y0) = (pts[i][0], pts[i][1]);
    let (x1, y1) = (pts[i + 1][0], pts[i + 1][1]);
    let t = (x - x0) / (x1 - x0);
    match shape {
        CurveShape::Linear => {
            let t = if bend.abs() < 1e-9 {
                t
            } else {
                (bend * t).exp_m1() / bend.exp_m1()
            };
            y0 + (y1 - y0) * t
        }
        CurveShape::Exp => {
            // `curve` has checked that both ends are non-zero and on the same side of 0
            let sign = y0.signum();
            sign * y0.abs() * (y1.abs() / y0.abs()).powf(t)
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

/// Step function: at each integer index `xs[i]`, hold value `ys[i]`.
pub fn steps(xs: &[f64], ys: &[f64], size: usize) -> Result<Table, String> {
    assert!(size > 0);
    if xs.len() != ys.len() || xs.is_empty() {
        return Err("`steps` needs matching x and y arrays".into());
    }
    if xs.windows(2).any(|w| w[0] > w[1]) {
        return Err("the x positions of `steps` must rise".into());
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

/// Polynomial evaluated over -1..1 mapped onto the table.
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

/// Chebyshev transfer function: `amps[0]` weights T1(x), etc., over -1..1.
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

/// Window function.
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
    Gauss,
}

/// Deterministic pseudo-random table, every value within -1..1. `seed` is required for replay identity.
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
                let (a, b) = (next(), next());
                a.min(b) * 2.0 - 1.0
            }
            Dist::Triangle => next() + next() - 1.0,
            Dist::Exponential => {
                // a long tail, cut off at 8 so the table stays within -1..1
                let u = next().max(1e-12);
                (-u.ln()).min(8.0) / 4.0 - 1.0
            }
            Dist::Bilateral => {
                let u = next().max(1e-12);
                let size = (-u.ln()).min(8.0) / 8.0;
                if next() < 0.5 { size } else { -size }
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
                k as f64 / 10.0 - 1.0
            }
            Dist::Gauss => {
                // Box-Muller, three deviations to the edge
                let (u, v) = (next().max(1e-12), next());
                let z = (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos();
                (z / 3.0).clamp(-1.0, 1.0)
            }
        })
        .collect();
    Table::from_f64(data, None)
}

/// The strength and phase of harmonics 1..=`count` of a one cycle table, as `harmonics(amps, phases:)` takes
/// them: amplitudes as measured, phases in cycles. Harmonics past what the table can hold read as 0.
pub fn spectrum(table: &Table, count: usize) -> (Vec<f64>, Vec<f64>) {
    let n = table.len();
    let mut amps = Vec::with_capacity(count);
    let mut phases = Vec::with_capacity(count);
    for h in 1..=count {
        if h >= n.div_ceil(2) {
            amps.push(0.0);
            phases.push(0.0);
            continue;
        }
        let (mut sin, mut cos) = (0.0, 0.0);
        for (i, &v) in table.data().iter().enumerate() {
            let t = std::f64::consts::TAU * ((h * i) % n) as f64 / n as f64;
            sin += f64::from(v) * t.sin();
            cos += f64::from(v) * t.cos();
        }
        let (a, b) = (2.0 * sin / n as f64, 2.0 * cos / n as f64);
        let amp = a.hypot(b);
        amps.push(amp);
        phases.push(if amp > 1e-9 {
            b.atan2(a) / std::f64::consts::TAU
        } else {
            0.0
        });
    }
    (amps, phases)
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
        let t = harmonics(&[1.0], None, 64, true);
        let peak = t.data().iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 1.0).abs() < 1e-5, "{peak}");
    }

    #[test]
    fn spectrum_recovers_what_harmonics_built() {
        let amps = [1.0, 0.0, 0.5, 0.25];
        let phases = [0.0, 0.0, 0.25, 0.1];
        let t = harmonics(&amps, Some(&phases), 256, false);
        let (found, found_phases) = spectrum(&t, 4);
        for h in [0, 2, 3] {
            assert!((found[h] - amps[h]).abs() < 1e-4, "{h}: {}", found[h]);
            assert!((found_phases[h] - phases[h]).abs() < 1e-4, "{h}");
        }
        assert!(found[1].abs() < 1e-4);
        // past what 256 entries can hold
        assert_eq!(spectrum(&t, 200).0[199], 0.0);
    }

    #[test]
    fn harmonics_keeps_relative_level_when_not_normalized() {
        let t = harmonics(&[0.25], None, 64, false);
        let peak = t.data().iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.25).abs() < 1e-5, "{peak}");
    }

    #[test]
    fn exp_curve_from_zero_is_refused_and_bend_works_through_zero() {
        assert!(curve(&[[0.0, 0.0], [1.0, 1.0]], 8, CurveShape::Exp, 0.0).is_err());
        let rise = curve(&[[0.0, 0.0], [1.0, 1.0]], 5, CurveShape::Linear, 4.0).unwrap();
        assert!((rise.get(0) - 0.0).abs() < 1e-6 && (rise.get(4) - 1.0).abs() < 1e-6);
        assert!(rise.get(2) < 0.25, "slow start: {}", rise.get(2));
        assert!(curve(&[[0.0, 0.0], [1.0, 1.0]], 5, CurveShape::Cubic, 1.0).is_err());
    }

    #[test]
    fn steps_must_rise() {
        assert!(steps(&[4.0, 0.0], &[1.0, 2.0], 8).is_err());
    }

    #[test]
    fn every_random_distribution_stays_within_one() {
        for dist in [
            Dist::Uniform,
            Dist::Linear,
            Dist::Triangle,
            Dist::Exponential,
            Dist::Bilateral,
            Dist::Cauchy,
            Dist::Poisson,
            Dist::Gauss,
        ] {
            for seed in 1..40 {
                let t = random(512, dist, seed);
                assert!(t.data().iter().all(|v| v.abs() <= 1.0), "{dist:?}");
            }
        }
    }

    #[test]
    fn curve_linear_endpoints() {
        let t = curve(&[[0.0, 0.0], [1.0, 1.0]], 5, CurveShape::Linear, 0.0).unwrap();
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
