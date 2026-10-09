//! niminal's DSP runtime.
//!
//! The engine deals only in plain numbers: frequencies in hz, gains as linear
//! factors, times in seconds. Units (`800hz`, `-6db`, `1/8beat`) are the
//! business of the language and score layers, which convert before building a
//! graph. There is no OS-specific code here, so the crate builds for WebAssembly.

mod graph;
mod master;
mod mixer;
mod opcode;
pub mod ops;
mod voice;

pub use master::Master;
pub use mixer::{Buses, ChainInput, MAX_CHANNELS, Mixer, Route, TrackDef, Transfer, VoiceId, execution_order};
pub use graph::{Graph, GraphBuilder, GraphError, Src};
pub use opcode::{BLOCK, MAX_INPUTS, Opcode, Port, ProcessCtx, TAIL_LEVEL};
pub use voice::Voice;
