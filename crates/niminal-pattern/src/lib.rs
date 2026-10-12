//! Patterns: values arranged over cycles of exact (rational) time, queried
//! rather than unrolled, with the mini-notation and transformations that
//! niminal's live-coding layer is written in.

mod mini;
mod pattern;
mod rational;

pub use mini::{Hit, MiniError, grid, parse, parse_with};
pub use pattern::{Hap, MAX_REPEATS, Param, Pattern, Span, euclid_steps, inner_join, unit_random, with_budget};
pub use rational::Rational;
