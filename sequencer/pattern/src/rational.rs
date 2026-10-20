use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// An exact fraction. Pattern time is rational so that subdividing a cycle into
/// thirds, fifths or sevenths never drifts the way floating point would.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    n: i64,
    d: i64,
}

/// Greatest common divisor without any division, which is slow on 128 bits.
fn gcd64(mut a: u64, mut b: u64) -> u64 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Rational {
    pub const ZERO: Rational = Rational { n: 0, d: 1 };
    pub const ONE: Rational = Rational { n: 1, d: 1 };

    /// `n / d` in lowest terms. Panics if `d` is zero.
    pub fn new(n: i64, d: i64) -> Self {
        Self::from_wide(i128::from(n), i128::from(d))
    }

    pub fn int(n: i64) -> Self {
        Rational { n, d: 1 }
    }

    fn from_wide(n: i128, d: i128) -> Self {
        assert!(d != 0, "a rational can't have a zero denominator");
        // Nearly every pattern time fits in 64 bits, where this is far cheaper.
        if let (Ok(n), Ok(d)) = (i64::try_from(n), i64::try_from(d))
            && n != i64::MIN
            && d != i64::MIN
        {
            let g = gcd64(n.unsigned_abs(), d.unsigned_abs()).max(1) as i64;
            let (mut n, mut d) = (n / g, d / g);
            if d < 0 {
                n = -n;
                d = -d;
            }
            return Rational { n, d };
        }
        let g = gcd(n, d).max(1);
        let (mut n, mut d) = (n / g, d / g);
        if d < 0 {
            n = -n;
            d = -d;
        }
        // A result too big for 64 bits is scaled down to the nearest fraction
        // that fits, rather than crashing: only absurd times get here.
        while i64::try_from(n).is_err() || i64::try_from(d).is_err() {
            n /= 2;
            d = (d / 2).max(1);
        }
        Rational { n: n as i64, d: d as i64 }
    }

    /// The closest simple fraction to `x`, for turning a decimal such as `0.25`
    /// into `1/4`. Exact for anything with a short decimal expansion.
    pub fn from_f64(x: f64) -> Self {
        let mut d = 1i64;
        while (x * d as f64).fract().abs() > 1e-9 && d < 1_000_000 {
            d *= 10;
        }
        Rational::new((x * d as f64).round() as i64, d)
    }

    pub fn numer(self) -> i64 {
        self.n
    }

    pub fn denom(self) -> i64 {
        self.d
    }

    pub fn is_zero(self) -> bool {
        self.n == 0
    }

    pub fn is_integer(self) -> bool {
        self.d == 1
    }

    pub fn floor(self) -> i64 {
        self.n.div_euclid(self.d)
    }

    pub fn ceil(self) -> i64 {
        -((-self.n).div_euclid(self.d))
    }

    /// The part after the whole number, always in `0..1`.
    pub fn fract(self) -> Rational {
        self - Rational::int(self.floor())
    }

    pub fn to_f64(self) -> f64 {
        self.n as f64 / self.d as f64
    }

    pub fn min(self, other: Rational) -> Rational {
        if self <= other { self } else { other }
    }

    pub fn max(self, other: Rational) -> Rational {
        if self >= other { self } else { other }
    }
}

impl From<i64> for Rational {
    fn from(n: i64) -> Self {
        Rational::int(n)
    }
}

impl Add for Rational {
    type Output = Rational;
    fn add(self, o: Rational) -> Rational {
        Rational::from_wide(
            i128::from(self.n) * i128::from(o.d) + i128::from(o.n) * i128::from(self.d),
            i128::from(self.d) * i128::from(o.d),
        )
    }
}

impl Sub for Rational {
    type Output = Rational;
    fn sub(self, o: Rational) -> Rational {
        self + -o
    }
}

impl Mul for Rational {
    type Output = Rational;
    fn mul(self, o: Rational) -> Rational {
        Rational::from_wide(i128::from(self.n) * i128::from(o.n), i128::from(self.d) * i128::from(o.d))
    }
}

impl Div for Rational {
    type Output = Rational;
    fn div(self, o: Rational) -> Rational {
        if o.n == 0 {
            // never reached by a program that checks its divisors; this keeps a live session alive if one slips through
            return Rational::ZERO;
        }
        Rational::from_wide(i128::from(self.n) * i128::from(o.d), i128::from(self.d) * i128::from(o.n))
    }
}

impl Neg for Rational {
    type Output = Rational;
    fn neg(self) -> Rational {
        Rational { n: self.n.saturating_neg(), d: self.d }
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (i128::from(self.n) * i128::from(other.d)).cmp(&(i128::from(other.n) * i128::from(self.d)))
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.d == 1 { write!(f, "{}", self.n) } else { write!(f, "{}/{}", self.n, self.d) }
    }
}

impl fmt::Debug for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i64, d: i64) -> Rational {
        Rational::new(n, d)
    }

    #[test]
    fn normalises_to_lowest_terms_with_a_positive_denominator() {
        assert_eq!(r(2, 4), r(1, 2));
        assert_eq!(r(3, -6), r(-1, 2));
        assert_eq!(r(0, 5), Rational::ZERO);
        assert_eq!(r(6, 3).to_string(), "2");
        assert_eq!(r(-1, 3).to_string(), "-1/3");
    }

    #[test]
    fn arithmetic_is_exact() {
        assert_eq!(r(1, 3) + r(1, 6), r(1, 2));
        assert_eq!(r(1, 2) - r(1, 3), r(1, 6));
        assert_eq!(r(2, 3) * r(3, 4), r(1, 2));
        assert_eq!(r(1, 2) / r(1, 4), Rational::int(2));
        assert_eq!(-r(1, 2), r(-1, 2));
        // thirds add up to exactly one, which floating point would not give
        assert_eq!(r(1, 3) + r(1, 3) + r(1, 3), Rational::ONE);
    }

    #[test]
    fn ordering_and_rounding() {
        assert!(r(1, 3) < r(1, 2));
        assert!(r(-1, 2) < r(-1, 3));
        assert_eq!(r(7, 2).floor(), 3);
        assert_eq!(r(-7, 2).floor(), -4);
        assert_eq!(r(7, 2).ceil(), 4);
        assert_eq!(r(-7, 2).ceil(), -3);
        assert_eq!(r(6, 2).ceil(), 3);
        assert_eq!(r(7, 2).fract(), r(1, 2));
        assert_eq!(r(-1, 4).fract(), r(3, 4));
        assert_eq!(r(1, 3).min(r(1, 2)), r(1, 3));
        assert_eq!(r(1, 3).max(r(1, 2)), r(1, 2));
    }

    #[test]
    fn decimals_become_simple_fractions() {
        assert_eq!(Rational::from_f64(0.25), r(1, 4));
        assert_eq!(Rational::from_f64(1.5), r(3, 2));
        assert_eq!(Rational::from_f64(-0.125), r(-1, 8));
        assert_eq!(Rational::from_f64(3.0), Rational::int(3));
        assert_eq!(Rational::from_f64(0.2), r(1, 5));
    }

    #[test]
    fn absurd_sizes_are_scaled_down_instead_of_crashing() {
        let big = Rational::int(i64::MAX);
        let squared = big * big;
        assert!(squared > Rational::int(i64::MAX / 2));
        assert!(-Rational::int(i64::MIN) > Rational::ZERO);
        assert_eq!(big / Rational::ZERO, Rational::ZERO);
        let _ = big + big + big;
    }

    #[test]
    #[should_panic(expected = "zero denominator")]
    fn zero_denominator_panics() {
        let _ = r(1, 0);
    }
}
