//! Normalized finite cosine series, evaluated without looping over every partial.
use std::f64::consts::TAU;

pub(super) fn sample(phase: f64, partials: usize, chroma: f64) -> f32 {
    if partials == 0 {
        return 0.0;
    }
    let theta = TAU * phase;
    let n = partials as f64;
    let a = chroma.abs().clamp(0.001, 100.0);
    // Negative chroma alternates the signs of successive partials.
    let theta = if chroma < 0.0 {
        theta + std::f64::consts::PI
    } else {
        theta
    };
    if (a - 1.0).abs() < 1e-5 {
        let half_sine = (theta * 0.5).sin();
        return if half_sine.abs() < 1e-8 {
            1.0
        } else {
            (((n + 0.5) * theta).sin() / (2.0 * half_sine) - 0.5) as f32 / partials as f32
        };
    }
    // For rising spectra, reverse the geometric weights to avoid a^N overflow.
    let r = a.min(1.0 / a);
    let rn = r.powi(partials as i32);
    let denominator = 1.0 - 2.0 * r * theta.cos() + r * r;
    let numerator = if a < 1.0 {
        theta.cos() - r - rn * ((n + 1.0) * theta).cos() + rn * r * (n * theta).cos()
    } else {
        (n * theta).cos() - r * ((n + 1.0) * theta).cos() - rn + rn * r * theta.cos()
    };
    let norm = (1.0 - rn) / (1.0 - r);
    if denominator.abs() < 1e-12 {
        1.0
    } else {
        (numerator / denominator / norm) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_form_matches_the_harmonic_sum_and_stays_normalized() {
        for n in [1, 3, 16, 100] {
            for a in [0.2_f64, 0.9, 1.0, 1.1, 3.0] {
                for phase in [0.0, 0.013, 0.2, 0.5, 0.999] {
                    let weights: Vec<_> = (0..n)
                        .map(|i| {
                            if a <= 1.0 {
                                a.powi(i as i32)
                            } else {
                                a.powi(i as i32 - n as i32 + 1)
                            }
                        })
                        .collect();
                    let expected = weights
                        .iter()
                        .enumerate()
                        .map(|(i, w)| w * (TAU * phase * (i + 1) as f64).cos())
                        .sum::<f64>()
                        / weights.iter().sum::<f64>();
                    let got = sample(phase, n, a);
                    assert!(
                        (got as f64 - expected).abs() < 1e-5,
                        "{n} {a} {phase}: {got} vs {expected}"
                    );
                    assert!(got.abs() <= 1.00001);
                }
            }
        }
    }
}
