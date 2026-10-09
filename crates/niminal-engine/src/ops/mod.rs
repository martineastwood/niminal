//! Built-in opcodes.

mod custom;
mod env;
mod filter;
mod math;
mod osc;

pub use custom::{BinOp, Custom, Fn1, Func, Instr, Kernel};
pub use env::{Curve, Env, Segment};
pub use filter::{FilterMode, Svf};
pub use math::{Add, Gain, Mul, Sub};
pub use osc::{Osc, Wave};
