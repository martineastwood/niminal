//! Built-in opcodes.

mod custom;
mod delay;
mod env;
mod filter;
mod math;
mod osc;
mod pan;
mod reverb;
mod sample;

pub use custom::{BinOp, Custom, Fn1, Func, Instr, Kernel};
pub use delay::Delay;
pub use env::{Curve, Env, Segment};
pub use filter::{FilterMode, Svf};
pub use math::{Add, Gain, Mul, Sub};
pub use osc::{Osc, Wave};
pub use pan::{PanChannel, pan_gains};
pub use reverb::Reverb;
pub use sample::{SampleData, Sampler};
