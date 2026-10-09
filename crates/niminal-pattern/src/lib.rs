//! Patterns: values arranged over cycles of exact (rational) time, queried
//! rather than unrolled, with the mini-notation and transformations that
//! niminal's live-coding layer is written in.

mod mini;
mod pattern;
mod rational;

pub use mini::{Hit, MiniError, grid, parse};
pub use pattern::{Hap, Pattern, Span, euclid_steps};
pub use rational::Rational;
