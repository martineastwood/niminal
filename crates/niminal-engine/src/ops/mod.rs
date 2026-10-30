//! Built-in opcodes.

mod allpass;
mod biquad;
mod bitcrush;
mod buffer;
mod chorus;
mod comb;
mod compress;
mod control;
mod convolve;
mod custom;
mod dc_block;
mod delay;
mod distortion;
mod env;
mod eq;
mod fdn;
mod filter;
mod fold;
mod follow;
mod formant;
mod gate;
mod glitch;
mod granular;
mod impulse;
mod ladder;
mod lag;
mod lanes;
mod limiter;
mod math;
mod modal;
mod multisample;
mod noise;
mod osc;
mod pan;
mod phaser;
mod phasor;
mod pitch_shift;
mod resonator;
mod reverb;
mod sample;
mod sample_hold;
mod spectral;
mod tap;

pub use allpass::Allpass;
pub use biquad::{Biquad, BiquadMode};
pub use bitcrush::Bitcrush;
pub use buffer::{BufferRead, BufferReadBank, BufferRecord, BufferWrite};
pub use chorus::Chorus;
pub use comb::Comb;
pub use compress::Compressor;
pub use control::Control;
pub use convolve::{Convolve, IrSpectrum};
pub use custom::{
    ArraySource, BinOp, Cmp, Custom, Fn1, FrameScope, Func, Instr, Interp, Kernel, LaneSrc, Native,
    NativeInput, Reduce, ResetScope, read_array, read_at, rem,
};
pub use dc_block::DcBlock;
pub use delay::Delay;
pub use distortion::Distortion;
pub use env::{Curve, Env, EnvField, Segment, shape};
pub use eq::{Eq, EqMode};
pub use fdn::Fdn;
pub use filter::{FilterMode, Svf};
pub use fold::{Clip, Fold};
pub use follow::Follow;
pub use formant::Formant;
pub use gate::Gate;
pub use glitch::{
    DEFAULT_MAX_SECONDS as GLITCH_MAX_SECONDS, Freeze, RepeatLast, RewindLast, Scatter, SlowDown,
};
pub use granular::{GRANULAR_MONITOR_FIELDS, Granular, GranularConfig};
pub use impulse::Impulse;
pub use ladder::Ladder;
pub use lag::Lag;
pub use limiter::Limiter;
pub use math::{
    Add, ArrayRead, Atan2, BlockHold, Compare, Context, Delay1, Div, Gain, Hold, Max, Min, Mul,
    NoteStart, NoteTime, Pow, ReadContext, Released, Rem, SampleRate, Select, Sub, TableRead,
    Waveshape,
};
pub use modal::Modal;
pub use multisample::{AmpEnv, MultiSampler, Region, RoundRobin};
pub use noise::{Noise, NoiseColor};
pub use osc::{Osc, Wave, Wavetable};
pub use pan::{PanChannel, pan_gains};
pub use phaser::Phaser;
pub use phasor::Phasor;
pub use pitch_shift::PitchShift;
pub use resonator::Resonator;
pub use reverb::Reverb;
pub use sample::{KitSampler, LoopMode, SampleInterp, Sampler};
pub use sample_hold::SampleHold;
pub use spectral::{SpectralDelay, SpectralFreeze, valid_fft_size};
pub use tap::{Tap, TapOut};

mod lifetime;
pub use lifetime::FinishWhen;

mod reset;
pub use reset::ResetOn;

mod spawn;
pub use spawn::Spawn;

mod oversample;
pub use oversample::Oversample;

mod control_rate;
pub use control_rate::{ControlRate, Ramp};

mod latency;
pub use latency::SampleDelay;
mod kernel_latency;

mod report;
pub use report::{Report, ReportOut};

mod stream;
pub use stream::{STREAM_PAGE, StreamCache, StreamOut};

mod transients;
pub use transients::{detect_transients, detect_transients_at};
