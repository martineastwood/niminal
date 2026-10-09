//! Channel layouts: what a multichannel signal's channels mean, and how one
//! layout is turned into another.
//!
//! The engine only knows channel counts. Layouts live here, in the compiler,
//! which turns conversions and panning into ordinary gain and sum nodes.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Mono,
    Stereo,
    /// L R Ls Rs
    Quad,
    /// L R C LFE Ls Rs
    Surround51,
    /// L R C LFE Lb Rb Ls Rs
    Surround71,
    /// N unnamed channels.
    Channels(usize),
}

/// -3db, the usual coefficient when folding one channel into two.
const MINUS_3DB: f64 = std::f64::consts::FRAC_1_SQRT_2;

impl Layout {
    pub fn channels(self) -> usize {
        match self {
            Layout::Mono => 1,
            Layout::Stereo => 2,
            Layout::Quad => 4,
            Layout::Surround51 => 6,
            Layout::Surround71 => 8,
            Layout::Channels(n) => n,
        }
    }

    /// The layout with this many channels, if there is a named one.
    pub fn from_count(n: usize) -> Layout {
        match n {
            1 => Layout::Mono,
            2 => Layout::Stereo,
            _ => Layout::Channels(n),
        }
    }

    /// Where each channel's speaker sits, in degrees from straight ahead
    /// (positive to the right). `None` for a channel that is never panned to.
    pub fn speaker_angles(self) -> Vec<Option<f32>> {
        match self {
            Layout::Mono => vec![Some(0.0)],
            Layout::Stereo => vec![Some(-30.0), Some(30.0)],
            Layout::Quad => vec![Some(-45.0), Some(45.0), Some(-135.0), Some(135.0)],
            Layout::Surround51 => vec![Some(-30.0), Some(30.0), Some(0.0), None, Some(-110.0), Some(110.0)],
            Layout::Surround71 => vec![
                Some(-30.0),
                Some(30.0),
                Some(0.0),
                None,
                Some(-150.0),
                Some(150.0),
                Some(-90.0),
                Some(90.0),
            ],
            // Unnamed channels are spaced evenly round the circle.
            Layout::Channels(n) => (0..n).map(|i| Some(i as f32 * 360.0 / n as f32)).collect(),
        }
    }

    /// Index of the left, right and centre channels, where the layout has them.
    fn front(self) -> (Option<usize>, Option<usize>, Option<usize>) {
        match self {
            Layout::Mono => (None, None, Some(0)),
            Layout::Stereo | Layout::Quad => (Some(0), Some(1), None),
            Layout::Surround51 | Layout::Surround71 => (Some(0), Some(1), Some(2)),
            Layout::Channels(_) => (None, None, None),
        }
    }

    /// Indices of the surround (side or rear-side) channels, left then right.
    fn surrounds(self) -> Option<(usize, usize)> {
        match self {
            Layout::Quad => Some((2, 3)),
            Layout::Surround51 => Some((4, 5)),
            Layout::Surround71 => Some((6, 7)),
            _ => None,
        }
    }
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Layout::Mono => f.write_str("mono"),
            Layout::Stereo => f.write_str("stereo"),
            Layout::Quad => f.write_str("quad"),
            Layout::Surround51 => f.write_str("surround(5.1)"),
            Layout::Surround71 => f.write_str("surround(7.1)"),
            Layout::Channels(n) => write!(f, "ch[{n}]"),
        }
    }
}

/// `matrix[out][in]` is how much of input channel `in` goes to output `out`.
pub type Matrix = Vec<Vec<f64>>;

