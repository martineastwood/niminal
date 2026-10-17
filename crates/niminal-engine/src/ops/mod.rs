//! Built-in opcodes.

mod bitcrush;
mod buffer;
mod chorus;
mod compress;
mod control;
mod custom;
mod delay;
mod env;
mod filter;
mod fold;
mod math;
mod noise;
mod osc;
mod pan;
mod reverb;
mod sample;
mod tap;

pub use bitcrush::Bitcrush;
pub use buffer::{BufferRead, BufferRecord, BufferWrite};
pub use chorus::Chorus;
pub use compress::Compressor;
pub use control::Control;
pub use custom::{BinOp, Cmp, Custom, Fn1, Func, Instr, Interp, Kernel, ResetScope, ArraySource, Native, NativeInput, read_array, read_at, rem};
pub use delay::Delay;
pub use env::{Curve, Env, EnvField, Segment, shape};
pub use filter::{FilterMode, Svf};
pub use fold::{Clip, Fold};
pub use math::{
    Add, BlockHold, Compare, Context, Delay1, Div, Gain, Hold, Max, Min, Mul, NoteStart, NoteTime, Pow, ReadContext, Released, Rem, SampleRate,
    Select, Sub,
    ArrayRead, TableRead, Waveshape,
};
pub use noise::{Noise, NoiseColor};
pub use osc::{Osc, Wave, Wavetable};
pub use pan::{PanChannel, pan_gains};
pub use reverb::Reverb;
pub use sample::{LoopMode, Sampler};
pub use tap::{Tap, TapOut};

mod lifetime;
pub use lifetime::FinishWhen;

mod reset;
pub use reset::ResetOn;
