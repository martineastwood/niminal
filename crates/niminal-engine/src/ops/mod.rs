//! Built-in opcodes.

mod env;
mod filter;
mod math;
mod osc;

pub use env::{Curve, Env, Segment};
pub use filter::{FilterMode, Svf};
pub use math::{Add, Gain, Mul, Sub};
pub use osc::{Osc, Wave};