/// The automatic conversion from one layout to another, following the spec:
/// mono is centred anywhere; stereo goes to the front pair of a surround
/// layout; surround folds down to stereo with standard coefficients (the LFE is
/// dropped). `None` where there is no automatic rule.
pub fn conversion(from: Layout, to: Layout) -> Option<Matrix> {
    if from == to {
        return Some(identity(to.channels()));
    }
    let mut m = vec![vec![0.0; from.channels()]; to.channels()];

    match (from, to) {
        // Anything folded to mono is averaged.
        (_, Layout::Mono) => {
            let share = 1.0 / from.channels() as f64;
            m[0].iter_mut().for_each(|c| *c = share);
        }
        (Layout::Mono, _) => {
            let (l, r, c) = to.front();
            match (c, l, r) {
                (Some(c), ..) => m[c][0] = 1.0,
                (None, Some(l), Some(r)) => {
                    m[l][0] = MINUS_3DB;
                    m[r][0] = MINUS_3DB;
                }
                _ => return None,
            }
        }
        // The front pair of a bigger layout.
        (Layout::Stereo, _) => {
            let (Some(l), Some(r), _) = to.front() else { return None };
            m[l][0] = 1.0;
            m[r][1] = 1.0;
        }
        (_, Layout::Stereo) => {
            let (Some(l), Some(r), c) = from.front() else { return None };
            m[0][l] = 1.0;
            m[1][r] = 1.0;
            if let Some(c) = c {
                m[0][c] = MINUS_3DB;
                m[1][c] = MINUS_3DB;
            }
            let (ls, rs) = from.surrounds()?;
            m[0][ls] = MINUS_3DB;
            m[1][rs] = MINUS_3DB;
            if from == Layout::Surround71 {
                // the rear pair of 7.1 follows the side pair
                m[0][4] = MINUS_3DB;
                m[1][5] = MINUS_3DB;
            }
        }
        _ => return None,
    }
    Some(m)
}

fn identity(n: usize) -> Matrix {
    (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Matrix, input: &[f64]) -> Vec<f64> {
        m.iter().map(|row| row.iter().zip(input).map(|(c, x)| c * x).sum()).collect()
    }

    #[test]
    fn channel_counts_and_names() {
        assert_eq!(Layout::Surround51.channels(), 6);
        assert_eq!(Layout::Surround71.channels(), 8);
        assert_eq!(Layout::Channels(3).to_string(), "ch[3]");
        assert_eq!(Layout::Surround51.to_string(), "surround(5.1)");
        assert_eq!(Layout::from_count(2), Layout::Stereo);
        assert_eq!(Layout::Surround51.speaker_angles().len(), 6);
        assert_eq!(Layout::Surround51.speaker_angles()[3], None, "no panning to the LFE");
    }

    #[test]
    fn mono_is_centred_wherever_it_goes() {
        let to_51 = conversion(Layout::Mono, Layout::Surround51).unwrap();
        assert_eq!(apply(&to_51, &[1.0]), [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        let to_stereo = apply(&conversion(Layout::Mono, Layout::Stereo).unwrap(), &[1.0]);
        assert!((to_stereo[0] - MINUS_3DB).abs() < 1e-12 && to_stereo[0] == to_stereo[1]);
    }

    #[test]
    fn stereo_goes_to_the_front_pair() {
        let m = conversion(Layout::Stereo, Layout::Surround51).unwrap();
        assert_eq!(apply(&m, &[0.5, 0.25]), [0.5, 0.25, 0.0, 0.0, 0.0, 0.0]);
        let q = conversion(Layout::Stereo, Layout::Quad).unwrap();
        assert_eq!(apply(&q, &[1.0, 2.0]), [1.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn surround_folds_down_to_stereo_without_the_lfe() {
        let m = conversion(Layout::Surround51, Layout::Stereo).unwrap();
        // L R C LFE Ls Rs
        let out = apply(&m, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(out, [1.0, 0.0], "the LFE never reaches stereo");
        let centre = apply(&m, &[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!((centre[0] - MINUS_3DB).abs() < 1e-12 && centre[0] == centre[1]);
        let surround = apply(&m, &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0]);
        assert!((surround[0] - MINUS_3DB).abs() < 1e-12 && (surround[1] - MINUS_3DB).abs() < 1e-12);

        let quad = apply(&conversion(Layout::Quad, Layout::Stereo).unwrap(), &[1.0, 1.0, 1.0, 1.0]);
        assert!((quad[0] - (1.0 + MINUS_3DB)).abs() < 1e-12);
    }

    #[test]
    fn anything_folds_to_mono_by_averaging() {
        let m = conversion(Layout::Stereo, Layout::Mono).unwrap();
        assert_eq!(apply(&m, &[1.0, 0.0]), [0.5]);
        assert!((apply(&conversion(Layout::Surround51, Layout::Mono).unwrap(), &[1.0; 6])[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn same_layout_is_the_identity_and_odd_pairs_have_no_rule() {
        let m = conversion(Layout::Quad, Layout::Quad).unwrap();
        assert_eq!(apply(&m, &[1.0, 2.0, 3.0, 4.0]), [1.0, 2.0, 3.0, 4.0]);
        assert!(conversion(Layout::Quad, Layout::Surround51).is_none());
        assert!(conversion(Layout::Channels(3), Layout::Stereo).is_none());
        assert!(conversion(Layout::Surround51, Layout::Surround71).is_none());
    }
}
