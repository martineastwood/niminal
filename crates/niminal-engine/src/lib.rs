//! niminal's DSP runtime.
//!
//! The engine deals only in plain numbers: frequencies in hz, gains as linear
//! factors, times in seconds. Units (`800hz`, `-6db`, `1/8beat`) are the
//! business of the language and score layers, which convert before building a
//! graph. There is no OS-specific code here, so the crate builds for WebAssembly.

mod array;
pub mod denormals;
mod graph;
mod master;
mod mapping;
pub use mapping::ParamMapping;
mod mixer;
mod opcode;
pub mod ops;
mod table;
mod voice;

pub use array::{Array, wrapped_index};
pub use master::{DEFAULT_CEILING, Master};
pub use mixer::{Buses, DEFAULT_MAX_VOICES, ChainInput, MAX_CHANNELS, Mixer, Route, Stems, TrackDef, Transfer, VoiceId, execution_order};
pub use graph::{Graph, GraphBuilder, GraphError, Src};
pub use opcode::{BLOCK, Clock, MAX_INPUTS, Opcode, Port, ProcessCtx, TAIL_LEVEL};
pub use table::{Buffer, Edge, Table};
pub use voice::{Priority, Voice, VoiceRule};
