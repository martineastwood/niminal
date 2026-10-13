//! Built-in opcodes.

mod control;
mod custom;
mod delay;
mod env;
mod filter;
mod math;
mod osc;
mod pan;
mod reverb;
mod sample;
mod tap;

pub use control::Control;
pub use custom::{BinOp, Cmp, Custom, Fn1, Func, Instr, Interp, Kernel, List, Native, NativeInput, read_at, rem};
pub use delay::Delay;
pub use env::{Curve, Env, EnvField, Segment, shape};
pub use filter::{FilterMode, Svf};
pub use math::{
    Add, BlockHold, Compare, Delay1, Div, Gain, Hold, Max, Min, Mul, NoteTime, Pow, Released, Rem, SampleRate,
    Select, Sub,
    TableRead,
};
pub use osc::{Osc, Wave};
pub use pan::{PanChannel, pan_gains};
pub use reverb::Reverb;
pub use sample::{SampleData, Sampler};
pub use tap::{Tap, TapOut};
